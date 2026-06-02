//! Waz (Phase 3c subtask A1): Localize to a forever "unlimited" stub.
//!
//! Historical Responsibilities: "Monthly AI Request Quota" model for warp.dev server-side RPC driver.
//! Waz adopts BYOP (Bring Your Own Provider), where users pay the LLM provider themselves.
//! You should never be constrained by concepts such as "remaining requests / upgrade CTA / purchase additional credits" in the cloud.
//!
//! Write constraints:
//! * 30+ UI subscription points (`subscribe_to_model(&AIRequestUsageModel::handle(ctx), ...)`)
//!   Retained, but the event is no longer triggered by any path → Subscription callback becomes a silent no-op forever.
//! * Overflow uses `RequestLimitInfo` / `RequestUsageInfo` / `BonusGrant` /
//!   `BonusGrantScope` / `RequestLimitRefreshDuration` /
//!   `BuyCreditsBannerDisplayState` / `AIRequestUsageModelEvent` /
//!   The file of `AMBIENT_AGENT_TRIAL_CREDIT_THRESHOLD` (`workspaces/gql_convert.rs`,
//!   `ai_assistant/requests.rs`、`ai_assistant/mod.rs`、
//!   `settings/ai.rs`、`settings/ai_tests.rs`、`workspace/bonus_grant_notification_model.rs`、
//!   `settings_view/ai_page.rs`、
//!   `terminal/view/ambient_agent/first_time_setup.rs`、`agent_view/agent_message_bar.rs`)
//!   Not within the writing domain of this task → These type definitions and equivalent construction capabilities must continue to be retained in the stub,
//!   Only strip business logic such as RPC/caching/metering.

use crate::{server_time::ServerTimestamp, workspaces::workspace::WorkspaceUid};
use chrono::{DateTime, Utc};
use instant::Instant;
use serde::{Deserialize, Serialize};
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BonusGrantType {
    AmbientOnly,
    Any,
}

