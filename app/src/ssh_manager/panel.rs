//! SSH Manager main panel - Tool Panel on the left content: tree list + toolbar + right-click menu
//! + Folder inline rename.
//!
//! UX rules:
//! - **Click server**: direct connection (open terminal pane and run ssh). To edit right click.
//! - **Click folder**: Only select (highlight); right-click "Rename" to edit the name or enter it immediately after creating a new one.
//! - **Enter the rename state immediately after creating a new folder** (Drive style).
//! - Right click on server: Edit / Connect / Delete
//! - Right click folder: create new folder / create new server / rename / delete
//! - Right click on a blank space: New folder/New server
//!
//! Visual polishing reference `app/src/drive/index.rs` constants (ITEM_FONT_SIZE=14 / indent 16 /
//! row padding 4×8).

use std::collections::HashMap;

use pathfinder_geometry::vector::Vector2F;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    AcceptedByDropTarget, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Dismiss, Draggable, DraggableState, DropTarget, DropTargetData, Element,
    Empty, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, SavePosition, Stack, Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use warp_ssh_manager::{
    AuthType, KeychainSecretStore, NodeKind, SecretKind, SshNode, SshRepository,
    SshSecretStore, SshServerInfo,
};

use crate::editor::{
    EditorView, Event as EditorEvent, SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::ssh_manager::candidates::{CandidateRow, CandidatesViewModel};
use crate::ssh_manager::{SshTreeChangedEvent, SshTreeChangedNotifier};

// ----Visual constants (refer to Drive) ----
const TOOLBAR_BUTTON_SIZE: f32 = 26.0;
const TOOLBAR_ICON_SIZE: f32 = 14.0;
const ITEM_PADDING_VERTICAL: f32 = 5.0;
const ITEM_PADDING_HORIZONTAL: f32 = 8.0;
const ITEM_ICON_TEXT_SPACING: f32 = 8.0;
const ITEM_MARGIN_BOTTOM: f32 = 2.0;
const ITEM_ICON_SIZE: f32 = 14.0;
const FOLDER_DEPTH_INDENT: f32 = 16.0;
const PANEL_HORIZONTAL_PADDING: f32 = 8.0;

const CONTEXT_MENU_WIDTH: f32 = 200.0;
const CONTEXT_MENU_ITEM_PADDING_V: f32 = 7.0;
const CONTEXT_MENU_ITEM_PADDING_H: f32 = 12.0;
const MAX_CONTEXT_MENU_ITEMS: usize = 4;
const SSH_PANEL_POSITION_ID: &str = "ssh_manager_panel_root";

#[derive(Clone, Debug)]
pub enum SshManagerPanelAction {
    AddFolder,
    AddServer,
    DeleteSelected,
    Connect,
    Edit,
    CloneServer(String),
    /// Click a row and the processing logic is based on the node type:
    /// - server: Select + emit OpenSshTerminal (direct connection)
    /// - folder: selected only
    Click(String),
    StartRename(String),
    CommitRename,
    CancelRename,
    OpenContextMenu {
        target: Option<String>,
        position: Vector2F,
    },
    DismissContextMenu,
    /// Drag and drop completed → Move `node_id` to `new_parent_id` (None = root).
    MoveNode {
        node_id: String,
        new_parent_id: Option<String>,
    },
    /// Collapse/expand a single folder. Server nodes are ignored.
    ToggleNodeCollapsed(String),
    /// Top button: Smart switching — any folder is also expanded → collapsed; otherwise, it is fully expanded.
    ToggleAllFolders,
    /// Double-click the server line = connection (open a new tab). Folder double click = two toggle offsets no-op.
    DoubleClick(String),
    /// "Candidates" section: Copy a candidate from `~/.ssh/config` into the save tree.
    ImportCandidate {
        alias: String,
    },
    /// Re-read `~/.ssh/config` (the user clicks the Refresh button after changing the config).
    RefreshCandidates,
    /// Collapse/expand the "Candidates" section (manually collapse the list if it is long).
    ToggleCandidatesSection,
}

#[derive(Clone, Debug)]
pub enum SshManagerPanelEvent {
    /// When the user right-clicks "Edit" and selects a server, the central pane should open/focus the editing of that server.
    /// (`Workspace::open_ssh_server`)。
    OpenServerEditor {
        node_id: String,
    },
    /// The user clicks the server or right-clicks "Connect" to request to open the terminal pane and run ssh +
    /// SecretInjector。
    OpenSshTerminal {
        node_id: String,
        server: SshServerInfo,
    },
    PersistenceError(String),
}

struct RenameState {
    node_id: String,
    editor: ViewHandle<EditorView>,
}

/// The content field of a single candidate line - put several Options that are concerned when rendering into a struct,
/// Avoid too long argument lists for `render_candidate_row` (clippy::too_many_arguments).
struct CandidateRowFields<'a> {
    alias: &'a str,
    hostname: Option<&'a str>,
    user: Option<&'a str>,
    port: Option<u16>,
    added: bool,
}

/// Theme color matching - imported rows use muted, ordinary rows use main.
#[derive(Copy, Clone)]
struct CandidateRowColors {
    main: warp_core::ui::theme::Fill,
    muted: warp_core::ui::theme::Fill,
}

/// Drag drop point metadata. `parent_id = None` means dragging to the blank space of the panel (replacing it to the root);
/// `Some(folder_id)` means dragging into the folder; **not allowed** to drag directly to the server (server
/// There cannot be children) - in this case drop_data is interpreted as "drag to the sibling position of server", that is
/// `parent_id = server.parent_id`, expanded when dispatching action.
#[derive(Debug, Clone)]
struct SshDropData {
    parent_id: Option<String>,
}

impl DropTargetData for SshDropData {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct SshManagerPanel {
    nodes: Vec<SshNode>,
    depths: HashMap<String, usize>,
    selected_id: Option<String>,

    add_folder_btn: MouseStateHandle,
    add_server_btn: MouseStateHandle,
    toggle_all_btn: MouseStateHandle,
    row_states: HashMap<String, MouseStateHandle>,
    /// DraggableState per row — maintains drag progress across renders, so must be cached in view state.
    row_drag_states: HashMap<String, DraggableState>,

    context_menu_position: Option<Vector2F>,
    context_menu_target: Option<String>,
    context_menu_item_states: Vec<MouseStateHandle>,

    /// The node currently being renamed (editor + node_id).
    rename_state: Option<RenameState>,

    /// `~/.ssh/config` Candidate view models - PRODUCT.md decision A/B/C/D/E.
    candidates: ModelHandle<CandidatesViewModel>,
    /// The hover state(key = alias) of each candidate row.
    candidate_row_states: HashMap<String, MouseStateHandle>,
    /// The hover state (key = alias) of the "+" / "Added" button for each candidate row.
    candidate_add_states: HashMap<String, MouseStateHandle>,
    /// Section header Refresh / Toggle button hover state.
    candidates_refresh_btn: MouseStateHandle,
    candidates_toggle_btn: MouseStateHandle,
}

impl SshManagerPanel {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let candidates = ctx.add_model(|_| CandidatesViewModel::new());

        let mut me = Self {
            nodes: Vec::new(),
            depths: HashMap::new(),
            selected_id: None,
            add_folder_btn: MouseStateHandle::default(),
            add_server_btn: MouseStateHandle::default(),
            toggle_all_btn: MouseStateHandle::default(),
            row_states: HashMap::new(),
            row_drag_states: HashMap::new(),
            context_menu_position: None,
            context_menu_target: None,
            context_menu_item_states: (0..MAX_CONTEXT_MENU_ITEMS)
                .map(|_| MouseStateHandle::default())
                .collect(),
            rename_state: None,
            candidates,
            candidate_row_states: HashMap::new(),
            candidate_add_states: HashMap::new(),
            candidates_refresh_btn: MouseStateHandle::default(),
            candidates_toggle_btn: MouseStateHandle::default(),
        };
        // The first time the panel is opened → read ssh_config(PRODUCT.md decision A) immediately.
        me.candidates.update(ctx, |vm, ctx| vm.refresh(ctx));
        me.refresh_tree(ctx);

