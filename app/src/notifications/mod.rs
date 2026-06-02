//! Notification Center (mailbox + toast).
//!
//! Rebuilt after accidental deletion by 002ce467 cloud-removal, keeping only the local paths unrelated to the cloud:
//! - Task completion/error notifications for the software's own BYOP agent (Oz)
//! - Status notifications for third-party CLI agents (Claude Code / Codex / DeepSeek, etc.)
//!
//! Module layout:
//! - `item`         Data model (`NotificationItem` / `NotificationItems`, etc.)
//! - `item_rendering` Single notification item UI (shared between mailbox and toast)
//! - `model`        Singleton `NotificationsModel` (subscribes to history / cli session models, outputs notifications)
//! - `view`         `NotificationMailboxView` (main mailbox panel)
//! - `toast_stack`  `AgentNotificationToastStack` (bottom-right toast)
//! - `telemetry`    Notification center-related telemetry events (`NotificationsTelemetryEvent`)

pub(crate) mod item;
pub(crate) mod item_rendering;
pub mod model;
pub(crate) mod telemetry;
pub mod toast_stack;
pub mod view;

pub(crate) use item::{
    NotificationCategory, NotificationFilter, NotificationId, NotificationItem, NotificationItems,
    NotificationSourceAgent,
};
pub use toast_stack::AgentNotificationToastStack;
pub use view::{NotificationMailboxView, NotificationMailboxViewEvent};

pub fn init(app: &mut warpui::AppContext) {
    NotificationMailboxView::init(app);
}
