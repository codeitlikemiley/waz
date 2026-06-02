pub(crate) mod convert_conversation;
mod convert_from;
mod convert_to;

pub use ai::agent::convert::ConvertToAPITypeError;
use ai::api_keys::ApiKeyManager;
pub use convert_from::{
    user_inputs_from_messages, ConversionParams, ConvertAPIMessageToClientOutputMessage,
    MaybeAIAgentOutputMessage, MessageToAIAgentOutputMessageError,
};

use futures_lite::Stream;
use serde::Serialize;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use warp_core::channel::ChannelState;
use warp_core::execution_mode::AppExecutionMode;
use warp_core::features::FeatureFlag;

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::{
    ai::api_error::AIApiError,
    ai::{blocklist::SessionContext, llms::LLMId},
};

use super::{AIAgentInput, MCPContext, MCPServer, RequestMetadata, RunningCommand, Suggestions};
use crate::ai::blocklist::{BlocklistAIPermissions, RequestInput};
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::facts::{AIFact, AIFactObjectModel};
use crate::ai::mcp::templatable_manager::TemplatableMCPServerInfo;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::cloud_object::model::generic_string_model::GenericStringObjectId;
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::StoredObject;
use crate::settings::AISettings;
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;
use crate::workspaces::user_workspaces::UserWorkspaces;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{AppContext, EntityId, SingletonEntity as _};

/// Unique, server-generated conversation-scoped token to be roundtripped to the API when sending
/// requests that follow-up within a given conversation.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerConversationToken(String);

impl ServerConversationToken {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn debug_link(&self) -> String {
        format!("{}://debug/maa/{}", ChannelState::url_scheme(), self.as_str())
    }

    pub fn conversation_link(&self) -> String {
        format!(
            "{}://conversation/{}",
            ChannelState::url_scheme(),
            self.as_str()
        )
    }
}

impl From<ServerConversationToken> for String {
    fn from(value: ServerConversationToken) -> Self {
        value.0
    }
}

#[derive(Debug, Clone)]
pub struct RequestParams {
    pub input: Vec<AIAgentInput>,
    pub conversation_token: Option<ServerConversationToken>,
    pub forked_from_conversation_token: Option<ServerConversationToken>,
    pub ambient_agent_task_id: Option<AmbientAgentTaskId>,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub existing_suggestions: Option<Suggestions>,
    pub metadata: Option<RequestMetadata>,
    pub session_context: SessionContext,
    pub model: LLMId,
    #[allow(unused)]
    pub coding_model: LLMId,
    pub cli_agent_model: LLMId,
    pub computer_use_model: LLMId,
    pub is_memory_enabled: bool,
    /// Waz BYOP exclusive: A snapshot of global Rules (`AIFact::Memory`) created by the user in Settings → Agents → Rules.
    /// Loaded once from `ObjectStoreModel` in `new()` and plumbed along with the request to `chat_stream::build_chat_request` → `prompt_renderer`,
    /// then rendered into the system prompt by `partials/user_rules.j2`.
    /// Collected only when `is_memory_enabled` is true and not trashed; sorted lexicographically by `(name, content)`
    /// for prompt cache stability.
    pub user_rules: Vec<(Option<String>, String)>,
    pub warp_drive_context_enabled: bool,
    pub context_window_limit: Option<u32>,
    pub mcp_context: Option<MCPContext>,
    pub planning_enabled: bool,
    should_redact_secrets: bool,