        ctx.subscribe_to_model(
            &SshTreeChangedNotifier::handle(ctx),
            |me, _, event, ctx| match event {
                SshTreeChangedEvent::TreeChanged => me.refresh_tree(ctx),
            },
        );

        me
    }

    fn refresh_tree(&mut self, ctx: &mut ViewContext<Self>) {
        match warp_ssh_manager::with_conn(|c| Ok(SshRepository::list_nodes(c)?)) {
            Ok(nodes) => {
                self.depths = compute_depths(&nodes);
                self.nodes = sort_for_display(nodes, &self.depths);
                if let Some(id) = self.selected_id.clone() {
                    if !self.nodes.iter().any(|n| n.id == id) {
                        self.selected_id = None;
                    }
                }
                // If the node being renamed is deleted externally, clear rename_state
                if let Some(rs) = self.rename_state.as_ref() {
                    if !self.nodes.iter().any(|n| n.id == rs.node_id) {
                        self.rename_state = None;
                    }
                }
            }
            Err(e) => {
                log::error!("ssh_manager: failed to load tree: {e:?}");
                ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            }
        }

        let active_ids: std::collections::HashSet<&str> =
            self.nodes.iter().map(|n| n.id.as_str()).collect();
        self.row_states
            .retain(|k, _| active_ids.contains(k.as_str()));
        self.row_drag_states
            .retain(|k, _| active_ids.contains(k.as_str()));
        for n in &self.nodes {
            self.row_states.entry(n.id.clone()).or_default();
            self.row_drag_states.entry(n.id.clone()).or_default();
        }

        // Tree changes → Recalculate "Added" set (PRODUCT.md decision E). "Imported" button
        // `server.host == candidate.alias` determination - and writing of ImportCandidate
        // Semantic alignment (decision I: `server.host = alias` when importing).
        let hosts = list_server_hosts();
        self.candidates
            .update(ctx, |vm, ctx| vm.on_tree_changed(hosts, ctx));
        self.sync_candidate_row_states(ctx);

        ctx.notify();
    }

    /// Let the key collection of `candidate_row_states` / `candidate_add_states` be the same as the current
    /// The alias of candidates view-model is consistent. Delete redundant hover state (release memory),
    /// Fill in the missing alias with a default state (the first hover after the row is added will not lose the state).
    fn sync_candidate_row_states(&mut self, ctx: &mut ViewContext<Self>) {
        let aliases: Vec<String> = self
            .candidates
            .as_ref(ctx)
            .rows()
            .into_iter()
            .filter_map(|r| match r {
                CandidateRow::Candidate { alias, .. } => Some(alias),
                CandidateRow::Header { .. }
                | CandidateRow::NotFound { .. }
                | CandidateRow::Empty { .. }
                | CandidateRow::Error { .. } => None,
            })
            .collect();
        let alias_set: std::collections::HashSet<&str> =
            aliases.iter().map(|s| s.as_str()).collect();
        self.candidate_row_states
            .retain(|k, _| alias_set.contains(k.as_str()));
        self.candidate_add_states
            .retain(|k, _| alias_set.contains(k.as_str()));
        for a in aliases {
            self.candidate_row_states.entry(a.clone()).or_default();
            self.candidate_add_states.entry(a).or_default();
        }
    }

    fn on_add_folder(&mut self, ctx: &mut ViewContext<Self>) {
        let parent = self.parent_for_new_node();
        let result = warp_ssh_manager::with_conn(|c| {
            let name = unique_name(c, parent.as_deref(), "New folder")?;
            Ok(SshRepository::create_folder(c, parent.as_deref(), &name)?)
        });
        match result {
            Ok(node) => {
                let new_id = node.id.clone();
                self.selected_id = Some(new_id.clone());
                self.refresh_tree(ctx);
                // Create new and rename — Drive habit.
                self.enter_rename(new_id, ctx);
            }
            Err(e) => {
                log::error!("ssh_manager: create folder failed: {e:?}");
                ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            }
        }
    }

