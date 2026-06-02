//! Telemetry events for the in-app notification mailbox / toast stack.
//!
//! This is a minimally cropped version of `AgentManagementTelemetryEvent` which was deleted during 002ce467 cloud-removal,
//! keeping only the variants actually still in use by the notification center (`item_rendering.rs`) — artifact click events +
//! tombstones that no longer exist but have their schema kept to maintain backward compatibility / future reconstruction.

use serde::Serialize;

/// Notification artifact type (used for telemetry).
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Plan,
    Branch,
    PullRequest,
}

/// Notification center-related telemetry events.
#[derive(Serialize, Debug)]
pub enum NotificationsTelemetryEvent {
    /// The user clicked the artifact button (plan / branch / PR) in a notification item
    ArtifactClicked { artifact_type: ArtifactType },
}
