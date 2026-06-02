#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bimap::BiMap;

use futures_util::stream::AbortHandle;
use string_offset::{ByteOffset, CharOffset};
use warp_core::features::FeatureFlag;
use warp_editor::content::buffer::{Buffer, ToBufferCharOffset};
use warp_editor::content::diff::{text_diff, TextDiff};
use warp_util::content_version::ContentVersion;
use warp_util::file::{FileId, FileLoadError, FileSaveError};
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use super::buffer_location::{BufferLocation, SyncClock};

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use warp_files::{FileModelEvent, FileModel};
        use warp_editor::content::text::IndentBehavior;
        use warp_editor::content::text::IndentUnit;
    }
}

/// State for a shared buffer including the file ID and buffer handle.
#[derive(Debug, Clone)]
pub struct BufferState {
    pub file_id: FileId,
    pub buffer: ModelHandle<Buffer>,
}

impl BufferState {
    pub fn new(file_id: FileId, buffer: ModelHandle<Buffer>) -> Self {
        Self { file_id, buffer }
    }
}

/// Tracks an active background diff parsing operation.
struct PendingDiffParse {
    abort_handle: AbortHandle,
}

/// Describes the backing store for a buffer's content.
enum BufferSource {
    /// Backed by the local filesystem (existing behavior).
    Local {
        base_content_version: Option<ContentVersion>,
    },
    /// Backed by a remote filesystem over the remote server protocol.
    Remote {
        remote_path: super::buffer_location::RemotePath,
        /// `None` while waiting for the `OpenBufferResponse`; `Some` once loaded.
        sync_clock: Option<SyncClock>,
    },
    /// Local file managed by the remote-server daemon.
    /// Owns the SyncClock for version tracking. Connection tracking
    /// is handled by ServerModel, not here — the buffer is a file-level
    /// concept shared across connections.
    ServerLocal {
        sync_clock: SyncClock,
        base_content_version: Option<ContentVersion>,
    },
}

struct InternalBufferState {
    buffer: WeakModelHandle<Buffer>,
    /// Tracks any active background diff parsing for auto-reload.
    pending_diff_parse: Option<PendingDiffParse>,
    source: BufferSource,
}

impl InternalBufferState {
    /// Returns the base content version for local/server-local buffers,
    /// `None` for remote.
    ///
    /// Remote buffers return `None` because they don't use the file-watcher
    /// auto-reload path. Version tracking for remote buffers is handled by
    /// `SyncClock` instead.
    fn base_content_version(&self) -> Option<ContentVersion> {
        match &self.source {
            BufferSource::Local {
                base_content_version,
            }
            | BufferSource::ServerLocal {
                base_content_version,
                ..
            } => *base_content_version,
            BufferSource::Remote { .. } => None,
        }
    }

    /// Sets the base content version. Applicable to Local and ServerLocal buffers.
    fn set_base_content_version(&mut self, version: ContentVersion) {
        match &mut self.source {
            BufferSource::Local {
                base_content_version,
            }
            | BufferSource::ServerLocal {
                base_content_version,
                ..
            } => {
                *base_content_version = Some(version);
            }
            BufferSource::Remote { .. } => {}
        }
    }

    /// Whether this buffer has been loaded (has content).
    fn is_loaded(&self) -> bool {
        match &self.source {
            BufferSource::Local {
                base_content_version,
            }
            | BufferSource::ServerLocal {
                base_content_version,
                ..
            } => base_content_version.is_some(),
            // Remote buffers are loaded once the OpenBufferResponse arrives
            // and populates the sync clock.
            BufferSource::Remote { sync_clock, .. } => sync_clock.is_some(),
        }
    }
}

pub enum GlobalBufferModelEvent {
    BufferLoaded {
        file_id: FileId,
        content_version: ContentVersion,
    },
    FailedToLoad {
        file_id: FileId,
        error: Rc<FileLoadError>,
    },
    BufferUpdatedFromFileEvent {
        file_id: FileId,
        success: bool,
        content_version: ContentVersion,
    },
    FileSaved {
        file_id: FileId,
    },
    FailedToSave {
        file_id: FileId,
        error: Rc<FileSaveError>,
    },
    /// A remote buffer update conflicted with local edits.
    /// The UI should present a resolution dialog.
    RemoteBufferConflict {
        file_id: FileId,
    },
    /// A server-local buffer was updated from a file-watcher event.
    /// Carries the incremental diff edits for the ServerModel to push
    /// to connected clients as `BufferUpdatedPush`.
    ///
    /// Note: this event is NOT emitted from `apply_client_edit`. See that
    /// method's doc comment for the V0 single-client limitation.
    ServerLocalBufferUpdated {
        file_id: FileId,
        /// Incremental edits with 1-indexed character offsets (matching `CharOffset`).
        edits: Vec<CharOffsetEdit>,
        new_server_version: ContentVersion,
        expected_client_version: ContentVersion,
    },
}

impl GlobalBufferModelEvent {
    pub fn file_id(&self) -> FileId {
        match self {
            GlobalBufferModelEvent::BufferLoaded { file_id, .. }
            | GlobalBufferModelEvent::FailedToLoad { file_id, .. }
            | GlobalBufferModelEvent::BufferUpdatedFromFileEvent { file_id, .. }
            | GlobalBufferModelEvent::FileSaved { file_id, .. }
            | GlobalBufferModelEvent::FailedToSave { file_id, .. }
            | GlobalBufferModelEvent::RemoteBufferConflict { file_id, .. }
            | GlobalBufferModelEvent::ServerLocalBufferUpdated { file_id, .. } => *file_id,
        }
    }
}

/// A text edit using 1-indexed character offsets (matching `CharOffset`).
///
/// Used to carry incremental edits in `ServerLocalBufferUpdated` events
/// and `handle_buffer_updated_push` without coupling `GlobalBufferModel`
/// to proto types. Offsets use the same 1-indexed coordinate system as
/// the buffer's `CharOffset`, so no conversion is needed at the boundary.
pub struct CharOffsetEdit {
    pub start: CharOffset,
    pub end: CharOffset,
    pub text: String,
}

/// Global singleton model for managing shared buffers across editors.
///
/// This allows multiple editors to share the same buffer when editing the same file,
/// enabling consistent content synchronization and more efficient memory usage.
pub struct GlobalBufferModel {
    location_to_id: BiMap<BufferLocation, FileId>,
    buffers: HashMap<FileId, InternalBufferState>,
}