    /// Import a candidate in `~/.ssh/config` as a new saved server.
    ///
    /// Field mapping strictly follows TECH.md §3.3 / PRODUCT.md decision I/J/K:
    /// - `server.host = alias` (retains OpenSSH alias semantics and can still be used when starting `ssh`
    ///   `ProxyJump` and other commands in `~/.ssh/config`);
    /// - `port = candidate.port.unwrap_or(22)`("port=None → 22" for decision K);
    /// - `auth_type = Key if identity_file.is_some() else Password`(decision J);
    /// - `notes = "Imported from <resolved path>"` (for users to trace back to the source in the future).
    ///
    /// Write through `SshRepository::create_server`, follow the same path as manual "New server"
    /// Persistence path - any schema changes to this SQLite row, the import path automatically follows.
    /// After completion, emit `OpenServerEditor` (consistent with manual creation) + broadcast
    /// `SshTreeChangedEvent::TreeChanged` causes the `Added` badge to flip immediately.
    fn on_import_candidate(&mut self, alias: String, ctx: &mut ViewContext<Self>) {
        let candidate = self
            .candidates
            .read(ctx, |vm, _| vm.find_candidate(&alias).cloned());
        let Some(c) = candidate else {
            log::warn!("ssh_manager: ImportCandidate alias not found: {alias}");
            return;
        };
        let path_display = self
            .candidates
            .read(ctx, |vm, _| vm.path_display())
            .unwrap_or_default();

        let auth_type = if c.identity_file.is_some() {
            AuthType::Key
        } else {
            AuthType::Password
        };
        let info = SshServerInfo {
            node_id: String::new(),
            // PRODUCT.md decision I: Save the alias instead of the resolved HostName.
            host: c.alias.clone(),
            port: c.port.unwrap_or(22),
            username: c.user.clone().unwrap_or_default(),
            auth_type,
            key_path: c
                .identity_file
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            startup_command: None,
            notes: Some(format!("Imported from {path_display}")),
            last_connected_at: None,
        };

        let parent = self.parent_for_new_node();
        let result = warp_ssh_manager::with_conn(|conn| {
            // The "automatic avoidance" logic with the same name as the manual "New server" (unique_name);
            // The first candidate uses an alias as the name, and adds "2", "3"...
            let name = unique_name(conn, parent.as_deref(), &c.alias)?;
            Ok(SshRepository::create_server(
                conn,
                parent.as_deref(),
                &name,
                &info,
            )?)
        });
        match result {
            Ok(node) => {
                let new_id = node.id.clone();
                self.selected_id = Some(new_id.clone());
                self.refresh_tree(ctx);
                // Consistent with manual new creation: Open the central editing pane and let the user fill in the password/tweak fields.
                ctx.emit(SshManagerPanelEvent::OpenServerEditor { node_id: new_id });
                // Broadcast tree changes - the Candidates section's added_aliases are refreshed accordingly.
                SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
                    ctx.emit(SshTreeChangedEvent::TreeChanged);
                });
            }
            Err(e) => {
                log::error!("ssh_manager: import candidate failed: {e:?}");
                ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            }
        }
    }

    fn on_add_server(&mut self, ctx: &mut ViewContext<Self>) {
        let parent = self.parent_for_new_node();
        let info_template = SshServerInfo::new_default(String::new());
        let result = warp_ssh_manager::with_conn(|c| {
            let name = unique_name(c, parent.as_deref(), "New server")?;
            Ok(SshRepository::create_server(
                c,
                parent.as_deref(),
                &name,
                &info_template,
            )?)
        });
        match result {
            Ok(node) => {
                let new_id = node.id.clone();
                self.selected_id = Some(new_id.clone());
                self.refresh_tree(ctx);
                // After the server is created, open the central editing pane (user-filled fields)—name editing and fields
                // Make changes there together, not inline editing in the tree.
                ctx.emit(SshManagerPanelEvent::OpenServerEditor { node_id: new_id });
            }
            Err(e) => {
                log::error!("ssh_manager: create server failed: {e:?}");
                ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            }
        }
    }

    fn on_clone_server(&mut self, source_id: &str, ctx: &mut ViewContext<Self>) {
        let source_id = source_id.to_string();
        let result = warp_ssh_manager::with_conn(|c| {
            let source_info = SshRepository::get_server(c, &source_id)?
                .ok_or_else(|| warp_ssh_manager::SshRepositoryError::NotFound(source_id.clone()))?;
            let source_node = SshRepository::list_nodes(c)?
                .into_iter()
                .find(|n| n.id == source_id)
                .ok_or_else(|| warp_ssh_manager::SshRepositoryError::NotFound(source_id.clone()))?;

            let parent = source_node.parent_id;
            let cloned_info = SshServerInfo::clone_from_template(&source_info, String::new());
            let name = unique_name(c, parent.as_deref(), &format!("{} (copy)", source_node.name))?;

            let new_node = SshRepository::create_server(c, parent.as_deref(), &name, &cloned_info)?;

            // The source server has been verified to exist above, and the password/private key password in the keychain is directly copied to the new node.
            let store = KeychainSecretStore;
            if let Ok(Some(password)) = store.get(&source_id, SecretKind::Password) {
                let _ = store.set(&new_node.id, SecretKind::Password, &password);
            }
            if let Ok(Some(passphrase)) = store.get(&source_id, SecretKind::Passphrase) {
                let _ = store.set(&new_node.id, SecretKind::Passphrase, &passphrase);
            }

            Ok(new_node)
        });
        match result {
            Ok(node) => {
                let new_id = node.id.clone();
                self.selected_id = Some(new_id.clone());
                self.refresh_tree(ctx);
                ctx.emit(SshManagerPanelEvent::OpenServerEditor { node_id: new_id });
            }
            Err(e) => {
                log::error!("ssh_manager: clone server failed: {e:?}");
                ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            }
        }
    }

    fn on_delete_selected(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        let result = warp_ssh_manager::with_conn(|c| Ok(SshRepository::delete_node(c, &id)?));
        if let Err(e) = result {
            log::error!("ssh_manager: delete failed: {e:?}");
            ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            return;
        }
        let store = KeychainSecretStore;
        let _ = store.delete(&id, SecretKind::Password);
        let _ = store.delete(&id, SecretKind::Passphrase);
        let _ = store.delete(&id, SecretKind::RootPassword);

        self.selected_id = None;
        self.refresh_tree(ctx);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
    }

    fn on_connect(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        self.dispatch_connect_for(&id, ctx);
    }

    fn dispatch_connect_for(&self, id: &str, ctx: &mut ViewContext<Self>) {
        let kind = self.nodes.iter().find(|n| n.id == id).map(|n| n.kind);
        if !matches!(kind, Some(NodeKind::Server)) {
            return;
        }
        let server = warp_ssh_manager::with_conn(|c| Ok(SshRepository::get_server(c, id)?))
            .ok()
            .flatten();
        if let Some(server) = server {
            ctx.emit(SshManagerPanelEvent::OpenSshTerminal {
                node_id: id.to_string(),
                server,
            });
        }
    }

    fn on_edit(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        let kind = self.nodes.iter().find(|n| n.id == id).map(|n| n.kind);
        if !matches!(kind, Some(NodeKind::Server)) {
            // "edit" = rename for folder
            self.enter_rename(id, ctx);
            return;
        }
        ctx.emit(SshManagerPanelEvent::OpenServerEditor { node_id: id });
    }

    /// Double-click server = connection (open a new tab). Folder double click = two toggle cancel each other, no-op.
    fn on_double_click(&mut self, id: String, ctx: &mut ViewContext<Self>) {
        let kind = self.nodes.iter().find(|n| n.id == id).map(|n| n.kind);
        if matches!(kind, Some(NodeKind::Server)) {
            self.dispatch_connect_for(&id, ctx);
        }
    }

    /// Toggles the collapsed state of a single folder; ignored for server nodes.
    fn on_toggle_node_collapsed(&mut self, node_id: &str, ctx: &mut ViewContext<Self>) {
        let kind = self.nodes.iter().find(|n| n.id == node_id).map(|n| n.kind);
        if !matches!(kind, Some(NodeKind::Folder)) {
            return;
        }
        let new_collapsed = !self
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .map(|n| n.is_collapsed)
            .unwrap_or(false);
        let id = node_id.to_string();
        let result = warp_ssh_manager::with_conn(move |c| {
            Ok(SshRepository::set_collapsed(c, &id, new_collapsed)?)
        });
        if let Err(e) = result {
            log::error!("ssh_manager: toggle collapse failed: {e:?}");
            ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            return;
        }
        self.refresh_tree(ctx);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
    }

    /// Top buttons: Any folder is currently expanded → Collapse all; all are collapsed → Expand all.
    fn on_toggle_all_folders(&mut self, ctx: &mut ViewContext<Self>) {
        let any_expanded = self
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Folder) && !n.is_collapsed);
        let new_collapsed = any_expanded; // At least one expands → all is collapsed; otherwise all is displayed
        let result = warp_ssh_manager::with_conn(|c| {
            Ok(SshRepository::set_all_folders_collapsed(c, new_collapsed)?)
        });
        if let Err(e) = result {
            log::error!("ssh_manager: toggle all failed: {e:?}");
            ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            return;
        }
        self.refresh_tree(ctx);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
    }

    /// Whether the node is visually visible — hidden if any ancestor folder is collapsed.
    /// root-level nodes are always visible.
    fn is_visible(&self, node: &SshNode) -> bool {
        let mut cursor = node.parent_id.as_deref();
        while let Some(pid) = cursor {
            let parent = match self.nodes.iter().find(|n| n.id == pid) {
                Some(p) => p,
                None => return true, // Data is inconsistent, displayed for safety reasons
            };
            if matches!(parent.kind, NodeKind::Folder) && parent.is_collapsed {
                return false;
            }
            cursor = parent.parent_id.as_deref();
        }
        true
    }

    fn on_click(&mut self, id: String, ctx: &mut ViewContext<Self>) {
        // Click on another row = exit current rename (commit)
        if self
            .rename_state
            .as_ref()
            .map(|rs| rs.node_id != id)
            .unwrap_or(false)
        {
            self.commit_rename(ctx);
        }

        self.selected_id = Some(id.clone());
        let kind = self.nodes.iter().find(|n| n.id == id).map(|n| n.kind);
        match kind {
            Some(NodeKind::Server) => {
                // Click server = selected only. **Double-click to connect**(`on_double_click`).
            }
            Some(NodeKind::Folder) => {
                // Click the folder = collapse/expand toggle (selected already done above)
                self.on_toggle_node_collapsed(&id, ctx);
                return; // on_toggle has ctx.notify internally
            }
            None => {}
        }
        ctx.notify();
    }

    fn on_open_context_menu(
        &mut self,
        target: Option<String>,
        position: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        // Turn off rename before opening the menu (otherwise the rename buffer will be lost).
        if self.rename_state.is_some() {
            self.commit_rename(ctx);
        }
        if let Some(t) = target.as_ref() {
            self.selected_id = Some(t.clone());
        }
        self.context_menu_target = target;
        self.context_menu_position = Some(position);
        ctx.notify();
    }

    fn on_dismiss_context_menu(&mut self, ctx: &mut ViewContext<Self>) {
        self.context_menu_position = None;
        self.context_menu_target = None;
        ctx.notify();
    }

    fn enter_rename(&mut self, node_id: String, ctx: &mut ViewContext<Self>) {
        let current_name = self
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .map(|n| n.name.clone())
            .unwrap_or_default();

        let editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = warp_core::ui::appearance::Appearance::as_ref(ctx);
            let theme = appearance.theme();
            let options = SingleLineEditorOptions {
                is_password: false,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_subheading()),
                    font_family_override: Some(appearance.ui_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: theme.active_ui_text_color(),
                        disabled_color: theme.disabled_ui_text_color(),
                        hint_color: theme.disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_buffer_text(&current_name, ctx);
            editor
        });

        // Listen for Enter / Blurred → commit; Escape → cancel.
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| match event {
            EditorEvent::Enter => me.commit_rename(ctx),
            EditorEvent::Blurred => me.commit_rename(ctx),
            EditorEvent::Escape => me.cancel_rename(ctx),
            _ => {}
        });

        ctx.focus(&editor);
        self.rename_state = Some(RenameState { node_id, editor });
        ctx.notify();
    }

    fn commit_rename(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(rs) = self.rename_state.take() else {
            return;
        };
        let new_name = rs.editor.as_ref(ctx).buffer_text(ctx).trim().to_string();
        if new_name.is_empty() {
            // Name cannot be empty: Cancel
            ctx.notify();
            return;
        }
        let id = rs.node_id.clone();
        let result =
            warp_ssh_manager::with_conn(|c| Ok(SshRepository::rename_node(c, &id, &new_name)?));
        if let Err(e) = result {
            log::error!("ssh_manager: rename failed: {e:?}");
            ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            return;
        }
        self.refresh_tree(ctx);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
    }

    fn cancel_rename(&mut self, ctx: &mut ViewContext<Self>) {
        self.rename_state = None;
        ctx.notify();
    }

    /// Check whether moving `dragged` to `new_parent` will create a cycle (`new_parent` is
    /// If the descendant/self/of `dragged` is already the current parent, it will be directly rejected (save writing once).
    fn move_is_legal(&self, dragged: &str, new_parent: Option<&str>) -> bool {
        // Move under self: forbidden
        if Some(dragged) == new_parent {
            return false;
        }
        // Unmoved:reject (avoid idempotent writing)
        let current_parent = self
            .nodes
            .iter()
            .find(|n| n.id == dragged)
            .and_then(|n| n.parent_id.as_deref());
        if current_parent == new_parent {
            return false;
        }
        // Move the folder to its own descendants: prohibit (ring)
        if let Some(target_parent) = new_parent {
            let mut cursor = Some(target_parent);
            while let Some(id) = cursor {
                if id == dragged {
                    return false;
                }
                cursor = self
                    .nodes
                    .iter()
                    .find(|n| n.id == id)
                    .and_then(|n| n.parent_id.as_deref());
            }
        }
        true
    }

    fn on_move_node(
        &mut self,
        node_id: String,
        new_parent_id: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.move_is_legal(&node_id, new_parent_id.as_deref()) {
            // Upgrade to warn: This log is easier to find than debug when the user cannot see the effect of dragging.
            // Most false comes from "drag to parent / drag to self".
            let current_parent = self
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .and_then(|n| n.parent_id.clone());
            log::warn!(
                "ssh_manager: move rejected. node={node_id} current_parent={current_parent:?} target_parent={new_parent_id:?}"
            );
            return;
        }
        // sort_order takes the current maximum value of the target parent +1 (ranked at the end). Simplified way:
        // Use i32::MAX/2 to tell the SQL layer to put it last (followed by normalize). Go here SQL
        // Query to get true next_sort_order.
        let result = warp_ssh_manager::with_conn(|c| {
            use diesel::prelude::*;
            use persistence::schema::ssh_nodes;
            let max: Option<i32> = match new_parent_id.as_deref() {
                Some(p) => ssh_nodes::table
                    .filter(ssh_nodes::parent_id.eq(p))
                    .select(diesel::dsl::max(ssh_nodes::sort_order))
                    .first(c)?,
                None => ssh_nodes::table
                    .filter(ssh_nodes::parent_id.is_null())
                    .select(diesel::dsl::max(ssh_nodes::sort_order))
                    .first(c)?,
            };
            let next_sort = max.unwrap_or(-1) + 1;
            Ok(SshRepository::move_node(
                c,
                &node_id,
                new_parent_id.as_deref(),
                next_sort,
            )?)
        });
        if let Err(e) = result {
            log::error!("ssh_manager: move failed: {e:?}");
            ctx.emit(SshManagerPanelEvent::PersistenceError(e.to_string()));
            return;
        }
        self.refresh_tree(ctx);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
    }

    fn parent_for_new_node(&self) -> Option<String> {
        let id = self.selected_id.as_ref()?;
        let node = self.nodes.iter().find(|n| &n.id == id)?;
        match node.kind {
            NodeKind::Folder => Some(node.id.clone()),
            NodeKind::Server => node.parent_id.clone(),
        }
    }

    fn render_toolbar(
        &self,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let icon_color = theme.sub_text_color(theme.background());

        let make_btn = |icon: crate::ui_components::icons::Icon,
                        state: MouseStateHandle,
                        action: SshManagerPanelAction|
         -> Box<dyn Element> {
            let icon_el = ConstrainedBox::new(icon.to_warpui_icon(icon_color).finish())
                .with_width(TOOLBAR_ICON_SIZE)
                .with_height(TOOLBAR_ICON_SIZE)
                .finish();
            Hoverable::new(state, move |_| {
                Container::new(
                    ConstrainedBox::new(icon_el)
                        .with_width(TOOLBAR_BUTTON_SIZE)
                        .with_height(TOOLBAR_BUTTON_SIZE)
                        .finish(),
                )
                .with_uniform_padding(2.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(action.clone());
            })
            .finish()
        };

        // Left group: New button
        let left_group = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(make_btn(
                crate::ui_components::icons::Icon::Folder,
                self.add_folder_btn.clone(),
                SshManagerPanelAction::AddFolder,
            ))
            .with_child(make_btn(
                crate::ui_components::icons::Icon::Plus,
                self.add_server_btn.clone(),
                SshManagerPanelAction::AddServer,
            ))
            .with_main_axis_size(MainAxisSize::Min)
            .finish();

        // Right: Collapse/Expand All button — smart switch. Any folder currently expanded → shown
        // ChevronUp (meaning "folded"), otherwise ChevronDown (meaning "expanded") is displayed.
        let any_expanded = self
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Folder) && !n.is_collapsed);
        let toggle_icon = if any_expanded {
            crate::ui_components::icons::Icon::ChevronUp
        } else {
            crate::ui_components::icons::Icon::ChevronDown
        };
        let right_group = make_btn(
            toggle_icon,
            self.toggle_all_btn.clone(),
            SshManagerPanelAction::ToggleAllFolders,
        );

        // The entire toolbar: aligns the left and right ends (MainAxisAlignment::SpaceBetween).
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(left_group)
            .with_child(right_group)
            .finish()
    }

    /// "Candidates" section - A list of importable hosts parsed from `~/.ssh/config`.
    ///
    /// Sections are displayed above the saved tree; the layout style (line height, indentation, font size) remains consistent with the tree.
    /// There is only one more Refresh button + collapsed chevron in the section header. Each candidate line is followed by a
    /// "+" or "Added" badge (PRODUCT.md decision E).
    fn render_candidates(
        &self,
        appearance: &warp_core::ui::appearance::Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let rows = self.candidates.as_ref(app).rows();
        if rows.is_empty() {
            // The refresh has not been adjusted yet - the section will not be rendered at all (this should not happen when the panel is not mounted yet,
            // Because it is called immediately in new(), but it’s just to be on the safe side.)
            return Empty::new().finish();
        }

        let muted = theme.sub_text_color(theme.background());
        let main = theme.main_text_color(theme.background());

        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for row in rows {
            match row {
                CandidateRow::Header {
                    path_display,
                    count,
                    can_refresh,
                } => {
                    col.add_child(self.render_candidates_header(
                        &path_display,
                        count,
                        can_refresh,
                        appearance,
                        app,
                    ));
                }
                CandidateRow::NotFound { path_display } => {
                    col.add_child(self.render_candidates_message(
                        &crate::t!(
                            "workspace-left-panel-ssh-manager-candidates-not-found",
                            path = path_display
                        ),
                        muted,
                        appearance,
                    ));
                }
                CandidateRow::Empty { path_display } => {
                    col.add_child(self.render_candidates_message(
                        &crate::t!(
                            "workspace-left-panel-ssh-manager-candidates-empty",
                            path = path_display
                        ),
                        muted,
                        appearance,
                    ));
                }
                CandidateRow::Error {
                    path_display,
                    message,
                } => {
                    // Error lines use error red - `ui_error_color` returns ColorU directly,
                    // Use the same routine as the "excess word counter" in `ai_assistant/panel.rs`.
                    let err_color: pathfinder_color::ColorU = theme.ui_error_color();
                    col.add_child(self.render_candidates_message_color(
                        &crate::t!(
                            "workspace-left-panel-ssh-manager-candidates-error",
                            path = path_display,
                            error = message
                        ),
                        err_color,
                        appearance,
                    ));
                }
                CandidateRow::Candidate {
                    alias,
                    hostname,
                    user,
                    port,
                    identity_file: _,
                    added,
                } => {
                    col.add_child(self.render_candidate_row(
                        CandidateRowFields {
                            alias: &alias,
                            hostname: hostname.as_deref(),
                            user: user.as_deref(),
                            port,
                            added,
                        },
                        CandidateRowColors { main, muted },
                        appearance,
                    ));
                }
            }
        }

        col.with_main_axis_size(MainAxisSize::Min).finish()
    }

    fn render_candidates_header(
        &self,
        path_display: &str,
        count: usize,
        can_refresh: bool,
        appearance: &warp_core::ui::appearance::Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let icon_color = theme.sub_text_color(theme.background());
        let muted = theme.sub_text_color(theme.background());

        // Folded state chevron(▶) vs expanded state (▼) —— is_expanded is taken directly from view-model.
        let expanded = self.candidates.as_ref(app).is_expanded();
        let chevron_icon = if expanded {
            crate::ui_components::icons::Icon::ChevronDown
        } else {
            crate::ui_components::icons::Icon::ChevronRight
        };
        let chevron_el = ConstrainedBox::new(chevron_icon.to_warpui_icon(icon_color).finish())
            .with_width(ITEM_ICON_SIZE)
            .with_height(ITEM_ICON_SIZE)
            .finish();

        let header_text = crate::t!(
            "workspace-left-panel-ssh-manager-candidates-header",
            path = path_display
        );
        let label = Text::new_inline(
            header_text,
            appearance.ui_font_family(),
            appearance.ui_font_subheading(),
        )
        .with_color(muted.into())
        .finish();

        let count_label = Text::new_inline(
            format!("({count})"),
            appearance.ui_font_family(),
            appearance.ui_font_body(),
        )
        .with_color(muted.into())
        .finish();

        // Refresh button on the right - Refresh is allowed in any state (NotFound / Error / Loaded).
        let refresh_state = self.candidates_refresh_btn.clone();
        let refresh_icon = ConstrainedBox::new(
            crate::ui_components::icons::Icon::Refresh
                .to_warpui_icon(icon_color)
                .finish(),
        )
        .with_width(ITEM_ICON_SIZE)
        .with_height(ITEM_ICON_SIZE)
        .finish();
        let refresh_btn = if can_refresh {
            Hoverable::new(refresh_state, move |_| {
                Container::new(refresh_icon)
                    .with_uniform_padding(2.0)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(SshManagerPanelAction::RefreshCandidates);
            })
            .finish()
        } else {
            refresh_icon
        };

        // Whole row: chevron + label (eating the middle space) + count + Refresh button.
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(ITEM_ICON_TEXT_SPACING)
            .with_child(chevron_el)
            .with_child(label)
            .with_child(count_label)
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(8.0)
                    .finish(),
            )
            .with_child(refresh_btn)
            .with_main_axis_size(MainAxisSize::Min)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .finish();

        // Entire header click = toggle (similar to the folder row's single-click).
        let toggle_state = self.candidates_toggle_btn.clone();
        Hoverable::new(toggle_state, move |_| {
            Container::new(row)
                .with_padding_top(ITEM_PADDING_VERTICAL)
                .with_padding_bottom(ITEM_PADDING_VERTICAL)
                .with_padding_left(ITEM_PADDING_HORIZONTAL)
                .with_padding_right(ITEM_PADDING_HORIZONTAL)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(SshManagerPanelAction::ToggleCandidatesSection);
        })
        .finish()
    }

    fn render_candidates_message(
        &self,
        text: &str,
        color: warp_core::ui::theme::Fill,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        Container::new(
            Text::new_inline(
                text.to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_body(),
            )
            .with_color(color.into())
            .finish(),
        )
        .with_padding_top(ITEM_PADDING_VERTICAL)
        .with_padding_bottom(ITEM_PADDING_VERTICAL)
        .with_padding_left(ITEM_PADDING_HORIZONTAL + FOLDER_DEPTH_INDENT)
        .with_padding_right(ITEM_PADDING_HORIZONTAL)
        .finish()
    }

    /// Same as `render_candidates_message`, but receives `ColorU` - Error lines are themed
    /// `ui_error_color()` directly returns the red color to avoid having to go through Fill packaging again.
    fn render_candidates_message_color(
        &self,
        text: &str,
        color: pathfinder_color::ColorU,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        Container::new(
            Text::new_inline(
                text.to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_body(),
            )
            .with_color(color)
            .finish(),
        )
        .with_padding_top(ITEM_PADDING_VERTICAL)
        .with_padding_bottom(ITEM_PADDING_VERTICAL)
        .with_padding_left(ITEM_PADDING_HORIZONTAL + FOLDER_DEPTH_INDENT)
        .with_padding_right(ITEM_PADDING_HORIZONTAL)
        .finish()
    }

    fn render_candidate_row(
        &self,
        fields: CandidateRowFields<'_>,
        colors: CandidateRowColors,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        let CandidateRowFields {
            alias,
            hostname,
            user,
            port,
            added,
        } = fields;
        let CandidateRowColors { main, muted } = colors;
        let theme = appearance.theme();
        let icon = crate::ui_components::icons::Icon::Key
            .to_warpui_icon(theme.sub_text_color(theme.background()))
            .finish();
        let icon_el = ConstrainedBox::new(icon)
            .with_width(ITEM_ICON_SIZE)
            .with_height(ITEM_ICON_SIZE)
            .finish();

        // Primary label = alias; secondary label = "user@hostname:port" abbreviation, both are optional.
        // When imported, the font color of the entire line is dimmed (decision E: dimmed).
        let label_color = if added { muted } else { main };
        let alias_text = Text::new_inline(
            alias.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_subheading(),
        )
        .with_color(label_color.into())
        .finish();

        let mut subtitle_parts: Vec<String> = Vec::new();
        if let Some(u) = user {
            subtitle_parts.push(u.to_string());
        }
        if let Some(h) = hostname {
            // user@host; only host is displayed when there is no user
            let last = subtitle_parts.last_mut();
            match last {
                Some(s) => *s = format!("{s}@{h}"),
                None => subtitle_parts.push(h.to_string()),
            }
        }
        if let Some(p) = port {
            // Add :port to the end of the last paragraph; if user/hostname are both missing, use :port alone.
            if let Some(last) = subtitle_parts.last_mut() {
                *last = format!("{last}:{p}");
            } else {
                subtitle_parts.push(format!(":{p}"));
            }
        }
        let subtitle: Option<Box<dyn Element>> = if subtitle_parts.is_empty() {
            None
        } else {
            Some(
                Text::new_inline(
                    subtitle_parts.join(" "),
                    appearance.ui_font_family(),
                    appearance.ui_font_body(),
                )
                .with_color(muted.into())
                .finish(),
            )
        };

        let mut label_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(alias_text);
        if let Some(s) = subtitle {
            label_col.add_child(s);
        }
        let label_block = label_col.with_main_axis_size(MainAxisSize::Min).finish();

        // A "+" button or "Added" badge at the end of the line.
        let add_state = self
            .candidate_add_states
            .get(alias)
            .cloned()
            .unwrap_or_default();
        let alias_for_click = alias.to_string();
        let trailing: Box<dyn Element> = if added {
            // PRODUCT.md decision E: Imported → Display "Added" (no click interaction).
            Text::new_inline(
                crate::t!("workspace-left-panel-ssh-manager-candidates-added"),
                appearance.ui_font_family(),
                appearance.ui_font_body(),
            )
            .with_color(muted.into())
            .finish()
        } else {
            let plus_icon = ConstrainedBox::new(
                crate::ui_components::icons::Icon::Plus
                    .to_warpui_icon(theme.sub_text_color(theme.background()))
                    .finish(),
            )
            .with_width(ITEM_ICON_SIZE)
            .with_height(ITEM_ICON_SIZE)
            .finish();
            Hoverable::new(add_state, move |_| {
                Container::new(plus_icon)
                    .with_uniform_padding(2.0)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(SshManagerPanelAction::ImportCandidate {
                    alias: alias_for_click.clone(),
                });
            })
            .finish()
        };

        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(ITEM_ICON_TEXT_SPACING)
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(FOLDER_DEPTH_INDENT)
                    .finish(),
            )
            .with_child(icon_el)
            .with_child(label_block)
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(8.0)
                    .finish(),
            )
            .with_child(trailing)
            .with_main_axis_size(MainAxisSize::Min)
            .finish();

        let row_state = self
            .candidate_row_states
            .get(alias)
            .cloned()
            .unwrap_or_default();
        Hoverable::new(row_state, move |mouse| {
            let mut c = Container::new(row)
                .with_padding_top(ITEM_PADDING_VERTICAL)
                .with_padding_bottom(ITEM_PADDING_VERTICAL)
                .with_padding_left(ITEM_PADDING_HORIZONTAL)
                .with_padding_right(ITEM_PADDING_HORIZONTAL)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            if mouse.is_hovered() {
                c = c.with_background(internal_colors::fg_overlay_3(theme));
            }
            c.finish()
        })
        .finish()
    }

    fn render_tree(&self, appearance: &warp_core::ui::appearance::Appearance) -> Box<dyn Element> {
        let mut col = Flex::column();

        if self.nodes.is_empty() {
            let theme = appearance.theme();
            let muted = theme.sub_text_color(theme.background());
            col.add_child(
                Container::new(
                    Text::new_inline(
                        crate::t!("workspace-left-panel-ssh-manager-tree-empty"),
                        appearance.ui_font_family(),
                        appearance.ui_font_subheading(),
                    )
                    .with_color(muted.into())
                    .finish(),
                )
                .with_padding_top(20.0)
                .with_padding_bottom(20.0)
                .with_padding_left(ITEM_PADDING_HORIZONTAL)
                .with_padding_right(ITEM_PADDING_HORIZONTAL)
                .finish(),
            );
        } else {
            for node in &self.nodes {
                if !self.is_visible(node) {
                    continue;
                }
                col.add_child(self.render_row(node, appearance));
            }
        }
        let inner = col
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .finish();
        // Right click on blank space = OpenContextMenu of node None.
        let hoverable = Hoverable::new(MouseStateHandle::default(), move |_| inner)
            .on_right_click(|ctx, _, position| {
                let offset = match ctx.element_position_by_id(SSH_PANEL_POSITION_ID) {
                    Some(bounds) => position - bounds.origin(),
                    None => position,
                };
                ctx.dispatch_typed_action(SshManagerPanelAction::OpenContextMenu {
                    target: None,
                    position: offset,
                });
            })
            .finish();
        // The entire tree area is also a drop target, parent_id=None means dragging to the root.
        // Row-level DropTarget has a higher priority (smaller), so dragging it to a folder will still lead to the folder.
        DropTarget::new(hoverable, SshDropData { parent_id: None }).finish()
    }

    fn render_row(
        &self,
        node: &SshNode,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let depth = self.depths.get(&node.id).copied().unwrap_or(0);
        let is_selected = self.selected_id.as_deref() == Some(node.id.as_str());
        let is_renaming = self
            .rename_state
            .as_ref()
            .map(|rs| rs.node_id == node.id)
            .unwrap_or(false);

        let icon = match node.kind {
            NodeKind::Folder => crate::ui_components::icons::Icon::Folder,
            NodeKind::Server => crate::ui_components::icons::Icon::Key,
        };
        let icon_color = theme.sub_text_color(theme.background());
        let icon_el = ConstrainedBox::new(icon.to_warpui_icon(icon_color).finish())
            .with_width(ITEM_ICON_SIZE)
            .with_height(ITEM_ICON_SIZE)
            .finish();

        // Add chevron (▼ expand / ▶ fold) in front of the Folder line; use equal-width blank space for the Server line
        // Align all rows of icons.
        let chevron_el: Box<dyn Element> = match node.kind {
            NodeKind::Folder => {
                let chevron_icon = if node.is_collapsed {
                    crate::ui_components::icons::Icon::ChevronRight
                } else {
                    crate::ui_components::icons::Icon::ChevronDown
                };
                ConstrainedBox::new(chevron_icon.to_warpui_icon(icon_color).finish())
                    .with_width(ITEM_ICON_SIZE)
                    .with_height(ITEM_ICON_SIZE)
                    .finish()
            }
            NodeKind::Server => ConstrainedBox::new(Empty::new().finish())
                .with_width(ITEM_ICON_SIZE)
                .finish(),
        };

        // Right half — text or rename input box.
        // EditorView must be rendered in a limited-width container, otherwise element.rs:1670 will
        // panic("infinite width constraint on buffer elements"). child of Flex::row
        // There is no column-stretch semantics, so ConstrainedBox is included here to give a fixed width.
        let label_or_editor: Box<dyn Element> = if is_renaming {
            let editor_handle = self
                .rename_state
                .as_ref()
                .map(|rs| rs.editor.clone())
                .expect("is_renaming implies rename_state.is_some");
            let input = appearance
                .ui_builder()
                .text_input(editor_handle)
                .with_style(UiComponentStyles {
                    padding: Some(Coords {
                        left: 4.0,
                        right: 4.0,
                        top: 1.0,
                        bottom: 1.0,
                    }),
                    background: Some(theme.surface_2().into()),
                    border_color: Some(theme.accent().into()),
                    border_width: Some(1.0),
                    border_radius: Some(CornerRadius::with_all(Radius::Pixels(3.0))),
                    font_size: Some(appearance.ui_font_subheading()),
                    ..Default::default()
                })
                .build()
                .finish();
            ConstrainedBox::new(input).with_width(180.0).finish()
        } else {
            Text::new_inline(
                node.name.clone(),
                appearance.ui_font_family(),
                appearance.ui_font_subheading(),
            )
            .with_color(theme.main_text_color(theme.background()).into())
            .finish()
        };

        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(ITEM_ICON_TEXT_SPACING)
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(depth as f32 * FOLDER_DEPTH_INDENT)
                    .finish(),
            )
            .with_child(chevron_el)
            .with_child(icon_el)
            .with_child(label_or_editor)
            .with_main_axis_size(MainAxisSize::Min)
            .finish();

        let state = self.row_states.get(&node.id).cloned().unwrap_or_default();
        let id_for_click = node.id.clone();
        let id_for_double_click = node.id.clone();
        let id_for_right_click = node.id.clone();

        // Don't accept click/right click when renaming (leave it to EditorView).
        if is_renaming {
            return Container::new(row)
                .with_padding_top(ITEM_PADDING_VERTICAL)
                .with_padding_bottom(ITEM_PADDING_VERTICAL)
                .with_padding_left(ITEM_PADDING_HORIZONTAL)
                .with_padding_right(ITEM_PADDING_HORIZONTAL)
                .with_margin_bottom(ITEM_MARGIN_BOTTOM)
                .finish();
        }

        let hoverable = Hoverable::new(state, move |_| {
            let mut c = Container::new(row)
                .with_padding_top(ITEM_PADDING_VERTICAL)
                .with_padding_bottom(ITEM_PADDING_VERTICAL)
                .with_padding_left(ITEM_PADDING_HORIZONTAL)
                .with_padding_right(ITEM_PADDING_HORIZONTAL)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            if is_selected {
                c = c.with_background(internal_colors::fg_overlay_3(theme));
            }
            c.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(SshManagerPanelAction::Click(id_for_click.clone()));
        })
        .on_double_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(SshManagerPanelAction::DoubleClick(
                id_for_double_click.clone(),
            ));
        })
        .on_right_click(move |ctx, _, position| {
            let offset = match ctx.element_position_by_id(SSH_PANEL_POSITION_ID) {
                Some(bounds) => position - bounds.origin(),
                None => position,
            };
            ctx.dispatch_typed_action(SshManagerPanelAction::OpenContextMenu {
                target: Some(id_for_right_click.clone()),
                position: offset,
            });
        })
        .finish();

        // Wrap row into an element that can be dragged or dropped.
        //
        // **Key nesting**: `DropTarget(Container(Draggable(Hoverable)))`.
        // There will be bugs without the Container layer - `Draggable::origin()` returns `child.origin()`
        // (`crates/warpui_core/src/elements/drag/draggable.rs:746-757`), while
        // The child is painted to drag_origin in the Dragging state, causing child.origin() =
        // ghost location. As a result, when DropTarget is directly applied to Draggable, bounds follows ghost.
        // Run → drop target is always under the mouse and cannot drop to other lines. Container.origin/size
        // Lock the layout value in your own paint (`container.rs:288 self.origin = ...`),
        // Provide stable bounds for DropTarget.
        let drag_state = self
            .row_drag_states
            .get(&node.id)
            .cloned()
            .unwrap_or_default();
        let dragged_id = node.id.clone();
        let draggable = Draggable::new(drag_state, hoverable)
            .with_accepted_by_drop_target_fn(move |drop_data, _app| {
                if drop_data.as_any().downcast_ref::<SshDropData>().is_some() {
                    AcceptedByDropTarget::Yes
                } else {
                    AcceptedByDropTarget::No
                }
            })
            .on_drop(move |ctx, _app, _bounds, data| {
                if let Some(drop) = data.and_then(|d| d.as_any().downcast_ref::<SshDropData>()) {
                    ctx.dispatch_typed_action(SshManagerPanelAction::MoveNode {
                        node_id: dragged_id.clone(),
                        new_parent_id: drop.parent_id.clone(),
                    });
                }
            })
            .finish();

        // A Container that locks the layout origin in the middle — see comments above.
        let stable_anchor = Container::new(draggable).finish();

        let drop_parent_id = match node.kind {
            NodeKind::Folder => Some(node.id.clone()),
            NodeKind::Server => node.parent_id.clone(),
        };
        DropTarget::new(
            stable_anchor,
            SshDropData {
                parent_id: drop_parent_id,
            },
        )
        .finish()
    }

    fn context_menu_items(&self) -> Vec<(String, SshManagerPanelAction)> {
        match self.context_menu_target.as_ref() {
            None => vec![
                (
                    crate::t!("workspace-left-panel-ssh-manager-menu-new-folder"),
                    SshManagerPanelAction::AddFolder,
                ),
                (
                    crate::t!("workspace-left-panel-ssh-manager-menu-new-server"),
                    SshManagerPanelAction::AddServer,
                ),
            ],
            Some(id) => {
                let kind = self.nodes.iter().find(|n| &n.id == id).map(|n| n.kind);
                match kind {
                    Some(NodeKind::Folder) => vec![
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-new-folder"),
                            SshManagerPanelAction::AddFolder,
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-new-server"),
                            SshManagerPanelAction::AddServer,
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-rename"),
                            SshManagerPanelAction::StartRename(id.clone()),
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-delete"),
                            SshManagerPanelAction::DeleteSelected,
                        ),
                    ],
                    Some(NodeKind::Server) => vec![
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-edit"),
                            SshManagerPanelAction::Edit,
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-connect"),
                            SshManagerPanelAction::Connect,
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-clone"),
                            SshManagerPanelAction::CloneServer(id.clone()),
                        ),
                        (
                            crate::t!("workspace-left-panel-ssh-manager-menu-delete"),
                            SshManagerPanelAction::DeleteSelected,
                        ),
                    ],
                    None => vec![],
                }
            }
        }
    }

    fn render_context_menu(
        &self,
        appearance: &warp_core::ui::appearance::Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let items = self.context_menu_items();
        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (i, (label, action)) in items.into_iter().enumerate() {
            let state = self
                .context_menu_item_states
                .get(i)
                .cloned()
                .unwrap_or_default();
            let label_el = Text::new_inline(
                label,
                appearance.ui_font_family(),
                appearance.ui_font_subheading(),
            )
            .with_color(theme.main_text_color(theme.background()).into())
            .finish();
            let row_action = action.clone();
            let item = Hoverable::new(state, move |mouse| {
                let mut c = Container::new(label_el)
                    .with_padding_top(CONTEXT_MENU_ITEM_PADDING_V)
                    .with_padding_bottom(CONTEXT_MENU_ITEM_PADDING_V)
                    .with_padding_left(CONTEXT_MENU_ITEM_PADDING_H)
                    .with_padding_right(CONTEXT_MENU_ITEM_PADDING_H)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)));
                if mouse.is_hovered() {
                    c = c.with_background(internal_colors::fg_overlay_3(theme));
                }
                c.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(row_action.clone());
                ctx.dispatch_typed_action(SshManagerPanelAction::DismissContextMenu);
            })
            .finish();
            col.add_child(item);
        }
        let menu_inner = ConstrainedBox::new(
            Container::new(col.with_main_axis_size(MainAxisSize::Min).finish())
                .with_background(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.surface_3().into()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_uniform_padding(4.0)
                .finish(),
        )
        .with_width(CONTEXT_MENU_WIDTH)
        .finish();

        Dismiss::new(menu_inner)
            .on_dismiss(|ctx, _| {
                ctx.dispatch_typed_action(SshManagerPanelAction::DismissContextMenu);
            })
            .finish()
    }
}

