#[cfg(not(target_family = "wasm"))]
use crate::ai::mcp::templatable::{TemplatableMCPServer, TemplatableMCPServerObjectModel};
use crate::{
    ai::{
        execution_profiles::{AIExecutionProfile, AIExecutionProfileObjectModel},
        facts::{AIFact, AIFactObjectModel},
    },
    auth::TEST_USER_UID,
    cloud_object::{
        model::{
            actions::{ObjectAction, ObjectActionHistory, ObjectActionType, ObjectActions},
            generic_string_model::{
                GenericStringModel, GenericStringObjectId, Serializer, StringModel,
            },
            persistence::{ObjectStoreEvent, ObjectStoreModel, UpdateSource},
            view::{Editor, EditorState, ObjectStoreViewModel},
        },
        GenericStoredObject, GenericStringObjectFormat, JsonObjectType, ObjectIdType, ObjectType,
        Owner, Revision, Space, StoredObject, StoredObjectEventEntrypoint, StoredObjectLocation,
        StoredObjectModel,
    },
    drive::{
        folders::{FolderId, FolderObjectModel},
        ObjectTypeAndId,
    },
    env_vars::{EnvVarCollection, EnvVarCollectionObjectModel},
    notebooks::{NotebookId, NotebookObjectModel},
    persistence::ModelEvent,
    server::ids::{ClientId, HashableId, ObjectUid, ServerId, SyncId, ToServerId},
    server_time::ServerTimestamp,
    settings::cloud_preferences::Preference,
    workflows::{
        workflow::Workflow,
        workflow_enum::{WorkflowEnum, WorkflowEnumObject, WorkflowEnumObjectModel},
        WorkflowId, WorkflowObjectModel,
    },
    workspaces::user_workspaces::UserWorkspaces,
};
use chrono::{DateTime, Utc};
use futures::channel::oneshot::{self, Receiver};
use itertools::Itertools;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::sync::{mpsc::SyncSender, Arc};
use warpui::r#async::FutureId;
use warpui::AppContext;
use warpui::{Entity, ModelContext, SingletonEntity};

lazy_static! {
    static ref DUPLICATE_OBJECT_NAME_REGEX: Regex =
        Regex::new(r" \((\d+)\)$").expect("regex should not fail to compile");
}

#[derive(Debug, PartialEq)]
pub enum OperationSuccessType {
    Success,
    Failure,
    Rejection,
    Denied(String),
    FeatureNotAvailable,
}

#[derive(Debug, PartialEq)]
pub enum ObjectOperation {
    Create { initiated_by: InitiatedBy },
    Update,
    MoveToFolder,
    MoveToDrive,
    Trash,
    TakeEditAccess,
    Untrash,
    Delete { initiated_by: InitiatedBy },
    EmptyTrash,
    UpdatePermissions,
    Leave,
}

#[derive(Debug)]
pub struct ObjectOperationResult {
    pub success_type: OperationSuccessType,
    pub operation: ObjectOperation,
    pub client_id: Option<ClientId>,
    pub server_id: Option<ServerId>,
    pub num_objects: Option<i32>, // counts number of objects (including descendants) deleted for permadeletion
}

#[derive(Debug)]
pub enum UpdateManagerEvent {
    ObjectOperationComplete { result: ObjectOperationResult },
    PreferencesUpdated { updated: Vec<Preference> },
    AmbientTaskUpdated { timestamp: DateTime<Utc> },
}

/// An enum for choosing the behavior of the fetch_single_cloud_object function.
pub enum FetchSingleObjectOption {
    /// Perform the normal upsert behavior.
    None,
    /// Perform the normal upsert behavior, but additionally force overwrite the
    /// in-memory object to whatever the server object is.
    ForceOverwrite,
    /// Only perform the normal upsert behavior if the object doesn't already
    /// exist in-memory.
    IgnoreIfExists,
}

/// An enum that defines whether the action was initiated by the user or the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiatedBy {
    User,
    System,
}
#[derive(Debug)]
pub struct GenericStringObjectInput<T, S>
where
    T: StringModel<
            StoredObjectType = GenericStoredObject<GenericStringObjectId, GenericStringModel<T, S>>,
        > + 'static,
    S: Serializer<T> + 'static,
{
    pub id: ClientId,
    pub model: GenericStringModel<T, S>,
    pub initial_folder_id: Option<SyncId>,
    pub entrypoint: StoredObjectEventEntrypoint,
}

/// The UpdateManager is responsible for delegating work
/// when there is an update to an object (e.g. via a user interaction or
/// a message from the server). Specifically, it will
/// - write to SQLite
/// - interact with the ObjectStoreModel to update the in-memory state used by the object views
/// - interact with the SyncQueue by enqueueing an event
pub struct UpdateManager {
    model_event_sender: Option<SyncSender<ModelEvent>>,
    spawned_futures: Vec<FutureId>,
}

impl UpdateManager {
    pub fn new(
        model_event_sender: Option<SyncSender<ModelEvent>>,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        Self {
            model_event_sender,
            spawned_futures: Default::default(),
        }
    }

    #[cfg(test)]
    pub fn mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(None, ctx)
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn spawned_futures(&self) -> &[FutureId] {
        &self.spawned_futures
    }

    fn save_to_db(&self, events: impl IntoIterator<Item = ModelEvent>) {
        let model_event_sender = self.model_event_sender.clone();
        if let Some(model_event_sender) = &model_event_sender {
            for event in events {
                if let Err(e) = model_event_sender.send(event) {
                    log::error!("Error saving to database: {e:?}");
                }
            }
        }
    }