impl GlobalBufferModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        #[cfg(feature = "local_fs")]
        _ctx.subscribe_to_model(&FileModel::handle(_ctx), Self::handle_file_model_events);

        Self {
            location_to_id: BiMap::new(),
            buffers: HashMap::new(),
        }
    }

    /// For client apps only: subscribe to the buffer push events of `RemoteServerManager` to apply the `BufferUpdated`
    /// pushed from the remote daemon to the local Remote buffer.
    ///
    /// Must be explicitly called by the client app when registering `GlobalBufferModel` — **cannot** be placed in
    /// `new()`: the remote-server daemon will also register `GlobalBufferModel` (used for
    /// server-side synchronization of ServerLocal buffers), but the daemon does not register `RemoteServerManager`.
    /// If `RemoteServerManager::handle()` were called in `new()`, it would panic with "never registered",
    /// causing the daemon to crash immediately upon startup.
    #[cfg(feature = "local_tty")]
    pub fn subscribe_to_remote_server_manager(ctx: &mut ModelContext<Self>) {
        use remote_server::manager::{RemoteServerManager, RemoteServerManagerEvent};
        let mgr = RemoteServerManager::handle(ctx);
        ctx.subscribe_to_model(&mgr, |me, event, ctx| {
            if let RemoteServerManagerEvent::BufferUpdated {
                host_id,
                path,
                new_server_version,
                expected_client_version,
                edits,
            } = event
            {
                // wire offset is 1-indexed char offset (aligned with CharOffset).
                // Use saturation conversion to avoid truncating the high bits on 32-bit platforms `as usize`; equivalent on native 64-bit.
                let char_edits: Vec<_> = edits
                    .iter()
                    .map(|e| CharOffsetEdit {
                        start: CharOffset::from(
                            usize::try_from(e.start_offset).unwrap_or(usize::MAX),
                        ),
                        end: CharOffset::from(usize::try_from(e.end_offset).unwrap_or(usize::MAX)),
                        text: e.text.clone(),
                    })
                    .collect();
                me.handle_buffer_updated_push(
                    host_id,
                    path,
                    *new_server_version,
                    *expected_client_version,
                    &char_edits,
                    ctx,
                );
            }
        });
    }

    /// Scan through all buffers and deallocate any that are no longer in use.
    pub fn remove_deallocated_buffers(&mut self, ctx: &mut ModelContext<Self>) {
        let ids_to_remove: HashSet<FileId> = self
            .buffers
            .iter()
            .filter_map(|(id, state)| {
                if state.buffer.upgrade(ctx).is_none() {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        if ids_to_remove.is_empty() {
            return;
        }

        for id in &ids_to_remove {
            self.location_to_id.remove_by_right(id);
        }

        for id in ids_to_remove {
            self.buffers.remove(&id);

            #[cfg(feature = "local_fs")]
            {
                let file_model = FileModel::handle(ctx);
                file_model.update(ctx, |file_model, ctx| {
                    file_model.cancel(id);
                    file_model.unsubscribe(id, ctx);
                });
            }
        }
    }

    pub fn buffer_loaded(&self, file_id: FileId) -> bool {
        self.buffers
            .get(&file_id)
            .map(|state| state.is_loaded())
            .unwrap_or(false)
    }

    fn cleanup_file_id(&mut self, file_id: FileId, _ctx: &mut ModelContext<Self>) {
        self.location_to_id.remove_by_right(&file_id);

        self.buffers.remove(&file_id);

        #[cfg(feature = "local_fs")]
        {
            let file_model = FileModel::handle(_ctx);
            file_model.update(_ctx, |file_model, ctx| {
                file_model.cancel(file_id);
                file_model.unsubscribe(file_id, ctx);
            });
        }
    }

    /// Actively close a buffer: remove it from the client map; for remote buffers, also send
    /// `CloseBuffer` to let the daemon release the memory buffer.
    ///
    /// Does not depend on whether `WeakHandle` is invalid - `remove_deallocated_buffers` only in handle
    /// has been cleared when dropped, and the buffer in the tab closing path usually still has a strong reference (`TabData`
    /// Holds `LocalCodeEditorView` indirect strong reference to `Buffer`). If you don’t take the initiative to clean up,
    /// `InternalBufferState` remains after closing the tab. It will disappear next time you open the same remote file.
    /// "Return existing buffer if already open" branch of `open_remote_buffer`
    /// Reuse old buffers containing unsaved edits, causing the illusion of "seeming saved".
    #[cfg_attr(not(feature = "local_tty"), allow(unused_variables))]
    pub fn close_buffer(&mut self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        // Remote buffer: send CloseBuffer to let the daemon release the memory buffer.
        #[cfg(feature = "local_tty")]
        if let Some(state) = self.buffers.get(&file_id) {
            if let BufferSource::Remote { remote_path, .. } = &state.source {
                let host_id = remote_path.host_id.clone();
                let path_str = remote_path.path.as_str().to_string();
                let manager = remote_server::manager::RemoteServerManager::handle(ctx);
                if let Some(client) = manager.as_ref(ctx).client_for_host(&host_id) {
                    client.close_buffer(path_str);
                }
            }
        }

        self.cleanup_file_id(file_id, ctx);
    }

    /// Returns the buffer handle if it is 1) still exists + active 2) loaded.
    fn buffer_handle_for_id(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<ModelHandle<Buffer>> {
        let state = self.buffers.get(&file_id)?;

        // If the buffer hasn't been loaded yet, don't return a model handle.
        if !state.is_loaded() {
            log::info!("Cannot return handle for unloaded buffers");
            return None;
        }

        match state.buffer.upgrade(ctx) {
            Some(handle) => Some(handle),
            None => {
                // Clean up deallocated buffers.
                self.cleanup_file_id(file_id, ctx);
                None
            }
        }
    }

    /// Once we finish reading the file's content from the disk, populate the buffer with the content.
    /// For initial load (is_loaded_from_file_system == true), this is synchronous.
    /// For auto-reload (is_loaded_from_file_system == false), this spawns a background task for diff computation.
    fn populate_buffer_with_read_content(
        &mut self,
        file_id: FileId,
        content: &str,
        base_version: ContentVersion,
        new_version: ContentVersion,
        is_initial_load: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            self.cleanup_file_id(file_id, ctx);
            log::warn!("Cannot populate buffer with content due to deallocated model handle");
            return;
        };

        if is_initial_load {
            // Initial load: use synchronous replace_all since there's nothing to preserve
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all(content, ctx);
                buffer.set_version(new_version);
            });

            state.set_base_content_version(new_version);

            ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                file_id,
                content_version: new_version,
            });
        } else if FeatureFlag::IncrementalAutoReload.is_enabled() {
            // Auto-reload: spawn background task for diff computation
            Self::start_background_diff_parse(
                file_id,
                state,
                buffer,
                content,
                base_version,
                new_version,
                ctx,
            );
        } else {
            // Fallback: synchronous replace_all (non-incremental)
            buffer.update(ctx, |buffer, ctx| {
                buffer.replace_all(content, ctx);
                buffer.set_version(new_version);
            });

            state.set_base_content_version(new_version);

            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: true,
                content_version: new_version,
            });
        }
    }

    /// Spawns a background task to compute the diff between current buffer content and new content.
    /// On completion, applies the diff edits to the buffer.
    fn start_background_diff_parse(
        file_id: FileId,
        state: &mut InternalBufferState,
        buffer: ModelHandle<Buffer>,
        new_content: &str,
        base_version: ContentVersion,
        new_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) {
        // Abort any existing diff parse for this file
        if let Some(pending) = state.pending_diff_parse.take() {
            pending.abort_handle.abort();
        }

        // Move owned strings to the background thread
        let old_text = buffer.as_ref(ctx).text().into_string();
        let new_content_owned = new_content.to_string();

        let handle = ctx.spawn(
            async move { text_diff(&old_text, &new_content_owned).await },
            move |me, diff: TextDiff, ctx| {
                me.apply_diff_result(file_id, diff, base_version, new_version, ctx);
            },
        );

        // Store the abort handle so we can cancel if a newer update arrives
        state.pending_diff_parse = Some(PendingDiffParse {
            abort_handle: handle.abort_handle(),
        });
    }

    /// Called when background diff parsing completes. Applies the diff edits to the buffer.
    fn apply_diff_result(
        &mut self,
        file_id: FileId,
        diff: TextDiff,
        base_version: ContentVersion,
        new_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        // Clear the pending diff parse state
        state.pending_diff_parse = None;

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            self.cleanup_file_id(file_id, ctx);
            return;
        };

        // Verify the buffer still matches the expected base version.
        // This also correctly handles the case where a client edit arrives
        // during the background diff parse: apply_client_edit modifies the
        // buffer version, so this check will fail and we discard the stale
        // diff rather than incorrectly bumping the server version.
        if !buffer.as_ref(ctx).version_match(&base_version) {
            log::info!("Buffer version changed during diff parsing, aborting apply");
            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: false,
                content_version: base_version,
            });
            return;
        }

        let is_server_local = matches!(state.source, BufferSource::ServerLocal { .. });

        // For ServerLocal buffers, convert byte-range edits to 1-indexed
        // char-offset edits BEFORE applying the diff, because the byte
        // ranges in diff.edits reference the old (pre-edit) buffer content.
        // Uses the buffer's native byte→char offset conversion.
        let char_offset_edits: Option<Vec<CharOffsetEdit>> = if is_server_local {
            let buffer_ref = buffer.as_ref(ctx);
            Some(
                diff.edits
                    .iter()
                    .map(|(range, text)| {
                        // +1: 0-indexed text byte offset → 1-indexed buffer byte offset
                        let start =
                            ByteOffset::from(range.start + 1).to_buffer_char_offset(buffer_ref);
                        let end = ByteOffset::from(range.end + 1).to_buffer_char_offset(buffer_ref);
                        CharOffsetEdit {
                            start,
                            end,
                            text: text.clone(),
                        }
                    })
                    .collect(),
            )
        } else {
            None
        };

        // Apply the diff edits
        buffer.update(ctx, |buffer, ctx| {
            if diff.is_empty() {
                // No actual changes to content, but still need to update version
                buffer.set_version(new_version);
                return;
            }
            let char_edits = diff.to_char_offset_edits(buffer);
            buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
        });

        state.set_base_content_version(new_version);

        if let Some(char_offset_edits) = char_offset_edits {
            if let BufferSource::ServerLocal { sync_clock, .. } = &mut state.source {
                let new_sv = sync_clock.bump_server();
                ctx.emit(GlobalBufferModelEvent::ServerLocalBufferUpdated {
                    file_id,
                    edits: char_offset_edits,
                    new_server_version: new_sv,
                    expected_client_version: sync_clock.client_version,
                });
            }
        } else {
            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                file_id,
                success: true,
                content_version: new_version,
            });
        }
    }

    #[cfg(feature = "local_fs")]
    fn handle_file_model_events(&mut self, event: &FileModelEvent, ctx: &mut ModelContext<Self>) {
        match event {
            FileModelEvent::FileLoaded {
                content,
                id,
                version,
            } => {
                // For initial load, base_version and new_version are the same
                self.populate_buffer_with_read_content(*id, content, *version, *version, true, ctx);
            }
            FileModelEvent::FailedToLoad { id, error } => {
                ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                    file_id: *id,
                    error: error.clone(),
                });
            }
            FileModelEvent::FileUpdated {
                id,
                content,
                base_version,
                new_version,
            } => {
                if let Some(buffer) = self.buffer_handle_for_id(*id, ctx) {
                    if buffer.as_ref(ctx).version_match(base_version) {
                        self.populate_buffer_with_read_content(
                            *id,
                            content,
                            *base_version,
                            *new_version,
                            false,
                            ctx,
                        );
                    } else {
                        // Buffer version doesn't match the event's base_version.
                        // Check if the buffer has no user edits (matches our internal
                        // base_content_version). If so, it's safe to start a fresh
                        // diff parse from the actual buffer version to the new content.
                        let internal_base_version = self
                            .buffers
                            .get(id)
                            .and_then(|state| state.base_content_version());
                        let has_no_user_edits = internal_base_version
                            .is_some_and(|v| buffer.as_ref(ctx).version_match(&v));

                        if has_no_user_edits {
                            // No user edits: safe to reload from the actual buffer
                            // version. This handles both:
                            log::info!(
                                "Starting fresh diff parse for file update (no user edits, \
                                 internal base {:?}, event base {:?})",
                                internal_base_version,
                                *base_version
                            );
                            let actual_version = buffer.as_ref(ctx).version();
                            self.populate_buffer_with_read_content(
                                *id,
                                content,
                                actual_version,
                                *new_version,
                                false,
                                ctx,
                            );
                        } else {
                            log::info!("Not updating global buffer due to version conflict");

                            // Abort any pending diff parse since the buffer has
                            // user edits that we must not overwrite.
                            if let Some(state) = self.buffers.get_mut(id) {
                                if let Some(pending) = state.pending_diff_parse.take() {
                                    pending.abort_handle.abort();
                                }
                            }

                            if internal_base_version != Some(*base_version) {
                                log::warn!(
                                    "Internal global buffer base version {:?} mismatches file model base version {:?}",
                                    internal_base_version,
                                    *base_version
                                );
                            }

                            ctx.emit(GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                                file_id: *id,
                                success: false,
                                content_version: *base_version,
                            });
                        }
                    }
                }
            }
            FileModelEvent::FileSaved { id, version } => {
                // Make sure base content version is updated after a save is performed.
                // This avoids us flagging the incoming update from file watcher as conflict changes.
                if let Some(state) = self.buffers.get_mut(id) {
                    state.set_base_content_version(*version);
                }
                ctx.emit(GlobalBufferModelEvent::FileSaved { file_id: *id });
            }
            FileModelEvent::FailedToSave { id, error } => {
                ctx.emit(GlobalBufferModelEvent::FailedToSave {
                    file_id: *id,
                    error: error.clone(),
                });
            }
        }
    }

    /// Save the content of a tracked buffer to disk via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn save(
        &self,
        file_id: FileId,
        content: String,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, content, version, ctx)
        })
    }

    /// Rename a file and save its content via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn rename_and_save(
        &self,
        file_id: FileId,
        new_path: PathBuf,
        content: String,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.rename_and_save(file_id, new_path, content, version, ctx)
        })
    }

    /// Delete a file via FileModel.
    #[cfg(feature = "local_fs")]
    pub fn delete(
        &self,
        file_id: FileId,
        version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.delete(file_id, version, ctx)
        })
    }

    /// Remove a tracked buffer, cleaning up FileModel state.
    /// Used when a new file is deleted before ever being saved to a permanent location.
    pub fn remove(&mut self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        self.cleanup_file_id(file_id, ctx);
    }

    /// Look up the file path for a tracked local buffer.
    pub fn file_path(&self, file_id: FileId) -> Option<&Path> {
        match self.location_to_id.get_by_right(&file_id) {
            Some(BufferLocation::Local(path)) => Some(path.as_path()),
            _ => None,
        }
    }

    /// Get the base content version (last known on-disk version) for a tracked buffer.
    pub fn base_version(&self, file_id: FileId) -> Option<ContentVersion> {
        self.buffers
            .get(&file_id)
            .and_then(|state| state.base_content_version())
    }

    /// Discard any in progress changes and reload the buffer with the canonical version from the file system.
    #[cfg(feature = "local_fs")]
    pub fn discard_unsaved_changes(&mut self, path: &Path, ctx: &mut ModelContext<Self>) {
        if let Some(id) = self
            .location_to_id
            .get_by_left(&BufferLocation::Local(path.to_path_buf()))
            .cloned()
        {
            let path_clone = path.to_path_buf();
            ctx.spawn(
                async move { FileModel::read_content_for_file(&path_clone).await },
                move |me, content, ctx| match content {
                    Ok(content) => {
                        // Consider this reload as a "new" version. This prevents any race condition when there is another
                        // auto-reload while we are reading out the latest content.
                        let new_version = ContentVersion::new();
                        // For discard, we get the current base version from the buffer state
                        let base_version = me
                            .buffers
                            .get(&id)
                            .and_then(|state| {
                                state.buffer.upgrade(ctx).map(|b| b.as_ref(ctx).version())
                            })
                            .unwrap_or(new_version);
                        FileModel::handle(ctx).update(ctx, |file_model, _ctx| {
                            file_model.set_version(id, new_version);
                        });
                        me.populate_buffer_with_read_content(
                            id,
                            &content,
                            base_version,
                            new_version,
                            false,
                            ctx,
                        );
                    }
                    Err(e) => ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                        file_id: id,
                        error: e.into(),
                    }),
                },
            );
        }
    }

    /// Remap an existing buffer from `old_file_id` to a new path, preserving the buffer
    /// content and unsaved edits. Re-registers the new path with FileModel.
    ///
    /// Used for file rename.
    #[cfg(feature = "local_fs")]
    pub fn rename(
        &mut self,
        old_file_id: FileId,
        new_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) -> Option<BufferState> {
        let old_state = self.buffers.remove(&old_file_id)?;
        let buffer = old_state.buffer.upgrade(ctx)?;

        self.location_to_id.remove_by_right(&old_file_id);

        // Cancel + unsubscribe old FileId from FileModel.
        let file_model = FileModel::handle(ctx);
        file_model.update(ctx, |file_model, ctx| {
            file_model.cancel(old_file_id);
            file_model.unsubscribe(old_file_id, ctx);
        });

        Some(self.register_buffer_for_path(new_path, buffer, old_state.base_content_version(), ctx))
    }

    /// Adopt an existing buffer under a new path without reading from disk.
    /// Used by `save_as` to register a newly-created file with GlobalBufferModel.
    #[cfg(feature = "local_fs")]
    pub fn register(
        &mut self,
        path: PathBuf,
        buffer: ModelHandle<Buffer>,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        let buffer_version = buffer.as_ref(ctx).version();
        self.register_buffer_for_path(path, buffer, Some(buffer_version), ctx)
    }

    /// Shared helper: register `buffer` under `path` with FileModel and store internal state.
    /// After the LSP goes offline, it no longer attempts to synchronize buffer changes with the LSP.
    #[cfg(feature = "local_fs")]
    fn register_buffer_for_path(
        &mut self,
        path: PathBuf,
        buffer: ModelHandle<Buffer>,
        base_content_version: Option<ContentVersion>,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        // If a buffer is already registered for this path, clean up the old entry
        // to avoid orphaning the previous FileId in `self.buffers`.
        if let Some(old_file_id) = self
            .location_to_id
            .get_by_left(&BufferLocation::Local(path.clone()))
            .copied()
        {
            self.cleanup_file_id(old_file_id, ctx);
        }

        let buffer_version = buffer.as_ref(ctx).version();
        let file_id = FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            let id = file_model.register_file_path(&path, true, ctx);
            file_model.set_version(id, buffer_version);
            id
        });

        self.location_to_id
            .insert(BufferLocation::Local(path.clone()), file_id);
        self.buffers.insert(
            file_id,
            InternalBufferState {
                buffer: buffer.downgrade(),
                pending_diff_parse: None,
                source: BufferSource::Local {
                    base_content_version,
                },
            },
        );

        BufferState::new(file_id, buffer)
    }

    /// Open a buffer at the given location.
    ///
    /// Dispatches to the appropriate private opener based on the location variant.
    /// If a buffer already exists for this location and is loaded, returns the
    /// existing `BufferState`.
    pub fn open(&mut self, location: BufferLocation, ctx: &mut ModelContext<Self>) -> BufferState {
        match location {
            #[cfg(feature = "local_fs")]
            BufferLocation::Local(path) => self.open_local(path, false, ctx),
            #[cfg(not(feature = "local_fs"))]
            BufferLocation::Local(_) => {
                unimplemented!("Local buffers require the local_fs feature")
            }
            BufferLocation::Remote(remote_path) => self.open_remote_buffer(remote_path, ctx),
        }
    }

    /// Open a local buffer for the given file path.
    ///
    /// If a buffer already exists for this path and is loaded, returns the existing BufferState.
    /// If no buffer exists, creates a new Buffer and BufferState using FileModel.
    /// File system updates are automatically subscribed to for all buffers.
    ///
    /// When `is_server_local` is true, the buffer is created with a `ServerLocal`
    /// source (with a `SyncClock`) instead of a plain `Local` source.
    #[cfg(feature = "local_fs")]
    fn open_local(
        &mut self,
        path: PathBuf,
        is_server_local: bool,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        if let Some(id) = self
            .location_to_id
            .get_by_left(&BufferLocation::Local(path.clone()))
            .cloned()
        {
            debug_assert!(self.buffers.contains_key(&id));
            if let Some(state) = self.buffers.get(&id) {
                if let Some(handle) = state.buffer.upgrade(ctx) {
                    // Only emit buffer loaded if the base content version is set.
                    if state.is_loaded() {
                        ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                            file_id: id,
                            content_version: handle.as_ref(ctx).version(),
                        });
                    }
                    return BufferState::new(id, handle.clone());
                }
            }
        }

        self.create_new_buffer(&path, is_server_local, ctx)
    }

    #[cfg(feature = "local_fs")]
    fn create_new_buffer(
        &mut self,
        path: &Path,
        is_server_local: bool,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        // Open file through FileModel to get FileId
        // Always subscribe to updates for GlobalBufferModel created buffers
        let file_id =
            FileModel::handle(ctx).update(ctx, |file_model, ctx| file_model.open(path, true, ctx));

        // Create new buffer
        let buffer = ctx.add_model(|_| {
            // This sets the default indentation behavior. The editor will override this if it can load the grammar config
            // for the given file path.
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });

        self.location_to_id
            .insert(BufferLocation::Local(path.to_path_buf()), file_id);
        let source = if is_server_local {
            BufferSource::ServerLocal {
                sync_clock: SyncClock::new(),
                base_content_version: None,
            }
        } else {
            BufferSource::Local {
                base_content_version: None,
            }
        };
        self.buffers.insert(
            file_id,
            InternalBufferState {
                buffer: buffer.downgrade(),
                pending_diff_parse: None,
                source,
            },
        );

        BufferState::new(file_id, buffer)
    }

    /// Attempts to retrieve specific lines from an in-memory buffer for the given file path.
    /// Returns `Some(Vec<(usize, String)>)` if the file is loaded in a buffer, `None` otherwise.
    ///
    /// This is a fast, synchronous operation that avoids disk I/O.
    ///
    /// # Arguments
    /// * `path` - Path to the file
    /// * `line_numbers` - A list of 0-based line numbers to retrieve. Supports non-consecutive lines.
    ///
    /// # Returns
    /// A vector of (line_number, line_content) tuples for each requested line that exists.
    /// Lines that don't exist in the buffer are omitted from the result.
    pub fn get_lines_for_file(
        &mut self,
        path: &Path,
        line_numbers: Vec<usize>,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Vec<(usize, String)>> {
        use warp_editor::content::text::LineCount;

        if line_numbers.is_empty() {
            return Some(Vec::new());
        }

        let file_id = self
            .location_to_id
            .get_by_left(&BufferLocation::Local(path.to_path_buf()))?;
        let buffer = self.buffer_handle_for_id(*file_id, ctx)?;

        let buffer_ref = buffer.as_ref(ctx);
        let total_lines = (buffer_ref.max_point().row + 1) as usize;

        let mut lines = Vec::with_capacity(line_numbers.len());
        for line_idx in line_numbers {
            if line_idx >= total_lines {
                continue;
            }
            // Convert 0-based line index to 1-based LineCount
            let line_count = LineCount::from(line_idx + 1);
            let line_start = buffer_ref.line_start(line_count);
            let line_end = buffer_ref.line_end(line_count);
            let line_text = buffer_ref.text_in_range(line_start..line_end).into_string();
            lines.push((line_idx, line_text));
        }

        Some(lines)
    }

    // ── Remote buffer operations (client side) ────────────────────────

    /// Open a remote buffer identified by a `RemotePath`.
    ///
    /// Sends `OpenBuffer` to the remote server, creates a local `Buffer` model,
    /// and sets up bidirectional sync via `BufferEvent` → `BufferEdit`.
    ///
    /// Returns a `BufferState` immediately (buffer content is populated asynchronously).
    #[cfg_attr(not(feature = "local_tty"), allow(unused_variables, unused_mut))]
    fn open_remote_buffer(
        &mut self,
        remote_path: super::buffer_location::RemotePath,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        let location = BufferLocation::Remote(remote_path.clone());

        // Return existing buffer if already open.
        if let Some(id) = self.location_to_id.get_by_left(&location).cloned() {
            if let Some(state) = self.buffers.get(&id) {
                if let Some(handle) = state.buffer.upgrade(ctx) {
                    if state.is_loaded() {
                        ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                            file_id: id,
                            content_version: handle.as_ref(ctx).version(),
                        });
                    }
                    return BufferState::new(id, handle.clone());
                }
            }
        }

        let file_id = FileId::new();
        // TODO(ssh-remote): buffer can be edited before the initial content (OpenBufferResponse) arrives,
        // user edits within the loading window will be overwritten and lost by replace_all(). We need to
        // add read-only support at the Buffer/editor layer, then lock this buffer during the sync_clock=None period.
        let buffer = ctx.add_model(|_| Buffer::default());

        // Store state with sync_clock = None (set to Some on OpenBufferResponse).
        self.location_to_id.insert(location, file_id);
        self.buffers.insert(
            file_id,
            InternalBufferState {
                buffer: buffer.downgrade(),
                pending_diff_parse: None,
                source: BufferSource::Remote {
                    remote_path: remote_path.clone(),
                    sync_clock: None,
                },
            },
        );

        #[cfg(feature = "local_tty")]
        {
            use warp_editor::content::buffer::BufferEvent;

            // Extract fields before moving remote_path into the buffer source.
            let path_str = remote_path.path.as_str().to_string();
            let host_id = remote_path.host_id.clone();

            // Subscribe to buffer content changes so edits are sent back to the daemon.
            let client_for_sub = {
                let manager = remote_server::manager::RemoteServerManager::handle(ctx);
                manager.as_ref(ctx).client_for_host(&host_id).cloned()
            };
            if let Some(client) = client_for_sub {
                let path_for_edit = path_str.clone();
                ctx.subscribe_to_model(&buffer, move |me, event, ctx| {
                    if let BufferEvent::ContentChanged { delta, origin, .. } = event {
                        // Skip server-originated changes to prevent echo loop.
                        // Server pushes applied via insert_at_char_offset_ranges
                        // emit ContentChanged with SystemEdit origin.
                        if !origin.from_user() {
                            return;
                        }

                        // Look up the sync clock to get the expected server version
                        // and bump the client version.
                        let Some(state) = me.buffers.get_mut(&file_id) else {
                            return;
                        };
                        let BufferSource::Remote { sync_clock, .. } = &mut state.source else {
                            return;
                        };
                        let Some(sync_clock) = sync_clock.as_mut() else {
                            return;
                        };
                        let expected_sv = sync_clock.server_version.as_u64();
                        let new_cv = ContentVersion::new();
                        sync_clock.client_version = new_cv;

                        // Build incremental edits from the ContentChanged delta.
                        let Some(buffer) = state.buffer.upgrade(ctx) else {
                            return;
                        };
                        let edits: Vec<remote_server::proto::TextEdit> = delta
                            .precise_deltas
                            .iter()
                            .map(|d| {
                                // Wire offsets are 1-indexed (matching CharOffset).
                                let text = buffer
                                    .as_ref(ctx)
                                    .text_in_range(d.resolved_range.clone())
                                    .into_string();
                                remote_server::proto::TextEdit {
                                    start_offset: d.replaced_range.start.as_usize() as u64,
                                    end_offset: d.replaced_range.end.as_usize() as u64,
                                    text,
                                }
                            })
                            .collect();
                        // Delivery failure indicates that the connection is dead. The daemon cannot receive this edit, but the local
                        // buffer has progressed — marked as a conflict, triggering UI resynchronization.
                        if let Err(e) = client.send_buffer_edit(
                            path_for_edit.clone(),
                            expected_sv,
                            new_cv.as_u64(),
                            edits,
                        ) {
                            log::error!(
                                "Failed to send remote buffer edit for {path_for_edit}: {e}"
                            );
                            ctx.emit(GlobalBufferModelEvent::RemoteBufferConflict { file_id });
                        }
                    }
                });
            }

            // Look up the client on the main thread, then send OpenBuffer asynchronously.
            let manager = remote_server::manager::RemoteServerManager::handle(ctx);
            let Some(client) = manager.as_ref(ctx).client_for_host(&host_id).cloned() else {
                log::warn!("No remote server client for host {host_id:?}");
                // Clean up the failed file_id so that subsequent retries can resend the OpenBuffer.
                self.cleanup_file_id(file_id, ctx);
                ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                    file_id,
                    error: Rc::new(FileLoadError::DoesNotExist),
                });
                return BufferState::new(file_id, buffer);
            };

            ctx.spawn(
                async move {
                    client
                        .open_buffer(path_str)
                        .await
                        .map_err(|e| format!("{e}"))
                },
                move |me, result, ctx| match result {
                    Ok(response) => {
                        let Some(state) = me.buffers.get_mut(&file_id) else {
                            return;
                        };
                        if let BufferSource::Remote { sync_clock, .. } = &mut state.source {
                            *sync_clock = Some(SyncClock::from_wire(response.server_version, 0));
                        }
                        let Some(buffer) = state.buffer.upgrade(ctx) else {
                            return;
                        };
                        let version = ContentVersion::new();
                        buffer.update(ctx, |buffer, ctx| {
                            buffer.replace_all(&response.content, ctx);
                            buffer.set_version(version);
                        });
                        ctx.emit(GlobalBufferModelEvent::BufferLoaded {
                            file_id,
                            content_version: version,
                        });
                    }
                    Err(error) => {
                        log::warn!("Failed to open remote buffer: {error}");
                        // Clean up the failed file_id so that subsequent retries can resend the OpenBuffer.
                        me.cleanup_file_id(file_id, ctx);
                        ctx.emit(GlobalBufferModelEvent::FailedToLoad {
                            file_id,
                            error: Rc::new(FileLoadError::DoesNotExist),
                        });
                    }
                },
            );
        }

        BufferState::new(file_id, buffer)
    }

    /// Handle an incoming `BufferUpdatedPush` from the remote server.
    ///
    /// Accepts incremental edits (1-indexed char offsets matching `CharOffset`)
    /// and applies them to the local buffer via `insert_at_char_offset_ranges`.
    /// If the expected client version doesn't match, a conflict event is emitted.
    #[cfg_attr(not(feature = "local_tty"), allow(dead_code))]
    pub fn handle_buffer_updated_push(
        &mut self,
        host_id: &warp_core::HostId,
        path: &str,
        new_server_version: u64,
        expected_client_version: u64,
        edits: &[CharOffsetEdit],
        ctx: &mut ModelContext<Self>,
    ) {
        // Find the buffer by scanning for a Remote source with matching host+path.
        let file_id = self.buffers.iter().find_map(|(id, state)| {
            if let BufferSource::Remote { remote_path, .. } = &state.source {
                if remote_path.host_id == *host_id && remote_path.path.as_str() == path {
                    return Some(*id);
                }
            }
            None
        });

        let Some(file_id) = file_id else {
            log::warn!("BufferUpdatedPush for unknown remote buffer: {path}");
            return;
        };

        let Some(state) = self.buffers.get_mut(&file_id) else {
            return;
        };

        let BufferSource::Remote { sync_clock, .. } = &mut state.source else {
            return;
        };
        let Some(sync_clock) = sync_clock.as_mut() else {
            return;
        };

        let expected_cv = ContentVersion::from_wire_u64(expected_client_version);
        if sync_clock.server_push_matches(expected_cv) {
            // Accept the update — apply edits incrementally.
            sync_clock.server_version = ContentVersion::from_wire_u64(new_server_version);

            let Some(buffer) = state.buffer.upgrade(ctx) else {
                return;
            };

            let new_version = ContentVersion::new();
            buffer.update(ctx, |buffer, ctx| {
                let max_offset = buffer.max_charoffset();
                let char_edits: Vec<(std::ops::Range<CharOffset>, String)> = edits
                    .iter()
                    .map(|edit| {
                        let start = std::cmp::min(edit.start, max_offset);
                        let end = std::cmp::min(edit.end, max_offset);
                        (start..end, edit.text.clone())
                    })
                    .collect();

                buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
            });
        } else {
            // Conflict — local edits diverged from server.
            log::info!(
                "Remote buffer conflict for {path}: expected C={expected_client_version}, \
                 local C={:?}",
                sync_clock.client_version
            );
            ctx.emit(GlobalBufferModelEvent::RemoteBufferConflict { file_id });
        }
    }

    // ── Server-local buffer operations (daemon side) ────────────────

    /// Open a server-local buffer for the given file path on the daemon.
    ///
    /// Delegates to `open_local` with `is_server_local = true` so the buffer
    /// is created directly with a `ServerLocal` source and `SyncClock`.
    #[cfg(feature = "local_fs")]
    pub fn open_server_local(
        &mut self,
        path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) -> BufferState {
        self.open_local(path, true, ctx)
    }

    /// Apply a client edit to a server-local buffer.
    ///
    /// If `expected_server_version` matches the buffer's current server version,
    /// the edits are applied to the in-memory buffer (no disk write) and the
    /// client version is updated. Returns `true` if accepted, `false` if rejected
    /// (stale edit — silently discarded, per `BufferEdit` proto spec).
    ///
    /// V0 limitation (single-client per buffer):
    /// this intentionally does NOT emit `ServerLocalBufferUpdated`. That event
    /// would broadcast the edit to every other connection that has the buffer
    /// open, but `SyncClock.client_version` is daemon-wide rather than
    /// per-connection, so there is no safe `expected_client_version` to put
    /// in a `BufferUpdatedPush` targeted at peer connection C (its local
    /// `client_version` is independent of A's). Until `SyncClock` becomes
    /// per-connection, only one client should hold a writable view of a
    /// given remote buffer at a time.
    ///
    /// TODO(ssh-remote, multi-client): make `SyncClock.client_version` a
    /// `HashMap<ConnectionId, ContentVersion>` and forward A's edits to
    /// peers with the per-peer expected `client_version`.
    #[cfg(feature = "local_fs")]
    pub fn apply_client_edit(
        &mut self,
        file_id: FileId,
        edits: &[remote_server::proto::TextEdit],
        expected_server_version: ContentVersion,
        new_client_version: ContentVersion,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return false;
        };

        let BufferSource::ServerLocal { sync_clock, .. } = &mut state.source else {
            return false;
        };

        if !sync_clock.client_edit_matches(expected_server_version) {
            log::info!(
                "Rejected client edit: expected S={:?}, actual S={:?}",
                expected_server_version,
                sync_clock.server_version
            );
            return false;
        }

        sync_clock.client_version = new_client_version;

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            return false;
        };

        // Wire offsets are 1-indexed (matching CharOffset), so no conversion needed.
        let new_version = ContentVersion::new();
        buffer.update(ctx, |buffer, ctx| {
            let max_offset = buffer.max_charoffset();
            // Wire offset saturating conversion + clamp to the end of the buffer, double defense.
            let char_edits: Vec<(std::ops::Range<CharOffset>, String)> = edits
                .iter()
                .map(|edit| {
                    let start = CharOffset::from(
                        usize::try_from(edit.start_offset)
                            .unwrap_or(usize::MAX)
                            .min(max_offset.as_usize()),
                    );
                    let end = CharOffset::from(
                        usize::try_from(edit.end_offset)
                            .unwrap_or(usize::MAX)
                            .min(max_offset.as_usize()),
                    );
                    (start..end, edit.text.clone())
                })
                .collect();

            buffer.insert_at_char_offset_ranges(char_edits, new_version, ctx);
        });

        true
    }

    /// Save a server-local buffer to disk.
    #[cfg(feature = "local_fs")]
    pub fn save_server_local(
        &mut self,
        file_id: FileId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        let Some(state) = self.buffers.get(&file_id) else {
            return Err(FileSaveError::RemoteError("Buffer not found".to_string()));
        };
        let Some(buffer) = state.buffer.upgrade(ctx) else {
            return Err(FileSaveError::RemoteError("Buffer deallocated".to_string()));
        };
        let content = buffer.as_ref(ctx).text().into_string();
        // Use the current version of the buffer to avoid version sync misalignment with the daemon.
        let version = buffer.as_ref(ctx).version();
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, content, version, ctx)
        })
    }

    /// Resolve a conflict by accepting the client's content.
    /// Replaces the buffer content, updates the sync clock, and saves to disk.
    #[cfg(feature = "local_fs")]
    pub fn resolve_conflict(
        &mut self,
        file_id: FileId,
        acknowledged_server_version: ContentVersion,
        current_client_version: ContentVersion,
        client_content: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), FileSaveError> {
        let Some(state) = self.buffers.get_mut(&file_id) else {
            return Err(FileSaveError::RemoteError("Buffer not found".to_string()));
        };

        if let BufferSource::ServerLocal { sync_clock, .. } = &mut state.source {
            // Reject stale conflict resolutions: if the server-side version has changed after the client sees the conflict,
            // force-overwriting will lose the newer server-side edits.
            if sync_clock.server_version != acknowledged_server_version {
                return Err(FileSaveError::RemoteError(
                    "Stale conflict resolution".to_string(),
                ));
            }
            sync_clock.server_version = acknowledged_server_version;
            sync_clock.client_version = current_client_version;
        } else {
            return Err(FileSaveError::RemoteError(
                "Buffer is not server-local".to_string(),
            ));
        }

        let Some(buffer) = state.buffer.upgrade(ctx) else {
            return Err(FileSaveError::RemoteError("Buffer deallocated".to_string()));
        };

        let new_version = ContentVersion::new();
        buffer.update(ctx, |buffer, ctx| {
            buffer.replace_all(client_content, ctx);
            buffer.set_version(new_version);
        });

        // Save to disk. Note: the buffer content has already been replaced
        // in memory above. If the save fails, memory and disk will diverge.
        // Use the current version of the buffer, and do not update base_content_version until the save is successful
        // (FileSaved callback) to avoid version sync misalignment with the daemon.
        let content = client_content.to_string();
        let save_version = buffer.as_ref(ctx).version();
        FileModel::handle(ctx).update(ctx, |file_model, ctx| {
            file_model.save(file_id, content, save_version, ctx)
        })
    }

    // ── Public accessors ──────────────────────────────────────────────

    /// Returns the buffer text content for a given `FileId`.
    pub fn content_for_file(&self, file_id: FileId, ctx: &warpui::AppContext) -> Option<String> {
        let state = self.buffers.get(&file_id)?;
        let buffer = state.buffer.upgrade(ctx)?;
        Some(buffer.as_ref(ctx).text().into_string())
    }

    /// Returns a reference to the `SyncClock` for a server-local buffer.
    pub fn sync_clock_for_server_local(&self, file_id: FileId) -> Option<&SyncClock> {
        let state = self.buffers.get(&file_id)?;
        match &state.source {
            BufferSource::ServerLocal { sync_clock, .. } => Some(sync_clock),
            BufferSource::Local { .. } | BufferSource::Remote { .. } => None,
        }
    }

    /// Returns whether a buffer is a `ServerLocal` source.
    #[cfg(test)]
    pub fn is_server_local(&self, file_id: FileId) -> bool {
        self.buffers
            .get(&file_id)
            .is_some_and(|state| matches!(state.source, BufferSource::ServerLocal { .. }))
    }

    /// Whether this buffer is a client `Remote` buffer (remote SSH file).
    ///
    /// Used by the editor to determine during saving: remote files cannot go through local `FileModel` (no local path,
    /// which would yield `NoFilePath`), but must go through the buffer-sync `SaveBuffer` protocol.
    #[cfg(feature = "local_tty")]
    pub fn is_remote(&self, file_id: FileId) -> bool {
        self.buffers
            .get(&file_id)
            .is_some_and(|state| matches!(state.source, BufferSource::Remote { .. }))
    }

    /// Client: persist the current content of the remote buffer to the daemon-side disk.
    ///
    /// The daemon's memory buffer has already been synchronized in real-time with user edits via `BufferEdit`
    /// (see the subscription to `ContentChanged` in `open_remote_buffer`), so we only need to send a `SaveBuffer`
    /// to trigger daemon disk writes. After a successful request, emit `FileSaved` to let the editor/tabs update their saved state.
    #[cfg(feature = "local_tty")]
    pub fn save_remote_buffer(&self, file_id: FileId, ctx: &mut ModelContext<Self>) {
        let Some(BufferLocation::Remote(remote_path)) =
            self.location_to_id.get_by_right(&file_id).cloned()
        else {
            log::warn!("save_remote_buffer: file_id {file_id:?} is not a Remote buffer");
            return;
        };
        let host_id = remote_path.host_id.clone();
        let path_str = remote_path.path.as_str().to_string();

        let manager = remote_server::manager::RemoteServerManager::handle(ctx);
        let Some(client) = manager.as_ref(ctx).client_for_host(&host_id).cloned() else {
            log::warn!("save_remote_buffer: host {host_id:?} has no remote server client");
            // Notify the editor of the save failure to avoid remaining in a false "saved" state.
            ctx.emit(GlobalBufferModelEvent::FailedToSave {
                file_id,
                error: Rc::new(FileSaveError::RemoteError(format!(
                    "Remote host {host_id:?} is not connected"
                ))),
            });
            return;
        };

        ctx.spawn(
            async move {
                client
                    .save_buffer(path_str)
                    .await
                    .map_err(|e| format!("{e}"))
            },
            move |_me, result, ctx| match result {
                Ok(response) => {
                    use remote_server::proto::save_buffer_response::Result as SaveResult;
                    match response.result {
                        Some(SaveResult::Success(_)) | None => {
                            ctx.emit(GlobalBufferModelEvent::FileSaved { file_id });
                        }
                        Some(SaveResult::Error(err)) => {
                            // Bubble up the remote save failure to the editor to display a failure prompt.
                            ctx.emit(GlobalBufferModelEvent::FailedToSave {
                                file_id,
                                error: Rc::new(FileSaveError::RemoteError(err.message)),
                            });
                        }
                    }
                }
                Err(error) => {
                    // Bubble up transmission/protocol errors to the editor as well.
                    ctx.emit(GlobalBufferModelEvent::FailedToSave {
                        file_id,
                        error: Rc::new(FileSaveError::RemoteError(format!(
                            "SaveBuffer request failed: {error}"
                        ))),
                    });
                }
            },
        );
    }
}

impl Entity for GlobalBufferModel {
    type Event = GlobalBufferModelEvent;
}

impl SingletonEntity for GlobalBufferModel {}
