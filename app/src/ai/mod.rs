//! This module should houses all horizontal/cross-cutting AI functionality throughout
//! Waz (including Agent Mode).
//!
//! The side panel Waz AI implementation lives in `super::ai_assistant`.
pub(crate) mod agent;
pub(crate) mod agent_conversations_model;
pub(crate) mod agent_events;
pub(crate) mod agent_providers;
pub(crate) mod agent_tips;
pub(crate) mod ai_document_view;
pub mod ambient_agents;
pub(crate) mod api_error;
pub(crate) mod artifact_download;
pub mod artifacts;
pub(crate) mod attachment_utils;
#[cfg(not(target_family = "wasm"))]
pub mod aws_credentials;
pub(crate) mod block_context;
pub(crate) mod blocklist;
pub(crate) mod byop_compaction;
pub(crate) mod byop_readiness;
pub mod control_code_parser;
pub(crate) mod conversation_navigation;
pub(crate) mod conversation_status_ui;
pub(crate) mod conversation_utils;
pub(crate) mod document;
pub(crate) mod harness_display;
pub(crate) mod llms;
pub mod onboarding;
pub(crate) mod predict;
pub(crate) mod project_rules_persister;
pub mod request_usage_model;
pub(crate) mod restored_conversations;
pub(crate) mod skills;
pub(crate) mod voice;
pub use agent_tips::*;
pub use request_usage_model::*;
use warpui::AppContext;
#[cfg(not(target_family = "wasm"))]
pub mod agent_sdk;
// Waz Wave 7-3: `ambient_agent_settings` has been physically removed with the ambient-agent UI subsystem.
// Waz Wave 7-2: The CLI/Forms/Environment Preparation link for Cloud environments has been deleted;
// Native object data types are still staged here for use by ObjectStoreModel deserialization and filtering of existing views.
pub mod execution_profiles;
pub mod facts;
// Waz Wave 6-8:`generate_block_title` with `BlockClient::generate_shared_block_title`
// The stub is removed together - the only consumption point is the BlockClient trait signature, and there is no other local code path.
pub(crate) mod loading;
pub mod mcp;

pub(crate) use ai::paths;

pub fn init(app: &mut AppContext) {
    blocklist::keyboard_navigable_buttons::init(app);
    blocklist::block::number_shortcut_buttons::init(app);
    blocklist::toggleable_items::init(app);
    blocklist::suggested_agent_mode_workflow_modal::init(app);
    blocklist::suggested_rule_modal::init(app);
    ai_document_view::init(app);
}