/// Threshold of ambient-only credits at which we surface upgrade/CTA UI。
///
/// Waz: will never be reached in localized scenarios (because `ambient_only_credits_remaining` is always `None`),
/// Constant definitions are still retained for compatibility with external imports.
pub const AMBIENT_AGENT_TRIAL_CREDIT_THRESHOLD: i32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BonusGrantScope {
    User,
    Workspace(WorkspaceUid),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum BuyCreditsBannerDisplayState {
    #[default]
    Hidden,
    OutOfCredits,
    MonthlyLimitReached,
}

#[derive(Clone, Debug)]
pub struct BonusGrant {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub cost_cents: i32,
    pub expiration: Option<chrono::DateTime<chrono::Utc>>,
    pub grant_type: BonusGrantType,
    pub reason: String,
    pub user_facing_message: Option<String>,
    pub request_credits_granted: i32,
    pub request_credits_remaining: i32,
    pub scope: BonusGrantScope,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum RequestLimitRefreshDuration {
    Weekly,
    Monthly,
    EveryTwoWeeks,
}

/// History: Snapshot of "Monthly Request Quota" issued by the server.
/// Waz: reserved only as type shell (`AISettings::update_quota_info` / `ai_assistant/requests.rs`
/// This structure will also be constructed when writing files outside the domain). `AIRequestUsageModel` no longer holds / caches / updates it.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct RequestLimitInfo {
    pub limit: usize,
    pub num_requests_used_since_refresh: usize,
    pub next_refresh_time: ServerTimestamp,
    pub is_unlimited: bool,
    pub request_limit_refresh_duration: RequestLimitRefreshDuration,
    pub is_unlimited_voice: bool,
    #[serde(default)]
    pub voice_request_limit: usize,
    #[serde(default)]
    pub voice_requests_used_since_last_refresh: usize,
    #[serde(default)]
    pub max_files_per_repo: usize,
    #[serde(default)]
    pub embedding_generation_batch_size: usize,
}

fn default_voice_requests_limit() -> usize {
    10000
}

impl Default for RequestLimitInfo {
    /// Waz: No cloud quota, the default value is considered "unlimited".
    fn default() -> Self {
        Self {
            limit: usize::MAX,
            num_requests_used_since_refresh: 0,
            next_refresh_time: ServerTimestamp::new(Utc::now() + chrono::Duration::days(365)),
            is_unlimited: true,
            request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
            is_unlimited_voice: true,
            voice_request_limit: default_voice_requests_limit(),
            voice_requests_used_since_last_refresh: 0,
            max_files_per_repo: usize::MAX,
            embedding_generation_batch_size: 100,
        }
    }
}

#[cfg(test)]
impl RequestLimitInfo {
    pub fn new_for_test(limit: usize, num_requests_used_since_refresh: usize) -> Self {
        Self {
            limit,
            num_requests_used_since_refresh,
            ..Self::default()
        }
    }
}

/// History: Aggregation structure returned by server-side `getRequestLimitInfo`.
/// Waz: reserved only as a type shell (`ai_assistant/requests.rs` will still construct this type).
/// `AIRequestUsageModel` no longer consumes it.
pub struct RequestUsageInfo {
    pub request_limit_info: RequestLimitInfo,
    pub bonus_grants: Vec<BonusGrant>,
}

/// Waz:Model no longer holds any state.
pub struct AIRequestUsageModel;

impl Entity for AIRequestUsageModel {
    type Event = AIRequestUsageModelEvent;
}

/// Waz: retain enum definition to be compatible with subscription callback `match` mode;
/// `AIRequestUsageModel` no longer emit any variant after localization → all subscription callbacks become silent no-op.
pub enum AIRequestUsageModelEvent {
    RequestUsageUpdated,
    RequestBonusRefunded {
        requests_refunded: i32,
        server_conversation_id: String,
        request_id: String,
    },
}

impl AIRequestUsageModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }

    #[cfg(test)]
    pub fn new_for_test(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }

    pub fn last_update_time(&self) -> Option<Instant> {
        None
    }

    /// Waz: no cloud backend, no-op.
    pub fn refresh_request_usage_async(&mut self, _ctx: &mut ModelContext<Self>) {}

    /// Waz (localization): always returns true, BYOP local operation is not subject to cloud quota constraints.
    pub fn has_requests_remaining(&self) -> bool {
        true
    }

    /// Waz (localization): always returns true.
    /// AI availability only depends on whether the user has configured an API key (independently controlled by `ApiKeyManager`),
    /// Should not be determined by cloud metering components such as `request_limit_info`.
    pub fn has_any_ai_remaining(&self, _ctx: &AppContext) -> bool {
        true
    }

    /// Waz (localization): no cloud metering, fixed return 0.
    pub fn requests_used(&self) -> usize {
        0
    }

    /// Waz (localization): No cloud metering, fixed return 0.0.
    pub fn request_percentage_used(&self) -> f32 {
        0.0
    }

    /// Waz (localization): no cloud limit, fixed return `usize::MAX`.
    pub fn request_limit(&self) -> usize {
        usize::MAX
    }

    /// Waz (localization): forward placeholder time.
    pub fn next_refresh_time(&self) -> DateTime<Utc> {
        Utc::now() + chrono::Duration::days(365)
    }

    /// Waz (localization): Always unlimited.
    pub fn is_unlimited(&self) -> bool {
        true
    }

    pub fn refresh_duration_to_string(&self) -> String {
        "monthly".to_string()
    }

    /// Waz (localization): bonus grants do not exist for local users.
    pub fn bonus_grants(&self) -> &[BonusGrant] {
        &[]
    }

    /// Waz (localization): Local users have no concept of ambient-only credits.
    pub fn ambient_only_credits_remaining(&self) -> Option<i32> {
        None
    }

    /// Waz (localization): Local users have no concept of workspace bonus credits.
    pub fn total_workspace_bonus_credits_remaining(&self, _uid: WorkspaceUid) -> i32 {
        0
    }

    /// Waz (localization): Local users have no concept of workspace bonus credits.
    pub fn total_current_workspace_bonus_credits_remaining(&self, _ctx: &AppContext) -> i32 {
        0
    }

    /// Waz (localization): The purchase of additional credits business is not applicable.
    pub fn compute_buy_addon_credits_banner_display_state(
        &self,
        _ctx: &AppContext,
    ) -> BuyCreditsBannerDisplayState {
        BuyCreditsBannerDisplayState::Hidden
    }

    /// Waz (localization): no-op.
    pub fn dismiss_buy_credits_banner(&mut self, _ctx: &mut ModelContext<Self>) {}

    /// Waz (localization): no-op.
    pub fn enable_buy_credits_banner(&mut self, _ctx: &mut ModelContext<Self>) {}

    /// Waz (localization): Voice input is not limited by cloud quota and always returns true.
    pub fn can_request_voice(&self) -> bool {
        true
    }
}

impl SingletonEntity for AIRequestUsageModel {}