    /// User-provided API keys for AI providers (BYO API Key).
    pub api_keys: Option<warp_multi_agent_api::request::settings::ApiKeys>,
    pub allow_use_of_warp_credits_with_byok: bool,
    pub autonomy_level: warp_multi_agent_api::AutonomyLevel,
    pub isolation_level: warp_multi_agent_api::IsolationLevel,
    pub web_search_enabled: bool,
    pub computer_use_enabled: bool,
    pub ask_user_question_enabled: bool,
    pub research_agent_enabled: bool,
    pub supported_tools_override: Option<Vec<warp_multi_agent_api::ToolType>>,
    /// Waz BYOP exclusive: Local conversation ID, used only for request-readiness diagnostic logs.
    pub byop_conversation_id: Option<AIConversationId>,
    /// Waz BYOP exclusive: Non-persistent diagnostic correlation ID within a single request.
    pub byop_readiness_attempt_id: Option<String>,
    /// The conversation ID of the parent agent that spawned this child agent, if any.
    pub parent_agent_id: Option<String>,
    /// The display name for this agent (e.g. "Agent 1"), assigned by the orchestrator.
    pub agent_name: Option<String>,
    /// Waz BYOP exclusive: The associated LRC (Long Running Command) block ID when initiating this request.
    /// Filled during the first tag-in round and subsequent rounds of CLI subagents that have entered agent control.
    /// Used to keep the BYOP prompt / tools bound to the current PTY, avoiding the model starting another shell to operate on the same TUI.
    pub lrc_command_id: Option<String>,
    /// Waz BYOP exclusive: The current snapshot of LRC. `UserQuery.running_command` only covers user input rounds;
    /// subsequent rounds like auto-resume / tool result need to carry the latest PTY content through here.
    pub lrc_running_command: Option<RunningCommand>,
    /// Waz BYOP local conversation compaction sidecar snapshot (the controller puts `conversation.compaction_state.clone()` here).
    /// Used by `chat_stream::build_chat_request` to:
    ///   1. Filter messages in [`crate::ai::byop_compaction::state::CompactionState::hidden_message_ids`]
    ///   2. Insert "summarized user/assistant pairs" at hidden interval positions
    ///   3. Replace `ToolCallResult`s whose `tool_output_compacted_at` is not empty with placeholders
    ///   4. Cut head + append `SUMMARY_TEMPLATE` as a user message on the `AIAgentInput::SummarizeConversation` path
    ///
    /// Default `None` = compatible path (no compaction).
    pub compaction_state: Option<crate::ai::byop_compaction::state::CompactionState>,
    /// Waz BYOP repair sidecar snapshot. Used read-only by the serializer; not deserialized or persisted as JSON during request construction.
    pub byop_repair_state: crate::ai::byop_readiness::RepairStateStatus,
    /// Waz BYOP exclusive: Whether this round needs to simulate the upstream `CreateTask` process to upgrade the optimistic CLI subtask.
    /// Needed only for the first round when the user has just tagged in; subsequent rounds with an existing CLI subagent reuse the task and must not spawn again.
    pub lrc_should_spawn_subagent: bool,
    /// Waz BYOP exclusive: The task to which this round's response should be written. Root task for normal conversation;
    /// corresponding subtask for subsequent rounds of CLI subagent.
    pub byop_target_task_id: Option<String>,
}

/// Collects a snapshot of global Rules (`AIFact::Memory`) created by the user in Settings → Agents → Rules,
/// used to inject into the BYOP system prompt (Issue #116).
///
/// - Filters trashed items.
/// - Sorts lexicographically by `(name, content)` to avoid order drift across requests caused by HashMap iteration
///   (which would otherwise bypass the upstream Anthropic / OpenAI prompt cache).
///
/// Does not check `is_memory_enabled` internally, leaving the gate to be controlled by the caller. This allows the function
/// to be independently tested as pure collection logic without relying on singletons like `AISettings`.
pub(crate) fn collect_user_rules(
    object_store_model: &ObjectStoreModel,
) -> Vec<(Option<String>, String)> {
    let mut rules: Vec<(Option<String>, String)> = object_store_model
        .get_all_objects_of_type::<GenericStringObjectId, AIFactObjectModel>()
        .filter(|ai_fact| !ai_fact.is_trashed(object_store_model))
        .map(|ai_fact| match &ai_fact.model().string_model {
            AIFact::Memory(memory) => (memory.name.clone(), memory.content.clone()),
        })
        .collect();
    rules.sort();
    rules
}

pub type Event = Result<warp_multi_agent_api::ResponseEvent, Arc<AIApiError>>;

#[cfg(not(target_family = "wasm"))]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event> + Send + 'static>>;

// The WASM version of this type has no bound on `Send`, which is an unnecessary bound when
// targeting wasm because the browser is single-threaded (and we don't leverage WebWorkers for async
// execution in WoW).
#[cfg(target_family = "wasm")]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event>>>;

#[derive(Debug, Clone)]
pub struct ConversationData {
    pub id: AIConversationId,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub server_conversation_token: Option<ServerConversationToken>,
    pub forked_from_conversation_token: Option<ServerConversationToken>,
    pub ambient_agent_task_id: Option<AmbientAgentTaskId>,
    pub existing_suggestions: Option<Suggestions>,
}