    /// Remove team-owned objects in response to leaving a team.
    pub fn remove_team_objects(&mut self, left_team_uid: ServerId, ctx: &mut ModelContext<Self>) {
        let object_store_model = ObjectStoreModel::handle(ctx);
        let objects_to_remove = object_store_model
            .as_ref(ctx)
            .all_cloud_objects_in_space(
                Space::Team {
                    team_uid: left_team_uid,
                },
                ctx,
            )
            .map(|object| object.object_type_and_id())
            .collect_vec();

        // First, delete in-memory from ObjectStoreModel and object actions.
        object_store_model.update(ctx, |object_store_model, ctx| {
            for object in objects_to_remove.iter() {
                object_store_model.delete_object(object.sync_id(), ctx);
            }
        });
        ObjectActions::handle(ctx).update(ctx, |object_actions, ctx| {
            for object in objects_to_remove.iter() {
                object_actions.delete_actions_for_object(&object.uid(), ctx);
            }
        });

        // Then, delete from SQLite.
        let object_ids_and_types = objects_to_remove
            .into_iter()
            .map(|object| (object.sync_id(), object.object_id_type()))
            .collect();
        self.save_to_db([ModelEvent::DeleteObjects {
            ids: object_ids_and_types,
        }]);
    }

    pub fn resync_object(
        &mut self,
        object_type_and_id: &ObjectTypeAndId,
        ctx: &mut ModelContext<Self>,
    ) {
        // Waz (Wave 4): resync originally meant "re-entering the SyncQueue to push local changes to the server".
        // After localization, this is itself a one-way SQLite write, and the calling point only needs a lightweight check.
        let _ = (object_type_and_id, ctx);
    }

    /// Out-of-band (from the regular poll) refresh of updated objects.
    pub fn refresh_updated_objects(&mut self, ctx: &mut ModelContext<Self>) {
        // Waz localization: currently there is no cloud object polling source.
        // Keep this method only to be compatible with old calling points, without triggering network I/O.
        let _ = ctx;
    }

    fn save_in_memory_object_to_sqlite(
        &mut self,
        object_store_model: &ObjectStoreModel,
        uid: &ObjectUid,
    ) {
        if let Some(cloud_object) = object_store_model.get_by_uid(uid) {
            self.save_to_db([cloud_object.upsert_event()]);
        }
    }

    fn save_in_memory_object_metadata_to_sqlite(
        &mut self,
        object_store_model: &ObjectStoreModel,
        uid: &ObjectUid,
        hashed_sqlite_id: &str,
    ) {
        if let Some(cloud_object) = object_store_model.get_by_uid(uid) {
            let metadata = cloud_object.metadata().clone();
            let event = ModelEvent::UpdateObjectMetadata {
                id: hashed_sqlite_id.to_string(),
                metadata,
            };
            self.save_to_db([event]);
        }
    }

    /// The Waz local version no longer fetches single cloud objects; the signature is preserved only for compatibility with old call sites.
    ///
    /// Returns A `Receiver<()>` that completes when the fetch operation is done.
    /// This receiver can be used to wait for the fetch operation to complete before proceeding.
    pub fn fetch_single_cloud_object(
        &mut self,
        server_id: &ServerId,
        fetch_single_object_option: FetchSingleObjectOption,
        ctx: &mut ModelContext<Self>,
    ) -> Receiver<()> {
        let _ = fetch_single_object_option;
        let _ = ctx;
        let (fetch_cloud_object_tx, fetch_cloud_object_rx) = oneshot::channel::<()>();
        log::debug!("Waz skips single object fetch from cloud: {server_id:?}");
        let _ = fetch_cloud_object_tx.send(());
        fetch_cloud_object_rx
    }

    /// Replace an object's data with its conflicting version. If the object does not have a
    /// conflict, this has no effect.
    pub fn replace_object_with_conflict(&mut self, uid: &ObjectUid, ctx: &mut ModelContext<Self>) {
        let object_store_model_handle = ObjectStoreModel::handle(ctx);

        // Update the in-memory model first, and check for conflicts.
        let had_conflicts =
            object_store_model_handle.update(
                ctx,
                |object_store_model, ctx| match object_store_model.get_mut_by_uid(uid) {
                    Some(object) if object.has_conflicting_changes() => {
                        object.replace_object_with_conflict();
                        ctx.emit(ObjectStoreEvent::ObjectUpdated {
                            type_and_id: object.object_type_and_id(),
                            source: UpdateSource::External,
                        });
                        true
                    }
                    _ => false,
                },
            );

        // Update SQLite, but only if the in-memory model was updated.
        if had_conflicts {
            self.save_in_memory_object_to_sqlite(object_store_model_handle.as_ref(ctx), uid);
        }
    }

    pub fn update_ai_fact(
        &mut self,
        ai_fact: AIFact,
        ai_fact_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            AIFactObjectModel::new(ai_fact),
            ai_fact_id,
            revision_ts,
            ctx,
        );
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn update_templatable_mcp_server(
        &mut self,
        templatable_mcp_server: TemplatableMCPServer,
        templatable_mcp_server_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            TemplatableMCPServerObjectModel::new(templatable_mcp_server),
            templatable_mcp_server_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_workflow(
        &mut self,
        workflow: Workflow,
        workflow_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            WorkflowObjectModel::new(workflow),
            workflow_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_workflow_enum(
        &mut self,
        workflow_enum: WorkflowEnum,
        workflow_enum_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            WorkflowEnumObjectModel::new(workflow_enum),
            workflow_enum_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_env_var_collection(
        &mut self,
        env_var_collection: EnvVarCollection,
        env_var_collection_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            EnvVarCollectionObjectModel::new(env_var_collection),
            env_var_collection_id,
            revision_ts,
            ctx,
        );
    }

    pub fn update_notebook_data(
        &mut self,
        data: Arc<String>,
        notebook_id: SyncId,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&notebook_id).cloned();
        if let Some(notebook) = object_store_model.get_notebook(&notebook_id) {
            let new_notebook = NotebookObjectModel {
                title: notebook.model().title.to_owned(),
                data: data.to_string(),
                ai_document_id: notebook.model().ai_document_id,
                conversation_id: notebook.model().conversation_id.clone(),
            };
            self.update_object(new_notebook, notebook_id, revision, ctx);
        } else {
            log::warn!("Expected notebook to be in model with id {notebook_id:?}");
        }
    }

    pub fn update_notebook_title(
        &mut self,
        title: Arc<String>,
        notebook_id: SyncId,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&notebook_id).cloned();
        if let Some(notebook) = object_store_model.get_notebook(&notebook_id) {
            let new_notebook = NotebookObjectModel {
                title: title.to_string(),
                data: notebook.model().data.to_owned(),
                ai_document_id: notebook.model().ai_document_id,
                conversation_id: notebook.model().conversation_id.clone(),
            };
            self.update_object(new_notebook, notebook_id, revision, ctx);
        } else {
            log::warn!("Expected notebook to be in model with id {notebook_id:?}");
        }
    }