impl Entity for SshManagerPanel {
    type Event = SshManagerPanelEvent;
}

impl TypedActionView for SshManagerPanel {
    type Action = SshManagerPanelAction;

    fn handle_action(&mut self, action: &SshManagerPanelAction, ctx: &mut ViewContext<Self>) {
        match action {
            SshManagerPanelAction::AddFolder => self.on_add_folder(ctx),
            SshManagerPanelAction::AddServer => self.on_add_server(ctx),
            SshManagerPanelAction::DeleteSelected => self.on_delete_selected(ctx),
            SshManagerPanelAction::Connect => self.on_connect(ctx),
            SshManagerPanelAction::Edit => self.on_edit(ctx),
            SshManagerPanelAction::CloneServer(id) => self.on_clone_server(id, ctx),
            SshManagerPanelAction::Click(id) => self.on_click(id.clone(), ctx),
            SshManagerPanelAction::StartRename(id) => self.enter_rename(id.clone(), ctx),
            SshManagerPanelAction::CommitRename => self.commit_rename(ctx),
            SshManagerPanelAction::CancelRename => self.cancel_rename(ctx),
            SshManagerPanelAction::OpenContextMenu { target, position } => {
                self.on_open_context_menu(target.clone(), *position, ctx)
            }
            SshManagerPanelAction::DismissContextMenu => self.on_dismiss_context_menu(ctx),
            SshManagerPanelAction::MoveNode {
                node_id,
                new_parent_id,
            } => self.on_move_node(node_id.clone(), new_parent_id.clone(), ctx),
            SshManagerPanelAction::ToggleNodeCollapsed(id) => {
                self.on_toggle_node_collapsed(id, ctx)
            }
            SshManagerPanelAction::ToggleAllFolders => self.on_toggle_all_folders(ctx),
            SshManagerPanelAction::DoubleClick(id) => self.on_double_click(id.clone(), ctx),
            SshManagerPanelAction::ImportCandidate { alias } => {
                self.on_import_candidate(alias.clone(), ctx)
            }
            SshManagerPanelAction::RefreshCandidates => {
                self.candidates.update(ctx, |vm, ctx| vm.refresh(ctx));
                self.sync_candidate_row_states(ctx);
                ctx.notify();
            }
            SshManagerPanelAction::ToggleCandidatesSection => {
                self.candidates
                    .update(ctx, |vm, ctx| vm.toggle_expanded(ctx));
                ctx.notify();
            }
        }
    }
}

