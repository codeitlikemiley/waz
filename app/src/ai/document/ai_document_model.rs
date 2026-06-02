#![allow(warnings)]

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

// TODO(vorporeal): Remove this re-export at some point.
pub use ai::document::{AIDocumentId, AIDocumentVersion};
use anyhow;
use chrono::{DateTime, Local, Utc};
use itertools::Itertools;
use uuid::Uuid;
use warpui::{AppContext, Entity, EntityId, ModelContext, ModelHandle, SingletonEntity, WindowId};

use crate::ai::ai_document_view::DEFAULT_PLANNING_DOCUMENT_TITLE;
use crate::global_resource_handles::GlobalResourceHandlesProvider;
use crate::persistence::ModelEvent;
use crate::{
    ai::{
        agent::{conversation::AIConversationId, AIAgentActionId},
        execution_profiles::profiles::AIExecutionProfilesModel,
    },
    appearance::Appearance,
    notebooks::{
        editor::{
            model::{FileLinkResolutionContext, NotebooksEditorModel, RichTextEditorModelEvent},
            rich_text_styles,
        },
        post_process_notebook,
    },
    settings::FontSettings,
    terminal::{
        model::session::{active_session::ActiveSession, Session},
        TerminalView,
    },
    throttle::throttle,
};
use ai::diff_validation::DiffDelta;
use warp_editor::{model::RichTextEditorModel, render::model::RichTextStyles};
use warpui::color::ColorU;

/// The frequency at which we check for modifications and save the AI document to the server.
/// Uses the same 2-second period as notebooks for consistency.
const SAVE_PERIOD: Duration = Duration::from_secs(2);

struct AIDocumentSaveRequest {
    document_id: AIDocumentId,
}

/// The status of saving an AI Document.
///
/// In openWarp, the plan is only written to the local SQLite, not to the cloud Drive, therefore
/// only `Saved` and `NotSaved` states are retained.
pub enum AIDocumentSaveStatus {
    /// Saved to local SQLite
    Saved,
    /// Not saved (only used as a fallback when there is no document)
    NotSaved,
}

impl AIDocumentSaveStatus {
    pub fn is_saved(&self) -> bool {
        matches!(self, AIDocumentSaveStatus::Saved)
    }
}

/// Tracks whether user edits to a planning document are known by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AIDocumentUserEditStatus {
    /// Document content matches what agent generated; no user edits
    UpToDate,
    /// User has made edits that diverge from agent-generated content.
    /// When dirty, the document is always attached to the next query.
    Dirty,
}

impl AIDocumentUserEditStatus {
    pub fn is_dirty(&self) -> bool {
        matches!(self, AIDocumentUserEditStatus::Dirty)
    }
}

#[derive(Debug, Clone)]
pub struct AIDocumentEarlierVersion {
    pub title: String,
    pub version: AIDocumentVersion,
    pub editor: ModelHandle<NotebooksEditorModel>,
    pub created_at: DateTime<Local>,
    pub restored_from: Option<AIDocumentVersion>,
}

#[derive(Debug, Clone)]
pub struct AIDocument {
    pub title: String,
    pub version: AIDocumentVersion,
    pub editor: ModelHandle<NotebooksEditorModel>,
    pub user_edit_status: AIDocumentUserEditStatus,
    pub conversation_id: AIConversationId,
    pub created_at: DateTime<Local>,
    pub restored_from: Option<AIDocumentVersion>,
    /// The set of pane group entity IDs in which this document is currently visible.
    pub visible_in_pane_groups: HashSet<EntityId>,
}

pub enum AIDocumentInstance {
    Current(AIDocument),
    Earlier(AIDocumentEarlierVersion),
}

impl AIDocumentInstance {
    pub fn get_version(&self) -> AIDocumentVersion {
        match self {
            AIDocumentInstance::Current(doc) => doc.version,
            AIDocumentInstance::Earlier(doc) => doc.version,
        }
    }

    pub fn get_title(&self) -> String {
        match self {
            AIDocumentInstance::Current(doc) => doc.title.clone(),
            AIDocumentInstance::Earlier(doc) => doc.title.clone(),
        }
    }

    pub fn get_editor(&self) -> ModelHandle<NotebooksEditorModel> {
        match self {
            AIDocumentInstance::Current(doc) => doc.editor.clone(),
            AIDocumentInstance::Earlier(doc) => doc.editor.clone(),
        }
    }
}