impl RequestParams {
    #[cfg(test)]
    pub(crate) fn new_for_test(
        input: Vec<AIAgentInput>,
        tasks: Vec<warp_multi_agent_api::Task>,
    ) -> Self {
        Self {
            input,
            conversation_token: None,
            forked_from_conversation_token: None,
            ambient_agent_task_id: None,
            byop_target_task_id: tasks.first().map(|task| task.id.clone()),
            tasks,
            existing_suggestions: None,
            metadata: None,
            session_context: SessionContext::new_for_test(),
            model: LLMId::from("byop:test"),
            coding_model: LLMId::from("byop:test"),
            cli_agent_model: LLMId::from("byop:test"),
            computer_use_model: LLMId::from("byop:test"),
            is_memory_enabled: false,
            user_rules: Vec::new(),
            warp_drive_context_enabled: false,
            context_window_limit: None,
            mcp_context: None,
            planning_enabled: true,
            should_redact_secrets: false,
            api_keys: None,
            allow_use_of_warp_credits_with_byok: false,
            autonomy_level: warp_multi_agent_api::AutonomyLevel::Supervised,
            isolation_level: warp_multi_agent_api::IsolationLevel::None,
            web_search_enabled: false,
            computer_use_enabled: false,
            ask_user_question_enabled: false,
            research_agent_enabled: false,
            supported_tools_override: None,
            byop_conversation_id: Some(AIConversationId::new()),
            byop_readiness_attempt_id: None,
            parent_agent_id: None,
            agent_name: None,
            lrc_command_id: None,
            lrc_running_command: None,
            compaction_state: None,
            byop_repair_state: Default::default(),
            lrc_should_spawn_subagent: false,
        }
    }