impl View for SshManagerPanel {
    fn ui_name() -> &'static str {
        "SshManagerPanel"
    }

    fn on_focus(&mut self, _focus_ctx: &FocusContext, _ctx: &mut ViewContext<Self>) {}

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = warp_core::ui::appearance::Appearance::as_ref(app);

        let toolbar = Container::new(self.render_toolbar(appearance))
            .with_uniform_padding(8.0)
            .finish();

        // PRODUCT.md §2:Candidates section is above the saved tree, sharing the same panel
        // Horizontal padding. The section returns Empty when the view-model has not been refreshed and will not occupy
        // high.
        let candidates_section = Container::new(self.render_candidates(appearance, app))
            .with_padding_left(PANEL_HORIZONTAL_PADDING - ITEM_PADDING_HORIZONTAL)
            .with_padding_right(PANEL_HORIZONTAL_PADDING - ITEM_PADDING_HORIZONTAL)
            .finish();

        let tree = Container::new(self.render_tree(appearance))
            .with_padding_left(PANEL_HORIZONTAL_PADDING - ITEM_PADDING_HORIZONTAL)
            .with_padding_right(PANEL_HORIZONTAL_PADDING - ITEM_PADDING_HORIZONTAL)
            .finish();

        // Let the tree occupy the remaining vertical space - so that the root DropTarget covers the bottom of the panel,
        // The user can drag the data in the blank space at the bottom of the tree to root(`SshDropData{parent_id:None}`).
        let tree_filled = warpui::elements::Shrinkable::new(1.0, tree).finish();

        let panel_content = Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(toolbar)
                .with_child(candidates_section)
                .with_child(tree_filled)
                .finish(),
        )
        .finish();

        let positioned_panel = SavePosition::new(panel_content, SSH_PANEL_POSITION_ID).finish();

        let Some(position) = self.context_menu_position else {
            return positioned_panel;
        };

        let menu_el = self.render_context_menu(appearance);
        let positioning = OffsetPositioning::offset_from_parent(
            position,
            ParentOffsetBounds::ParentByPosition,
            ParentAnchor::TopLeft,
            ChildAnchor::TopLeft,
        );

        let mut stack = Stack::new();
        stack.add_child(positioned_panel);
        stack.add_positioned_overlay_child(menu_el, positioning);
        stack.finish()
    }
}