    /// Attempts to move the object identified by `object_id`
    /// to the folder identified by `folder_id`, then persists the local metadata
    /// changes in sqlite.
    #[allow(clippy::too_many_arguments)]
    fn move_object_to_folder(
        &mut self,
        server_id: ServerId,
        object_type: ObjectType,
        owner: Owner,
        destination_folder: Option<FolderId>,
        _current_folder: Option<SyncId>,
        _current_metadata_last_updated_ts: Option<ServerTimestamp>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Waz: The cloud move RPC has been deleted. This is folded here into a local direct write and clears
        // the has_pending_metadata_change bit.
        let _ = (object_type, owner, destination_folder);
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(obj) = object_store_model.get_mut_by_uid(&server_id.uid()) {
                obj.metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
            }
            ctx.notify();
        });
        self.save_in_memory_object_to_sqlite(ObjectStoreModel::as_ref(ctx), &server_id.uid());
        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::MoveToFolder,
                client_id: None,
                server_id: Some(server_id),
                num_objects: None,
            },
        });
        ctx.notify();
    }

    /// Attempts to move the object identified by `object_id`
    /// to the root of the drive identified by `destination_owner`.
    /// Waz (Wave 6-7): The original remote leg called the `transfer_*_owner` series of stubs, always returning `Ok(true)`.
    /// After running once, it clears `has_pending_permissions_change` + emits a Success toast. This is folded here into a local direct write.
    /// `move_object_to_drive_failed` / `revert_workflow_on_failed_move` are retired accordingly.
    #[allow(clippy::too_many_arguments)]
    fn move_object_to_drive(
        &mut self,
        server_id: ServerId,
        object_type: ObjectType,
        destination_owner: Owner,
        _current_folder: Option<SyncId>,
        _current_owner: Owner,
        _current_permissions_last_updated_ts: Option<ServerTimestamp>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Copying workflow enums to the target owner locally still needs to be done — `update_object` /
        // `create_object` are both local stubs, and this call is a pure local model action.
        if object_type == ObjectType::Workflow {
            let _ = self.copy_workflow_enums_to_drive(server_id, destination_owner, ctx);
        }

        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(obj) = object_store_model.get_mut_by_uid(&server_id.uid()) {
                obj.metadata_mut()
                    .pending_changes_statuses
                    .has_pending_permissions_change = false;
            }
            ctx.notify();
        });
        self.save_in_memory_object_to_sqlite(ObjectStoreModel::as_ref(ctx), &server_id.uid());
        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::MoveToDrive,
                client_id: None,
                server_id: Some(server_id),
                num_objects: None,
            },
        });
        ctx.notify();
    }

    /// Given a workflow_id and a destination drive, make a copy of all referenced workflow enums in the destination drive.
    /// Returns the original workflow object if it was modified (in case a future revert is needed), otherwise returns None.
    fn copy_workflow_enums_to_drive(
        &mut self,
        server_id: ServerId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Workflow> {
        let workflow_id = SyncId::ServerId(server_id);
        let workflow = ObjectStoreModel::as_ref(ctx).get_workflow(&workflow_id);

        if let Some(workflow) = workflow {
            let original_workflow = workflow.model().data.clone();
            let mut workflow_model = original_workflow.clone();

            // Duplicate all enums associated with the workflow
            let enums = workflow_model.get_enum_ids();
            for enum_id in enums.iter() {
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                let object: Option<&WorkflowEnumObject> =
                    object_store_model.get_object_of_type(enum_id);
                let Some(object) = object else {
                    log::error!("Could not find referenced workflow enum to copy over to the new space, skipping");
                    continue;
                };

                let client_id = ClientId::new();

                // Create a duplicate enum in the new space with a new client ID
                self.create_object(
                    object.model().clone(),
                    owner,
                    client_id,
                    StoredObjectEventEntrypoint::Unknown,
                    true,
                    None,
                    // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
                    // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
                    InitiatedBy::User,
                    ctx,
                );

                workflow_model.replace_object_id(*enum_id, SyncId::ClientId(client_id));
            }

            // Update the workflow with the new enum IDs, if there are any
            if !enums.is_empty() {
                self.update_workflow(workflow_model, workflow_id, None, ctx);
                Some(original_workflow)
            } else {
                None
            }
        } else {
            log::error!(
                "Tried to move workflow enums to new space but could not find associated workflow",
            );
            None
        }
    }

    // This method moves an object from its current location to a new location.
    // Since moving is an online-only operation, this operation does NOT go through the sync queue.
    pub fn move_object_to_location(
        &mut self,
        object_id: ObjectTypeAndId,
        new_location: StoredObjectLocation,
        ctx: &mut ModelContext<Self>,
    ) {
        // If we are moving into the trash, we really mean to trash the object
        if let StoredObjectLocation::Trash = new_location {
            return self.trash_object(object_id, ctx);
        }

        // A move operation does not make sense offline,
        // so early return if we don't have a server ID for whatever reason.
        let uid = object_id.uid();
        let Some(server_id) = object_id.server_id() else {
            return;
        };

        let Some((
            object_current_owner,
            object_current_folder,
            object_type,
            has_pending_online_only_change,
            curr_metadata_ts,
            curr_permissions_ts,
        )) = ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
            let object = model.get_by_uid(&uid)?;
            Some((
                object.permissions().owner,
                object.metadata().folder_id,
                object.into(),
                object.metadata().has_pending_online_only_change(),
                object.metadata().metadata_last_updated_ts,
                object.permissions().permissions_last_updated_ts,
            ))
        })
        else {
            return;
        };

        // We disallow stacked online-only changes so early return
        // if there's already one pending for this object.
        if has_pending_online_only_change {
            return;
        }

        // Apply a pending, optimistic update and then try to sync the move with the server.
        // We only update the in-memory data but don't persist anything in sqlite until the server confirms the move.
        // Todo: this logic shouldn't need to match based on Space versus Folder. Once we have moving across spaces in MoveObject,
        // we should simplify this to a unified call to move_object that sends the new space AND the new folder.
        let mut not_supported = false;
        match new_location {
            StoredObjectLocation::Space(destination_space) => {
                match UserWorkspaces::as_ref(ctx).space_to_owner(destination_space, ctx) {
                    Some(destination_owner) => {
                        if destination_owner == object_current_owner {
                            // If the space is staying the same, then the move must be to move to the root of the space.
                            ObjectStoreModel::handle(ctx).update(ctx, |model, ctx| {
                                model.update_object_location(&uid, None, None, ctx);
                            });
                            self.move_object_to_folder(
                                server_id,
                                object_type,
                                object_current_owner,
                                None,
                                object_current_folder,
                                curr_metadata_ts,
                                ctx,
                            );
                        } else {
                            ObjectStoreModel::handle(ctx).update(ctx, |model, ctx| {
                                model.update_object_location(
                                    &uid,
                                    Some(destination_owner),
                                    None,
                                    ctx,
                                );
                            });
                            self.move_object_to_drive(
                                server_id,
                                object_type,
                                destination_owner,
                                object_current_folder,
                                object_current_owner,
                                curr_permissions_ts,
                                ctx,
                            );
                        }
                    }
                    None => {
                        // We couldn't map the space to a valid owner (most likely, it's the
                        // "shared" space).
                        not_supported = true;
                    }
                }
            }
            StoredObjectLocation::Folder(SyncId::ServerId(destination_folder_id)) => {
                // If we're moving across folders, then the space must be staying the same.
                ObjectStoreModel::handle(ctx).update(ctx, |model, ctx| {
                    model.update_object_location(
                        &uid,
                        None,
                        Some(SyncId::ServerId(destination_folder_id)),
                        ctx,
                    );
                });
                self.move_object_to_folder(
                    server_id,
                    object_type,
                    object_current_owner,
                    Some(destination_folder_id.into()),
                    object_current_folder,
                    curr_metadata_ts,
                    ctx,
                );
            }
            _ => {
                not_supported = true;
            }
        }

        // In all other cases, just immediately revert the optimistic update since
        // we won't be trying to move the object and we don't want the object to appear
        // as pending.
        if not_supported {
            ObjectStoreModel::handle(ctx).update(ctx, |model, ctx| {
                model.update_object_location(
                    &uid,
                    Some(object_current_owner),
                    object_current_folder,
                    ctx,
                );
            });
        }

        ctx.notify();
    }

    pub fn duplicate_object(
        &mut self,
        object_type_and_id: &ObjectTypeAndId,
        ctx: &mut ModelContext<Self>,
    ) {
        match object_type_and_id {
            ObjectTypeAndId::Notebook(notebook_id) => {
                self.duplicate_object_internal::<NotebookId, NotebookObjectModel>(notebook_id, ctx);
            }
            ObjectTypeAndId::Workflow(workflow_id) => {
                self.duplicate_object_internal::<WorkflowId, WorkflowObjectModel>(workflow_id, ctx);
            }
            ObjectTypeAndId::GenericStringObject { object_type, id } => {
                if let GenericStringObjectFormat::Json(JsonObjectType::EnvVarCollection) =
                    object_type
                {
                    self.duplicate_object_internal::<GenericStringObjectId, EnvVarCollectionObjectModel>(
                        id, ctx,
                    );
                } else {
                    log::error!("Tried to duplicate an unsupported type: json object");
                    debug_assert!(false, "Tried to duplicate an unsupported type: json object");
                }
            }
            ObjectTypeAndId::Folder(_) => {
                // Duplicating folders not currently supported.
                log::error!("Tried to duplicate an unsupported type: folder");
                debug_assert!(false, "Tried to duplicate an unsupported type: folder");
            }
        }
    }

    fn duplicate_object_internal<K, M>(&mut self, id: &SyncId, ctx: &mut ModelContext<Self>)
    where
        K: HashableId
            + ToServerId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        let (duplicate_model, client_id, owner, initial_folder_id, entrypoint) = {
            let object_store_model = ObjectStoreModel::as_ref(ctx);
            let object: GenericStoredObject<K, M> = object_store_model
                .get_object_of_type(id)
                .expect("object should exist in order to be duplicated")
                .clone();
            let client_id = ClientId::new();
            let owner = object.permissions.owner;
            let initial_folder_id = object.metadata.folder_id;
            let entrypoint = StoredObjectEventEntrypoint::Unknown;
            let mut duplicate_model = object.model().clone();
            let duplicate_name = self.get_next_duplicate_object_name(
                &object as &dyn StoredObject,
                object_store_model,
                ctx,
            );
            duplicate_model.set_display_name(&duplicate_name);
            (
                duplicate_model,
                client_id,
                owner,
                initial_folder_id,
                entrypoint,
            )
        };
        self.create_object(
            duplicate_model,
            owner,
            client_id,
            entrypoint,
            true,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    pub fn create_ai_fact(
        &mut self,
        ai_fact: AIFact,
        client_id: ClientId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            AIFactObjectModel::new(ai_fact),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn create_templatable_mcp_server(
        &mut self,
        templatable_mcp_server: TemplatableMCPServer,
        client_id: ClientId,
        owner: Owner,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            TemplatableMCPServerObjectModel::new(templatable_mcp_server),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            initiated_by,
            ctx,
        );
    }

    #[allow(dead_code)]
    pub fn create_ai_execution_profile(
        &mut self,
        ai_execution_profile: AIExecutionProfile,
        client_id: ClientId,
        owner: Owner,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            AIExecutionProfileObjectModel::new(ai_execution_profile),
            owner,
            client_id,
            Default::default(),
            false,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(dead_code)]
    pub fn update_ai_execution_profile(
        &mut self,
        ai_execution_profile: AIExecutionProfile,
        ai_execution_profile_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.update_object(
            AIExecutionProfileObjectModel::new(ai_execution_profile),
            ai_execution_profile_id,
            revision_ts,
            ctx,
        );
    }

    pub fn delete_ai_execution_profile(
        &mut self,
        ai_execution_profile_id: SyncId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.delete_object_by_user(
            ObjectTypeAndId::GenericStringObject {
                object_type: GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile),
                id: ai_execution_profile_id,
            },
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_notebook(
        &mut self,
        client_id: ClientId,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
        model: NotebookObjectModel,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            model,
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    fn get_next_duplicate_object_name(
        &self,
        original_cloud_object: &dyn StoredObject,
        object_store_model: &ObjectStoreModel,
        app: &AppContext,
    ) -> String {
        let original_name = original_cloud_object.display_name();

        // Iterate through items in the same folder as the original object that are of the
        // same type, and populate a hashset with those names.
        let same_type_and_folder_names = object_store_model
            .active_cloud_objects_in_location_without_descendents(
                original_cloud_object.location(object_store_model, app),
                app,
            )
            .filter(|&object| object.object_type() == original_cloud_object.object_type())
            .map(|object| object.display_name())
            .collect::<HashSet<String>>();

        // Start with "{original_object_name} ({original_object_name's count + 1})".
        // Keep incrementing by one if there already exists an object of the same type in
        // the same folder (using the hashset generated above).
        let mut duplicate_name = get_duplicate_object_name(&original_name);
        while same_type_and_folder_names.contains(&duplicate_name) {
            duplicate_name = get_duplicate_object_name(&duplicate_name);
        }
        duplicate_name
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_workflow(
        &mut self,
        workflow: Workflow,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            WorkflowObjectModel::new(workflow),
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_workflow_enum(
        &mut self,
        workflow_enum: WorkflowEnum,
        owner: Owner,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            WorkflowEnumObjectModel::new(workflow_enum),
            owner,
            client_id,
            entrypoint,
            force_expand,
            None,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_env_var_collection(
        &mut self,
        client_id: ClientId,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
        model: EnvVarCollectionObjectModel,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            model,
            owner,
            client_id,
            entrypoint,
            force_expand,
            initial_folder_id,
            // When adding the initiated_by parameter to this function call, InitiatedBy::User was set as a default value.
            // This can be changed to InitiatedBy::System if this action was automatically kicked off by the system and we do not want a user facing toast.
            InitiatedBy::User,
            ctx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_folder(
        &mut self,
        name: String,
        owner: Owner,
        client_id: ClientId,
        initial_folder_id: Option<SyncId>,
        force_expand: bool,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        self.create_object(
            // TODO(INT-789): support creating folders as warp packs
            FolderObjectModel::new(&name, false),
            owner,
            client_id,
            Default::default(),
            force_expand,
            initial_folder_id,
            initiated_by,
            ctx,
        );
    }

    /// Creates a new local stored object with the given model.
    ///
    /// Waz (Localization): Same as `update_object` — the original implementation enqueued to `SyncQueue` and waited for the server's creation ack.
    /// After localization, it only keeps creating the in-memory object and writing to SQLite. The object permanently exists as a client_id
    /// and is no longer promoted to a server_id. The `entrypoint` and `initiated_by` parameters are preserved to keep the interface stable.
    #[allow(clippy::too_many_arguments)]
    pub fn create_object<K, M>(
        &mut self,
        model: M,
        owner: Owner,
        client_id: ClientId,
        entrypoint: StoredObjectEventEntrypoint,
        force_expand: bool,
        initial_folder_id: Option<SyncId>,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) where
        K: HashableId
            + ToServerId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        // Waz: The cloud-sync queue is removed, and these two parameters are only used for `create_object_queue_item` construction;
        // keep the interface to avoid affecting the signatures of 30+ calling sites.
        let _ = entrypoint;
        let _ = initiated_by;

        let object_id = SyncId::ClientId(client_id);
        let initial_editor_uid = TEST_USER_UID.to_string();

        // Update in-memory model.
        ObjectStoreModel::handle(ctx).update(ctx, move |object_store_model, ctx| {
            let mut object = GenericStoredObject::<K, M>::new_local(
                model.clone(),
                owner,
                initial_folder_id,
                client_id,
            );
            object.metadata.current_editor_uid = Some(initial_editor_uid.clone());
            object_store_model.create_object(object_id, object, ctx);

            if force_expand {
                object_store_model.force_expand_object_and_ancestors(object_id, ctx);
            }
        });

        // Update sqlite.
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        if let Some(object) = object_store_model.get_object_of_type::<K, M>(&object_id) {
            self.save_to_db([object.upsert_event()]);
        }
    }

    /// Updates a local stored object with a new model.
    ///
    /// Waz (Localization): No cloud = no server ack. Original implementation: update in-memory -> mark as `InFlight` ->
    /// write to SQLite -> enqueue in `SyncQueue` (wait for server response before decrementing `InFlight`). After localization,
    /// the two cloud-sync legs are removed, only keeping: update in-memory + write to SQLite. The object sync_status forever stays at the initial
    /// `NoLocalChanges` (local write is the "completed" semantic). The `revision_ts` parameter is preserved to maintain interface signature stability,
    /// and is ignored in the local branch (to be cleaned up together during the Phase 2d-4b renaming).
    pub fn update_object<K, M>(
        &mut self,
        model: M,
        object_id: SyncId,
        revision_ts: Option<Revision>,
        ctx: &mut ModelContext<Self>,
    ) where
        K: HashableId
            + ToServerId
            + std::fmt::Debug
            + Into<String>
            + Clone
            + Copy
            + Send
            + Sync
            + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        let _ = revision_ts; // Waz: No server-side revision coordination, ignored.

        // Update in-memory model.
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            object_store_model.update_object_from_edit(model.clone(), object_id, ctx);
            ctx.notify();
        });

        // Update sqlite.
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        if let Some(object) = object_store_model.get_object_of_type::<K, M>(&object_id) {
            self.save_to_db([object.upsert_event()]);
        };
    }

    // Takes a generic SyncId and records the action.
    pub fn record_object_action(
        &mut self,
        id_and_type: ObjectTypeAndId,
        action_type: ObjectActionType,
        data: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Take the action timestamp from the client.
        let action_timestamp = Utc::now();

        // Update in-memory model.
        let object_action = ObjectActions::handle(ctx).update(ctx, |object_actions_model, ctx| {
            object_actions_model.insert_action(
                id_and_type.uid(),
                id_and_type.sqlite_uid_hash(),
                action_type.clone(),
                data.clone(),
                action_timestamp,
                ctx,
            )
        });

        // Update sqlite.
        self.save_to_db([ModelEvent::InsertObjectAction { object_action }]);

        // Waz (Wave 4): Originally entered SyncQueue at the end to report RecordObjectAction. After the SyncQueue was deleted entirely,
        // the local SQLite record itself is considered "completed".
        let _ = (id_and_type, action_type, data, action_timestamp);
    }

    fn maybe_overwrite_object_action_history(
        &mut self,
        history: &ObjectActionHistory,
        ctx: &mut ModelContext<Self>,
    ) {
        ObjectActions::handle(ctx).update(ctx, |object_actions_model, ctx| {
            // Accept this action history if we don't have any actions for this object OR the server's latest action
            // for this object is at least as recent as our latest synced action for this object
            let latest_processed_at_ts =
                object_actions_model.get_latest_processed_at_ts(&history.uid);
            if latest_processed_at_ts
                .is_none_or(|client_ts| client_ts <= history.latest_processed_at_timestamp)
            {
                // Overwrite the history for this object.
                object_actions_model.overwrite_action_history_for_object(
                    &history.uid,
                    history.actions.clone(),
                    ctx,
                );
            }
        });
    }

    /// Overwrites the actions in SQLite for a specified set of objects with the actions that
    /// are currently in the ObjectActions singleton model.
    fn sync_actions_for_objects_to_sqlite(
        &mut self,
        object_uids: Vec<&ObjectUid>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Retrieve the objects from the ObjectActions model
        let actions = ObjectActions::handle(ctx).read(ctx, |object_actions_model, _ctx| {
            object_actions_model.get_actions_for_objects(object_uids)
        });

        // Overwrite the actions for those objects in sqlite
        let actions_to_sync: Vec<ObjectAction> = actions.values().flatten().cloned().collect();
        self.save_to_db([ModelEvent::SyncObjectActions { actions_to_sync }]);
    }

    /// Sets the notebooks current editor in memory. SQLite is not updated until we receive
    /// server confirmation.
    fn set_notebook_current_editor(
        &self,
        notebook_id: &SyncId,
        editor_uid: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(notebook) = object_store_model.get_notebook_mut(notebook_id) {
                notebook.metadata.set_current_editor(editor_uid);
                ctx.notify();
            }
        });
    }

    /// Waz: cloud notebook edit lease has been deleted. Fold here to grant editing position locally,
    /// Preserve the method signature for the `notebooks/notebook.rs` call site.
    pub fn grab_notebook_edit_access(
        &mut self,
        notebook_id: SyncId,
        _optimistically_grant_access: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        // If the object isn't known to the server yet, we should not proceed
        let SyncId::ServerId(_server_id) = notebook_id else {
            return;
        };

        self.set_notebook_current_editor(&notebook_id, Some(TEST_USER_UID.to_string()), ctx);
    }

    /// Waz: The cloud notebook edit lease has been deleted. It is folded here to directly clear the editing rights locally.
    pub fn give_up_notebook_edit_access(
        &mut self,
        notebook_id: SyncId,
        ctx: &mut ModelContext<Self>,
    ) {
        // If the object isn't known to the server yet, we should not proceed
        let SyncId::ServerId(_server_id) = notebook_id else {
            return;
        };

        let current_editor = ObjectStoreViewModel::as_ref(ctx)
            .object_current_editor(&notebook_id.uid(), ctx)
            .unwrap_or(Editor::no_editor());

        // Only give up access if the current user has edit access
        if matches!(current_editor.state, EditorState::CurrentUser) {
            self.set_notebook_current_editor(&notebook_id, None, ctx);
        }
    }

    /// Optimistically marks the object as trashed, updates the metadata sync status to pending, and returns both
    /// the metadata timestamp and the newly-set trashed timestamp. We need to check the metadata timestamp
    /// in the case where we need to revert this (i.e. if there was a rtc message in the meantime, we shouldn't
    /// overwrite the values and don't need to).
    // TODO: we currently set trashed_ts here with the client's clock, but we should revise this metadata flow
    // to get the timestamp from the server instead.
    fn mark_object_trashed_and_return_timestamps(
        &self,
        uid: &ObjectUid,
        ctx: &mut ModelContext<Self>,
    ) -> (Option<ServerTimestamp>, Option<ServerTimestamp>) {
        let timestamp = ServerTimestamp::new(Utc::now());
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(object) = object_store_model.get_mut_by_uid(uid) {
                // Here, we write a timestamp to the trashed_ts field. The client will eventually update to
                // the canonical version of the timestamp once it receives an rtc message from the server.

                object.metadata_mut().trashed_ts = Some(timestamp);
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = true;
                ctx.emit(ObjectStoreEvent::ObjectTrashed {
                    type_and_id: object.object_type_and_id(),
                    source: UpdateSource::Local,
                });
                ctx.notify();
                (
                    object.metadata().metadata_last_updated_ts,
                    object.metadata().trashed_ts,
                )
            } else {
                (None, None)
            }
        })
    }

    pub fn trash_object(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        // Waz (decentralization branch): Local objects (without server_id) go through pure local trashing —
        // mark trashed_ts + write to SQLite. **Do not emit ObjectOperationComplete**,
        // because its multiple consumers (notebooks/env_vars/cloud_object/view) all `.expect` a server_id;
        // the Drive UI has already been notified via the ObjectStoreEvent::ObjectTrashed emitted inside mark_object_trashed_and_return_timestamps.
        let Some(server_id) = id.server_id() else {
            let hashed_id = id.uid();
            self.mark_object_trashed_and_return_timestamps(&hashed_id, ctx);
            // Waz: Local objects never get a server ack to clear has_pending_metadata_change.
            // It must be manually cleared before saving to SQLite, otherwise the `if !has_pending_metadata_change` branch in upsert_stored_object
            // will skip writing the trashed_ts field, causing the loaded trashed_ts from SQLite to be NULL after a restart, and making the object reappear in PERSONAL.
            ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, _| {
                if let Some(object) = object_store_model.get_mut_by_uid(&hashed_id) {
                    object
                        .metadata_mut()
                        .pending_changes_statuses
                        .has_pending_metadata_change = false;
                }
                self.save_in_memory_object_to_sqlite(object_store_model, &hashed_id);
            });
            ctx.notify();
            return;
        };

        let hashed_id = id.uid();
        // If there's a pending online-only operation for this object, don't trash it.
        let Some(has_pending_online_only_operation) =
            ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
                model
                    .get_by_uid(&hashed_id)
                    .map(|object| object.metadata().has_pending_online_only_change())
            })
        else {
            return;
        };

        if has_pending_online_only_operation {
            return;
        }

        self.mark_object_trashed_and_return_timestamps(&hashed_id, ctx);
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, _| {
            if let Some(object) = object_store_model.get_mut_by_uid(&hashed_id) {
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
            }

            let hashed_sqlite_id = server_id.sqlite_type_and_uid_hash(id.object_id_type());
            self.save_in_memory_object_metadata_to_sqlite(
                object_store_model,
                &hashed_id,
                &hashed_sqlite_id,
            );
        });

        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::Trash,
                client_id: None,
                server_id: Some(ServerId::from_string_lossy(&hashed_id)),
                num_objects: None,
            },
        });
        ctx.notify();
    }

    pub fn untrash_object(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        // Waz: Local object untrash — clear trashed_ts + emit ObjectUntrashed + write to SQLite.
        // Do not emit ObjectOperationComplete (same comment as trash_object).
        let Some(server_id) = id.server_id() else {
            let hashed_id = id.uid();
            // Waz: Local object untrash — clear trashed_ts and at the same time clear
            // has_pending_metadata_change (local branch has no server ack),
            // otherwise upsert_stored_object will skip writing trashed_ts, and SQLite will still have the old value.
            ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
                if let Some(object) = object_store_model.get_mut_by_uid(&hashed_id) {
                    object.metadata_mut().trashed_ts = None;
                    object
                        .metadata_mut()
                        .pending_changes_statuses
                        .has_pending_metadata_change = false;
                    ctx.emit(ObjectStoreEvent::ObjectUntrashed {
                        type_and_id: object.object_type_and_id(),
                        source: UpdateSource::Local,
                    });
                }
            });
            ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, _| {
                self.save_in_memory_object_to_sqlite(object_store_model, &hashed_id);
            });
            ctx.notify();
            return;
        };

        let hashed_id = id.uid();
        // If there's a pending online-only operation for this object, don't untrash it.
        let Some(has_pending_online_only_operation) =
            ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
                model
                    .get_by_uid(&hashed_id)
                    .map(|object| object.metadata().has_pending_online_only_change())
            })
        else {
            return;
        };

        if has_pending_online_only_operation {
            return;
        }

        // Waz: The cloud untrash RPC has been deleted. This is folded here into a local direct write and clears the pending_untrash bit.
        ObjectStoreModel::handle(ctx).update(ctx, |object_store_model, ctx| {
            if let Some(object) = object_store_model.get_mut_by_uid(&hashed_id) {
                object.metadata_mut().trashed_ts = None;
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .has_pending_metadata_change = false;
                object
                    .metadata_mut()
                    .pending_changes_statuses
                    .pending_untrash = false;
                ctx.emit(ObjectStoreEvent::ObjectUntrashed {
                    type_and_id: object.object_type_and_id(),
                    source: UpdateSource::Local,
                });
            }
            self.save_in_memory_object_to_sqlite(object_store_model, &hashed_id);
        });

        let _ = server_id;

        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::Untrash,
                client_id: None,
                server_id: Some(ServerId::from_string_lossy(&hashed_id)),
                num_objects: None,
            },
        });
        ctx.notify();
    }

    pub fn delete_object_by_user(&mut self, id: ObjectTypeAndId, ctx: &mut ModelContext<Self>) {
        self.delete_object_with_initiated_by(id, InitiatedBy::User, ctx);
    }

    pub fn delete_object_with_initiated_by(
        &mut self,
        id: ObjectTypeAndId,
        initiated_by: InitiatedBy,
        ctx: &mut ModelContext<Self>,
    ) {
        // If the object isn't known to the server yet, we can't delete it.
        let Some(server_id) = id.server_id() else {
            return;
        };

        let uid = id.uid();
        // If there's a pending online-only operation for this object, don't delete it.
        let Some((has_pending_online_only_operation, has_pending_delete)) =
            ObjectStoreModel::handle(ctx).read(ctx, |model, _| {
                model.get_by_uid(&uid).map(|object| {
                    (
                        object.metadata().has_pending_online_only_change(),
                        object.metadata().pending_changes_statuses.pending_delete,
                    )
                })
            })
        else {
            return;
        };

        if has_pending_online_only_operation || has_pending_delete {
            return;
        }

        // Waz: The cloud delete RPC has been deleted. This is folded here into a local direct removal.
        let num_deleted_objects =
            self.on_object_delete_success(vec![SyncId::ServerId(server_id)], ctx);
        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type: OperationSuccessType::Success,
                operation: ObjectOperation::Delete { initiated_by },
                client_id: None,
                server_id: Some(ServerId::from_string_lossy(&uid)),
                num_objects: Some(num_deleted_objects),
            },
        });
        ctx.notify();
    }

    pub fn empty_trash(&mut self, space: Space, ctx: &mut ModelContext<Self>) {
        // Waz: Empty Trash goes through a pure local path. The original implementation called the upstream cloud empty_trash API;
        // without auth or a server, it would fail directly with `Failed to get access token` after 3 retries, and the Trash UI would remain unchanged.
        // Local branch: directly traverse ObjectStoreModel to find objects with matching owner and is_trashed,
        // collect SyncIds, and reuse `on_object_delete_success` (which already handles memory + SQLite deletion and actions cleanup).
        let owner = match UserWorkspaces::as_ref(ctx).space_to_owner(space, ctx) {
            Some(owner) => owner,
            None => {
                // TODO: For the Shared space, this should delete every object that's shared with the user
                // and trashed.
                log::warn!("Tried to empty trash in unsupported space {space:?}");
                return;
            }
        };

        let object_store_model_handle = ObjectStoreModel::handle(ctx);
        let deleted_ids: Vec<SyncId> =
            object_store_model_handle.read(ctx, |object_store_model, _| {
                object_store_model
                    .cloud_objects()
                    .filter(|object| {
                        object.permissions().owner == owner && object.is_trashed(object_store_model)
                    })
                    .map(|object| object.sync_id())
                    .collect()
            });

        let num_deleted_objects = self.on_object_delete_success(deleted_ids, ctx);

        let success_type = if num_deleted_objects == 0 {
            OperationSuccessType::Rejection
        } else {
            OperationSuccessType::Success
        };

        ctx.emit(UpdateManagerEvent::ObjectOperationComplete {
            result: ObjectOperationResult {
                success_type,
                operation: ObjectOperation::EmptyTrash,
                client_id: None,
                server_id: None,
                num_objects: Some(num_deleted_objects),
            },
        });
        ctx.notify();
    }

    pub fn on_object_delete_success(
        &mut self,
        deleted_ids: Vec<SyncId>,
        ctx: &mut ModelContext<'_, UpdateManager>,
    ) -> i32 {
        let object_store_model_handle = ObjectStoreModel::handle(ctx);
        let all_object_uids: Vec<ObjectUid> = deleted_ids.iter().map(|&id| id.uid()).collect();

        // This variable counts the number of objects deleted client-side in each Empty Trash action,
        // because the server returns everything in the db, including objects that have already been marked for deletion
        let mut num_deleted_objects = 0;
        let mut sync_ids_and_types: Vec<(SyncId, ObjectIdType)> = Vec::new();
        object_store_model_handle.update(ctx, |object_store_model, ctx| {
            (sync_ids_and_types, num_deleted_objects) =
                object_store_model.delete_objects_by_id(all_object_uids.clone(), ctx);
        });

        // Deleted the actions associated with these objects too.
        ObjectActions::handle(ctx).update(ctx, |object_actions, ctx| {
            for uid in all_object_uids.clone() {
                object_actions.delete_actions_for_object(&uid, ctx);
            }
        });

        // Return early if empty
        if num_deleted_objects == 0 {
            return num_deleted_objects;
        }

        // Delete objects from sqlite. This will also delete their actions.
        self.save_to_db([ModelEvent::DeleteObjects {
            ids: sync_ids_and_types,
        }]);

        num_deleted_objects
    }

    pub fn rename_folder(
        &mut self,
        folder_id: SyncId,
        new_name: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let revision = object_store_model.current_revision(&folder_id).cloned();
        if let Some(folder) = object_store_model.get_folder(&folder_id) {
            let new_folder = FolderObjectModel {
                name: new_name,
                is_open: folder.model().is_open,
                is_warp_pack: folder.model().is_warp_pack,
            };
            self.update_object(new_folder, folder_id, revision, ctx);
        } else {
            log::warn!("Attempted to rename folder that doesn't exist with id: {folder_id:?}");
        }
    }
}