    pub fn new(
        terminal_view_id: Option<EntityId>,
        session_context: SessionContext,
        request_input: &RequestInput,
        conversation: ConversationData,
        metadata: Option<RequestMetadata>,
        app: &AppContext,
    ) -> Self {
        let ai_settings = AISettings::as_ref(app);
        let is_memory_enabled = ai_settings.is_memory_enabled(app);
        let warp_drive_context_enabled = ai_settings.is_warp_drive_context_enabled(app);

        // Waz BYOP fixes Issue #116: gated on `is_memory_enabled`, the specific collection logic
        // is extracted into the pure function `collect_user_rules`, taking only `&ObjectStoreModel` as input for easier testing,
        // without depending on the full AppContext singleton set.
        let user_rules = if is_memory_enabled {
            collect_user_rules(ObjectStoreModel::as_ref(app))
        } else {
            Vec::new()
        };

        // Build MCP context - either grouped by server or flat lists based on feature flag
        let mcp_context = if FeatureFlag::MCPGroupedServerContext.is_enabled() {
            // Group MCP tools and resources by server
            let templatable_manager = TemplatableMCPServerManager::as_ref(app);

            let mut active_servers: Vec<&TemplatableMCPServerInfo> = templatable_manager
                .get_active_templatable_servers()
                .values()
                .copied()
                .collect();

            // If file-based MCP servers are enabled, add active servers in scope of
            // the user's current working directory
            if let Some(cwd) = session_context.current_working_directory() {
                active_servers.extend(
                    templatable_manager
                        .get_active_file_based_servers(Path::new(cwd), app)
                        .values(),
                );
            }

            // Include any ephemeral MCP servers started via the Oz CLI.
            active_servers.extend(
                templatable_manager
                    .get_active_cli_spawned_servers()
                    .values(),
            );

            let servers: Vec<MCPServer> = active_servers
                .into_iter()
                .map(|server| MCPServer {
                    name: server.name().to_string(),
                    description: server.description().unwrap_or_default().to_string(),
                    id: server.installation_id().to_string(),
                    resources: server.resources().to_vec(),
                    tools: server.tools().to_vec(),
                })
                .collect();

            if servers.is_empty() {
                None
            } else {
                #[allow(deprecated)]
                Some(MCPContext {
                    resources: vec![],
                    tools: vec![],
                    servers,
                })
            }
        } else {
            // Flat lists of resources and tools
            let templatable_mcp_manager = TemplatableMCPServerManager::as_ref(app);
            let resources = templatable_mcp_manager
                .resources()
                .cloned()
                .collect::<Vec<_>>();
            let tools = templatable_mcp_manager.tools().cloned().collect::<Vec<_>>();

            #[allow(deprecated)]
            (!resources.is_empty() || !tools.is_empty()).then_some(MCPContext {
                resources,
                tools,
                servers: vec![],
            })
        };

        let should_redact_secrets = get_secret_obfuscation_mode(app).should_redact_secret();

        let user_workspaces = UserWorkspaces::as_ref(app);
        let api_keys = ApiKeyManager::as_ref(app).api_keys_for_request(
            user_workspaces.is_byo_api_key_enabled(),
            user_workspaces.is_aws_bedrock_credentials_enabled(app),
        );
        let allow_use_of_warp_credits_with_byok =
            *AISettings::as_ref(app).can_use_warp_credits_with_byok;

        let app_execution_mode = AppExecutionMode::as_ref(app);
        let autonomy_level = if app_execution_mode.is_autonomous() {
            warp_multi_agent_api::AutonomyLevel::Unsupervised
        } else {
            warp_multi_agent_api::AutonomyLevel::Supervised
        };

        let isolation_level = if app_execution_mode.is_sandboxed() {
            warp_multi_agent_api::IsolationLevel::Sandbox
        } else {
            warp_multi_agent_api::IsolationLevel::None
        };

        let web_search_enabled =
            BlocklistAIPermissions::as_ref(app).get_web_search_enabled(app, terminal_view_id);
        let research_agent_enabled = app
            .private_user_preferences()
            .read_value("ResearchAgentEnabled")
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let is_ambient_agent = conversation.ambient_agent_task_id.is_some();
        let computer_use_enabled = FeatureFlag::AgentModeComputerUse.is_enabled()
            && BlocklistAIPermissions::as_ref(app)
                .get_computer_use_setting(app, terminal_view_id)
                .is_enabled()
            && computer_use::is_supported_on_current_platform()
            && (FeatureFlag::LocalComputerUse.is_enabled() || is_ambient_agent);
        let ask_user_question_enabled = BlocklistAIPermissions::as_ref(app)
            .get_ask_user_question_setting(app, terminal_view_id)
            != crate::ai::execution_profiles::AskUserQuestionPermission::Never;

        let byop_target_task_id = if request_input.input_messages.len() == 1 {
            request_input
                .input_messages
                .keys()
                .next()
                .map(ToString::to_string)
        } else {
            None
        };

        // Reconcile the persisted override against the active base model's
        // current `LLMContextWindow` instead of trusting whatever was stored
        // last. If the active model isn't configurable or has been removed
        // server-side, drop the override; otherwise clamp it to the model's
        // current `[min, max]` range.
        let context_window_limit = {
            let profile_data = AIExecutionProfilesModel::as_ref(app)
                .active_profile(terminal_view_id, app)
                .data()
                .clone();
            profile_data
                .configurable_context_window(app)
                .and_then(|cw| {
                    profile_data
                        .context_window_limit
                        .map(|v| v.clamp(cw.min, cw.max))
                })
        };

        Self {
            input: request_input.all_inputs().cloned().collect(),
            conversation_token: conversation.server_conversation_token,
            forked_from_conversation_token: conversation.forked_from_conversation_token,
            ambient_agent_task_id: conversation.ambient_agent_task_id,
            tasks: conversation.tasks,
            existing_suggestions: conversation.existing_suggestions,
            context_window_limit,
            metadata,
            session_context,
            model: request_input.model_id.clone(),
            coding_model: request_input.coding_model_id.clone(),
            cli_agent_model: request_input.cli_agent_model_id.clone(),
            computer_use_model: request_input.computer_use_model_id.clone(),
            is_memory_enabled,
            user_rules,
            warp_drive_context_enabled,
            mcp_context,
            planning_enabled: true,
            should_redact_secrets,
            api_keys,
            allow_use_of_warp_credits_with_byok,
            autonomy_level,
            isolation_level,
            web_search_enabled,
            computer_use_enabled,
            ask_user_question_enabled,
            research_agent_enabled,
            supported_tools_override: request_input.supported_tools_override.clone(),
            byop_conversation_id: Some(conversation.id),
            byop_readiness_attempt_id: None,
            parent_agent_id: None,
            agent_name: None,
            lrc_command_id: None,
            lrc_running_command: None,
            lrc_should_spawn_subagent: false,
            byop_target_task_id,
            // BYOP-only: backfilled by the controller before dispatching to the BYOP executor (setter style,
            // avoiding crossing through ConversationRequestData / non-BYOP paths).
            compaction_state: None,
            byop_repair_state: Default::default(),
        }
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