// --- helpers --------------------------------------------------------------

fn sort_for_display(nodes: Vec<SshNode>, depths: &HashMap<String, usize>) -> Vec<SshNode> {
    use std::collections::BTreeMap;
    let mut by_parent: BTreeMap<Option<String>, Vec<SshNode>> = BTreeMap::new();
    for n in nodes {
        by_parent.entry(n.parent_id.clone()).or_default().push(n);
    }
    for v in by_parent.values_mut() {
        v.sort_by_key(|n| (n.sort_order, n.name.clone()));
    }
    let mut out = Vec::with_capacity(depths.len());
    fn walk(
        parent: Option<&String>,
        by_parent: &BTreeMap<Option<String>, Vec<SshNode>>,
        out: &mut Vec<SshNode>,
    ) {
        if let Some(children) = by_parent.get(&parent.cloned()) {
            for c in children {
                out.push(c.clone());
                walk(Some(&c.id), by_parent, out);
            }
        }
    }
    walk(None, &by_parent, &mut out);
    out
}

fn compute_depths(nodes: &[SshNode]) -> HashMap<String, usize> {
    let by_id: HashMap<&str, &SshNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut depths = HashMap::with_capacity(nodes.len());
    for n in nodes {
        let mut d = 0;
        let mut p = n.parent_id.as_deref();
        while let Some(pid) = p {
            d += 1;
            p = by_id.get(pid).and_then(|nn| nn.parent_id.as_deref());
            if d > 64 {
                break;
            }
        }
        depths.insert(n.id.clone(), d);
    }
    depths
}