/// Return the newly duplicated object's name based on the original object's name. E.g.:
/// - "my object name" -> "my object name (1)"
pub fn get_duplicate_object_name(original_name: &str) -> String {
    match DUPLICATE_OBJECT_NAME_REGEX
        .captures(original_name)
        .and_then(|caps| caps.get(1))
        .and_then(|num| num.as_str().parse::<usize>().ok())
    {
        Some(num) => {
            let new_num = num.saturating_add(1);

            // edge case check for when the duplicate number is usize::MAX
            if new_num == usize::MAX {
                format!("{original_name} (1)")
            } else {
                DUPLICATE_OBJECT_NAME_REGEX
                    .replace(original_name, format!(" ({new_num})"))
                    .to_string()
            }
        }
        None => format!("{original_name} (1)"),
    }
}

impl Entity for UpdateManager {
    type Event = UpdateManagerEvent;
}

impl SingletonEntity for UpdateManager {}

// Phase 2c-2: Delete `update_manager_test.rs` (7500+ lines of cloud synchronization behavior tests):
// After `update_object` is localized in Waz, all cloud assertions became invalid; this file was originally part of the Phase 2d-4a
// full file deletion scope, deleted in advance to avoid accumulating 12 `#[ignore]`s. Other consumers of `server/cloud_objects/`
// (listener / update_manager itself) will go offline entirely in Phase 2d-4a.
