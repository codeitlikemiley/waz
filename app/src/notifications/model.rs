//! Notification Center data model (Singleton).
//!
//! When 002ce467 cloud-removal deleted `agent_management`, this model was cleared along with it, but:
//! - Task completion/error notifications for the software's own BYOP agent (Oz)
//! - Status notifications for third-party CLI agents (Claude / Codex / DeepSeek, etc.)
//!
//! Still need to go through the Notification Center. This module is a streamlined version of the pre-deletion `AgentNotificationsModel`:
//! - Removed `ActiveAgentViewsModel` subscription (which was the status source for managing cloud-side views, now deleted).
//!   Instead of using `is_conversation_open` to determine "whether the conversation view is still open", we now query
//!   `BlocklistAIHistoryModel::conversation()` to check "whether the conversation is still in memory".
//! - Removed `AgentManagementEvent::ConversationNeedsAttention` (legacy toast path,
//!   replaced by mailbox/toast_stack).
//! - Removed legacy `should_trigger_notification` check (now only using the mailbox path).

use std::collections::HashMap;

use warp_core::features::FeatureFlag;
use warp_core::send_telemetry_from_ctx;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::BlocklistAIHistoryEvent;
use crate::notifications::item::{
    NotificationCategory, NotificationId, NotificationItem, NotificationItems, NotificationOrigin,
    NotificationSourceAgent,
};
use crate::server::telemetry::TelemetryEvent;
use crate::settings::AISettings;
use crate::terminal::cli_agent_sessions::{
    CLIAgentSessionStatus, CLIAgentSessionsModel, CLIAgentSessionsModelEvent,
};
use crate::terminal::CLIAgent;
use crate::workspace::util::is_terminal_view_in_same_tab;
use crate::workspace::{Workspace, WorkspaceRegistry};
use crate::BlocklistAIHistoryModel;

/// Singleton model for the Notification Center:
/// - Pushes notifications to the mailbox when key changes occur in BYOP agent conversation status
///   (`BlocklistAIHistoryModel`) and CLI agent session status (`CLIAgentSessionsModel`);
/// - Maintains `pending_artifacts` (accumulated artifacts for the current turn of each conversation)
///   and flushes them along with notifications in final states.
pub struct NotificationsModel {
    notifications: NotificationItems,
    /// Accumulated artifacts for the current turn; drained into notifications in final states (Success/Cancelled/Error),
    /// cleared in InProgress state.
    pub(crate) pending_artifacts: HashMap<AIConversationId, Vec<Artifact>>,
}

impl Entity for NotificationsModel {
    type Event = NotificationsEvent;
}

impl SingletonEntity for NotificationsModel {}