/// Pull the `host` field of all ssh_servers lines at once. Returns an empty Vec on failure - the candidate segment's
/// The "Added" badge will be rendered as "no imported items" when SQLite temporarily hangs, so as not to cause
/// The entire panel collapsed.
fn list_server_hosts() -> Vec<String> {
    use diesel::prelude::*;
    use persistence::schema::ssh_servers;
    warp_ssh_manager::with_conn(|conn| {
        let hosts: Vec<String> = ssh_servers::table.select(ssh_servers::host).load(conn)?;
        Ok(hosts)
    })
    .unwrap_or_else(|e| {
        log::warn!("ssh_manager: failed to list server hosts for candidates: {e:?}");
        Vec::new()
    })
}

fn unique_name(
    conn: &mut diesel::sqlite::SqliteConnection,
    parent: Option<&str>,
    base: &str,
) -> Result<String, anyhow::Error> {
    use diesel::prelude::*;
    use persistence::schema::ssh_nodes;
    let existing: Vec<String> = match parent {
        Some(p) => ssh_nodes::table
            .filter(ssh_nodes::parent_id.eq(p))
            .select(ssh_nodes::name)
            .load(conn)?,
        None => ssh_nodes::table
            .filter(ssh_nodes::parent_id.is_null())
            .select(ssh_nodes::name)
            .load(conn)?,
    };
    let set: std::collections::HashSet<String> = existing.into_iter().collect();
    if !set.contains(base) {
        return Ok(base.to_string());
    }
    for i in 2..1000 {
        let cand = format!("{base} {i}");
        if !set.contains(&cand) {
            return Ok(cand);
        }
    }
    Ok(format!("{base} {}", uuid::Uuid::new_v4()))
}
