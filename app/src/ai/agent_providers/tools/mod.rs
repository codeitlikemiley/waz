//! Bidirectional translation registry for OpenAI tool calling in BYOP mode.
//!
//! Each warp's built-in tool (variant of `api::message::tool_call::Tool`) corresponds to one
//! [`OpenAiTool`] Description: function name + JSON Schema + reverse parsing args + execution
//! The result is serialized into a string for the upstream model to see.
//!
//! ## Subset of current implementation (Phase 3a first batch)
//!
//! - `run_shell_command`
//! - `read_files`
//!
//! Subsequent rounds of extensions: `grep` / `file_glob_v2` / `apply_file_diffs` / `call_mcp_tool`, etc.
//!
//! ## Closed loop description
//!
//! The model returns `tool_calls` → `from_args` is translated into `tool_call::Tool` → we emit
//! `Message::ToolCall { tool_call_id, tool }` → warp own `convert_from.rs`
//! Automatically translated into `AIAgentAction` → executor takes profile permission/pop-up window → execute → result
//! Automatically write back conversation → trigger next round of byop request → our `result_to_json`
//! Serialize the result into the content of `role=tool, tool_call_id=...` and send it to the upstream.

pub mod ask;
pub mod coerce;
pub mod documents;
pub mod edit;
pub mod exa;
pub mod files;
pub mod long_shell;
pub mod markers;
pub mod mcp;
pub mod search;
pub mod shell;
pub mod skill;
pub mod suggest;
pub mod todowrite;
pub mod web_runtime;
pub mod webfetch;
pub mod websearch;
pub mod tmp_ai;

use anyhow::Result;
use serde_json::Value;
use warp_multi_agent_api as api;

use crate::ai::agent::AIAgentActionResult;

/// A bidirectional adaptation description of a tool.
///
/// **Naming history**: At first BYOP only connected OpenAI compatible protocols, and later switched to genai SDK across 5 types of adapters
/// (OpenAI / OpenAIResp / Gemini / Anthropic / Ollama). The structure name follows `OpenAiTool`
/// Keep git blame, but the JSON Schema carried is the OpenAPI standard, and each adapter is internally used by genai
/// Automatically rewrite to their respective native formats (such as Anthropic input_schema, Gemini function_declarations).
pub struct OpenAiTool {
    /// The function name given to the upstream LLM (the model is called by this name in the response).
    pub name: &'static str,
    /// Description given to LLM.
    pub description: &'static str,
    /// Parameters JSON Schema (OpenAPI standard). Return a closure to avoid constructing serde_json::Value in const.
    pub parameters: fn() -> Value,
    /// Reverse parsing: args JSON string returned by upstream model → warp internal `tool_call::Tool` variant.
    pub from_args: fn(args: &str) -> Result<api::message::tool_call::Tool>,
    /// Convert the `Result` variant corresponding to the tool in ToolCallResult into JSON readable by the upstream model.
    /// Returns `None` when there is no matching variant (letting the caller fallback to generic serialization).
    pub result_to_json: fn(&api::message::tool_call_result::Result) -> Option<Value>,
}

impl OpenAiTool {
    /// Turn genai `Tool` (for feeding `ChatRequest.tools`).
    pub fn to_genai_tool(&self) -> genai::chat::Tool {
        genai::chat::Tool::new(self.name)
            .with_description(self.description)
            .with_schema((self.parameters)())
    }
}

/// Registry: All supported BYOP tools.
pub const REGISTRY: &[&OpenAiTool] = &[
    &shell::RUN_SHELL_COMMAND,
    &files::READ_FILES,
    &search::GREP,
    &search::FILE_GLOB_V2,
    &edit::APPLY_FILE_DIFFS,
    &long_shell::WRITE_TO_LONG_RUNNING_SHELL_COMMAND,
    &long_shell::READ_SHELL_COMMAND_OUTPUT,
    &ask::ASK_USER_QUESTION,
    &skill::READ_SKILL,
    // Local document system (AIDocumentModel)
    &documents::READ_DOCUMENTS,
    &documents::EDIT_DOCUMENTS,
    &documents::CREATE_DOCUMENTS,
    // User suggestion class (local channel + UI)
    &suggest::SUGGEST_NEW_CONVERSATION,
    &suggest::SUGGEST_PROMPT,
    // UI marker (no side effects, signals to the front end)
    &markers::OPEN_CODE_REVIEW,
    &markers::TRANSFER_SHELL_CONTROL,
    // Local todo list (BYOP self-synthesized Message::UpdateTodos, without protobuf executor)
    &todowrite::TODOWRITE,
    // BYOP-only network tools: not mapped to protobuf executor variant, used by chat_stream
    // Intercept by name before parse_incoming_tool_call, and directly adjust web_runtime to run HTTP.
    // When gating:profile.web_search_enabled=false, build_tools_array will be filtered out.
    &webfetch::WEBFETCH,
    &websearch::WEBSEARCH,
];

/// Press OpenAI function name to check the registry.
pub fn lookup(name: &str) -> Option<&'static OpenAiTool> {
    REGISTRY.iter().copied().find(|t| t.name == name)
}