/// Source of an update to an AI document.
#[derive(Debug, Clone, Copy)]
pub enum AIDocumentUpdateSource {
    User,
    Agent,
    Restoration,
}

#[derive(Debug, Clone)]
pub struct AIDocumentModel {
    documents: HashMap<AIDocumentId, AIDocument>,
    earlier_versions: HashMap<AIDocumentId, Vec<AIDocumentEarlierVersion>>,
    /// The latest document ID for each conversation ID.
    /// Tracking separately to ensure the latest document in a conversation is set whenever we create a document.
    /// Otherwise, we'd need to keep track of timestamps to determine the latest document in a conversation.
    latest_document_id_by_conversation_id: HashMap<AIConversationId, AIDocumentId>,
    content_dirty_flags: HashMap<AIDocumentId, bool>,
    save_tx: async_channel::Sender<AIDocumentSaveRequest>,
    /// Mapping from (conversation_id, action_id, document_index) for streaming CreateDocuments
    /// tool calls to the corresponding AI document ID.
    streaming_create_documents: HashMap<(AIConversationId, AIAgentActionId, usize), AIDocumentId>,
}

impl AIDocumentModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Setup throttled save channel
        let (save_tx, save_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            throttle(SAVE_PERIOD, save_rx),
            Self::handle_save_request,
            |_, _| {},
        );

        Self {
            documents: HashMap::new(),
            earlier_versions: HashMap::new(),
            latest_document_id_by_conversation_id: HashMap::new(),
            content_dirty_flags: HashMap::new(),
            save_tx,
            streaming_create_documents: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let (save_tx, _save_rx) = async_channel::unbounded();
        Self {
            documents: HashMap::new(),
            earlier_versions: HashMap::new(),
            latest_document_id_by_conversation_id: HashMap::new(),
            content_dirty_flags: HashMap::new(),
            save_tx,
            streaming_create_documents: HashMap::new(),
        }
    }

    /// Returns the local save status of the document. Under openWarp, as long as the document still exists in memory,
    /// its markdown content has already been written to SQLite via the throttled save channel,
    /// and therefore it is always `Saved`.
    pub fn get_document_save_status(&self, id: &AIDocumentId) -> AIDocumentSaveStatus {
        // openWarp does not use cloud sync; Plan content is automatically written to local SQLite,
        // and the document is considered saved as long as it exists.
        if self.documents.contains_key(id) {
            AIDocumentSaveStatus::Saved
        } else {
            AIDocumentSaveStatus::NotSaved
        }
    }

    /// Create a new document with default title/content and return its ID.
    pub fn create_document(
        &mut self,
        title: impl Into<String>,
        content: impl Into<String>,
        conversation_id: AIConversationId,
        file_link_resolution_context: Option<FileLinkResolutionContext>,
        ctx: &mut ModelContext<Self>,
    ) -> AIDocumentId {
        let id = AIDocumentId::new();
        self.create_document_internal(
            id,
            title,
            content,
            AIDocumentUpdateSource::Agent,
            conversation_id,
            file_link_resolution_context,
            Local::now(),
            ctx,
        );
        id
    }

    /// Create a document from an existing local AI document id and content.
    ///
    /// openWarp localization: the original `create_document_from_notebook` depended on the cloud notebook to provide
    /// title + sync_id; the local path only needs id + content, which is used only for placeholder reconstruction
    /// during "session restoration when the plan is not in memory".
    pub fn create_document_with_id(
        &mut self,
        ai_document_id: AIDocumentId,
        title: impl Into<String>,
        content: impl Into<String>,
        conversation_id: AIConversationId,
        file_link_resolution_context: Option<FileLinkResolutionContext>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_document_internal(
            ai_document_id,
            title,
            content,
            AIDocumentUpdateSource::Restoration,
            conversation_id,
            file_link_resolution_context,
            Local::now(),
            ctx,
        );
    }

    fn create_document_internal(
        &mut self,
        id: AIDocumentId,
        title: impl Into<String>,
        content: impl Into<String>,
        source: AIDocumentUpdateSource,
        conversation_id: AIConversationId,
        file_link_resolution_context: Option<FileLinkResolutionContext>,
        created_at: DateTime<Local>,
        ctx: &mut ModelContext<Self>,
    ) {
        let editor = Self::create_editor_model(content, file_link_resolution_context, ctx);

        // Subscribe to editor content changes
        ctx.subscribe_to_model(&editor, move |me, event, ctx| {
            me.handle_editor_event(&id, event, ctx);
        });

        let doc = AIDocument {
            title: title.into(),
            version: AIDocumentVersion::default(),
            editor,
            user_edit_status: AIDocumentUserEditStatus::UpToDate,
            conversation_id,
            created_at,
            restored_from: None,
            visible_in_pane_groups: HashSet::new(),
        };
        self.latest_document_id_by_conversation_id
            .insert(conversation_id, id);
        self.documents.insert(id, doc);
        // Emit event for document creation
        ctx.emit(AIDocumentModelEvent::DocumentUpdated {
            document_id: id,
            version: AIDocumentVersion::default(),
            source,
        });
        // Enqueue a save so the initial content is persisted to SQLite.
        // The editor subscription misses the first ContentChanged from
        // create_editor_model because it fires before the subscription is
        // wired up.
        self.enqueue_save(&id);
    }

    /// Returns an existing streaming document for a CreateDocuments tool call, or creates one.
    /// Returns true for the boolean return value if a new document was created.
    ///
    /// This is keyed by (conversation_id, action_id, document_index) so that streaming updates
    /// for the same tool call map to the same document.
    pub fn get_or_create_streaming_document_for_create_documents(
        &mut self,
        conversation_id: AIConversationId,
        action_id: &AIAgentActionId,
        document_index: usize,
        title: impl Into<String>,
        initial_content: impl Into<String>,
        file_link_resolution_context: Option<FileLinkResolutionContext>,
        ctx: &mut ModelContext<Self>,
    ) -> (AIDocumentId, bool) {
        let key = (conversation_id, action_id.clone(), document_index);
        if let Some(existing_id) = self.streaming_create_documents.get(&key) {
            return (*existing_id, false);
        }

        let id = AIDocumentId::new();
        self.create_document_internal(
            id,
            title,
            initial_content,
            AIDocumentUpdateSource::Agent,
            conversation_id,
            file_link_resolution_context,
            Local::now(),
            ctx,
        );
        self.streaming_create_documents.insert(key, id);
        (id, true)
    }

    /// Apply a streamed agent-origin update to the given document's content.
    ///
    /// This is used for incremental tool call updates and does not modify the
    /// user edit status.
    pub fn apply_streamed_agent_update(
        &mut self,
        id: &AIDocumentId,
        new_title: &str,
        new_content: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };

        doc.title = new_title.to_owned();
        let editor_handle = doc.editor.clone();
        editor_handle.update(ctx, |editor, editor_ctx| {
            editor.update_to_new_markdown(&post_process_notebook(new_content), editor_ctx);
        });

        ctx.emit(AIDocumentModelEvent::DocumentUpdated {
            document_id: *id,
            version: doc.version,
            source: AIDocumentUpdateSource::Agent,
        });
    }

    pub fn is_document_creation_streaming(&self, id: &AIDocumentId) -> bool {
        self.streaming_create_documents
            .values()
            .any(|doc_id| *doc_id == *id)
    }

    /// Returns the streaming document ID for a CreateDocuments tool call document, if any.
    pub fn streaming_document_id_for_create_documents(
        &self,
        conversation_id: &AIConversationId,
        action_id: &AIAgentActionId,
        document_index: usize,
    ) -> Option<AIDocumentId> {
        self.streaming_create_documents
            .get(&(*conversation_id, action_id.clone(), document_index))
            .copied()
    }

    /// Clears all streaming document mappings for the given CreateDocuments action.
    pub fn clear_streaming_documents_for_action(
        &mut self,
        conversation_id: &AIConversationId,
        action_id: &AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.streaming_create_documents
            .retain(|(conv_id, act_id, _), _| conv_id != conversation_id || act_id != action_id);
        ctx.emit(AIDocumentModelEvent::StreamingDocumentsCleared(
            *conversation_id,
        ));
    }

    pub fn clear_streaming_documents_for_conversation(
        &mut self,
        conversation_id: &AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.streaming_create_documents
            .retain(|(conv_id, _, _), _| conv_id != conversation_id);
        ctx.emit(AIDocumentModelEvent::StreamingDocumentsCleared(
            *conversation_id,
        ));
    }

    /// Get a copy of the current document by id.
    pub fn get_current_document(&self, id: &AIDocumentId) -> Option<AIDocument> {
        self.documents.get(id).cloned()
    }

    /// Deletes the given document and its version history.
    pub fn delete_document(&mut self, id: &AIDocumentId) {
        self.documents.remove(id);
        self.earlier_versions.remove(id);
    }

    pub fn get_document_id_by_conversation_id(&self, id: AIConversationId) -> Option<AIDocumentId> {
        self.latest_document_id_by_conversation_id.get(&id).cloned()
    }

    /// Get all documents for a given conversation, sorted by `created_at` ascending
    /// (oldest first, most recent last).
    pub fn get_all_documents_for_conversation(
        &self,
        conversation_id: AIConversationId,
    ) -> Vec<(AIDocumentId, AIDocument)> {
        let mut docs: Vec<_> = self
            .documents
            .iter()
            .filter(|(_, doc)| doc.conversation_id == conversation_id)
            .map(|(id, doc)| (*id, doc.clone()))
            .collect();
        docs.sort_by_key(|(_, doc)| doc.created_at);
        docs
    }

    pub fn get_conversation_id_for_document_id(
        &self,
        id: &AIDocumentId,
    ) -> Option<AIConversationId> {
        self.documents.get(id).map(|doc| doc.conversation_id)
    }

    fn get_current_document_mut(&mut self, id: &AIDocumentId) -> Option<&mut AIDocument> {
        self.documents.get_mut(id)
    }

    /// Set the user edit status for a document.
    /// Returns whether the document was found.
    pub fn set_user_edit_status(
        &mut self,
        id: &AIDocumentId,
        status: AIDocumentUserEditStatus,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        if let Some(doc) = self.get_current_document_mut(id) {
            doc.user_edit_status = status;
            ctx.emit(AIDocumentModelEvent::DocumentUserEditStatusUpdated {
                document_id: *id,
                status,
            });
            true
        } else {
            false
        }
    }

    /// Mark a document as visible (or not) in the given pane group.
    pub fn set_document_visible(
        &mut self,
        id: &AIDocumentId,
        pane_group_id: EntityId,
        is_visible: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(doc) = self.get_current_document_mut(id) else {
            return;
        };
        let changed = if is_visible {
            doc.visible_in_pane_groups.insert(pane_group_id)
        } else {
            doc.visible_in_pane_groups.remove(&pane_group_id)
        };
        if changed {
            ctx.emit(AIDocumentModelEvent::DocumentVisibilityChanged(*id));
        }
    }

    /// Check if any document for the given conversation is visible in the specified pane group.
    pub fn is_document_visible_by_conversation_in_pane_group(
        &self,
        conversation_id: &AIConversationId,
        pane_group_id: EntityId,
    ) -> bool {
        self.documents.values().any(|doc| {
            doc.conversation_id == *conversation_id
                && doc.visible_in_pane_groups.contains(&pane_group_id)
        })
    }

    /// Check if any document for the given conversation is visible in any pane group.
    pub fn is_document_visible_by_conversation(&self, conversation_id: &AIConversationId) -> bool {
        self.documents.values().any(|doc| {
            doc.conversation_id == *conversation_id && !doc.visible_in_pane_groups.is_empty()
        })
    }

    /// Check if a specific document is visible in any pane group.
    pub fn is_document_visible(&self, document_id: &AIDocumentId) -> bool {
        self.documents
            .get(document_id)
            .is_some_and(|doc| !doc.visible_in_pane_groups.is_empty())
    }

    /// Get a copy of a document by id and version.
    /// Could be the current document if the version is the latest version, or an earlier version if the version is older.
    pub fn get_document(
        &self,
        id: &AIDocumentId,
        version: AIDocumentVersion,
    ) -> Option<AIDocumentInstance> {
        let current_document = self.get_current_document(id)?;
        if current_document.version == version {
            Some(AIDocumentInstance::Current(current_document))
        } else {
            let earlier_versions = self.earlier_versions.get(id)?;
            Some(AIDocumentInstance::Earlier(
                earlier_versions
                    .iter()
                    .find(|v| v.version == version)?
                    .clone(),
            ))
        }
    }

    /// Get the raw markdown content of a document by id.
    pub fn get_document_content(
        &self,
        id: &AIDocumentId,
        ctx: &warpui::AppContext,
    ) -> Option<String> {
        let doc = self.documents.get(id)?;
        Some(doc.editor.as_ref(ctx).markdown_unescaped(ctx))
    }

    /// Apply persisted content from SQLite on top of conversation-restored content.
    /// If the persisted content differs from the current editor content, update the
    /// editor and mark the document as Dirty so it gets attached to the next query.
    /// If the document doesn't exist (conversation wasn't restored), create it with
    /// the persisted content.
    pub fn apply_persisted_content(
        &mut self,
        id: AIDocumentId,
        persisted_content: &str,
        persisted_title: Option<&str>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(doc) = self.documents.get_mut(&id) else {
            // Document doesn't exist from conversation restoration.
            // Create it from the persisted content so the pane has something
            // to display. Use a synthetic conversation ID since the original
            // conversation wasn't restored.
            log::info!(
                "Creating document {id} from persisted SQLite content (conversation not restored)"
            );
            let title = persisted_title.unwrap_or(DEFAULT_PLANNING_DOCUMENT_TITLE);
            self.create_document_internal(
                id,
                title,
                persisted_content,
                AIDocumentUpdateSource::Restoration,
                // We don't have the conversation ID this is for - this is free floating and not connected to any conversation
                // so create a random one.
                AIConversationId::new(),
                None,
                Local::now(),
                ctx,
            );
            return;
        };

        let current_content = doc.editor.as_ref(ctx).markdown_unescaped(ctx);
        if current_content == persisted_content {
            log::info!(
                "Persisted SQLite content for document {id} is the same as the current content"
            );
            return;
        }

        log::info!("Applying persisted SQLite content for document {id} (content differs from conversation restoration)");
        doc.editor.update(ctx, |editor, editor_ctx| {
            let processed = post_process_notebook(persisted_content);
            editor.reset_with_markdown(&processed, editor_ctx);
        });

        // Mark as dirty so the updated plan is attached to the next agent query
        doc.user_edit_status = AIDocumentUserEditStatus::Dirty;
        let version = doc.version;
        ctx.emit(AIDocumentModelEvent::DocumentUpdated {
            document_id: id,
            version,
            source: AIDocumentUpdateSource::Restoration,
        });
        ctx.emit(AIDocumentModelEvent::DocumentUserEditStatusUpdated {
            document_id: id,
            status: AIDocumentUserEditStatus::Dirty,
        });
    }

    /// Update the title of a document.
    pub fn update_title(
        &mut self,
        id: &AIDocumentId,
        new_title: impl Into<String>,
        source: AIDocumentUpdateSource,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Some(doc) = self.documents.get_mut(id) {
            doc.title = new_title.into();
            ctx.emit(AIDocumentModelEvent::DocumentUpdated {
                document_id: *id,
                version: doc.version,
                source,
            });
        }
    }

    /// Create a new, unbound editor model with the given content.
    fn create_editor_model(
        content: impl Into<String>,
        file_link_resolution_context: Option<FileLinkResolutionContext>,
        ctx: &mut ModelContext<Self>,
    ) -> ModelHandle<NotebooksEditorModel> {
        ctx.add_model(|ctx| {
            // Get appearance and font settings from the app context
            let appearance = Appearance::as_ref(ctx);
            let font_settings = FontSettings::as_ref(ctx);
            // Use the same rich text styles as notebooks for consistency
            let styles = rich_text_styles(appearance, font_settings);

            let mut model = NotebooksEditorModel::new_unbound(styles, ctx);
            model.set_file_link_resolution_context(file_link_resolution_context);

            let content = content.into();
            if !content.is_empty() {
                // Post-process the content to remove extra newlines
                let processed_content = post_process_notebook(&content);
                model.reset_with_markdown(&processed_content, ctx);
            }
            model
        })
    }

    /// Save the current document state as a version and prepare for a new version.
    /// Returns a mutable reference to the document if successful.
    fn create_new_document_version(
        &mut self,
        id: &AIDocumentId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<&mut AIDocument> {
        let doc = self.documents.get_mut(id)?;

        // Create new editor instance to avoid persisting updates to older versions
        // Preserve the file link resolution context from the current editor, if any.
        let file_link_resolution_context = doc
            .editor
            .as_ref(ctx)
            .file_link_resolution_context()
            .cloned();
        let editor = Self::create_editor_model(
            doc.editor.as_ref(ctx).markdown_unescaped(ctx),
            file_link_resolution_context,
            ctx,
        );

        let earlier_version = AIDocumentEarlierVersion {
            title: doc.title.clone(),
            version: doc.version,
            editor,
            created_at: doc.created_at,
            restored_from: doc.restored_from,
        };

        self.earlier_versions
            .entry(*id)
            .or_insert_with(Vec::new)
            .push(earlier_version);

        doc.version = doc.version.next();
        doc.created_at = Local::now();
        doc.restored_from = None;

        Some(doc)
    }

    /// Apply diffs to a document.
    pub fn create_new_version_and_apply_diffs(
        &mut self,
        id: &AIDocumentId,
        diffs: Vec<DiffDelta>,
        source: AIDocumentUpdateSource,
        ctx: &mut ModelContext<Self>,
    ) -> Option<AIDocumentVersion> {
        if let Some(doc) = self.create_new_document_version(id, ctx) {
            doc.editor.update(ctx, |editor, editor_ctx| {
                editor.apply_diffs(diffs, editor_ctx);
            });
            let version = doc.version;
            // Agent edits create a new version, so they will be persisted by the sqlite writer
            ctx.emit(AIDocumentModelEvent::DocumentUpdated {
                document_id: *id,
                version,
                source,
            });
            // openWarp localization: no longer push new version content to the cloud notebook, all go to local SQLite.
            Some(version)
        } else {
            None
        }
    }

    /// Restore the initial version of a document.
    pub fn restore_document(
        &mut self,
        id: AIDocumentId,
        conversation_id: AIConversationId,
        title: impl Into<String>,
        content: impl Into<String>,
        created_at: DateTime<Local>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_document_internal(
            id,
            title,
            content,
            AIDocumentUpdateSource::Restoration,
            conversation_id,
            None,
            created_at,
            ctx,
        );

        // openWarp localization: plans are only stored in local SQLite, no longer querying sync_id from the cloud notebook.
    }

    /// This is used for restoring EditDocuments results where we already have the final content.
    /// Creates a new version of the document with directly-provided content.
    /// This expects `restore_document` to have already been called.
    pub fn restore_document_edit(
        &mut self,
        id: &AIDocumentId,
        new_content: impl Into<String>,
        created_at: DateTime<Local>,
        ctx: &mut ModelContext<Self>,
    ) -> Option<AIDocumentVersion> {
        if let Some(doc) = self.create_new_document_version(id, ctx) {
            let content = new_content.into();
            doc.editor.update(ctx, |editor, editor_ctx| {
                let processed_content = post_process_notebook(&content);
                editor.reset_with_markdown(&processed_content, editor_ctx);
            });
            doc.created_at = created_at;
            ctx.emit(AIDocumentModelEvent::DocumentUpdated {
                document_id: *id,
                version: doc.version,
                source: AIDocumentUpdateSource::Restoration,
            });
            Some(doc.version)
        } else {
            None
        }
    }

    /// Handle editor model events and enqueue saves for content changes.
    fn handle_editor_event(
        &mut self,
        document_id: &AIDocumentId,
        event: &RichTextEditorModelEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if let RichTextEditorModelEvent::ContentChanged(edit_origin) = event {
            self.enqueue_save(document_id);
            // Mark document as Dirty on user edit
            if edit_origin.from_user() {
                if let Some(doc) = self.documents.get_mut(document_id) {
                    if !doc.user_edit_status.is_dirty() {
                        doc.user_edit_status = AIDocumentUserEditStatus::Dirty;
                        ctx.emit(AIDocumentModelEvent::DocumentUserEditStatusUpdated {
                            document_id: *document_id,
                            status: AIDocumentUserEditStatus::Dirty,
                        });
                    }
                }
            }
        }
    }

    /// Enqueue a save for the given document.
    fn enqueue_save(&mut self, document_id: &AIDocumentId) {
        self.content_dirty_flags.insert(*document_id, true);
        if let Err(e) = self.save_tx.try_send(AIDocumentSaveRequest {
            document_id: *document_id,
        }) {
            log::error!("Error enqueueing content save for {}: {}", document_id, e);
        }
    }

    /// Handle save requests from the throttled channel.
    fn handle_save_request(
        &mut self,
        request: AIDocumentSaveRequest,
        ctx: &mut ModelContext<Self>,
    ) {
        if self
            .content_dirty_flags
            .get(&request.document_id)
            .copied()
            .unwrap_or(false)
        {
            // openWarp localization: no longer push content to the cloud notebook, only write to local SQLite.
            self.persist_content_to_sqlite(&request.document_id, ctx);
            self.content_dirty_flags.insert(request.document_id, false);
        }
    }

    /// Persist the current document content to SQLite for session restoration.
    fn persist_content_to_sqlite(&self, id: &AIDocumentId, ctx: &mut ModelContext<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let Some(sender) = GlobalResourceHandlesProvider::as_ref(ctx)
            .get()
            .model_event_sender
            .clone()
        else {
            return;
        };
        let content = doc.editor.as_ref(ctx).markdown_unescaped(ctx);
        let event = ModelEvent::SaveAIDocumentContent {
            document_id: id.to_string(),
            content,
            version: doc.version.0 as i32,
            title: doc.title.clone(),
        };
        if let Err(err) = sender.try_send(event) {
            log::error!("Error persisting AI document content for {id}: {err}");
        }
    }

    /// Get a specific version of a document by version.
    pub fn get_earlier_document_version(
        &self,
        id: &AIDocumentId,
        version: AIDocumentVersion,
    ) -> Option<&AIDocumentEarlierVersion> {
        self.earlier_versions
            .get(id)?
            .iter()
            .find(|v| v.version == version)
    }

    /// Get all earlier versions of a document.
    pub fn get_earlier_document_versions(
        &self,
        id: &AIDocumentId,
    ) -> Option<&Vec<AIDocumentEarlierVersion>> {
        self.earlier_versions.get(id)
    }

    /// Restore a document to a previous version, creating a new version in the process.
    /// Returns the new version number on success.
    pub fn revert_to_document_version(
        &mut self,
        id: &AIDocumentId,
        target_version: AIDocumentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<AIDocumentVersion, anyhow::Error> {
        // Find the target version
        let target_version_data = self
            .get_earlier_document_version(id, target_version)
            .ok_or_else(|| {
                anyhow::anyhow!("Version {} not found for document {}", target_version, id)
            })?;

        let title = target_version_data.title.clone();
        let content = target_version_data.editor.as_ref(ctx).markdown(ctx);

        // Save current state as a version and prepare for new version
        let doc = self
            .create_new_document_version(id, ctx)
            .ok_or_else(|| anyhow::anyhow!("Document {} not found", id))?;

        // Restore the document to the target version's content and title
        doc.title = title;
        doc.editor.update(ctx, |editor, editor_ctx| {
            editor.reset_with_markdown(&content, editor_ctx);
        });

        // Track which version this was restored from
        doc.restored_from = Some(target_version);

        // Mark document as Dirty so the updated plan will be attached on the next query
        doc.user_edit_status = AIDocumentUserEditStatus::Dirty;

        ctx.emit(AIDocumentModelEvent::DocumentUpdated {
            document_id: *id,
            version: doc.version,
            source: AIDocumentUpdateSource::User,
        });
        ctx.emit(AIDocumentModelEvent::DocumentUserEditStatusUpdated {
            document_id: *id,
            status: AIDocumentUserEditStatus::Dirty,
        });

        Ok(doc.version)
    }
}

impl AIDocumentEarlierVersion {
    pub fn get_content(&self, ctx: &warpui::AppContext) -> String {
        self.editor.as_ref(ctx).markdown_unescaped(ctx)
    }
}

pub enum AIDocumentModelEvent {
    /// Emitted when a document is created or updated.
    /// If the agent made an update that created a new version, this will be emitted
    /// with the new version number.
    /// If the user makes updates to the latest (current) version, this will be emitted
    /// repeatedly with the same latest (current) version.
    DocumentUpdated {
        document_id: AIDocumentId,
        version: AIDocumentVersion,
        source: AIDocumentUpdateSource,
    },
    /// When the AI Document has progressed from NotSaved -> Saving -> Saved
    DocumentSaveStatusUpdated(AIDocumentId),
    /// When the user edit status of a document changes
    DocumentUserEditStatusUpdated {
        document_id: AIDocumentId,
        status: AIDocumentUserEditStatus,
    },
    /// When streaming documents for a conversation are cleared
    StreamingDocumentsCleared(AIConversationId),
    DocumentVisibilityChanged(AIDocumentId),
}

impl Entity for AIDocumentModel {
    type Event = AIDocumentModelEvent;
}

impl SingletonEntity for AIDocumentModel {}

#[cfg(test)]
#[path = "ai_document_model_tests.rs"]
mod tests;