impl NotificationsModel {
    pub(crate) fn new(ctx: &mut ModelContext<Self>) -> Self {
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, move |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });

        let cli_sessions_model = CLIAgentSessionsModel::handle(ctx);
        ctx.subscribe_to_model(&cli_sessions_model, |me, event, ctx| {
            me.handle_cli_agent_session_event(event, ctx);
        });

        Self {
            notifications: NotificationItems::default(),
            pending_artifacts: HashMap::new(),
        }
    }

    pub(crate) fn notifications(&self) -> &NotificationItems {
        &self.notifications
    }

    pub(crate) fn mark_item_read(&mut self, id: NotificationId, ctx: &mut ModelContext<Self>) {
        if self.notifications.mark_item_read(id) {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    pub(crate) fn mark_all_items_read(&mut self, ctx: &mut ModelContext<Self>) {
        if self.notifications.mark_all_items_read() {
            ctx.emit(NotificationsEvent::AllNotificationsMarkedRead);
        }
    }

    /// Marks all notifications on the specified terminal view as read.
    pub(crate) fn mark_items_from_terminal_view_read(
        &mut self,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }
        if self
            .notifications
            .mark_all_terminal_view_items_as_read(terminal_view_id)
        {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    fn handle_cli_agent_session_event(
        &mut self,
        event: &CLIAgentSessionsModelEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }

        match event {
            CLIAgentSessionsModelEvent::Ended {
                terminal_view_id, ..
            } => {
                self.remove_notification_by_source(
                    NotificationOrigin::CLISession(*terminal_view_id),
                    ctx,
                );
            }
            CLIAgentSessionsModelEvent::Started { .. }
            | CLIAgentSessionsModelEvent::InputSessionChanged { .. }
            | CLIAgentSessionsModelEvent::SessionUpdated { .. } => {}
            CLIAgentSessionsModelEvent::StatusChanged {
                terminal_view_id,
                agent,
                status,
                session_context,
            } => match status {
                // agent resumed work -> invalidate previous notifications.
                CLIAgentSessionStatus::InProgress => {
                    self.remove_notification_by_source(
                        NotificationOrigin::CLISession(*terminal_view_id),
                        ctx,
                    );
                }
                CLIAgentSessionStatus::Success => {
                    let title = session_context
                        .display_title()
                        .unwrap_or_else(|| format!("{} completed", agent.display_name()));
                    let message = match agent {
                        CLIAgent::Codex => "Notification from Codex",
                        CLIAgent::DeepSeek => "Notification from DeepSeek",
                        CLIAgent::Antigravity => "Notification from Antigravity",
                        _ => "Task completed.",
                    };
                    self.add_notification(
                        title,
                        message.to_owned(),
                        NotificationCategory::Complete,
                        NotificationSourceAgent::CLI(*agent),
                        NotificationOrigin::CLISession(*terminal_view_id),
                        *terminal_view_id,
                        vec![],
                        ctx,
                    );
                }
                CLIAgentSessionStatus::Blocked { message } => {
                    let title = session_context
                        .display_title()
                        .unwrap_or_else(|| format!("{} needs attention", agent.display_name()));
                    self.add_notification(
                        title,
                        message
                            .clone()
                            .unwrap_or_else(|| "Waiting for input.".to_owned()),
                        NotificationCategory::Request,
                        NotificationSourceAgent::CLI(*agent),
                        NotificationOrigin::CLISession(*terminal_view_id),
                        *terminal_view_id,
                        vec![],
                        ctx,
                    );
                }
            },
        }
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        // When a conversation is explicitly deleted / cleaned up ephemerally, clear its notifications and pending artifacts as well.
        if let BlocklistAIHistoryEvent::DeletedConversation {
            conversation_id, ..
        }
        | BlocklistAIHistoryEvent::RemoveConversation {
            conversation_id, ..
        } = event
        {
            if FeatureFlag::HOANotifications.is_enabled() {
                self.pending_artifacts.remove(conversation_id);
                self.remove_notification_by_source(
                    NotificationOrigin::Conversation(*conversation_id),
                    ctx,
                );
            }
            return;
        }

        // Accumulate artifacts incrementally as they arrive during the turn.
        if let BlocklistAIHistoryEvent::UpdatedConversationArtifacts {
            conversation_id,
            artifact,
            ..
        } = event
        {
            if FeatureFlag::HOANotifications.is_enabled() {
                self.pending_artifacts
                    .entry(*conversation_id)
                    .or_default()
                    .push(artifact.clone());
            }
            return;
        }

        let BlocklistAIHistoryEvent::UpdatedConversationStatus {
            terminal_view_id,
            conversation_id,
            // Restoring a conversation on startup should not trigger notifications.
            is_restored: false,
        } = event
        else {
            return;
        };

        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }

        let ai_history_model = BlocklistAIHistoryModel::as_ref(ctx);
        let Some(updated_conversation) = ai_history_model.conversation(conversation_id) else {
            return;
        };

        if updated_conversation.should_exclude_from_navigation() {
            return;
        }

        let status = updated_conversation.status().clone();
        let latest_query = updated_conversation.latest_user_query();
        self.handle_history_event_for_mailbox(
            &status,
            *conversation_id,
            latest_query,
            *terminal_view_id,
            ctx,
        );
    }

    fn handle_history_event_for_mailbox(
        &mut self,
        status: &ConversationStatus,
        conversation_id: AIConversationId,
        latest_query: Option<String>,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let origin = NotificationOrigin::Conversation(conversation_id);

        // Conversation no longer exists in memory (evicted / deleted) -> no navigable target, clear related notifications directly.
        // This replaces the original `ActiveAgentViewsModel::is_conversation_open` check.
        if BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_none()
        {
            self.pending_artifacts.remove(&conversation_id);
            self.remove_notification_by_source(origin, ctx);
            return;
        }

        let title = latest_query.unwrap_or_else(|| "Agent task".to_owned());

        match status {
            // agent resumed work -> invalidate previous notifications.
            ConversationStatus::InProgress => {
                self.remove_notification_by_source(origin, ctx);
            }
            ConversationStatus::Success => {
                let artifacts = self.flush_pending_artifacts(conversation_id);
                self.add_notification(
                    title,
                    "Task completed.".to_owned(),
                    NotificationCategory::Complete,
                    NotificationSourceAgent::Oz,
                    origin,
                    terminal_view_id,
                    artifacts,
                    ctx,
                );
            }
            ConversationStatus::Cancelled => {
                let artifacts = self.flush_pending_artifacts(conversation_id);
                self.add_notification(
                    title,
                    "Task was cancelled.".to_owned(),
                    NotificationCategory::Complete,
                    NotificationSourceAgent::Oz,
                    origin,
                    terminal_view_id,
                    artifacts,
                    ctx,
                );
            }
            ConversationStatus::Blocked { blocked_action } => {
                self.add_notification(
                    title,
                    blocked_action.clone(),
                    NotificationCategory::Request,
                    NotificationSourceAgent::Oz,
                    origin,
                    terminal_view_id,
                    vec![],
                    ctx,
                );
            }
            ConversationStatus::Error => {
                let artifacts = self.flush_pending_artifacts(conversation_id);
                self.add_notification(
                    title,
                    "Something went wrong.".to_owned(),
                    NotificationCategory::Error,
                    NotificationSourceAgent::Oz,
                    origin,
                    terminal_view_id,
                    artifacts,
                    ctx,
                );
            }
        }
    }

    /// Deletes existing notifications for the specified source (if any) and emits an update event.
    fn remove_notification_by_source(
        &mut self,
        origin: NotificationOrigin,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.notifications.remove_by_origin(origin) {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    /// Drains the accumulated artifacts for the current turn of the specified conversation.
    pub(crate) fn flush_pending_artifacts(
        &mut self,
        conversation_id: AIConversationId,
    ) -> Vec<Artifact> {
        self.pending_artifacts
            .remove(&conversation_id)
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn add_notification(
        &mut self,
        title: String,
        message: String,
        category: NotificationCategory,
        agent: NotificationSourceAgent,
        origin: NotificationOrigin,
        terminal_view_id: EntityId,
        artifacts: Vec<Artifact>,
        ctx: &mut ModelContext<Self>,
    ) {
        if !*AISettings::as_ref(ctx).show_agent_notifications {
            return;
        }

        let is_visible = is_terminal_view_visible(terminal_view_id, ctx);
        let branch = resolve_git_branch_for_terminal_view(terminal_view_id, ctx);
        let item = NotificationItem::new(
            title,
            message,
            category,
            agent,
            origin,
            is_visible,
            terminal_view_id,
            artifacts,
            branch,
        );
        send_telemetry_from_ctx!(
            TelemetryEvent::AgentNotificationShown {
                agent_variant: agent.into(),
            },
            ctx
        );

        let id = item.id;
        self.notifications.push(item);
        ctx.emit(NotificationsEvent::NotificationAdded { id });
    }
}

#[derive(Clone, Debug)]
pub enum NotificationsEvent {
    /// A new notification has been added to the notification center.
    NotificationAdded { id: NotificationId },
    /// The read status of the notification has changed.
    NotificationUpdated,
    /// Mark all as read.
    AllNotificationsMarkedRead,
}

fn is_terminal_view_visible(terminal_view_id: EntityId, app: &AppContext) -> bool {
    let Some(active_id) = active_focused_terminal_id(app) else {
        return false;
    };
    active_id == terminal_view_id
        || is_terminal_view_in_same_tab(&active_id, &terminal_view_id, app)
}

fn resolve_git_branch_for_terminal_view(
    terminal_view_id: EntityId,
    app: &AppContext,
) -> Option<String> {
    for (_, workspace_handle) in WorkspaceRegistry::as_ref(app).all_workspaces(app) {
        for pane_group in workspace_handle.as_ref(app).tab_views() {
            let pane_group = pane_group.as_ref(app);
            for pane_id in pane_group.terminal_pane_ids() {
                if let Some(terminal_view) = pane_group.terminal_view_from_pane_id(pane_id, app) {
                    if terminal_view.id() == terminal_view_id {
                        return terminal_view.as_ref(app).current_git_branch(app);
                    }
                }
            }
        }
    }
    None
}

fn active_focused_terminal_id(app: &AppContext) -> Option<EntityId> {
    let active_window = app.windows().active_window()?;
    let workspace = app
        .views_of_type::<Workspace>(active_window)
        .and_then(|views| views.first().cloned())?;

    let workspace = workspace.as_ref(app);
    workspace.active_terminal_id(app)
}