/// Given a ToolCallResult, first find the corresponding tool in REGISTRY and use its `result_to_json`
/// Serialization; try MCP universal serialization if not found; then get a brief description to avoid panic.
pub fn serialize_result(result: &api::message::ToolCallResult) -> String {
    let inner = match &result.result {
        Some(r) => r,
        None => return r#"{"status":"cancelled"}"#.to_owned(),
    };
    for t in REGISTRY {
        if let Some(json) = (t.result_to_json)(inner) {
            return serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned());
        }
    }
    if let Some(json) = mcp::serialize_result(inner) {
        return serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned());
    }
    // Fallback: Unrecognized variants (tools that the user has not registered in subsequent rounds also go here).
    r#"{"status":"unsupported_tool_result"}"#.to_owned()
}

/// Serialize the `AIAgentActionResult` completed by the *current round of client execution* into a JSON string
/// Feed the upstream model (content of role=tool).
///
/// ## Why not just use `AIAgentActionResultType::Display`
///
/// `Display` impl renders structured results (especially `LongRunningCommandSnapshot`) into
/// `"Command 'bun repl' is long-running"` This type of one-line string, **completely discard block_id
/// (=command_id), grid_contents, is_alt_screen_active and other key fields**, leading to the next round
/// The model cannot get the command_id and cannot continue to read/write_to_long_running_*, and the long running command is completely useless.
///
/// ## Working principle
///
/// 1. Reuse the existing `TryFrom<AIAgentActionResult> in `app/src/ai/agent/api/convert_to.rs`
///    for api::request::input::user_inputs::user_input::Input`(covers all 25+ ActionResult
///    variant), get `Input::ToolCallResult { result, .. }`
/// 2. inner `*Result` type (such as `RunShellCommandResult`) and `api::message::tool_call_result::Result`
///    They share the same protobuf message, but the namespace of the outer enum is different, so it can be repackaged.
///    The outer enum reuses the existing per-tool `result_to_json` in `tools::REGISTRY`
///    (See `shell.rs::result_to_json` to take `LongRunningCommandSnapshot` into full JSON
///    contains command_id/output/is_alt_screen_active)
/// 3. Unrecognized variant returns `None`, and the caller fallsback to Display
///
/// ## Maintenance Note
///
/// When adding a BYOP tool, **the enum match here must be added to the variant** at the same time, otherwise the tool's
/// The current round of ActionResult will fallback to Display, losing structured fields.
pub fn serialize_action_result(action: &AIAgentActionResult) -> Option<String> {
    let msg_side = action_result_to_msg_result(action)?;
    for t in REGISTRY {
        if let Some(json) = (t.result_to_json)(&msg_side) {
            return Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned()));
        }
    }
    if let Some(json) = mcp::serialize_result(&msg_side) {
        return Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned()));
    }
    None
}

/// Convert the `AIAgentActionResult` executed by the current round of client to
/// `api::message::tool_call_result::Result` enum, for BYOP persistence as task.message.
///
/// Sharing the ReqR → MsgR mapping of `serialize_action_result`; the caller gets it and wraps it up
/// `Message::ToolCallResult { result: Some(...), context: None, tool_call_id }`。
pub fn action_result_to_msg_result(
    action: &AIAgentActionResult,
) -> Option<api::message::tool_call_result::Result> {
    use api::message::tool_call_result::Result as MsgR;
    use api::request::input::tool_call_result::Result as ReqR;
    use api::request::input::user_inputs::user_input::Input;

    let input: Input = action.clone().try_into().ok()?;
    let req_input: ReqR = match input {
        Input::ToolCallResult(tcr) => tcr.result?,
        _ => return None,
    };
    let msg_side = match req_input {
        ReqR::RunShellCommand(r) => MsgR::RunShellCommand(r),
        ReqR::WriteToLongRunningShellCommand(r) => MsgR::WriteToLongRunningShellCommand(r),
        ReqR::ReadShellCommandOutput(r) => MsgR::ReadShellCommandOutput(r),
        ReqR::ReadFiles(r) => MsgR::ReadFiles(r),
        ReqR::Grep(r) => MsgR::Grep(r),
        ReqR::FileGlobV2(r) => MsgR::FileGlobV2(r),
        ReqR::ApplyFileDiffs(r) => MsgR::ApplyFileDiffs(r),
        ReqR::CallMcpTool(r) => MsgR::CallMcpTool(r),
        ReqR::ReadMcpResource(r) => MsgR::ReadMcpResource(r),
        ReqR::AskUserQuestion(r) => MsgR::AskUserQuestion(r),
        ReqR::ReadSkill(r) => MsgR::ReadSkill(r),
        ReqR::ReadDocuments(r) => MsgR::ReadDocuments(r),
        ReqR::EditDocuments(r) => MsgR::EditDocuments(r),
        ReqR::CreateDocuments(r) => MsgR::CreateDocuments(r),
        ReqR::SuggestNewConversation(r) => MsgR::SuggestNewConversation(r),
        ReqR::SuggestPrompt(r) => MsgR::SuggestPrompt(r),
        ReqR::OpenCodeReview(r) => MsgR::OpenCodeReview(r),
        ReqR::TransferShellCommandControlToUser(r) => MsgR::TransferShellCommandControlToUser(r),
        _ => return None,
    };
    Some(msg_side)
}
