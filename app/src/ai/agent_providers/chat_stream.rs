//! Adaptation layer for chat completion + tool calling in BYOP mode (based on genai 0.5.3).
//!
//! Translates `RequestParams` into a genai `ChatRequest`, invokes the user-configured
//! provider via `Client::exec_chat_stream`, translates the response back into
//! `warp_multi_agent_api::ResponseEvent`, and hands off to the controller's own logic
//! (permissions / dialogs / execution / result write-back / triggering next turn) to close the loop.
//!
//! ## Explicit routing for 5 API protocols
//!
//! Instead of forcing all providers through OpenAI-compatible mode, `ServiceTargetResolver`
//! maps the `AgentProviderApiType` selected by the user in the settings UI one-to-one
//! to a genai `AdapterKind`:
//!
//! | ApiType        | AdapterKind  | Default endpoint                               |
//! |----------------|--------------|------------------------------------------------|
//! | OpenAi         | OpenAI       | https://api.openai.com/v1                      |
//! | OpenAiResp     | OpenAIResp   | https://api.openai.com/v1 (via /v1/responses)  |
//! | Gemini         | Gemini       | https://generativelanguage.googleapis.com/v1beta |
//! | Anthropic      | Anthropic    | https://api.anthropic.com                      |
//! | Ollama         | Ollama       | http://localhost:11434                         |
//!
//! The user-provided `base_url` always overrides the default. This way:
//! - DeepSeek / SiliconFlow / OpenRouter and other OpenAI-compatible providers select `OpenAi` with a custom base_url
//! - Explicitly selecting the adapter completely bypasses genai's default "identify by model name" behavior, avoiding misidentification
//!
//! Builds BYOP (Bring Your Own Provider) ChatRequests and processes streaming responses.
//!
//! This module is the core of Waz's BYOP agent path: it translates internal
//! - user query: `ChatMessage::user(text)`
//! - assistant text: `ChatMessage::assistant(text)`
//! **Key responsibilities**:
//! 1. `build_chat_request` — assembles system prompt, multi-turn history, tool definitions,
//!
//!    Anthropic prompt caching markers into a `genai::ChatRequest`.
//!
//! 2. `run_chat_stream` (entry point: `chat_stream`) — sends the request to the upstream model,
//!    parses SSE streaming responses, and produces `AIAgentUpdate` events.
//! - `Start` / `Chunk(text)` / `ReasoningChunk(text)` / `ToolCallChunk(tool_call)` / `End(StreamEnd)`
//!
//! 3. Handles edge cases that standard SDKs don't manage, including:
//!    - Reasoning content gating (DeepSeek / Kimi thinking-mode fields)
//!    - Multimodal binary attachment keep-alive for historical turns

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt;
use instant::Instant;
use serde_json::{json, Value};
use uuid::Uuid;
use warp_multi_agent_api as api;

use genai::adapter::AdapterKind;
use genai::chat::{
    Binary, BinarySource, CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole,
    ChatStreamEvent, ContentPart, MessageContent, Tool as GenaiTool, ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget, WebConfig};

use crate::ai::agent::api::{RequestParams, ResponseStream};
use crate::ai::agent::{AIAgentActionResult, AIAgentInput, RunningCommand, UserQueryMode};
use crate::ai::api_error::AIApiError;
use crate::ai::byop_compaction;
use crate::ai::byop_readiness::{
    classify_projection, AcceptedRepair, BlockedByopReadinessError, LiveToolCall,
    LiveToolCallState, ProjectedToolCall, ProjectedToolResult, ProjectionItem, ReadinessCategory,
    ReadinessContext, ReadinessDiagnosticCoalescer, ReadinessDiagnosticContext,
    ReadinessDiagnosticLevel, ReadinessReport, ReadinessState, ReadinessTriggerLayer,
    RedactedToolKind, RepairSource, RepairStateStatus, TerminalResultKind, ToolCallKey,
    ToolCallRef, ToolResultSource,
};
use crate::settings::AgentProviderApiType;
use ai::agent::convert::ConvertToAPITypeError;

use super::openai_compatible::OpenAiCompatibleError;
use super::tools;

// ---------------------------------------------------------------------------
// System prompt
// ---------------------------------------------------------------------------
/// Renders the SSH session info block appended to the system prompt.
///
/// Under legacy SSH, the PTY runs on the remote host, but `render_system` (via AIAgentContext)
/// reads OS/shell from the local client. This block corrects the LLM's inference.

use super::attachment_caps;
use super::prompt_renderer;
use super::user_context;
use crate::ai::agent::AIAgentContext;

    // If there's no `legacy_ssh_session`, return None — modern remote mode has its own context pipeline.
fn latest_input_context(input: &[AIAgentInput]) -> &[AIAgentContext] {
    for i in input.iter().rev() {
        if let Some(ctx) = i.context() {
            return ctx;
        }
    }
    &[]
}

    // Assemble: tell the model "you are in an SSH session targeting the following host",
    // this affects its choice of paths, package managers, and service management commands.
    // Format follows the existing `<env_context>` XML block convention (see default.j2).
fn render_running_command_context(rc: &RunningCommand) -> String {
    format!(
        "<attached_running_command command_id=\"{}\" is_alt_screen_active=\"{}\">\n  \
         <command>{}</command>\n  \
         <snapshot>\n{}\n  </snapshot>\n  \
         <instructions>This command is already running in the user's terminal. \
         Use `read_shell_command_output` with this command_id to inspect it, and \
         `write_to_long_running_shell_command` with this command_id to operate the program \
         through its PTY (in raw mode, use tokens like `<ESC>` and `<ENTER>` for control \
         keys). This command_id is valid even if the process was started by the user \
         rather than by run_shell_command. Do NOT spawn a new shell to control the same TUI.\
         </instructions>\n\
         </attached_running_command>",
        xml_attr(rc.block_id.as_str()),
        rc.is_alt_screen_active,
        xml_text(&rc.command),
        xml_text(&rc.grid_contents),
    )
}

/// In the LRC (long-running command) tag-in scenario, assembles the current PTY context
/// into a prefix block for the user message. Reuses the same `render_running_command_context`.
fn render_running_command_id_context(command_id: &str) -> String {
    format!(
        "<attached_running_command command_id=\"{}\">\n  \
         <instructions>This command is already running in the user's terminal. \
         Use `read_shell_command_output` with this command_id to inspect it, and \
         `write_to_long_running_shell_command` with this command_id to operate the program \
         through its PTY. Do NOT spawn a new shell to control the same TUI.</instructions>\n\
         </attached_running_command>",
        xml_attr(command_id),
    )
}

fn render_lrc_request_context(params: &RequestParams) -> Option<String> {
    params
        .lrc_running_command
        .as_ref()
        .map(render_running_command_context)
        .or_else(|| {
            params
                .lrc_command_id
                .as_deref()
                .map(render_running_command_id_context)
        })
}

/// Accumulator buffer for streaming assistant message construction.
///
/// During the build_chat_request message loop, each AgentText / ToolCall / AgentReasoning
/// from the same assistant turn is accumulated here. When flushed (via `flush_assistant_buffer`),
/// a single outbound `ChatMessage::assistant(...)` is emitted.
///
/// Purpose:
///
/// - Merge consecutive AgentText into one text ContentPart
/// - Bundle multiple ToolCalls into one assistant message (Anthropic/OpenAI require tool_calls
///
///   to be in the same assistant message)
/// - Attach accumulated reasoning_content to the assistant message
fn render_ssh_session_block(
    session_context: &crate::ai::blocklist::SessionContext,
) -> Option<String> {
    if !session_context.is_legacy_ssh() {
        return None;
    }
    let info = session_context.ssh_connection_info();
    let host = info
        .and_then(|i| i.host.as_deref())
        .map(xml_attr)
        .unwrap_or_else(|| "unknown".to_owned());
    let port = info
        .and_then(|i| i.port.as_deref())
        .map(xml_attr)
        .unwrap_or_else(|| "22".to_owned());

    Some(format!(
        "\n\n<ssh_session host=\"{host}\" port=\"{port}\">\n  \
         <fact>The active terminal PTY is currently inside an SSH session opened by the user from their local machine. \
         All shell commands you run via `run_shell_command` execute on the REMOTE host, not on the local client.</fact>\n  \
         <warning>The [Environment] block (OS / shell / working directory) above describes the LOCAL client and may not match the remote host. \
         If you need precise remote info, probe it directly (e.g. `uname -a`, `cat /etc/os-release`, `pwd`).</warning>\n  \
         <rules>\n    \
         - Run commands DIRECTLY (e.g. `uname -a`, `ls /`). Do NOT prepend `ssh {host} ...` — that opens a NESTED ssh session inside the current one.\n    \
         - Treat the working directory and home directory shown above with skepticism; they may reflect the local client.\n    \
         - When LRC tag-in mode is active (an `<attached_running_command>` block is present), prefer `write_to_long_running_shell_command` with that command_id to inject keystrokes into this same remote PTY. Spawning a new shell would create a separate local-side ssh client, not interact with the remote process the user is watching.\n  \
         </rules>\n\
         </ssh_session>"
    ))
}

/// Unique identifier for a ToolCall within a task, used for:
///
/// 1. Deduplication — the same tool call across root task and subtask only appears once
/// 2. Ordering — maintaining a stable ToolCall sequence within an assistant turn,
///    independent of HashMap iteration order
/// 3. Pairing — matching tool_call_id in ToolCallResult messages
///
///
/// `assistant_tool_call_message_id`: the message_id of the assistant message that
/// originally contained this ToolCall (one assistant message may have multiple tool calls).
/// `tool_call_id`: the individual tool call's id within that assistant message.
/// `task_id`: which task this tool call belongs to.
fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' | '\r' | '\t' => out.push(c),
            c if (c as u32) < 0x20 => out.push(' '),
            // DEL character → space
            '\u{7f}' => out.push(' '),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn xml_attr(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Multi-turn message conversion
// ---------------------------------------------------------------------------

/// Flushes `buf` into a complete `ChatMessage::assistant(...)`, including text, tool_calls,
/// and reasoning_content, then pushes it into `messages`.
///
/// Design note: genai's `ChatMessage` builder accumulates ContentParts into a `MessageContent`;
///
/// text + tool_calls + reasoning_content must all be added to the same message before push.
/// This function is called at every "turn boundary" (when the next message belongs to a different
/// role or a different task), ensuring each assistant turn is a single message.
///
/// `outbound_tool_groups` records which ToolCalls were serialized at which message index,
/// used by `repair_tool_call_pairs_for_accepted_history_gaps` to find the correct
/// insertion position for placeholder ToolResponses.
///
/// `force_echo_reasoning` handling:
/// - When true, if reasoning is empty, fills in an empty string placeholder
///
///   (satisfying DeepSeek/Kimi's "reasoning_content field must exist" validation)
/// - When false, reasoning is not attached at all
///
///   (avoiding Cerebras/Groq/OpenRouter's "unknown field reasoning_content" 400 error)
///
/// Example: DeepSeek-v4-flash thinking-mode endpoint, if a historical assistant message
/// doesn't carry reasoning_content, returns:
///
///   `Each assistant message must have a 'reasoning_content' field`
/// After the fix, all assistant messages carry reasoning_content (real or empty placeholder),
/// complying with the endpoint's strict validation.
///
/// Conversely, Groq's OpenAI-strict endpoint sees reasoning_content and returns:
///   `Unrecognized request argument supplied: reasoning_content`
const REASONING_ECHO_PLACEHOLDER: &str = " ";

#[derive(Default)]
struct AssistantBuffer {
    text: Option<String>,
    tool_calls: Vec<ToolCall>,
    tool_call_keys: Vec<ToolCallKey>,
    // Only create a message when there's actual content (text or tool_calls).
    // An empty buffer occurs when consecutive AgentReasoning messages appear without
    // an interleaved AgentText/ToolCall — don't generate an empty assistant message.
    reasoning: Option<String>,
    // Log diagnostic info: outbound assistant message content for BYOP debugging.
    // First check if it's worth logging to avoid unnecessary string formatting.
    force_echo_reasoning: bool,
}

impl AssistantBuffer {
    fn new(force_echo_reasoning: bool) -> Self {
        Self {
            force_echo_reasoning,
            ..Default::default()
        }
    }

    fn push_tool_call(&mut self, tool_call: ToolCall, key: ToolCallKey) {
        self.tool_calls.push(tool_call);
        self.tool_call_keys.push(key);
    }

    fn flush_into(&mut self, messages: &mut Vec<ChatMessage>) {
        let _ = self.flush_into_with_group(messages);
    }

    fn flush_into_with_group(
        &mut self,
        messages: &mut Vec<ChatMessage>,
    ) -> Option<OutboundAssistantToolGroup> {
        let reasoning = self.reasoning.take();
        let has_tool_calls = !self.tool_calls.is_empty();
        // Also append the accumulated reasoning to the assistant message.
        //
        // **Gate inversion**: when `force_echo_reasoning = false`, **never attach** reasoning,
        // even if this turn's stream received real reasoning (zai-glm / qwen3-thinking and similar
        // thinking models on the OpenAI-compatible path will emit reasoning_content chunks) —
        // because Cerebras / Groq / OpenRouter and other OpenAI-strict providers reject
        // 400 `wrong_api_format`(zerx-lab/warp #25)。
        //
        // `force_echo_reasoning = true` (DeepSeek api_type / OpenAI+kimi/moonshot):
        // - Has real reasoning → use it
        // - No reasoning → non-empty placeholder (satisfying "field must exist" validation)
        let echo_reasoning: Option<String> = if self.force_echo_reasoning {
            match reasoning {
                Some(r) if !r.is_empty() => Some(r),
                _ => Some(REASONING_ECHO_PLACEHOLDER.to_owned()),
            }
        } else {
            // Note: even if `reasoning` is Some(non-empty), discard it — see gate inversion explanation above.
            None
        };
        if let Some(t) = self.text.take() {
            let mut msg = ChatMessage::assistant(t);
            if has_tool_calls {
                // DeepSeek thinking mode requires every assistant message to carry
                // reasoning_content. When text + tool_calls are modeled as two
                // assistant messages by genai, the text one also needs a placeholder.
                if self.force_echo_reasoning {
                    msg = msg.with_reasoning_content(Some(REASONING_ECHO_PLACEHOLDER.to_owned()));
                }
            } else if let Some(r) = echo_reasoning.clone() {
                msg = msg.with_reasoning_content(Some(r));
            }
            messages.push(msg);
        }
        if has_tool_calls {
            // genai `From<Vec<ToolCall>> for ChatMessage` automatically produces
            // assistant role + MessageContent::from_tool_calls.
            let mut msg = ChatMessage::from(std::mem::take(&mut self.tool_calls));
            if let Some(r) = echo_reasoning {
                msg = msg.with_reasoning_content(Some(r));
            }
            let message_index = messages.len();
            messages.push(msg);
            Some(OutboundAssistantToolGroup {
                message_index,
                tool_call_keys: std::mem::take(&mut self.tool_call_keys),
            })
        } else {
            self.tool_call_keys.clear();
            None
        }
    }
}

#[derive(Debug, Clone)]
struct OutboundAssistantToolGroup {
    message_index: usize,
    tool_call_keys: Vec<ToolCallKey>,
}

fn flush_assistant_buffer(
    buf: &mut AssistantBuffer,
    messages: &mut Vec<ChatMessage>,
    outbound_tool_groups: &mut Vec<OutboundAssistantToolGroup>,
) {
    if let Some(group) = buf.flush_into_with_group(messages) {
        if !group.tool_call_keys.is_empty() {
            outbound_tool_groups.push(group);
        }
    }
}

/// Constructs a user `ChatMessage`, deciding whether to switch to
/// `MessageContent::Parts(Text + Binary[])` multimodal form based on model capability.
///
/// - No binaries → old path `ChatMessage::user(text)` plain text, consistent with P0 behavior
/// - Has binaries and model supports the corresponding mime → `Parts(vec![Text(text), Binary(...), ...])`,
///   genai adapter automatically adapts to wire protocol (OpenAI image_url/file, Anthropic image/document,
///   Gemini inline_data, etc.)
/// - Binaries but model doesn't support → log warn and skip that part, degrade to plain text (the
///   `<image .../>` / `<file binary=true .../>` placeholder in the prefix XML remains, so the LLM
fn build_user_message_with_binaries(
    text: String,
    binaries: Vec<user_context::UserBinary>,
    api_type: AgentProviderApiType,
    model_id: &str,
) -> ChatMessage {
    if binaries.is_empty() {
        return ChatMessage::user(text);
    }
    let caps = attachment_caps::caps_for(api_type, model_id);

    let mut parts: Vec<ContentPart> = Vec::with_capacity(1 + binaries.len());
    parts.push(ContentPart::Text(text));

    let mut error_replacements: Vec<(String, String)> = Vec::new();
    for bin in binaries {
        if !caps.supports_mime(&bin.content_type) {
            // Waz aligns with opencode `unsupportedParts` (packages/opencode/src/provider/transform.ts:305-341):
            // Unsupported mime types are not silently dropped; instead an ERROR text part is inserted,
            // letting the LLM inform the user. Wording strictly follows opencode's `ERROR: Cannot read
            // {name} (this model does not support {modality} input). Inform the user.`,
            let modality = mime_to_modality(&bin.content_type);
            let name = if bin.name.is_empty() {
                modality.to_string()
            } else {
                format!("\"{}\"", bin.name)
            };
            let err_text = format!(
                "ERROR: Cannot read {name} (this model does not support {modality} input). Inform the user."
            );
            error_replacements.push((bin.name.clone(), bin.content_type.clone()));
            parts.push(ContentPart::Text(err_text));
            continue;
        }
        parts.push(ContentPart::Binary(Binary::from_base64(
            bin.content_type,
            bin.data,
            Some(bin.name),
        )));
    }

    if !error_replacements.is_empty() {
        log::info!(
            "[byop] {} attachment(s) replaced with ERROR text — model {api_type:?}/{model_id} \
             does not support: {error_replacements:?}",
            error_replacements.len()
        );
    }

    // If all binaries were replaced with ERROR text (no real Binary parts), still keep the ERROR text
    // parts so the model can see them. Degenerate case (e.g., empty text + nothing added) falls back to plain text.
    if parts.len() == 1 {
        if let Some(ContentPart::Text(t)) = parts.into_iter().next() {
            return ChatMessage::user(t);
        }
        return ChatMessage::user("");
    }

    ChatMessage {
        role: ChatRole::User,
        content: MessageContent::from_parts(parts),
        options: None,
    }
}

/// MIME → modality string mapping. Aligned with opencode `mimeToModality`
/// (packages/opencode/src/provider/transform.ts:12-18).
fn mime_to_modality(mime: &str) -> &'static str {
    let lower = mime.trim().to_ascii_lowercase();
    if lower.starts_with("image/") {
        "image"
    } else if lower.starts_with("audio/") {
        "audio"
    } else if lower.starts_with("video/") {
        "video"
    } else if lower == "application/pdf" {
        "pdf"
    } else {
        "file"
    }
}

/// Collects `params.tasks` into a stable linear message sequence, fixing Issue #94.
///
/// `params.tasks` comes from [`crate::ai::agent::conversation::AIConversation::compute_active_tasks`],
/// which uses `HashMap::into_values()` to collect → inter-task ordering is nondeterministic.
/// The old implementation directly `flat_map(|t| t.messages.iter())` for naive concatenation;
/// in multi-task scenarios (LRC CLI subagent / `subagent` tool-derived subtasks), this had two defects:
///   1. Random inter-task ordering → historical turn user messages could be sorted to the end of
///      the sequence, treated by the upstream model as the "latest user message" (the direct
///      symptom of Issue #94).
///   2. When an LRC subagent spawns, the current turn's UserQuery is **simultaneously** copied
///      into both the root task and the new subtask (see the subtask UserQuery injection in
///      `generate_byop_output`; this copy is required for `CLISubagentView` rendering and
///
/// Fix: starting from the root task (no parent, or parent not in the tasks set), perform DFS;
/// when encountering a `Subagent` ToolCall, descend into the corresponding subtask — same deterministic
/// traversal as [`crate::ai::agent::task_store::TaskStore::all_linearized_messages`]; and
/// deduplicate UserQuery by `(request_id, query)`, discarding the LRC-copied subtask duplicate.
/// Different user turns have different `request_id`, so they won't be incorrectly removed; old data /
/// test stubs with empty `request_id` fall back to no-dedup to avoid false positives. Orphan tasks
fn collect_linearized_task_messages(tasks: &[api::Task]) -> Vec<&api::Message> {
    use std::collections::{HashMap, HashSet};

    if tasks.is_empty() {
        return Vec::new();
    }

    let by_id: HashMap<&str, &api::Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    // root = a task with no parent, or whose parent is not in the current tasks set.
    let root = tasks.iter().find(|t| match t.dependencies.as_ref() {
        None => true,
        Some(dep) => {
            dep.parent_task_id.is_empty() || !by_id.contains_key(dep.parent_task_id.as_str())
        }
    });

    fn push_msg<'a>(
        msg: &'a api::Message,
        out: &mut Vec<&'a api::Message>,
        seen_user_queries: &mut HashSet<(&'a str, &'a str)>,
    ) {
        if let Some(api::message::Message::UserQuery(u)) = &msg.message {
            if !msg.request_id.is_empty()
                && !seen_user_queries.insert((msg.request_id.as_str(), u.query.as_str()))
            {
                return;
            }
        }
        out.push(msg);
    }

    fn dfs<'a>(
        task: &'a api::Task,
        by_id: &HashMap<&'a str, &'a api::Task>,
        visited_tasks: &mut HashSet<&'a str>,
        out: &mut Vec<&'a api::Message>,
        seen_user_queries: &mut HashSet<(&'a str, &'a str)>,
    ) {
        if !visited_tasks.insert(task.id.as_str()) {
            return;
        }
        for msg in &task.messages {
            push_msg(msg, out, seen_user_queries);
            if let Some(api::message::Message::ToolCall(tc)) = &msg.message {
                if let Some(api::message::tool_call::Tool::Subagent(sub)) = &tc.tool {
                    if let Some(subtask) = by_id.get(sub.task_id.as_str()) {
                        dfs(subtask, by_id, visited_tasks, out, seen_user_queries);
                    }
                }
            }
        }
    }

    let mut out: Vec<&api::Message> = Vec::new();
    let mut visited_tasks: HashSet<&str> = HashSet::new();
    let mut seen_user_queries: HashSet<(&str, &str)> = HashSet::new();

    if let Some(root) = root {
        dfs(
            root,
            &by_id,
            &mut visited_tasks,
            &mut out,
            &mut seen_user_queries,
        );
    }

    // Orphan task fallback: sort by id for determinism.
    let mut orphans: Vec<&api::Task> = tasks
        .iter()
        .filter(|t| !visited_tasks.contains(t.id.as_str()))
        .collect();
    orphans.sort_by(|a, b| a.id.cmp(&b.id));
    for task in orphans {
        if !visited_tasks.insert(task.id.as_str()) {
            continue;
        }
        for msg in &task.messages {
            push_msg(msg, &mut out, &mut seen_user_queries);
        }
    }

    out
}

struct SerializerProjectionBuilder {
    items: Vec<ProjectionItem>,
    pending_tool_calls: Vec<ProjectedToolCall>,
    pending_task_id: Option<String>,
    pending_assistant_message_id: Option<String>,
    skipped_tool_results: HashSet<(String, String)>,
}

impl SerializerProjectionBuilder {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            pending_tool_calls: Vec::new(),
            pending_task_id: None,
            pending_assistant_message_id: None,
            skipped_tool_results: HashSet::new(),
        }
    }

    fn push_user_boundary(&mut self, task_id: String, message_id: String) {
        self.flush_tool_calls();
        self.items
            .push(ProjectionItem::user_boundary(task_id, message_id));
    }

    fn push_assistant_boundary(&mut self, task_id: String, message_id: String) {
        self.flush_tool_calls();
        self.items
            .push(ProjectionItem::assistant_boundary(task_id, message_id));
    }

    fn push_tool_call(
        &mut self,
        task_id: &str,
        message_id: &str,
        tool_call: &api::message::ToolCall,
    ) {
        use crate::ai::agent::task::helper::ToolCallExt;

        if tool_call.subagent().is_some() {
            self.skipped_tool_results
                .insert((task_id.to_owned(), tool_call.tool_call_id.clone()));
            return;
        }

        if self
            .pending_task_id
            .as_deref()
            .is_some_and(|pending_task_id| pending_task_id != task_id)
        {
            self.flush_tool_calls();
        }

        if self.pending_tool_calls.is_empty() {
            self.pending_task_id = Some(task_id.to_owned());
            self.pending_assistant_message_id = Some(message_id.to_owned());
        }

        let assistant_message_id = self
            .pending_assistant_message_id
            .clone()
            .unwrap_or_else(|| message_id.to_owned());
        self.pending_tool_calls.push(ProjectedToolCall::new(
            task_id,
            assistant_message_id,
            tool_call.tool_call_id.clone(),
            redacted_tool_kind_for_tool_call(tool_call),
        ));
    }

    fn push_tool_result(&mut self, result: ProjectedToolResult) {
        self.flush_tool_calls();
        self.items.push(ProjectionItem::tool_result(result));
    }

    fn should_skip_tool_result(&self, task_id: &str, tool_call_id: &str) -> bool {
        self.skipped_tool_results
            .contains(&(task_id.to_owned(), tool_call_id.to_owned()))
    }

    fn finish(mut self) -> Vec<ProjectionItem> {
        self.flush_tool_calls();
        self.items
    }

    fn flush_tool_calls(&mut self) {
        if self.pending_tool_calls.is_empty() {
            return;
        }

        let task_id = self.pending_task_id.take().unwrap_or_default();
        let assistant_message_id = self.pending_assistant_message_id.take().unwrap_or_default();
        self.items.push(ProjectionItem::assistant_tool_calls(
            task_id,
            assistant_message_id,
            std::mem::take(&mut self.pending_tool_calls),
        ));
    }
}

fn redacted_tool_kind_for_tool_call(tool_call: &api::message::ToolCall) -> RedactedToolKind {
    use crate::ai::agent::task::helper::ToolExt;

    tool_call
        .tool
        .as_ref()
        .map(|tool| RedactedToolKind::new(tool.name()))
        .unwrap_or_default()
}

fn current_input_result_kind(result: &AIAgentActionResult) -> TerminalResultKind {
    if result.result.is_cancelled() {
        TerminalResultKind::Cancellation
    } else {
        TerminalResultKind::Real
    }
}

fn persisted_tool_result_kind(
    msg: &api::Message,
    compacted_tool_msg_ids: Option<&std::collections::HashSet<String>>,
) -> TerminalResultKind {
    if compacted_tool_msg_ids.is_some_and(|ids| ids.contains(&msg.id)) {
        return TerminalResultKind::Compacted;
    }

    let Some(api::message::Message::ToolCallResult(tool_call_result)) = msg.message.as_ref() else {
        return TerminalResultKind::Real;
    };
    if tool_call_result.result.is_some() {
        return TerminalResultKind::Real;
    }

    let content = msg.server_message_data.trim();
    if content.is_empty() {
        return TerminalResultKind::Real;
    }

    match serde_json::from_str::<Value>(content) {
        Ok(value) => {
            if value
                .get("_byop_intercepted")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                TerminalResultKind::LocalInterception
            } else if value.get("error").and_then(Value::as_str) == Some("invalid_arguments") {
                TerminalResultKind::StructuredError
            } else {
                TerminalResultKind::Real
            }
        }
        Err(_)
            if content.contains("_byop_intercepted") || content.contains("invalid_arguments") =>
        {
            TerminalResultKind::UnreadableLocalInterception
        }
        Err(_) => TerminalResultKind::Real,
    }
}

fn current_input_task_id(params: &RequestParams) -> String {
    params
        .byop_target_task_id
        .clone()
        .or_else(|| params.tasks.first().map(|task| task.id.clone()))
        .unwrap_or_else(|| "current_input".to_owned())
}

fn build_serializer_readiness_projection(
    params: &RequestParams,
    all_msgs: &[&api::Message],
    summarize_head_end: Option<usize>,
    summary_inserts: &HashMap<String, String>,
    hidden_msg_ids: &std::collections::HashSet<String>,
    compacted_tool_msg_ids: &std::collections::HashSet<String>,
) -> Vec<ProjectionItem> {
    let mut builder = SerializerProjectionBuilder::new();

    for (idx, msg) in all_msgs.iter().enumerate() {
        if let Some(head_end) = summarize_head_end {
            if idx >= head_end {
                continue;
            }
        }

        if hidden_msg_ids.contains(&msg.id) {
            if summary_inserts.contains_key(&msg.id) {
                builder.push_user_boundary(msg.task_id.clone(), format!("summary_user:{}", msg.id));
                builder.push_assistant_boundary(
                    msg.task_id.clone(),
                    format!("summary_assistant:{}", msg.id),
                );
            }
            continue;
        }

        let Some(inner) = &msg.message else {
            continue;
        };

        match inner {
            api::message::Message::UserQuery(_) => {
                builder.push_user_boundary(msg.task_id.clone(), msg.id.clone());
            }
            api::message::Message::AgentOutput(_) => {
                builder.push_assistant_boundary(msg.task_id.clone(), msg.id.clone());
            }
            api::message::Message::AgentReasoning(_) => {}
            api::message::Message::ToolCall(tool_call) => {
                builder.push_tool_call(&msg.task_id, &msg.id, tool_call);
            }
            api::message::Message::ToolCallResult(tool_call_result) => {
                if builder.should_skip_tool_result(&msg.task_id, &tool_call_result.tool_call_id) {
                    continue;
                }
                builder.push_tool_result(ProjectedToolResult::new(
                    msg.task_id.clone(),
                    msg.id.clone(),
                    None,
                    tool_call_result.tool_call_id.clone(),
                    RedactedToolKind::default(),
                    ToolResultSource::PersistedHistory,
                    persisted_tool_result_kind(msg, Some(compacted_tool_msg_ids)),
                ));
            }
            api::message::Message::ServerEvent(_)
            | api::message::Message::SystemQuery(_)
            | api::message::Message::UpdateTodos(_)
            | api::message::Message::Summarization(_)
            | api::message::Message::CodeReview(_)
            | api::message::Message::UpdateReviewComments(_)
            | api::message::Message::WebSearch(_)
            | api::message::Message::WebFetch(_)
            | api::message::Message::DebugOutput(_)
            | api::message::Message::ArtifactEvent(_)
            | api::message::Message::InvokeSkill(_)
            | api::message::Message::MessagesReceivedFromAgents(_)
            | api::message::Message::ModelUsed(_)
            | api::message::Message::EventsFromAgents(_)
            | api::message::Message::PassiveSuggestionResult(_) => {}
        }
    }

    let current_task_id = current_input_task_id(params);
    for (idx, input) in params.input.iter().enumerate() {
        match input {
            AIAgentInput::UserQuery { .. }
            | AIAgentInput::InvokeSkill { .. }
            | AIAgentInput::ResumeConversation { .. }
            | AIAgentInput::SummarizeConversation { .. } => {
                builder.push_user_boundary(
                    current_task_id.clone(),
                    format!("current_input:{idx}:user"),
                );
            }
            AIAgentInput::ActionResult { result, .. } => {
                let tool_call_id = result.id.to_string();
                builder.push_tool_result(ProjectedToolResult::new(
                    result.task_id.to_string(),
                    format!("current_input:{idx}:{tool_call_id}"),
                    None,
                    tool_call_id,
                    RedactedToolKind::default(),
                    ToolResultSource::CurrentInput,
                    current_input_result_kind(result),
                ));
            }
            AIAgentInput::AutoCodeDiffQuery { .. }
            | AIAgentInput::InitProjectRules { .. }
            | AIAgentInput::TriggerPassiveSuggestion { .. }
            | AIAgentInput::CreateNewProject { .. }
            | AIAgentInput::CloneRepository { .. }
            | AIAgentInput::CodeReview { .. }
            | AIAgentInput::FetchReviewComments { .. }
            | AIAgentInput::StartFromAmbientRunPrompt { .. }
            | AIAgentInput::MessagesReceivedFromAgents { .. }
            | AIAgentInput::EventsFromAgents { .. }
            | AIAgentInput::PassiveSuggestionResult { .. } => {}
        }
    }

    builder.finish()
}

pub(crate) fn classify_byop_controller_readiness(params: &RequestParams) -> ReadinessReport {
    classify_byop_controller_readiness_with_live_tool_calls(params, Vec::new())
}

pub(crate) fn classify_byop_controller_readiness_with_live_tool_calls(
    params: &RequestParams,
    live_tool_calls: Vec<LiveToolCall>,
) -> ReadinessReport {
    let skipped_cancellation_results = current_input_cancellation_result_keys(params);
    let projection = build_controller_readiness_projection(params, &skipped_cancellation_results);
    let mut live_tool_calls_for_context =
        cancellation_live_tool_calls(params, &skipped_cancellation_results);
    live_tool_calls_for_context.extend(live_tool_calls);
    let context = ReadinessContext {
        repair_records: params.byop_repair_state.repair_records().to_vec(),
        live_tool_calls: live_tool_calls_for_context,
    };
    classify_projection(&projection, &context)
}

fn current_input_cancellation_result_keys(params: &RequestParams) -> HashSet<(String, String)> {
    params
        .input
        .iter()
        .filter_map(|input| {
            let AIAgentInput::ActionResult { result, .. } = input else {
                return None;
            };
            result
                .result
                .is_cancelled()
                .then(|| (result.task_id.to_string(), result.id.to_string()))
        })
        .collect()
}

fn cancellation_live_tool_calls(
    params: &RequestParams,
    skipped_cancellation_results: &HashSet<(String, String)>,
) -> Vec<LiveToolCall> {
    use crate::ai::agent::task::helper::ToolCallExt;

    params
        .tasks
        .iter()
        .flat_map(|task| task.messages.iter())
        .filter_map(|msg| {
            let api::message::Message::ToolCall(tool_call) = msg.message.as_ref()? else {
                return None;
            };
            if tool_call.subagent().is_some() {
                return None;
            }
            if !skipped_cancellation_results
                .contains(&(msg.task_id.clone(), tool_call.tool_call_id.clone()))
            {
                return None;
            }
            Some(LiveToolCall::new(
                ToolCallRef::new(
                    ToolCallKey::new(&msg.task_id, &msg.id, &tool_call.tool_call_id),
                    redacted_tool_kind_for_tool_call(tool_call),
                ),
                LiveToolCallState::CancellationRequested,
            ))
        })
        .collect()
}

fn build_controller_readiness_projection(
    params: &RequestParams,
    skipped_current_action_results: &HashSet<(String, String)>,
) -> Vec<ProjectionItem> {
    let mut builder = SerializerProjectionBuilder::new();

    for msg in params.tasks.iter().flat_map(|task| task.messages.iter()) {
        let Some(inner) = &msg.message else {
            continue;
        };

        match inner {
            api::message::Message::UserQuery(_) => {
                builder.push_user_boundary(msg.task_id.clone(), msg.id.clone());
            }
            api::message::Message::AgentOutput(_) => {
                builder.push_assistant_boundary(msg.task_id.clone(), msg.id.clone());
            }
            api::message::Message::AgentReasoning(_) => {}
            api::message::Message::ToolCall(tool_call) => {
                builder.push_tool_call(&msg.task_id, &msg.id, tool_call);
            }
            api::message::Message::ToolCallResult(tool_call_result) => {
                if builder.should_skip_tool_result(&msg.task_id, &tool_call_result.tool_call_id) {
                    continue;
                }
                builder.push_tool_result(ProjectedToolResult::new(
                    msg.task_id.clone(),
                    msg.id.clone(),
                    None,
                    tool_call_result.tool_call_id.clone(),
                    RedactedToolKind::default(),
                    ToolResultSource::PersistedHistory,
                    persisted_tool_result_kind(msg, None),
                ));
            }
            api::message::Message::ServerEvent(_)
            | api::message::Message::SystemQuery(_)
            | api::message::Message::UpdateTodos(_)
            | api::message::Message::Summarization(_)
            | api::message::Message::CodeReview(_)
            | api::message::Message::UpdateReviewComments(_)
            | api::message::Message::WebSearch(_)
            | api::message::Message::WebFetch(_)
            | api::message::Message::DebugOutput(_)
            | api::message::Message::ArtifactEvent(_)
            | api::message::Message::InvokeSkill(_)
            | api::message::Message::MessagesReceivedFromAgents(_)
            | api::message::Message::ModelUsed(_)
            | api::message::Message::EventsFromAgents(_)
            | api::message::Message::PassiveSuggestionResult(_) => {}
        }
    }

    let current_task_id = current_input_task_id(params);
    for (idx, input) in params.input.iter().enumerate() {
        match input {
            AIAgentInput::UserQuery { .. }
            | AIAgentInput::InvokeSkill { .. }
            | AIAgentInput::ResumeConversation { .. }
            | AIAgentInput::SummarizeConversation { .. } => {
                builder.push_user_boundary(
                    current_task_id.clone(),
                    format!("current_input:{idx}:user"),
                );
            }
            AIAgentInput::ActionResult { result, .. } => {
                let tool_call_id = result.id.to_string();
                if skipped_current_action_results
                    .contains(&(result.task_id.to_string(), tool_call_id.clone()))
                {
                    continue;
                }
                builder.push_tool_result(ProjectedToolResult::new(
                    result.task_id.to_string(),
                    format!("current_input:{idx}:{tool_call_id}"),
                    None,
                    tool_call_id,
                    RedactedToolKind::default(),
                    ToolResultSource::CurrentInput,
                    current_input_result_kind(result),
                ));
            }
            AIAgentInput::AutoCodeDiffQuery { .. }
            | AIAgentInput::InitProjectRules { .. }
            | AIAgentInput::TriggerPassiveSuggestion { .. }
            | AIAgentInput::CreateNewProject { .. }
            | AIAgentInput::CloneRepository { .. }
            | AIAgentInput::CodeReview { .. }
            | AIAgentInput::FetchReviewComments { .. }
            | AIAgentInput::StartFromAmbientRunPrompt { .. }
            | AIAgentInput::MessagesReceivedFromAgents { .. }
            | AIAgentInput::EventsFromAgents { .. }
            | AIAgentInput::PassiveSuggestionResult { .. } => {}
        }
    }

    builder.finish()
}

fn validate_byop_serializer_readiness(
    params: &RequestParams,
    all_msgs: &[&api::Message],
    summarize_head_end: Option<usize>,
    summary_inserts: &HashMap<String, String>,
    hidden_msg_ids: &std::collections::HashSet<String>,
    compacted_tool_msg_ids: &std::collections::HashSet<String>,
) -> Result<ReadinessReport, ConvertToAPITypeError> {
    let projection = build_serializer_readiness_projection(
        params,
        all_msgs,
        summarize_head_end,
        summary_inserts,
        hidden_msg_ids,
        compacted_tool_msg_ids,
    );
    let conversation_id = params
        .byop_conversation_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let request_attempt_id = params
        .byop_readiness_attempt_id
        .clone()
        .unwrap_or_else(|| "serializer-unknown".to_owned());
    validate_serializer_readiness_projection_with_repair_state(
        projection,
        &params.byop_repair_state,
        &ReadinessDiagnosticContext::new(
            &conversation_id,
            &request_attempt_id,
            ReadinessTriggerLayer::SerializerValidation,
        ),
    )
}

fn validate_serializer_readiness_projection(
    projection: Vec<ProjectionItem>,
) -> Result<ReadinessReport, ConvertToAPITypeError> {
    validate_serializer_readiness_projection_with_repair_state(
        projection,
        &RepairStateStatus::default(),
        &ReadinessDiagnosticContext::new(
            "test-conversation",
            "test-attempt",
            ReadinessTriggerLayer::SerializerValidation,
        ),
    )
}

fn validate_serializer_readiness_projection_with_repair_state(
    projection: Vec<ProjectionItem>,
    repair_state: &RepairStateStatus,
    diagnostic_context: &ReadinessDiagnosticContext<'_>,
) -> Result<ReadinessReport, ConvertToAPITypeError> {
    let context = ReadinessContext {
        repair_records: repair_state.repair_records().to_vec(),
        live_tool_calls: Vec::new(),
    };
    let report = classify_projection(&projection, &context);

    match &report.state {
        ReadinessState::Ready => {
            if let Some(error_category) = repair_state.error_category() {
                log::error!(
                    "[byop-readiness] serializer continuing with invalid repair sidecar \
                     category={error_category:?} projection_items={}",
                    projection.len()
                );
            }
            Ok(report)
        }
        ReadinessState::AcceptedHistoryRepair { repairs } => {
            log_accepted_history_repair(repairs, diagnostic_context);
            Ok(report)
        }
        ReadinessState::PendingToolResults { .. }
        | ReadinessState::NeedsCancellationCommit { .. }
        | ReadinessState::DuplicateToolResults { .. }
        | ReadinessState::OrphanToolResult { .. }
        | ReadinessState::OutOfOrderToolResult { .. }
        | ReadinessState::MissingResultWithoutRepairSource { .. } => {
            let category = report.state.category();
            let mut diagnostics = ReadinessDiagnosticCoalescer::default();
            diagnostics.log_state(
                &report.state,
                diagnostic_context,
                ReadinessDiagnosticLevel::Error,
            );
            diagnostics.finish(diagnostic_context, ReadinessDiagnosticLevel::Error);
            log::error!(
                "[byop-readiness] serializer blocked request category={category:?} \
                 projection_items={} ignored_repair_records={} trigger_layer=serializer_validation \
                 request_attempt_id={}",
                projection.len(),
                report.ignored_repair_records.len(),
                diagnostic_context.request_attempt_id
            );

            Err(ConvertToAPITypeError::Other(
                BlockedByopReadinessError::new(category).into(),
            ))
        }
    }
}

fn log_accepted_history_repair(
    repairs: &[AcceptedRepair],
    diagnostic_context: &ReadinessDiagnosticContext<'_>,
) {
    log::info!(
        "{}",
        accepted_history_repair_log_message(repairs, diagnostic_context)
    );
}

fn accepted_history_repair_log_message(
    repairs: &[AcceptedRepair],
    diagnostic_context: &ReadinessDiagnosticContext<'_>,
) -> String {
    let forked_history_count = repairs
        .iter()
        .filter(|repair| matches!(repair.record.source, RepairSource::ForkedHistory))
        .count();
    let restored_legacy_history_count = repairs
        .iter()
        .filter(|repair| matches!(repair.record.source, RepairSource::RestoredLegacyHistory))
        .count();

    format!(
        "[byop-readiness] serializer accepted history repair records={} \
         category={:?} forked_history={} restored_legacy_history={} conversation_id={} \
         trigger_layer=serializer_validation request_attempt_id={} repair_keys={:?}",
        repairs.len(),
        ReadinessCategory::AcceptedHistoryRepair,
        forked_history_count,
        restored_legacy_history_count,
        diagnostic_context.conversation_id,
        diagnostic_context.request_attempt_id,
        repairs
            .iter()
            .map(|repair| format!(
                "task_id={} assistant_tool_call_message_id={} tool_call_id={} redacted_tool_kind={}",
                repair.tool_call.key.task_id,
                repair.tool_call.key.assistant_tool_call_message_id,
                repair.tool_call.key.tool_call_id,
                repair.tool_call.redacted_tool_kind.as_str()
            ))
            .collect::<Vec<_>>()
    )
}

/// Translates RequestParams into a genai `ChatRequest` (including system + messages + tools).
///
/// `force_echo_reasoning`: determined by `super::reasoning::model_requires_reasoning_echo`.
/// When true, all assistant messages are forced to carry reasoning_content (empty string
/// placeholder), fixing thinking-mode endpoints with stricter validation like
fn build_chat_request(
    params: &RequestParams,
    force_echo_reasoning: bool,
    api_type: AgentProviderApiType,
    model_id: &str,
) -> Result<ChatRequest, ConvertToAPITypeError> {
    let agent_ctx = latest_input_context(&params.input);
    let plan_mode = is_plan_mode_turn(&params.input);
    let tool_names = available_tool_names(params);
    let query = params.input.iter().find_map(|input| match input {
        AIAgentInput::UserQuery { query, .. } => Some(query.as_str()),
        AIAgentInput::AutoCodeDiffQuery { query, .. } => Some(query.as_str()),
        AIAgentInput::CreateNewProject { query, .. } => Some(query.as_str()),
        _ => None,
    });
    let mut system_text = prompt_renderer::render_system(
        &params.model,
        agent_ctx,
        &tool_names,
        plan_mode,
        &params.user_rules,
        query,
    );
    // Waz: legacy SSH session profile patch. `render_system` goes through AIAgentContext,
    // where the OS/shell obtained is from the local client; under legacy SSH the PTY is
    // actually on the remote host, so an SSH state block is appended to correct the LLM's inference.
    if let Some(ssh_block) = render_ssh_session_block(&params.session_context) {
        system_text.push_str(&ssh_block);
    }
    // Note: Tool usage guidance for LRC / long-running commands (write_to_long_running_shell_command
    // + command_id + various modes and raw byte sequences) is already fully covered in
    // `prompts/system/default.j2:69-79`. The specific PTY context the user is currently in
    // (command name / alt-screen flag / grid content) is injected separately via the
    // `<attached_running_command>` XML block prefixed to the user message (see
    // `render_running_command_context` and the UserQuery branch in build_chat_request).

    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut outbound_tool_groups: Vec<OutboundAssistantToolGroup> = Vec::new();

    // Collect all task messages, using `collect_linearized_task_messages` for deterministic
    // DFS linearization + UserQuery deduplication (fixing Issue #94 — historical user
    // messages reordered to the end, or duplicated by LRC subagent copies). See that function's docs.
    let all_msgs: Vec<&api::Message> = collect_linearized_task_messages(&params.tasks);

    // Waz BYOP local session compaction: apply conversation.compaction_state to the message sequence.
    //   1. Filter out (user, assistant) pairs already covered by a compaction (`hidden_message_ids`)
    //   2. At the position of hidden intervals, insert a synthetic pair (user "compacted, summary below" +
    //      assistant summary text) message — this is done via `summary_inserts` index, emitted
    //      in-place during the main loop
    //
    // When the current input is `AIAgentInput::SummarizeConversation`: further use a selection algorithm
    // to trim messages to head (removing tail), and at the end of the input loop append `build_prompt(...)`
    // as a user message (using the full SUMMARY_TEMPLATE), so the upstream LLM outputs a structured summary.
    let is_summarization_request = params
        .input
        .iter()
        .any(|i| matches!(i, AIAgentInput::SummarizeConversation { .. }));
    let summarization_overflow = params.input.iter().any(|i| {
        matches!(
            i,
            AIAgentInput::SummarizeConversation { overflow: true, .. }
        )
    });
    let _ = summarization_overflow; // Currently used in the follow-up wording branch within the input loop; silence dead code for now

    let summary_inserts: std::collections::HashMap<String, String> =
        if let Some(state) = params.compaction_state.as_ref() {
            // user_msg_id → summary_text; when this user_msg_id is encountered (which would normally be hidden), replace with a synthetic summary pair
            state
                .completed()
                .iter()
                .filter_map(|c| {
                    c.summary_text
                        .as_ref()
                        .map(|s| (c.user_msg_id.clone(), s.clone()))
                })
                .collect()
        } else {
            std::collections::HashMap::new()
        };
    let hidden_msg_ids: std::collections::HashSet<String> = params
        .compaction_state
        .as_ref()
        .map(|s| s.hidden_message_ids())
        .unwrap_or_default();
    let compacted_tool_msg_ids: std::collections::HashSet<String> = params
        .compaction_state
        .as_ref()
        .map(|s| {
            // Collect all ToolCallResult message_ids that have tool_output_compacted_at set,
            // by iterating all_msgs and checking markers
            let mut out = std::collections::HashSet::new();
            for msg in &all_msgs {
                if let Some(api::message::Message::ToolCallResult(_)) = &msg.message {
                    if s.marker(&msg.id)
                        .and_then(|m| m.tool_output_compacted_at)
                        .is_some()
                    {
                        out.insert(msg.id.clone());
                    }
                }
            }
            out
        })
        .unwrap_or_default();

    // Summarization request path: use byop_compaction::algorithm::select to cut head; tail is not sent upstream
    let summarize_head_end: Option<usize> = if is_summarization_request {
        // Temporarily project into WarpMessageView for the select algorithm
        let state_for_select = params.compaction_state.clone().unwrap_or_default();
        let tool_names =
            byop_compaction::message_view::build_tool_name_lookup(all_msgs.iter().copied());
        let views =
            byop_compaction::message_view::project(&all_msgs, &state_for_select, &tool_names);
        let cfg = byop_compaction::CompactionConfig::default();
        let model_limit = byop_compaction::overflow::ModelLimit::FALLBACK;
        let result = byop_compaction::algorithm::select(&views, &cfg, model_limit, |slice| {
            slice
                .iter()
                .map(byop_compaction::algorithm::MessageRef::estimate_size)
                .sum()
        });
        // head_end is the upper bound of the "head interval" in views, same ordering as all_msgs
        Some(result.head_end)
    } else {
        None
    };

    let readiness_report = validate_byop_serializer_readiness(
        params,
        &all_msgs,
        summarize_head_end,
        &summary_inserts,
        &hidden_msg_ids,
        &compacted_tool_msg_ids,
    )?;

    let mut buf = AssistantBuffer::new(force_echo_reasoning);
    // Waz: call_ids of subagent ToolCalls that were skipped in the history — their
    // ToolCallResults must also be skipped, otherwise they become orphan tool_responses
    // `unexpected tool_use_id ... no corresponding tool_use block`。
    let mut skipped_subagent_call_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (idx, msg) in all_msgs.iter().enumerate() {
        // Summarization request: tail interval is not sent upstream (only send head + append SUMMARY_TEMPLATE at the end)
        if let Some(head_end) = summarize_head_end {
            if idx >= head_end {
                continue;
            }
        }
        if hidden_msg_ids.contains(&msg.id) {
            if let Some(summary_text) = summary_inserts.get(&msg.id) {
                flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);
                messages.push(ChatMessage::user(
                    "Conversation history was compacted. Below is the structured summary of all prior turns.".to_string(),
                ));
                messages.push(ChatMessage::assistant(summary_text.clone()));
            }
            continue;
        }
        let Some(inner) = &msg.message else {
            continue;
        };
        match inner {
            api::message::Message::UserQuery(u) => {
                flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);
                // Waz: multimodal keep-alive for historical turns. Warp's native path relies on
                // the cloud server to re-inject InputContext, but BYOP direct connection doesn't have
                // that layer, so `make_user_query_message` stores all binaries (image / pdf / audio)
                // into `UserQuery.context.images` at persistence time. Here we reverse-recover them
                // into UserBinary and go through `build_user_message_with_binaries`, so the model in
                // subsequent turns can still see previously pasted multimodal attachments. Unsupported
                // mimes are replaced with ERROR text (opencode unsupportedParts style), not silently dropped.
                let history_binaries: Vec<user_context::UserBinary> = u
                    .context
                    .as_ref()
                    .map(|ctx| {
                        ctx.images
                            .iter()
                            .filter(|b| !b.data.is_empty())
                            .enumerate()
                            .map(|(idx, b)| {
                                use base64::Engine;
                                user_context::UserBinary {
                                    name: format!("history-attachment-{}-{idx}", &msg.id),
                                    content_type: if b.mime_type.is_empty() {
                                        "application/octet-stream".to_string()
                                    } else {
                                        b.mime_type.clone()
                                    },
                                    data: base64::engine::general_purpose::STANDARD.encode(&b.data),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut history_prefixes: Vec<String> = Vec::new();
                if let Some(prefix) =
                    user_context::render_api_referenced_attachments(&u.referenced_attachments)
                {
                    history_prefixes.push(prefix);
                }
                let history_text = if history_prefixes.is_empty() {
                    u.query.clone()
                } else {
                    format!("{}\n\n{}", history_prefixes.join("\n\n"), u.query)
                };
                if history_binaries.is_empty() {
                    messages.push(ChatMessage::user(history_text));
                } else {
                    messages.push(build_user_message_with_binaries(
                        history_text,
                        history_binaries,
                        api_type,
                        model_id,
                    ));
                }
            }
            api::message::Message::AgentReasoning(r) => {
                // Attach the previous turn's reasoning to the next assistant message to be flushed.
                // genai 0.6's with_reasoning_content serializes per the current adapter:
                // DeepSeek/Kimi → reasoning_content field; Anthropic → thinking blocks.
                // Multiple AgentReasoning segments are accumulated (the same turn may stream
                // out multiple reasoning chunks that get persisted as multiple AgentReasoning messages).
                let next = r.reasoning.clone();
                if !next.is_empty() {
                    match buf.reasoning.as_mut() {
                        Some(existing) => existing.push_str(&next),
                        None => buf.reasoning = Some(next),
                    }
                }
            }
            api::message::Message::AgentOutput(a) => {
                if buf.text.is_some() || !buf.tool_calls.is_empty() {
                    flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);
                }
                buf.text = Some(a.text.clone());
            }
            api::message::Message::ToolCall(tc) => {
                // Waz BYOP: **virtual subagent tool_calls are NOT sent to the upstream model**.
                // In the LRC tag-in scenario, we synthesize a `Tool::Subagent { metadata: Cli }`
                // at the beginning of the chat_stream flow and write it into root.task.messages,
                // only to trigger the conversation to create a cli subtask + spawn the floating
                // window. It's not an actual tool call produced by the model; the model would be
                // confused if it saw it (extra tool call + no way to respond). Likewise, its
                use crate::ai::agent::task::helper::ToolCallExt;
                if tc.subagent().is_some() {
                    skipped_subagent_call_ids.insert(tc.tool_call_id.clone());
                    continue;
                }
                if buf
                    .tool_call_keys
                    .first()
                    .is_some_and(|key| key.task_id != msg.task_id)
                {
                    flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);
                }
                let (name, args_json) = serialize_outgoing_tool_call(
                    tc,
                    params.mcp_context.as_ref(),
                    &msg.server_message_data,
                );
                let assistant_message_id = buf
                    .tool_call_keys
                    .first()
                    .map(|key| key.assistant_tool_call_message_id.clone())
                    .unwrap_or_else(|| msg.id.clone());
                let key = ToolCallKey::new(&msg.task_id, assistant_message_id, &tc.tool_call_id);
                buf.push_tool_call(
                    ToolCall {
                        call_id: tc.tool_call_id.clone(),
                        fn_name: name,
                        fn_arguments: args_json,
                        thought_signatures: None,
                    },
                    key,
                );
            }
            api::message::Message::ToolCallResult(tcr) => {
                flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);
                // Waz: the corresponding ToolCall was already skipped (virtual subagent call)
                // → skip the result too, otherwise an orphan tool_response causes upstream 400.
                if skipped_subagent_call_ids.contains(&tcr.tool_call_id) {
                    continue;
                }
                // BYOP persisted ToolCallResult goes through server_message_data (content is already a JSON string);
                // server-side emit goes through the result oneof structured variant — compatible with both paths.
                let content = if compacted_tool_msg_ids.contains(&msg.id) {
                    // Compaction projection: pruned tool output is replaced with a placeholder; actual content is not sent upstream
                    r#"{"status":"compacted","note":"tool output was pruned by local compaction"}"#
                        .to_string()
                } else if tcr.result.is_some() {
                    tools::serialize_result(tcr)
                } else if !msg.server_message_data.is_empty() {
                    msg.server_message_data.clone()
                } else {
                    r#"{"status":"empty"}"#.to_owned()
                };
                messages.push(ChatMessage::from(ToolResponse::new(
                    tcr.tool_call_id.clone(),
                    content,
                )));
            }
            _ => {
                // Other message types (SystemQuery/UpdateTodos/...) are not sent upstream in BYOP for now.
            }
        }
    }
    flush_assistant_buffer(&mut buf, &mut messages, &mut outbound_tool_groups);

    // Current turn new input → append.
    for input in &params.input {
        match input {
            AIAgentInput::UserQuery {
                query,
                context,
                referenced_attachments,
                running_command,
                ..
            } => {
                // Current turn UserQuery's attachment-type context (Block / SelectedText / File / Image)
                // strictly aligns with warp's native path that goes through `api::InputContext.executed_shell_commands`
                // etc. fields, uploaded and then injected into the prompt by the backend. BYOP doesn't have that
                // backend layer, so it's directly prepended to the user message.
                // Environment-type context (env / git / skills / ...) is rendered into system by prompt_renderer
                // and does not overlap with this path.
                //
                // Waz: In the LRC tag-in scenario, `running_command: Some(...)` contains the full PTY context
                // (alt-screen grid_contents + command + is_alt_screen_active flag), rendered into an
                // `<attached_running_command>` XML block via `render_running_command_context`.
                // The model uses this to decide whether to call write_to_long_running_shell_command.
                // When not provided (normal conversation or controller didn't inject), falls back to
                // the short `lrc_command_id` context.
                // **P1-10 prompt cache optimization**: The LRC context block is **appended after the query**
                // rather than prefixed. Reason:
                //   - grid_contents changes every second with PTY state; it's "high-frequency volatile" content.
                //   - Placing it before the query makes the user message head unstable → the hash written by
                //     the last 2 Anthropic breakpoints in the messages segment always differs, reducing reuse value.
                //   - Placing it after the query means the same query (e.g., "exit nvim") across different PTY
                //     snapshots still shares the "user question" prefix, improving cross-call reuse potential.
                // Model behavior difference is minimal: whether instructions come first or context comes first,
                // the model can understand correctly. user_attachments' prefix (e.g., SelectedText / Block) still
                // goes in the prefix position since they represent content the user "explicitly selected".
                let mut suffixes: Vec<String> = Vec::new();
                let request_running_command = running_command
                    .as_ref()
                    .or(params.lrc_running_command.as_ref());
                if let Some(rc) = request_running_command {
                    suffixes.push(render_running_command_context(rc));
                } else if let Some(command_id) = params.lrc_command_id.as_deref() {
                    suffixes.push(render_running_command_id_context(command_id));
                }
                let mut prefixes: Vec<String> = Vec::new();
                let user_attachments = user_context::collect_user_attachments(context);
                if let Some(p) = &user_attachments.prefix {
                    prefixes.push(p.clone());
                }
                if let Some(p) = user_context::render_referenced_attachments(referenced_attachments)
                {
                    prefixes.push(p);
                }
                let full_text = match (prefixes.is_empty(), suffixes.is_empty()) {
                    (true, true) => query.clone(),
                    (false, true) => format!("{}\n\n{query}", prefixes.join("\n\n")),
                    (true, false) => format!("{query}\n\n{}", suffixes.join("\n\n")),
                    (false, false) => format!(
                        "{}\n\n{query}\n\n{}",
                        prefixes.join("\n\n"),
                        suffixes.join("\n\n"),
                    ),
                };
                log::info!(
                    "[byop-diag] build_chat_request UserQuery: query_len={} \
                     running_command={} prefixes={} suffixes={} full_text_len={} binaries={}",
                    query.len(),
                    match request_running_command {
                        Some(rc) => format!(
                            "Some(grid_len={} alt={})",
                            rc.grid_contents.len(),
                            rc.is_alt_screen_active
                        ),
                        None => "None".to_owned(),
                    },
                    prefixes.len(),
                    suffixes.len(),
                    full_text.len(),
                    user_attachments.binaries.len(),
                );
                messages.push(build_user_message_with_binaries(
                    full_text,
                    user_attachments.binaries,
                    api_type,
                    model_id,
                ));
            }
            AIAgentInput::ActionResult { result, .. } => {
                // The previous turn's model responded with tool_calls; after client-side execution,
                // the result comes through `params.input` rather than `params.tasks` history.
                // It must be serialized as a ToolResponse here, otherwise genai/upstream will
                // return 400 due to tool_call_id pairing failure.
                let tool_call_id = result.id.to_string();
                let content = tools::serialize_action_result(result).unwrap_or_else(|| {
                    serde_json::json!({ "result": result.result.to_string() }).to_string()
                });
                messages.push(ChatMessage::from(ToolResponse::new(tool_call_id, content)));
            }
            AIAgentInput::InvokeSkill {
                skill, user_query, ..
            } => {
                let mut composed = format!(
                    "Please follow the instructions for the skill \"{}\" below to perform the task:\n\n{}\n\n---\n",
                    skill.name, skill.content,
                );
                if let Some(uq) = user_query {
                    composed.push_str(&format!("Additional user instructions: {}", uq.query));
                }
                messages.push(ChatMessage::user(composed));
            }
            AIAgentInput::ResumeConversation { context } => {
                // BYOP doesn't have a server-side resume prompt injection layer. During LRC auto-resume,
                // the current PTY context must be explicitly re-attached, otherwise the error recovery
                // turn degrades to a normal conversation and re-selects shell tools.
                let mut prefixes: Vec<String> = Vec::new();
                if let Some(lrc_prefix) = render_lrc_request_context(params) {
                    prefixes.push(lrc_prefix);
                }
                let user_attachments = user_context::collect_user_attachments(context);
                if let Some(p) = &user_attachments.prefix {
                    prefixes.push(p.clone());
                }
                if !prefixes.is_empty() {
                    let full_text = format!("{}\n\nContinue.", prefixes.join("\n\n"));
                    messages.push(build_user_message_with_binaries(
                        full_text,
                        user_attachments.binaries,
                        api_type,
                        model_id,
                    ));
                }
            }
            AIAgentInput::SummarizeConversation {
                prompt,
                overflow: _,
            } => {
                // Waz BYOP local session compaction entry -- 1:1 aligned with opencode `compaction.ts processCompaction`.
                //
                // Earlier, the messages loop already trimmed the sequence to head based on `summarize_head_end`
                // (removing tail); here we append the final user message: `build_prompt(previous_summary, plugin_context)`,
                // which contains the SUMMARY_TEMPLATE (9-section Markdown template) + incremental summary anchor.
                //
                // The model will emit a structured Markdown summary text; after the controller receives
                // stream completion, it writes it back to conversation.compaction_state (see Phase 6 controller changes).
                let prev_summary = params
                    .compaction_state
                    .as_ref()
                    .and_then(|s| s.previous_summary())
                    .map(str::to_string);
                let mut anchor_context: Vec<String> = Vec::new();
                if let Some(custom) = prompt.as_ref().filter(|p| !p.is_empty()) {
                    // /compact <custom instructions> goes here — appends user instructions to the plugin_context segment
                    anchor_context
                        .push(format!("Additional instructions from the user:\n{custom}"));
                }
                let nextp =
                    byop_compaction::prompt::build_prompt(prev_summary.as_deref(), &anchor_context);
                messages.push(ChatMessage::user(nextp));
            }
            AIAgentInput::AutoCodeDiffQuery { .. }
            | AIAgentInput::CreateNewProject { .. }
            | AIAgentInput::CodeReview { .. } => {
                // Temporarily ignored
            }
            _ => {}
        }
    }

    if let ReadinessState::AcceptedHistoryRepair { repairs } = &readiness_report.state {
        repair_tool_call_pairs_for_accepted_history_gaps(
            &mut messages,
            repairs,
            &outbound_tool_groups,
        )?;
    }

    // Defensive sanitize: ensure messages don't end with assistant.
    // Anthropic / some gateways reject requests ending with assistant (prefill only for specific models),
    // and warp's `AIAgentInput::ResumeConversation` (handoff/auto-resume after error, etc.)
    // doesn't append a new user message, leaving the sequence ending on a historical assistant.
    // Unified fallback: if last is assistant, append an implicit user message so upstream can continue.
    ensure_ends_with_user(&mut messages);

    let mut tools_array = build_tools_array(params);

    // Anthropic path: set a 1h cache_control breakpoint on the **last tool** in the tools array,
    // making the entire tools segment a long-TTL static prefix (aligned with Zed
    // `crates/anthropic/src/completion.rs::254-258`)。
    //
    // Processing order `tools -> system -> messages`; long TTL must come before short TTL. In this path:
    // - tools last: 1h (here)
    // - system: 1h (`apply_caching_anthropic` on the ChatRole::System message)
    // - messages tail: 5m (`apply_caching_anthropic` on last 2 non-system)
    //
    // The tools segment changes least within a session (only when switching web_search / plan_mode / LRC),
    // hit rate is extremely high; 1h write at 2x base amortized over multiple reuses is effectively ~0 --
    // while also blocking the 1h-after-5m ordering error caused by external reverse proxies injecting
    // 5m on system (letting this single tools 1h breakpoint take over the entire tools+system static prefix).
    if matches!(api_type, AgentProviderApiType::Anthropic) {
        if let Some(last_tool) = tools_array.last_mut() {
            last_tool.cache_control = Some(CacheControl::Ephemeral1h);
        }
    }

    // Outbound message text is passed through to `serde_json` for JSON escape handling; no aggressive
    // character-level sanitize (referencing zed `into_anthropic` / opencode `provider/transform.ts`,
    // neither flattens control characters or replaces `\\` / `\"` at the outbound layer). Anthropic /
    // OpenAI / Gemini APIs and mainstream BYOP reverse proxies correctly handle serde_json's legal escapes.

    // Prompt caching (1:1 port from opencode `provider/transform.ts::applyCaching`):
    // - opencode selects first 2 system messages + last 2 non-system messages, uniformly marking
    //   anthropic.cacheControl / openaiCompatible.cache_control / bedrock.cachePoint
    //   and other multi-SDK compatible markers. Each AI SDK provider reads its corresponding key;
    // - We use rust-genai; the Anthropic adapter supports per-message `cache_control`,
    //   OpenAI / OpenAiResp adapter only recognizes `ChatOptions`-level prompt_cache_key /
    //   cache_control; DeepSeek / Gemini / Ollama server-side implicit caching, no client opt-in needed.
    // - Therefore we only apply per-message marking for the Anthropic path: push the system text as a
    //   ChatRole::System message to the head of messages with Ephemeral, then also mark the last two
    //   non-system messages as Ephemeral (corresponding to opencode's system+last 2 pattern).
    //   OpenAI's `prompt_cache_key` / `cache_control` is set in `build_chat_options`
    //   (request-level), also from the downstream fallback of the same set of opencode rules.
    let messages = if matches!(api_type, AgentProviderApiType::Anthropic) {
        let mut msgs: Vec<ChatMessage> = std::iter::once(ChatMessage::system(system_text.clone()))
            .chain(messages)
            .collect();
        apply_caching_anthropic(&mut msgs);
        msgs
    } else {
        messages
    };

    let mut req = ChatRequest::from_messages(messages);
    // In the Anthropic path, system is already included as a ChatRole::System message in messages,
    // so `with_system` is not set, to avoid the genai Anthropic adapter's limitation that the first
    // system message cannot carry cache_control (`adapter_impl.rs::into_anthropic_request_parts`).
    if !matches!(api_type, AgentProviderApiType::Anthropic) {
        req = req.with_system(system_text);
    }
    if !tools_array.is_empty() {
        req = req.with_tools(tools_array);
    }
    Ok(req)
}

const BYOP_DIAG_SNIPPET_CHARS: usize = 240;
const REPAIR_PLACEHOLDER_NOTE: &str =
    "tool result was unavailable in repaired conversation history";

fn is_placeholder_tool_response_content(content: &str) -> bool {
    if content == "(tool execution result not preserved)" {
        return true;
    }

    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(content) else {
        return false;
    };

    object.len() == 3
        && object.get("status").and_then(Value::as_str) == Some("unavailable")
        && matches!(
            object.get("reason").and_then(Value::as_str),
            Some("forked_history_repair" | "restored_legacy_history_repair")
        )
        && object.get("note").and_then(Value::as_str) == Some(REPAIR_PLACEHOLDER_NOTE)
}

fn insert_preferred_tool_response(
    responses_by_call_id: &mut HashMap<String, ToolResponse>,
    response: &ToolResponse,
) {
    let should_replace = match responses_by_call_id.get(&response.call_id) {
        None => true,
        Some(existing) => should_replace_tool_response(existing, response),
    };
    if should_replace {
        responses_by_call_id.insert(response.call_id.clone(), response.clone());
    }
}

fn should_replace_tool_response(existing: &ToolResponse, candidate: &ToolResponse) -> bool {
    is_placeholder_tool_response_content(&existing.content)
        || !is_placeholder_tool_response_content(&candidate.content)
}

fn snippet_for_log(s: &str, max_chars: usize) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:04x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn json_value_for_log(value: &Value) -> (usize, String) {
    let json = serde_json::to_string(value)
        .unwrap_or_else(|_| "<failed-to-serialize-json-value>".to_owned());
    (json.len(), snippet_for_log(&json, BYOP_DIAG_SNIPPET_CHARS))
}

fn binary_for_log(binary: &Binary) -> String {
    let name = binary
        .name
        .as_deref()
        .map(|n| snippet_for_log(n, 80))
        .unwrap_or_default();
    match &binary.source {
        BinarySource::Base64(data) => format!(
            "mime={} name={} source=base64 chars={}",
            binary.content_type,
            name,
            data.len()
        ),
        BinarySource::Url(url) => format!(
            "mime={} name={} source=url chars={} url={}",
            binary.content_type,
            name,
            url.len(),
            snippet_for_log(url, 120)
        ),
    }
}

fn log_chat_request_details(
    chat_req: &ChatRequest,
    model_id: &str,
    api_type: AgentProviderApiType,
) {
    let system_in_head = matches!(api_type, AgentProviderApiType::Anthropic)
        && chat_req
            .messages
            .first()
            .map(|m| matches!(m.role, ChatRole::System))
            .unwrap_or(false);
    let tool_count = chat_req.tools.as_ref().map(|t| t.len()).unwrap_or(0);
    let tool_names: Vec<String> = chat_req
        .tools
        .as_ref()
        .map(|tools| tools.iter().map(|t| t.name.as_str().to_owned()).collect())
        .unwrap_or_default();
    log::info!(
        "[byop-diag] request summary: adapter={:?} model={} system_len={} \
         system_in_messages_head={} messages={} tools={} tool_names={:?} \
         previous_response_id_present={} store={:?} system_snippet={:?}",
        adapter_kind_for(api_type),
        model_id,
        chat_req.system.as_deref().map(str::len).unwrap_or(0),
        system_in_head,
        chat_req.messages.len(),
        tool_count,
        tool_names,
        chat_req.previous_response_id.is_some(),
        chat_req.store,
        chat_req
            .system
            .as_deref()
            .map(|s| snippet_for_log(s, BYOP_DIAG_SNIPPET_CHARS))
            .unwrap_or_default(),
    );

    if let Some(tools) = &chat_req.tools {
        for (idx, tool) in tools.iter().enumerate() {
            let schema_len = tool
                .schema
                .as_ref()
                .and_then(|schema| serde_json::to_string(schema).ok())
                .map(|schema| schema.len())
                .unwrap_or(0);
            log::info!(
                "[byop-diag] request tool[{idx}]: name={} desc_len={} schema_len={} \
                 strict={:?} cache_control={:?}",
                tool.name.as_str(),
                tool.description.as_deref().map(str::len).unwrap_or(0),
                schema_len,
                tool.strict,
                tool.cache_control,
            );
        }
    }

    let flow: Vec<String> = chat_req
        .messages
        .iter()
        .enumerate()
        .map(|(idx, msg)| {
            let text_len: usize = msg.content.texts().iter().map(|t| t.len()).sum();
            let tool_call_ids: Vec<String> = msg
                .content
                .tool_calls()
                .iter()
                .map(|tc| tc.call_id.clone())
                .collect();
            let tool_response_ids: Vec<String> = msg
                .content
                .tool_responses()
                .iter()
                .map(|tr| tr.call_id.clone())
                .collect();
            format!(
                "{idx}:{:?}(text_len={text_len},tool_calls={tool_call_ids:?},tool_responses={tool_response_ids:?})",
                msg.role
            )
        })
        .collect();
    log::info!("[byop-diag] request message_flow={flow:?}");

    for (idx, msg) in chat_req.messages.iter().enumerate() {
        let mut text_count = 0;
        let mut text_total_len = 0;
        let mut first_text_snippet: Option<String> = None;
        let mut binary_summaries: Vec<String> = Vec::new();
        let mut tool_call_summaries: Vec<String> = Vec::new();
        let mut tool_response_summaries: Vec<String> = Vec::new();
        let mut thought_count = 0;
        let mut thought_total_len = 0;
        let mut reasoning_count = 0;
        let mut reasoning_total_len = 0;
        let mut custom_count = 0;

        for part in &msg.content {
            match part {
                ContentPart::Text(text) => {
                    text_count += 1;
                    text_total_len += text.len();
                    if first_text_snippet.is_none() {
                        first_text_snippet = Some(snippet_for_log(text, BYOP_DIAG_SNIPPET_CHARS));
                    }
                }
                ContentPart::Binary(binary) => {
                    binary_summaries.push(binary_for_log(binary));
                }
                ContentPart::ToolCall(tool_call) => {
                    let (args_len, args_snippet) = json_value_for_log(&tool_call.fn_arguments);
                    tool_call_summaries.push(format!(
                        "call_id={} name={} args_len={} args={} thought_signatures={}",
                        tool_call.call_id,
                        tool_call.fn_name,
                        args_len,
                        args_snippet,
                        tool_call
                            .thought_signatures
                            .as_ref()
                            .map(|s| s.len())
                            .unwrap_or(0)
                    ));
                }
                ContentPart::ToolResponse(tool_response) => {
                    tool_response_summaries.push(format!(
                        "call_id={} content_len={} placeholder={} content={}",
                        tool_response.call_id,
                        tool_response.content.len(),
                        is_placeholder_tool_response_content(&tool_response.content),
                        snippet_for_log(&tool_response.content, BYOP_DIAG_SNIPPET_CHARS)
                    ));
                }
                ContentPart::ThoughtSignature(thought) => {
                    thought_count += 1;
                    thought_total_len += thought.len();
                }
                ContentPart::ReasoningContent(reasoning) => {
                    reasoning_count += 1;
                    reasoning_total_len += reasoning.len();
                }
                ContentPart::Custom(_) => {
                    custom_count += 1;
                }
            }
        }

        let cache_control = msg
            .options
            .as_ref()
            .and_then(|options| options.cache_control.as_ref())
            .map(|cache| format!("{cache:?}"))
            .unwrap_or_else(|| "None".to_owned());
        log::info!(
            "[byop-diag] request message[{idx}]: role={:?} parts={} size={} \
             cache_control={} text_parts={} text_total_len={} first_text={:?} \
             binaries={:?} tool_calls={:?} tool_responses={:?} \
             thought_signatures={} thought_total_len={} reasoning_parts={} \
             reasoning_total_len={} custom_parts={}",
            msg.role,
            msg.content.len(),
            msg.content.size(),
            cache_control,
            text_count,
            text_total_len,
            first_text_snippet.unwrap_or_default(),
            binary_summaries,
            tool_call_summaries,
            tool_response_summaries,
            thought_count,
            thought_total_len,
            reasoning_count,
            reasoning_total_len,
            custom_count,
        );
    }

    for (idx, msg) in chat_req.messages.iter().enumerate() {
        let expected_call_ids: Vec<String> = msg
            .content
            .tool_calls()
            .iter()
            .map(|tc| tc.call_id.clone())
            .collect();
        if expected_call_ids.is_empty() {
            continue;
        }
        let next = chat_req.messages.get(idx + 1);
        let next_role = next.map(|m| format!("{:?}", m.role)).unwrap_or_default();
        let response_call_ids: Vec<String> = next
            .filter(|m| matches!(m.role, ChatRole::Tool))
            .map(|m| {
                m.content
                    .tool_responses()
                    .iter()
                    .map(|tr| tr.call_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let matched = response_call_ids == expected_call_ids;
        if matched {
            log::info!(
                "[byop-diag] request tool_pair idx={idx}: expected_call_ids={expected_call_ids:?} \
                 next_role={next_role} response_call_ids={response_call_ids:?}"
            );
        } else {
            log::warn!(
                "[byop-diag] request tool_pair mismatch idx={idx}: \
                 expected_call_ids={expected_call_ids:?} next_role={next_role} \
                 response_call_ids={response_call_ids:?}"
            );
        }
    }

    for (idx, msg) in chat_req.messages.iter().enumerate() {
        if !matches!(msg.role, ChatRole::Tool) {
            continue;
        }
        let response_call_ids: Vec<String> = msg
            .content
            .tool_responses()
            .iter()
            .map(|tr| tr.call_id.clone())
            .collect();
        let previous_expected: Vec<String> = idx
            .checked_sub(1)
            .and_then(|prev_idx| chat_req.messages.get(prev_idx))
            .filter(|prev| matches!(prev.role, ChatRole::Assistant))
            .map(|prev| {
                prev.content
                    .tool_calls()
                    .iter()
                    .map(|tc| tc.call_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if response_call_ids != previous_expected {
            log::warn!(
                "[byop-diag] request orphan_or_misordered_tool_response idx={idx}: \
                 response_call_ids={response_call_ids:?} previous_assistant_call_ids={previous_expected:?}"
            );
        }
    }
}

/// 1:1 port from opencode `provider/transform.ts::applyCaching` Anthropic branch:
/// marks the first 2 system messages + last 2 non-system messages with cache markers.
///
/// The genai Anthropic adapter in `into_anthropic_request_parts` applies
/// `MessageOptions::cache_control` to the last content part of that message,
/// behavior consistent with opencode setting lastContent.providerOptions.anthropic.cacheControl.
///
/// **TTL selection (revised from P0-4, aligned with Zed `crates/anthropic/src/completion.rs:219-274`)**:
/// Static prefix system uses 1h; session tail last 2 non-system uses 5m; meanwhile `build_chat_request`
/// marks the last tool in the Anthropic path with 1h (genai Tool struct now has a cache_control field).
///
/// **Mixed strategy motivation**:
/// - The old "all 1h" strategy cannot coexist with external 5m breakpoints. When BYOP
///   reverse proxies / upstream gateways inject default 5m at earlier positions (tools/system),
///   Anthropic's ordering constraint rejects the subsequent 1h:
///   `a ttl='1h' cache_control block must not come after a ttl='5m'`, triggering 400.
/// - New strategy: all long TTLs go first (tools / system 1h), short TTLs go at the end of
///   the sequence (messages 5m), strictly complying with Anthropic's processing order
///   `tools → system → messages`. If external proxies add extra 5m on tools/system, it just
///   becomes "two 5m upfront + tail 5m", no longer triggering 1h-after-5m.
///
/// **Cost impact**:
/// - tools/system is the static prefix within a session; 1h write cost at 2× base amortized
///   over multiple hits within the session is effectively ~0 (clearly lower total cost vs.
///   "all 1h" repeatedly rewriting the tail).
/// - messages tail changes every turn; 5m write cost at 1.25× base, next turn within 5min
///   hits for free renewal, no repeated rewrites, and no need to commit 1h's high one-time
///   write cost.
///
/// **TTL ordering constraint**: Anthropic API requires long TTL breakpoints to come before
/// short TTL (`https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
///  #mixing-different-ttls`). This function marks system as 1h and non-system tail as 5m;
///  the natural order system(1h) → messages(5m) is compliant. `build_chat_request` marks
///  tools tail with 1h which comes before system, so the overall order
///  tools(1h) → system(1h) → messages(5m) is still compliant.
/// genai checks this order in `into_anthropic_request_parts` and warns on violation (see
/// `lib/rust-genai/src/adapter/adapters/anthropic/adapter_impl.rs`).
fn apply_caching_anthropic(messages: &mut Vec<ChatMessage>) {
    let n = messages.len();
    if n == 0 {
        return;
    }
    let mut tag = vec![false; n];

    // first 2 system messages
    let mut sys_seen = 0;
    for (i, m) in messages.iter().enumerate() {
        if matches!(m.role, ChatRole::System) {
            tag[i] = true;
            sys_seen += 1;
            if sys_seen >= 2 {
                break;
            }
        }
    }
    // last 2 non-system messages
    let mut tail_seen = 0;
    for (i, m) in messages.iter().enumerate().rev() {
        if !matches!(m.role, ChatRole::System) {
            tag[i] = true;
            tail_seen += 1;
            if tail_seen >= 2 {
                break;
            }
        }
    }

    let original = std::mem::take(messages);
    *messages = original
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            if tag[i] {
                // ChatMessage doesn't have a direct with_cache_control; cache_control is on
                // `MessageOptions`, injected via `with_options`.
                // `MessageOptions: From<CacheControl>` is provided by genai
                // (`chat_message.rs::impl From<CacheControl> for MessageOptions`)。
                // Mixed TTL: system uses 1h (static prefix, multi-hit within session amortizes writes),
                // non-system uses 5m (session tail changes every turn, next turn within 5min hits for free renewal).
                // Order system(1h) → messages(5m) satisfies Anthropic ordering constraint.
                let ttl = if matches!(m.role, ChatRole::System) {
                    CacheControl::Ephemeral1h
                } else {
                    CacheControl::Ephemeral5m
                };
                m.with_options(ttl)
            } else {
                m
            }
        })
        .collect();
}

/// Only runs after the serializer has determined `AcceptedHistoryRepair`: converts
/// history gaps explicitly authorized by RepairRecord into outbound-only structured ToolResponses.
///
/// Normal missing, duplicate, orphan, or cross-boundary misordered cases have already been blocked
/// at the readiness validation stage; no placeholder results for normal flow, no write-back to history.
fn repair_tool_call_pairs_for_accepted_history_gaps(
    messages: &mut Vec<ChatMessage>,
    repairs: &[AcceptedRepair],
    outbound_tool_groups: &[OutboundAssistantToolGroup],
) -> Result<(), ConvertToAPITypeError> {
    use std::collections::{HashMap, HashSet};

    if repairs.is_empty() {
        return Ok(());
    }

    let repair_by_key: HashMap<ToolCallKey, &AcceptedRepair> = repairs
        .iter()
        .map(|repair| (repair.tool_call.key.clone(), repair))
        .collect();
    let group_by_message_index: HashMap<usize, &OutboundAssistantToolGroup> = outbound_tool_groups
        .iter()
        .map(|group| (group.message_index, group))
        .collect();
    let mut call_id_counts: HashMap<String, usize> = HashMap::new();
    for group in outbound_tool_groups {
        for key in &group.tool_call_keys {
            *call_id_counts.entry(key.tool_call_id.clone()).or_default() += 1;
        }
    }
    let mut placeholders_inserted: Vec<String> = Vec::new();
    let mut orphan_call_ids: Vec<String> = Vec::new();
    let mut missing_without_repair: Vec<String> = Vec::new();

    let original = std::mem::take(messages);
    let mut late_responses_by_unique_call_id: HashMap<String, ToolResponse> = HashMap::new();
    let mut late_response_call_ids: HashSet<String> = HashSet::new();
    for (idx, msg) in original.iter().enumerate() {
        if msg.role != genai::chat::ChatRole::Tool {
            continue;
        }

        let is_adjacent_to_group =
            idx > 0 && group_by_message_index.contains_key(&(idx.saturating_sub(1)));
        if is_adjacent_to_group {
            continue;
        }

        for resp in msg.content.tool_responses() {
            if call_id_counts.get(&resp.call_id) == Some(&1) {
                insert_preferred_tool_response(&mut late_responses_by_unique_call_id, resp);
                late_response_call_ids.insert(resp.call_id.clone());
            }
        }
    }

    let mut rewritten: Vec<ChatMessage> = Vec::with_capacity(original.len());
    let mut idx = 0;
    while idx < original.len() {
        let msg = original[idx].clone();
        if msg.role == genai::chat::ChatRole::Tool {
            orphan_call_ids.extend(
                msg.content
                    .tool_responses()
                    .iter()
                    .filter(|response| !late_response_call_ids.contains(&response.call_id))
                    .map(|response| response.call_id.clone()),
            );
            idx += 1;
            continue;
        }

        let Some(group) = group_by_message_index.get(&idx).copied() else {
            rewritten.push(msg);
            idx += 1;
            continue;
        };

        rewritten.push(msg);
        idx += 1;

        let mut responses_by_call_id: HashMap<String, ToolResponse> = HashMap::new();
        while idx < original.len() && original[idx].role == genai::chat::ChatRole::Tool {
            for resp in original[idx].content.tool_responses() {
                insert_preferred_tool_response(&mut responses_by_call_id, resp);
            }
            idx += 1;
        }

        let mut bundled: Vec<ToolResponse> = Vec::new();
        for key in &group.tool_call_keys {
            let cid = &key.tool_call_id;
            let mut response = responses_by_call_id.remove(cid);
            if call_id_counts.get(cid) == Some(&1) {
                if let Some(late_response) = late_responses_by_unique_call_id.remove(cid) {
                    response = match response {
                        Some(existing)
                            if !should_replace_tool_response(&existing, &late_response) =>
                        {
                            Some(existing)
                        }
                        _ => Some(late_response),
                    };
                }
            }

            match response {
                Some(resp) => bundled.push(resp),
                None => {
                    if let Some(repair) = repair_by_key.get(key) {
                        placeholders_inserted.push(cid.clone());
                        bundled.push(ToolResponse::new(
                            cid.clone(),
                            repair_placeholder_content(repair.record.source),
                        ));
                    } else {
                        missing_without_repair.push(cid.clone());
                    }
                }
            }
        }

        if !bundled.is_empty() {
            rewritten.push(ChatMessage::from(bundled));
        }

        if !responses_by_call_id.is_empty() {
            orphan_call_ids.extend(responses_by_call_id.into_keys());
        }
    }

    *messages = rewritten;

    if !orphan_call_ids.is_empty() {
        log::warn!(
            "[byop-diag] accepted_history_repair: dropped {} orphan ToolResponse(s): \
             orphan_call_ids={:?}",
            orphan_call_ids.len(),
            orphan_call_ids
        );
    }
    if !placeholders_inserted.is_empty() {
        log::info!(
            "[byop-diag] accepted_history_repair: inserted repair placeholder \
             ToolResponse for {} ToolCall(s): missing_call_ids={:?}",
            placeholders_inserted.len(),
            placeholders_inserted
        );
    }
    if !missing_without_repair.is_empty() {
        // When the readiness classifier has determined AcceptedHistoryRepair, every missing tool call should
        // have a corresponding authorization in repairs; if still missing here, it indicates the classifier
        // and serializer have inconsistent tool call key sources (e.g., future refactoring of projection or
        // outbound_tool_groups introduced divergence). Cannot send an illegal request with missing ToolResponses; must block.
        log::error!(
            "[byop-diag] accepted_history_repair: unauthorized missing ToolResponse from readiness: \
             missing_call_ids={:?}",
            missing_without_repair
        );
        return Err(ConvertToAPITypeError::Other(
            BlockedByopReadinessError::new(ReadinessCategory::MissingResultWithoutRepairSource)
                .into(),
        ));
    }
    Ok(())
}

fn repair_placeholder_content(source: RepairSource) -> String {
    json!({
        "status": "unavailable",
        "reason": source.placeholder_reason(),
        "note": REPAIR_PLACEHOLDER_NOTE,
    })
    .to_string()
}

/// Fallback: ensures messages end with user (or tool response).
///
/// Trigger scenario: `AIAgentInput::ResumeConversation` doesn't append a new user message,
/// directly resends history. Anthropic's native API rejects requests ending with assistant
/// assistant message prefill. The conversation must end with a user message.`),
/// retrying 3 times with the same payload → UI renders error block triggering flex panic.
///
/// When the last message is assistant, appends `ChatMessage::user("Continue.")` to prompt
/// the model to continue. Tool role (as a form of user input — model treats tool responses
/// as the next turn's starting point) is left untouched. Empty messages don't trigger.
fn ensure_ends_with_user(messages: &mut Vec<ChatMessage>) {
    use genai::chat::ChatRole;
    if let Some(last) = messages.last() {
        if last.role == ChatRole::Assistant {
            messages.push(ChatMessage::user("Continue."));
        }
    }
}

/// Reverse: serializes internal `tool_call::Tool` variant into (function name, arguments JSON Value)
/// for multi-turn history replay. The (name, args) here must strictly align with each tool's `name`
/// and `from_args` expected schema in `tools::REGISTRY`.
fn serialize_outgoing_tool_call(
    tc: &api::message::ToolCall,
    mcp_ctx: Option<&crate::ai::agent::MCPContext>,
    server_message_data: &str,
) -> (String, Value) {
    use api::message::tool_call::Tool;

    // BYOP from_args parse failure carrier restoration: written by make_tool_call_carrier_message,
    // tool oneof = None, original `<fn_name>\n<args_str>` encoded in server_message_data.
    // Must be identified before the main match, otherwise falls through to None=>"warp_internal_empty";
    // the upstream model seeing a nonexistent tool name would be even more confused and wouldn't know which call failed.
    if tc.tool.is_none() {
        if let Some((fn_name, raw_args)) = server_message_data.split_once('\n') {
            if !fn_name.is_empty() {
                let args_value = serde_json::from_str(raw_args)
                    .unwrap_or_else(|_| Value::String(raw_args.to_owned()));
                return (fn_name.to_owned(), args_value);
            }
        }
    }

    // Most legacy implementations return (String, String); here we change to (String, Value), parsing the string again.
    let (name, args_str) = match &tc.tool {
        Some(Tool::CallMcpTool(c)) => tools::mcp::serialize_outgoing_call(c, mcp_ctx),
        Some(Tool::ReadMcpResource(r)) => tools::mcp::serialize_outgoing_read_resource(r, mcp_ctx),
        Some(Tool::RunShellCommand(c)) => (
            "run_shell_command".to_owned(),
            json!({
                "command": c.command,
                "is_read_only": c.is_read_only,
                "uses_pager": c.uses_pager,
                "is_risky": c.is_risky,
            })
            .to_string(),
        ),
        Some(Tool::ReadFiles(r)) => {
            let files: Vec<Value> = r
                .files
                .iter()
                .map(|f| {
                    json!({
                        "path": f.name,
                        "line_ranges": f.line_ranges.iter().map(|lr| json!({
                            "start": lr.start, "end": lr.end
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            (
                "read_files".to_owned(),
                json!({ "files": files }).to_string(),
            )
        }
        Some(Tool::Grep(g)) => (
            "grep".to_owned(),
            json!({ "queries": g.queries, "path": g.path }).to_string(),
        ),
        Some(Tool::AskUserQuestion(a)) => {
            let questions: Vec<Value> = a
                .questions
                .iter()
                .map(|q| {
                    let (options, recommended_index, multi_select, supports_other) =
                        match &q.question_type {
                            Some(
                                api::ask_user_question::question::QuestionType::MultipleChoice(mc),
                            ) => (
                                mc.options
                                    .iter()
                                    .map(|o| o.label.clone())
                                    .collect::<Vec<_>>(),
                                mc.recommended_option_index,
                                mc.is_multiselect,
                                mc.supports_other,
                            ),
                            None => (vec![], 0, false, false),
                        };
                    json!({
                        "question": q.question,
                        "options": options,
                        "recommended_index": recommended_index,
                        "multi_select": multi_select,
                        "supports_other": supports_other,
                    })
                })
                .collect();
            (
                "ask_user_question".to_owned(),
                json!({ "questions": questions }).to_string(),
            )
        }
        Some(Tool::FileGlobV2(g)) => (
            "file_glob".to_owned(),
            json!({
                "patterns": g.patterns,
                "search_dir": g.search_dir,
                "limit": g.max_matches,
            })
            .to_string(),
        ),
        Some(Tool::ApplyFileDiffs(a)) => {
            let mut operations: Vec<Value> = Vec::new();
            for d in &a.diffs {
                operations.push(json!({
                    "op": "edit",
                    "file_path": d.file_path,
                    "search": d.search,
                    "replace": d.replace,
                }));
            }
            for f in &a.new_files {
                operations.push(json!({
                    "op": "create",
                    "file_path": f.file_path,
                    "content": f.content,
                }));
            }
            for f in &a.deleted_files {
                operations.push(json!({
                    "op": "delete",
                    "file_path": f.file_path,
                }));
            }
            (
                "apply_file_diffs".to_owned(),
                json!({ "summary": a.summary, "operations": operations }).to_string(),
            )
        }
        Some(Tool::WriteToLongRunningShellCommand(w)) => {
            use api::message::tool_call::write_to_long_running_shell_command::mode::Mode as M;
            let mode = match w.mode.as_ref().and_then(|m| m.mode.as_ref()) {
                Some(M::Raw(_)) => "raw",
                Some(M::Block(_)) => "block",
                _ => "line",
            };
            (
                "write_to_long_running_shell_command".to_owned(),
                json!({
                    "command_id": w.command_id,
                    "input": String::from_utf8_lossy(&w.input).to_string(),
                    "mode": mode,
                })
                .to_string(),
            )
        }
        Some(Tool::ReadDocuments(r)) => {
            let docs: Vec<Value> = r
                .documents
                .iter()
                .map(|d| {
                    json!({
                        "document_id": d.document_id,
                        "line_ranges": d.line_ranges.iter().map(|lr| json!({
                            "start": lr.start, "end": lr.end
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            (
                "read_documents".to_owned(),
                json!({ "documents": docs }).to_string(),
            )
        }
        Some(Tool::EditDocuments(e)) => {
            let diffs: Vec<Value> = e
                .diffs
                .iter()
                .map(|d| {
                    json!({
                        "document_id": d.document_id,
                        "search": d.search,
                        "replace": d.replace,
                    })
                })
                .collect();
            (
                "edit_documents".to_owned(),
                json!({ "diffs": diffs }).to_string(),
            )
        }
        Some(Tool::CreateDocuments(c)) => {
            let new_documents: Vec<Value> = c
                .new_documents
                .iter()
                .map(|d| json!({ "title": d.title, "content": d.content }))
                .collect();
            (
                "create_documents".to_owned(),
                json!({ "new_documents": new_documents }).to_string(),
            )
        }
        Some(Tool::SuggestNewConversation(s)) => (
            "suggest_new_conversation".to_owned(),
            json!({ "message_id": s.message_id }).to_string(),
        ),
        Some(Tool::SuggestPrompt(s)) => {
            use api::message::tool_call::suggest_prompt::DisplayMode;
            let (prompt, label) = match &s.display_mode {
                Some(DisplayMode::PromptChip(c)) => (c.prompt.clone(), c.label.clone()),
                Some(DisplayMode::InlineQueryBanner(b)) => (b.query.clone(), b.title.clone()),
                None => (String::new(), String::new()),
            };
            (
                "suggest_prompt".to_owned(),
                json!({ "prompt": prompt, "label": label }).to_string(),
            )
        }
        Some(Tool::OpenCodeReview(_)) => ("open_code_review".to_owned(), "{}".to_owned()),
        Some(Tool::TransferShellCommandControlToUser(t)) => (
            "transfer_shell_command_control_to_user".to_owned(),
            json!({ "reason": t.reason }).to_string(),
        ),
        Some(Tool::ReadSkill(r)) => {
            use api::message::tool_call::read_skill::SkillReference;
            // The model passes `name` in the previous turn, which `from_args` stores in the `SkillPath` slot;
            // when serializing back to the upstream conversation history, we convert it back to the `name` field
            // for consistency with the current JSON schema.
            // BundledSkillId uses the Display form `@warp-skill:<id>`, consistent with SkillReference Display.
            let name = match &r.skill_reference {
                Some(SkillReference::SkillPath(s)) => s.clone(),
                Some(SkillReference::BundledSkillId(id)) => format!("@warp-skill:{id}"),
                None => String::new(),
            };
            (
                "read_skill".to_owned(),
                json!({ "name": name }).to_string(),
            )
        }
        Some(Tool::ReadShellCommandOutput(r)) => {
            use api::message::tool_call::read_shell_command_output::Delay;
            let delay_seconds = match &r.delay {
                Some(Delay::Duration(d)) => Some(d.seconds),
                Some(Delay::OnCompletion(_)) | None => None,
            };
            let mut args = json!({ "command_id": r.command_id });
            if let Some(s) = delay_seconds {
                args["delay_seconds"] = json!(s);
            }
            ("read_shell_command_output".to_owned(), args.to_string())
        }
        Some(other) => {
            let variant_name = format!("{other:?}")
                .split('(')
                .next()
                .unwrap_or("UnknownVariant")
                .to_owned();
            (format!("warp_internal_{}", variant_name), "{}".to_owned())
        }
        None => ("warp_internal_empty".to_owned(), "{}".to_owned()),
    };
    let args_value: Value =
        serde_json::from_str(&args_str).unwrap_or(Value::Object(Default::default()));
    (name, args_value)
}

// ---------------------------------------------------------------------------
// Tools array
// ---------------------------------------------------------------------------

/// Whether the current turn's input contains `UserQueryMode::Plan` triggered by `/plan`.
///
/// Per-turn semantics: only checks whether the current `params.input` carries the Plan marker.
/// The current persistence path for historical task messages (`make_user_query_message`)
/// writes to the upstream proto with `..Default::default()`, **without the mode field**;
/// therefore plan state does not automatically persist across turns — users must re-add
/// the `/plan ` prefix for every query they want to keep read-only. This is an intentional
/// MVP design:
/// - Lowest implementation cost (no proto schema changes, no new session-level state machine)
/// - Consistent with Claude Code's `EnterPlanMode` "explicit enter/exit" semantics — except
///   here the exit action is implicit in "the next message without /plan"
fn is_plan_mode_turn(input: &[AIAgentInput]) -> bool {
    input.iter().any(|i| {
        matches!(
            i,
            AIAgentInput::UserQuery {
                user_query_mode: UserQueryMode::Plan,
                ..
            }
        )
    })
}

/// Built-in write/execute tool names that are hard-filtered in Plan Mode.
///
/// Logic safety net: even if the model ignores `partials/plan_mode.j2` guidance,
/// side effects cannot be triggered — tools not in the tool list simply cannot be called (the provider protocol layer directly rejects unknown functions).
///
/// **Write tools NOT blocked**: `create_documents` / `edit_documents`. They only touch
/// Waz Drive local document storage (AIDocumentModel); no filesystem access, no command
/// execution. Semantically, this is exactly Plan Mode's output archival action — the model
/// persists the final plan as a Drive doc; users can view/edit/reuse it in Drive UI.
///
/// The remaining read-only + Drive write subset: `read_files / grep / file_glob_v2 /
/// read_shell_command_output / ask_user_question / read_skill / read_documents /
/// create_documents / edit_documents / webfetch / websearch / mcp/*`。
const PLAN_MODE_BLOCKED_TOOLS: &[&str] = &[
    "run_shell_command",
    "apply_file_diffs",
    "write_to_long_running_shell_command",
    "open_code_review",
    "transfer_shell_command_control_to_user",
    "suggest_prompt",
];

/// Lists the tool names actually fed to the upstream model for the current turn
/// (built-in REGISTRY + current MCP tools), sharing the same gating logic as
/// `build_tools_array` (LRC / `web_search_enabled` / `suggest_new_conversation` /
/// `plan_mode`). Injected into the system prompt by `prompt_renderer` so templates
/// can dynamically render based on the actual available list instead of hardcoded
/// allow/block lists.
pub fn available_tool_names(params: &RequestParams) -> Vec<String> {
    let is_lrc = params.lrc_command_id.is_some();
    let web_enabled = params.web_search_enabled;
    let plan_mode = is_plan_mode_turn(&params.input);
    let mut names: Vec<String> = tools::REGISTRY
        .iter()
        .filter(|t| {
            if is_lrc && t.name == "run_shell_command" {
                return false;
            }
            if !web_enabled
                && (t.name == tools::webfetch::TOOL_NAME || t.name == tools::websearch::TOOL_NAME)
            {
                return false;
            }
            if t.name == "suggest_new_conversation" {
                return false;
            }
            if plan_mode && PLAN_MODE_BLOCKED_TOOLS.contains(&t.name) {
                return false;
            }
            true
        })
        .map(|t| t.name.to_owned())
        .collect();
    if let Some(ctx) = params.mcp_context.as_ref() {
        for (name, _description, _parameters) in tools::mcp::build_mcp_tool_defs(ctx) {
            names.push(name);
        }
    }
    let cwd = params.session_context.current_working_directory().as_deref().unwrap_or("");
    for (name, _description, _parameters) in tools::tmp_ai::build_tmp_tool_defs(cwd) {
        names.push(name);
    }
    names
}

fn build_tools_array(params: &RequestParams) -> Vec<GenaiTool> {
    // Waz A2: LRC tag-in scenario removes `run_shell_command`, forcing the model to use PTY operation tools.
    //
    // In alt-screen long-running commands (nvim/htop) + user tag-in state, **the most common model mistake** is
    // calling `run_shell_command` to run `taskkill nvim` / `Stop-Process nvim` (spawning a new process),
    // which has nothing to do with the currently running PTY and won't kill the target. **The correct approach** is
    // `write_to_long_running_shell_command(command_id, input=":q\n", mode=raw)`,
    // sending instructions directly to the current PTY.
    //
    // Real-world testing showed that system prompt guidance + RunningCommand context prefix alone are insufficient;
    // the model still prefers run_shell_command (it's simpler). The cleanest hard constraint is removing
    // the tool from the tools list entirely, so the model can only choose from PTY operation tools.
    //
    // Other tools are preserved (read_files/grep/ask_user_question etc.), allowing the model to do
    // necessary information gathering and follow-up questions.
    let is_lrc = params.lrc_command_id.is_some();
    let web_enabled = params.web_search_enabled;
    let plan_mode = is_plan_mode_turn(&params.input);
    // Waz BYOP: `suggest_prompt` chip UI has been restored via the view layer subscribing to
    // PromptSuggestionExecutorEvent (see `terminal/view.rs::
    // handle_suggest_prompt_executor_event`), so it can be exposed to the model.
    // `suggest_new_conversation` remains filtered: there's no existing popup component in UX,
    // the executor has been changed to fast-fail Cancelled (see
    // `action_model/execute/suggest_new_conversation.rs`); the filter is a redundant defense
    // to avoid unnecessary call noise.
    // Dynamic placeholder replacement: some tool descriptions contain `{{year}}` (e.g. websearch,
    // aligned with opencode websearch.ts:30-32 description getter), replaced with current year
    // at build time. The model sees the correct year in every description, avoiding pollution
    // from outdated years in training data.
    let current_year = chrono::Local::now().format("%Y").to_string();
    let mut out: Vec<GenaiTool> = tools::REGISTRY
        .iter()
        .filter(|t| {
            if is_lrc && t.name == "run_shell_command" {
                return false;
            }
            // BYOP web tools are gated by profile.web_search_enabled (not exposed to
            // the upstream model when the user has disabled the privacy toggle, to prevent
            // accidental external network requests).
            if !web_enabled
                && (t.name == tools::webfetch::TOOL_NAME || t.name == tools::websearch::TOOL_NAME)
            {
                return false;
            }
            // suggest_new_conversation: no UI implementation, executor changed to
            // fast-fail Cancelled in Waz. Filtered here to avoid the model calling it and
            // generating pointless tool_call→cancelled round-trips (pure token waste).
            if t.name == "suggest_new_conversation" {
                return false;
            }
            // Plan Mode: hard guardrail for `/plan`-triggered read-only mode, removing
            // write/execute tools. Double insurance with the system prompt's plan_mode.j2
            // guidance — even if the model ignores the prompt, tools not in the list cannot
            // trigger side effects (the provider protocol layer directly rejects unknown functions).
            if plan_mode && PLAN_MODE_BLOCKED_TOOLS.contains(&t.name) {
                return false;
            }
            true
        })
        .map(|t| {
            let description = if t.description.contains("{{year}}") {
                t.description.replace("{{year}}", &current_year)
            } else {
                t.description.to_owned()
            };
            GenaiTool::new(t.name)
                .with_description(description)
                .with_schema((t.parameters)())
        })
        .collect();

    if let Some(ctx) = params.mcp_context.as_ref() {
        for (name, description, parameters) in tools::mcp::build_mcp_tool_defs(ctx) {
            out.push(
                GenaiTool::new(name)
                    .with_description(description)
                    .with_schema(parameters),
            );
        }
    }
    let cwd = params.session_context.current_working_directory().as_deref().unwrap_or("");
    for (name, description, parameters) in tools::tmp_ai::build_tmp_tool_defs(cwd) {
        out.push(
            GenaiTool::new(name)
                .with_description(description)
                .with_schema(parameters),
        );
    }
    if is_lrc {
        log::info!(
            "[byop] LRC tag-in: tools array filtered (removed run_shell_command), \
             total tools={}",
            out.len()
        );
    }
    if plan_mode {
        log::info!(
            "[byop] Plan Mode: tools array filtered (removed write/exec tools: {:?}), \
             total tools={}",
            PLAN_MODE_BLOCKED_TOOLS,
            out.len()
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Client / routing
// ---------------------------------------------------------------------------

/// Maps `AgentProviderApiType` one-to-one to genai `AdapterKind`.
fn adapter_kind_for(api_type: AgentProviderApiType) -> AdapterKind {
    match api_type {
        AgentProviderApiType::OpenAi => AdapterKind::OpenAI,
        AgentProviderApiType::OpenAiResp => AdapterKind::OpenAIResp,
        AgentProviderApiType::Gemini => AdapterKind::Gemini,
        AgentProviderApiType::Anthropic => AdapterKind::Anthropic,
        AgentProviderApiType::Ollama => AdapterKind::Ollama,
        AgentProviderApiType::DeepSeek => AdapterKind::DeepSeek,
    }
}

/// Normalizes the user-provided `base_url` into an endpoint URL for genai adapters to
/// append their service paths.
///
/// All genai 0.6.x adapters assume the endpoint ends with `/` and already includes the
/// version path segment:
/// - Anthropic: `format!("{base_url}messages")` expects `…/v1/`
/// - Gemini: `format!("{base_url}models/{m}:streamGenerateContent")` expects `…/v1beta/`
/// - OpenAI / OpenAIResp / DeepSeek: `Url::join("chat/completions" or "responses")` expects `…/v1/`
/// - Ollama: `format!("{base_url}api/chat")` expects root path `…/`
///
/// Three typical user inputs:
/// 1. Host only (`https://ai.zerx.dev`) — the old default behavior of only appending a trailing `/`
///    would produce `https://ai.zerx.dev/messages`, missing `/v1/` and resulting in a 404.
///    **Here we auto-append the default version path segment based on api_type**
///    (Anthropic/OpenAI family→`/v1/`, Gemini→`/v1beta/`, Ollama: no suffix).
/// 2. Full path with version (`https://ai.zerx.dev/v1`) — only append trailing `/`, leave path intact.
/// 3. Empty — use [`AgentProviderApiType::default_base_url`].
fn normalize_endpoint_url(api_type: AgentProviderApiType, base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return api_type.default_base_url().to_owned();
    }

    // Parse failure (user entered a malformed URL) → fall back to the old "append trailing /"
    // behavior, letting the upstream report the error instead of panicking here.
    let parsed = match url::Url::parse(trimmed) {
        Ok(u) => u,
        Err(_) => {
            let stripped = trimmed.trim_end_matches('/');
            return format!("{stripped}/");
        }
    };

    // path == "/" or empty → user only provided the host; auto-append the api_type default version path segment.
    if parsed.path() == "/" || parsed.path().is_empty() {
        // Extract the path portion from default_base_url (e.g. "/v1/" / "/v1beta/" / "/").
        let default_path = url::Url::parse(api_type.default_base_url())
            .ok()
            .map(|u| u.path().to_owned())
            .unwrap_or_else(|| "/".to_owned());
        let host_part = trimmed.trim_end_matches('/');
        return format!("{host_part}{default_path}");
    }

    // User already provided a path → only ensure trailing `/` (genai format!/Url::join depend on it).
    let stripped = trimmed.trim_end_matches('/');
    format!("{stripped}/")
}

/// Constructs a genai Client. Created fresh per request (low overhead — Client internally
/// is just a reqwest::Client + adapter table). After `ServiceTargetResolver` captures the
/// current request's endpoint/key/api_type, each `exec_chat_stream` call is forced to route
/// to the specified AdapterKind, completely bypassing genai's default "identify by model name" logic.
pub(super) fn build_client(
    api_type: AgentProviderApiType,
    base_url: String,
    api_key: String,
) -> Client {
    let adapter_kind = adapter_kind_for(api_type);
    let endpoint_url = normalize_endpoint_url(api_type, &base_url);
    log::info!("[byop] build_client: adapter={adapter_kind:?} endpoint_url={endpoint_url}");
    let key_for_resolver = api_key.clone();
    let resolver = ServiceTargetResolver::from_resolver_fn(
        move |service_target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget { model, .. } = service_target;
            let endpoint = Endpoint::from_owned(endpoint_url.clone());
            let auth = AuthData::from_single(key_for_resolver.clone());
            // Override genai's "identify by model name" result with our specified AdapterKind,
            // but preserve model_name so the upstream service can correctly locate the model.
            let model = ModelIden::new(adapter_kind, model.model_name);
            Ok(ServiceTarget {
                endpoint,
                auth,
                model,
            })
        },
    );

    // Waz BYOP: SSE streams must not use gzip. `Accept-Encoding: gzip` causes nginx-like
    // proxies to compress the response; the server must flush a complete deflate frame before
    // the client can decode plaintext, breaking streaming semantics into ~K-byte bursts that
    // feel like "stuttering every few hundred milliseconds". zed/opencode use native fetch /
    // std HTTP without actively negotiating gzip on SSE, so the same proxy works fine for them.
    //
    // We explicitly construct `WebConfig` here even though genai default already has `gzip=false`
    // (fork modification).
    //
    // User-Agent is dynamically bound to the current app name (from
    // `ChannelState::app_id().application_name()`, registered by the entry bin:
    // `bin/oss.rs` → "Waz"; other channels carry their own names).
    // This allows upstream services to identify which branch build the request comes from,
    // and name changes will be followed automatically.
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = build_user_agent_header() {
        headers.insert(reqwest::header::USER_AGENT, value);
    }
    let web_config = WebConfig {
        gzip: false,
        default_headers: Some(headers),
        ..WebConfig::default()
    };
    Client::builder()
        .with_web_config(web_config)
        .with_service_target_resolver(resolver)
        .build()
}

/// Constructs the `User-Agent` header for BYOP outbound requests. Value format:
/// - `Waz/<git-tag>` — release builds with `GIT_RELEASE_TAG` injected
/// - `Waz` — Dev / local builds without version info
///
/// The app name is always taken from `ChannelState::app_id().application_name()`,
/// ensuring consistency with the `AppId` registered by the entry bin
/// (`bin/oss.rs` registers "Waz").
fn build_user_agent_header(
) -> Result<reqwest::header::HeaderValue, reqwest::header::InvalidHeaderValue> {
    let app_name = warp_core::channel::ChannelState::app_id()
        .application_name()
        .to_owned();
    let ua = match warp_core::channel::ChannelState::app_version() {
        Some(v) if !v.is_empty() => format!("{app_name}/{v}"),
        _ => app_name,
    };
    reqwest::header::HeaderValue::from_str(&ua)
}

/// Determines whether to inject `enable_thinking: true` for DashScope (Alibaba Cloud Bailian,
/// OpenAI-compatible path).
///
/// Aligned with opencode `transform.ts:931-938` (comments at provider/transform.ts L926+):
/// "DashScope does not enable thinking by default; reasoning models like qwen3 / qwq /
/// deepseek-r1 / kimi-k2.5 / qwen-plus require explicit `enable_thinking: true` to output
/// reasoning_content."
///
/// Match conditions (all must be satisfied):
/// 1. `api_type == OpenAi` (DashScope uses the OpenAI-compatible path)
/// 2. `effort_setting != Off` (respect the user's explicit disable; don't inject)
/// 3. base_url contains `dashscope.aliyuncs.com` / `dashscope.cn` / `dashscope-intl.aliyuncs.com`
/// 4. model_id does not contain `kimi-k2-thinking` (excluded by opencode; this model has thinking enabled by default)
/// 5. model_id matches the reasoning substring allowlist: `qwen3` / `qwq` / `deepseek-r1` / `kimi-k2.5` /
///    `kimi-k2-` / `qwen-plus` (to avoid injecting into pure chat models like qwen-turbo / qwen2.5)
fn dashscope_needs_enable_thinking(
    api_type: AgentProviderApiType,
    base_url: &str,
    model_id: &str,
    effort_setting: crate::settings::ReasoningEffortSetting,
) -> bool {
    if !matches!(api_type, AgentProviderApiType::OpenAi) {
        return false;
    }
    if matches!(effort_setting, crate::settings::ReasoningEffortSetting::Off) {
        return false;
    }
    let url = base_url.to_ascii_lowercase();
    let is_dashscope = url.contains("dashscope.aliyuncs.com")
        || url.contains("dashscope.cn")
        || url.contains("dashscope-intl.aliyuncs.com");
    if !is_dashscope {
        return false;
    }
    let id = model_id.to_ascii_lowercase();
    if id.contains("kimi-k2-thinking") {
        return false;
    }
    id.contains("qwen3")
        || id.contains("qwq")
        || id.contains("deepseek-r1")
        || id.contains("kimi-k2.5")
        || id.contains("kimi-k2-")
        || id.contains("qwen-plus")
}

/// Infers the upstream provider from base_url, used **only** to decide whether to send
/// `prompt_cache_key`.
///
/// Aligned with the `options()` function in opencode
/// `packages/opencode/src/provider/transform.ts`: opencode uses `providerID` to decide
/// whether to set `promptCacheKey`. Only these 5 providers send it: `openai` / `azure` /
/// `openrouter` / `venice` / `opencode*`. All other providers (including OpenAI-compatible
/// proxies / local services / most domestic cloud providers) never send it.
///
/// Waz doesn't have a `providerID` dimension — only the user-provided `base_url`.
/// Therefore we infer from `base_url`:
/// - `api.openai.com`           → "openai"
/// - `*.openai.azure.com`       → "azure"
/// - `openrouter.ai/api`        → "openrouter"
/// - `api.venice.ai/api`        → "venice"
/// - `opencode.ai/zen`          → "opencode"
///
/// All others return `None` (equivalent to not matching any branch in opencode).
///
/// Data source: the `api` field from the [models.dev](https://models.dev/api.json) provider
/// table. openai / azure / venice don't have an `api` field in models.dev (baked into SDKs),
/// so they are hardcoded here based on each SDK's default endpoint.
///
/// Intentionally limited to these 5 — this is opencode's actual allowlist. Do not expand
/// just because "other cache-compatible providers seem to exist". OpenRouter documentation
/// natively supports `prompt_cache_key` (snake_case); the other four use the OpenAI Chat
/// Completions path.
fn opencode_compatible_cache_provider(base_url: &str) -> bool {
    let u = base_url.to_ascii_lowercase();
    u.contains("api.openai.com")
        || u.contains(".openai.azure.com")
        || u.contains("openrouter.ai/api")
        || u.contains("api.venice.ai/api")
        || u.contains("opencode.ai/zen")
}

fn build_chat_options(
    api_type: AgentProviderApiType,
    base_url: &str,
    model_id: &str,
    effort_setting: crate::settings::ReasoningEffortSetting,
    extra_headers: Vec<(String, String)>,
    conversation_id: Option<&str>,
) -> ChatOptions {
    let mut opts = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_tool_calls(true)
        .with_capture_reasoning_content(true)
        .with_capture_usage(true)
        // Have genai extract <think>...</think> segments embedded in content by DeepSeek-style
        // models and place them into reasoning chunks, making the UI display cleaner.
        // Only takes effect on adapters that support this format.
        .with_normalize_reasoning_content(true);

    // Prompt caching (1:1 aligned with the `options()` function in opencode
    // `packages/opencode/src/provider/transform.ts`). Key points:
    //
    // 1. **The `prompt_cache_retention` field (genai `ChatOptions::cache_control`) is never sent**.
    //    opencode doesn't use this field anywhere; strict-schema-validating BYOP proxies
    //    (OpenCode Go / vLLM / lm-studio / most domestic proxies) reject with HTTP 400
    //    `Extra inputs are not permitted, field: 'prompt_cache_retention'`
    //    (issue #126). Even OpenAI officially has a 5min implicit cache by default;
    //    explicit declaration is unnecessary.
    //
    // 2. **`prompt_cache_key` is only sent for 5 providers known to opencode**:
    //    `openai` / `azure` / `openrouter` / `venice` / `opencode`. All other providers
    //    (including OpenAI-compatible proxies / domestic cloud / local services) never send it.
    //
    //    opencode uses `providerID` (user-selected config string) to make this decision;
    //    Waz doesn't have `providerID`, so we infer from `base_url` → see
    //    `opencode_compatible_cache_provider`. This is the only semantic use of base_url;
    //    do not extend it to determine behavior beyond caching.
    //
    // 3. Anthropic uses per-message cache_control (in `build_chat_request`),
    //    not handled here.
    // 4. DeepSeek / Gemini / Ollama use server-side implicit caching; skipped.
    if matches!(
        api_type,
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp
    ) && opencode_compatible_cache_provider(base_url)
    {
        if let Some(cid) = conversation_id {
            if !cid.is_empty() {
                opts = opts.with_prompt_cache_key(cid.to_owned());
            }
        }
    }

    // **Thinking depth level dispatch** (aligned with Zed `LanguageModelRequest::thinking_allowed`
    // handling per provider: when `thinking_allowed=false`, all providers send no thinking
    // fields; Anthropic / Google / Bedrock server defaults already have thinking off).
    //
    // - **Auto**: don't send anything, let genai use "model name suffix inference"
    //   (internal to OpenAI/Anthropic adapters).
    // - **Off + Anthropic / Gemini**: **completely skip `with_reasoning_effort`**, equivalent to
    //   Auto + model name without thinking suffix. The genai adapter takes the `(model, None)`
    //   inference branch, skipping `insert_anthropic_reasoning` / `thinkingConfig`; no thinking
    //   field is sent.
    //   ★ This conveniently avoids the vendor genai bug where `claude-opus-4-6` / `claude-sonnet-4-6`
    //   `support_adaptive` forcibly injects `thinking:{type:adaptive}`
    //   (`lib/rust-genai/src/adapter/adapters/anthropic/adapter_impl.rs:121-135`
    //   doesn't check whether effort is `None`).
    // - **Off + DeepSeek**: server-side `thinking_mode` is enabled by default (deepseek-v4-flash etc.),
    //   requiring explicit `extra_body.thinking.type=disabled` to turn off. Waz's local fork
    //   of genai already supports `ChatOptions::extra_body` top-level merging.
    // - **Off + OpenAI / OpenAiResp**: uses the `reasoning_effort: "none"` path
    //   (GPT-5 / codex accept `none`; o-series filtered by capability table).
    // - **Non-Off + model doesn't support reasoning**: skipped, to avoid injecting thinking
    //   parameters into older models (claude-3-5-haiku / gpt-4o / gemini-1.5-pro) that would
    //   be rejected with HTTP 400 by the upstream.
    use crate::settings::ReasoningEffortSetting as RE;
    match (api_type, effort_setting) {
        // Auto: don't send any parameters
        (_, RE::Auto) => {}

        // Anthropic + Off: don't send thinking field
        (AgentProviderApiType::Anthropic, RE::Off) => {
            log::info!(
                "[byop] Anthropic Off → skip reasoning_effort (model={model_id}); \
                 no thinking field sent"
            );
        }

        // Gemini + Off: don't send thinkingConfig
        (AgentProviderApiType::Gemini, RE::Off) => {
            log::info!(
                "[byop] Gemini Off → skip reasoning_effort (model={model_id}); \
                 no thinkingConfig sent"
            );
        }

        // DeepSeek + Off: explicit disabled
        (AgentProviderApiType::DeepSeek, RE::Off) => {
            log::info!(
                "[byop] DeepSeek Off → extra_body thinking.type=disabled (model={model_id})"
            );
            opts = opts.with_extra_body(json!({"thinking": {"type": "disabled"}}));
        }

        // Others (OpenAI / OpenAiResp / Ollama / any provider with non-Off setting):
        // Follow the capability-table-filtered reasoning_effort injection path
        _ => {
            if let Some(effort) = effort_setting.to_genai() {
                if super::reasoning::model_supports_reasoning(api_type, model_id) {
                    log::info!(
                        "[byop] reasoning_effort injected: model={model_id} setting={effort_setting:?}"
                    );
                    opts = opts.with_reasoning_effort(effort);
                } else {
                    log::info!(
                        "[byop] reasoning_effort SKIPPED: model={model_id} not in capability list \
                         (api_type={api_type:?} setting={effort_setting:?}); request sent without thinking params"
                    );
                }
            }
        }
    }

    // DashScope (Alibaba Cloud Bailian) OpenAI-compatible path requires explicit
    // `enable_thinking: true` to output reasoning. See `dashscope_needs_enable_thinking` docs.
    // Mutually exclusive with the DeepSeek Off extra_body above (DeepSeek uses the DeepSeek
    // api_type, DashScope uses the OpenAI api_type), so they never fire simultaneously.
    if dashscope_needs_enable_thinking(api_type, base_url, model_id, effort_setting) {
        log::info!(
            "[byop] DashScope reasoning model → extra_body enable_thinking=true \
             (model={model_id} setting={effort_setting:?})"
        );
        opts = opts.with_extra_body(json!({"enable_thinking": true}));
    }
    if !extra_headers.is_empty() {
        opts = opts.with_extra_headers(extra_headers);
    }

    opts
}

fn map_genai_error(err: genai::Error) -> OpenAiCompatibleError {
    use genai::Error as G;
    match err {
        // Actual parse failure: JSON deserialization stage
        G::StreamParse { .. }
        | G::SerdeJson(_)
        | G::JsonValueExt(_)
        | G::InvalidJsonResponseElement { .. } => OpenAiCompatibleError::Decode(format!("{err}")),

        // Network/streaming send stage failure (reqwest connection, TLS, DNS, timeout, stream interruption, etc.)
        G::WebStream { .. } | G::WebAdapterCall { .. } | G::WebModelCall { .. } => {
            OpenAiCompatibleError::Stream(format!("{err}"))
        }

        // Server-returned HTTP error status
        G::HttpError {
            status,
            body,
            canonical_reason,
        } => OpenAiCompatibleError::Status {
            status: status.as_u16(),
            body: if canonical_reason.is_empty() {
                body
            } else {
                format!("{canonical_reason}: {body}")
            },
        },

        // Everything else (request construction, auth, unsupported capabilities, etc.)
        // classified as generic error to avoid misleading as "parse failure"
        other => OpenAiCompatibleError::Other(format!("{other}")),
    }
}

// ---------------------------------------------------------------------------
// Main flow
// ---------------------------------------------------------------------------

/// BYOP configuration needed for title generation. May use the same provider as the main
/// request or a different one (user independently selected a title_model in the Profile Editor).
pub struct TitleGenInput {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub api_type: AgentProviderApiType,
    pub reasoning_effort: crate::settings::ReasoningEffortSetting,
}

pub struct ByopOutputInput {
    pub params: RequestParams,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub api_type: AgentProviderApiType,
    pub reasoning_effort: crate::settings::ReasoningEffortSetting,
    pub extra_headers: Vec<(String, String)>,
    pub task_id: String,
    pub target_task_id: String,
    pub needs_create_task: bool,
    pub lrc_command_id: Option<String>,
    pub lrc_should_spawn_subagent: bool,
    pub context_window: Option<u32>,
    pub cancellation_rx: futures::channel::oneshot::Receiver<()>,
}

/// `task_id`: the conversation's root task id (obtained from the history model on the controller side).
/// `target_task_id`: the task id where this turn's model output should be written; equals root
/// for normal conversations, or an existing subtask for subsequent CLI subagent turns.
/// `needs_create_task`: only the first turn (when root is still Optimistic) needs to emit `CreateTask`.
pub async fn generate_byop_output(
    input: ByopOutputInput,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    let ByopOutputInput {
        params,
        base_url,
        api_key,
        model_id,
        api_type,
        reasoning_effort,
        extra_headers,
        task_id,
        target_task_id,
        needs_create_task,
        lrc_command_id,
        lrc_should_spawn_subagent,
        context_window,
        cancellation_rx: _cancellation_rx,
    } = input;

    let force_echo_reasoning = super::reasoning::model_requires_reasoning_echo(api_type, &model_id);
    let chat_req = build_chat_request(&params, force_echo_reasoning, api_type, &model_id)?;
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|t| t.as_str().to_string())
        .unwrap_or_default();
    let chat_opts = build_chat_options(
        api_type,
        &base_url,
        &model_id,
        reasoning_effort,
        extra_headers,
        if conversation_id.is_empty() {
            None
        } else {
            Some(conversation_id.as_str())
        },
    );
    let client = build_client(api_type, base_url, api_key);
    let request_id = Uuid::new_v4().to_string();
    let mcp_context = params.mcp_context.clone();
    let cwd = params.session_context.current_working_directory().as_deref().map(|s| s.to_string());

    // ⚠️ BYOP persistence critical: under warp's own path, the following ClientActions are
    // emitted server-side to have the client write "non-model-produced" messages like
    // UserQuery / ToolCallResult back into task.messages, ensuring the next turn's
    // `params.tasks` snapshot is complete.
    //
    // BYOP removes cloud dependency; the client manages itself and the server doesn't exist,
    // so we must emit these write-back events ourselves. Otherwise the next turn's
    // `compute_active_tasks` would only see model output (reasoning/output/tool_call) and
    // miss the corresponding user_query and tool_call_result, severely breaking model context.
    //
    // Here, after the stream starts, we write the current turn's UserQuery / ToolCallResult
    // in the original order from `params.input`. When the user interrupts a pending tool and
    // continues input, the controller passes ActionResult → UserQuery; persistence cannot be
    // split into "all UserQuery first, all ActionResult after", otherwise history becomes
    // Assistant(tool_call) → UserQuery → ToolCallResult.
    //
    // Emit timing must be after CreateTask (task has been upgraded to Server state),
    // before model response starts (UI order: user display → thinking/answer).
    // Waz: multimodal persistence for historical turns. Beyond the query text, we also package
    // all multimodal binaries (image / pdf / audio / ...) from the current turn's
    // UserQuery.context into `UserQuery.context.images` for persistence (the proto field is
    // called images, but semantically it's a generic BinaryFile — `bytes data + mime_type`,
    // equivalent to opencode FilePart). This lets build_chat_request recover binaries from
    // historical messages in the next turn, continuing to inject them as ContentPart::Binary
    // upstream (unsupported MIME types are replaced with ERROR text by
    // build_user_message_with_binaries, consistent with opencode unsupportedParts).
    // Warp's own path doesn't need this step because the cloud server holds the InputContext;
    // BYOP direct connection requires client-side management.
    let pending_user_queries: Vec<(String, Vec<user_context::UserBinary>)> = params
        .input
        .iter()
        .filter_map(|i| match i {
            AIAgentInput::UserQuery { query, context, .. } => {
                let attachments = user_context::collect_user_attachments(context);
                Some((query.clone(), attachments.binaries))
            }
            _ => None,
        })
        .collect();
    // INFO-level one-line overview + one-line summary per message (role + text length + tool count
    // + reasoning marker); visible with default log config, useful for diagnosing whether
    // "history was fully sent upstream".
    //
    // Note: on the Anthropic path, `build_chat_request` pushes system text as `ChatMessage::system`
    // into messages[0] to apply `cache_control`, so `chat_req.system` will be None and `system_len`
    // shows as 0; the actual system content is still in messages[0] (see per-message report below).
    // To avoid misleading diagnostics, we add the `system_in_messages_head` hint here.
    log_chat_request_details(&chat_req, &model_id, api_type);

    // Diagnostics: construct a full ChatRequest JSON dump containing system / messages / tools,
    // saved into the stream closure. The actual Anthropic wire body is further transformed by
    // the genai adapter, but this already covers all raw strings passed to BYOP, sufficient
    // to pinpoint whether illegal escapes come from prompts, tool descriptions, schemas,
    // or tool results.
    let diag_body_json = serde_json::to_string(&json!({
        "model": &model_id,
        "chat_request": &chat_req,
    }))
    .unwrap_or_default();
    log::info!("[byop] diag_body_approx_len={}", diag_body_json.len());
    log::info!("[byop-diag] full_request_json={diag_body_json}");

    // Proactively scan raw text for "suspicious backslash sequences": serde_json serializes
    // a literal `\` in the source string as `\\`, so "two consecutive backslashes + u/x" in
    // the wire body means the original text has a literal `\u` / `\x` — this is the real risk
    // point where proxies mistakenly "restore `\\u` → `\u`" triggering invalid escape.
    // Source-string `\n` / `\r` / `\t` are output by serde_json as a single backslash + letter,
    // which are legitimate JSON escapes; proxies won't double-restore them, so they're not suspicious.
    fn scan_suspicious_backslash(label: &str, s: &str) {
        let bytes = s.as_bytes();
        let mut bs_hits: Vec<(usize, String)> = Vec::new();
        let mut ctrl_hits: Vec<(usize, u8)> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            // Literal `\\u` / `\\x` sequence (source string contains `\u` / `\x`).
            if b == b'\\'
                && i + 2 < bytes.len()
                && bytes[i + 1] == b'\\'
                && matches!(bytes[i + 2], b'u' | b'x')
            {
                let end = (i + 10).min(bytes.len());
                let snippet = String::from_utf8_lossy(&bytes[i..end]).to_string();
                if bs_hits.len() < 5 {
                    bs_hits.push((i, snippet));
                }
                // Skip this pair to avoid triggering multiple times at the same position.
                i += 3;
                continue;
            }
            // Raw control characters (byte 0x00-0x08, 0x0B-0x0C, 0x0E-0x1F).
            // serde_json escapes them as `\u00XX`, which is valid JSON; but some strict proxies
            // or intermediate encoding layers (base64, etc.) are most likely to break on these bytes.
            if (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')) && ctrl_hits.len() < 10 {
                ctrl_hits.push((i, b));
            }
            i += 1;
        }
        if !bs_hits.is_empty() {
            log::warn!("[byop] {label} suspicious literal '\\\\u'/'\\\\x' patterns: {bs_hits:?}");
        }
        if !ctrl_hits.is_empty() {
            log::warn!("[byop] {label} contains raw control chars (offset, byte): {ctrl_hits:?}");
        }
    }
    scan_suspicious_backslash("full_request_json", &diag_body_json);
    if let Some(sys) = chat_req.system.as_deref() {
        scan_suspicious_backslash("system", sys);
    }
    for (idx, m) in chat_req.messages.iter().enumerate() {
        if let Some(t) = m.content.first_text() {
            scan_suspicious_backslash(&format!("msg[{idx}]"), t);
        }
    }

    let stream = async_stream::stream! {
        // 1) StreamInit — always sent first so the UI can immediately show "thinking..."
        yield Ok(api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(
                api::response_event::StreamInit {
                    request_id: request_id.clone(),
                    conversation_id,
                    run_id: String::new(),
                },
            )),
        });

        // 2) First turn: CreateTask to upgrade Optimistic root → Server.
        if needs_create_task {
            yield Ok(create_task_event(&task_id));
        }

        // 3) Persist the input's UserQuery / ToolCallResult into task.messages.
        //    (Warp server path emits these from the backend; BYOP client must emit them itself,
        //    see comment above.)
        //    On tag-in first turn, write to root first, then the spawn branch below copies
        //    to the new subtask; subsequent CLI subagent turns write directly to target_task_id.
        let persistence_task_id = if lrc_should_spawn_subagent {
            task_id.as_str()
        } else {
            target_task_id.as_str()
        };
        let mut persistence_messages: Vec<api::Message> = Vec::new();
        let mut persistence_order: Vec<String> = Vec::new();
        for (input_idx, input) in params.input.iter().enumerate() {
            match input {
                AIAgentInput::UserQuery {
                    query,
                    context,
                    running_command,
                    ..
                } => {
                    let attachments = user_context::collect_user_attachments(context);
                    log::info!(
                        "[byop-diag] persistence input[{input_idx}]: task_id={} \
                         kind=UserQuery query_len={} binaries={} running_command={} \
                         lrc_command_id={} query={:?}",
                        persistence_task_id,
                        query.len(),
                        attachments.binaries.len(),
                        running_command.is_some(),
                        lrc_command_id.as_deref().unwrap_or(""),
                        snippet_for_log(query, BYOP_DIAG_SNIPPET_CHARS),
                    );
                    persistence_order.push(format!(
                        "{input_idx}:UserQuery(query_len={},binaries={})",
                        query.len(),
                        attachments.binaries.len()
                    ));
                    persistence_messages.push(make_user_query_message(
                        persistence_task_id,
                        &request_id,
                        query.clone(),
                        &attachments.binaries,
                    ));
                }
                AIAgentInput::ActionResult { result, .. } => {
                    let content = tools::serialize_action_result(result).unwrap_or_else(|| {
                        serde_json::json!({ "result": result.result.to_string() }).to_string()
                    });
                    log::info!(
                        "[byop-diag] persistence input[{input_idx}]: task_id={} \
                         kind=ActionResult call_id={} content_len={} content={:?}",
                        persistence_task_id,
                        result.id,
                        content.len(),
                        snippet_for_log(&content, BYOP_DIAG_SNIPPET_CHARS),
                    );
                    persistence_order.push(format!(
                        "{input_idx}:ActionResult(call_id={},content_len={})",
                        result.id,
                        content.len()
                    ));
                    persistence_messages.push(make_tool_call_result_message(
                        persistence_task_id,
                        &request_id,
                        result.id.to_string(),
                        content,
                    ));
                }
                _ => {}
            }
        }
        log::info!(
            "[byop-diag] persistence summary: request_id={} task_id={} emitted_messages={} \
             input_order={:?}",
            request_id,
            persistence_task_id,
            persistence_messages.len(),
            persistence_order,
        );
        if !persistence_messages.is_empty() {
            yield Ok(make_add_messages_event(persistence_task_id, persistence_messages));
        }

        // 3.5) LRC subagent spawn (aligned with the upstream cloud cli subagent injection path).
        //
        // When the request comes from alt-screen + agent tagged-in state, `lrc_command_id` carries
        // the current LRC block's id string. Here the client synthesizes two events:
        //   a) AddMessagesToTask(root, [<virtual subagent tool_call>])
        //      Attaches a ToolCall::Subagent { task_id=<new subtask>,
        //      metadata: Cli { command_id }, payload: "" } to root.messages.
        //      The conversation's `Task::new_subtask` matches this subagent_call by task_id
        //      from parent.messages, extracting SubagentParams to attach to the subtask.
        //   b) CreateTask(api::Task { id=<new subtask>, dependencies.parent_task_id=root })
        //      Triggers `apply_client_action::CreateTask`; since parent_id is non-empty,
        //      it takes the `new_subtask` path, then emits
        //      `BlocklistAIHistoryEvent::CreatedSubtask` →
        //      `cli_controller::handle_history_model_event` sees cli_subagent_block_id
        //      is non-empty, emits `CLISubagentEvent::SpawnedSubagent` → terminal_view
        //      creates a `CLISubagentView` floating panel, inserted into `cli_subagent_views` map.
        //
        // Switches subsequent chunk emit's task_id to subtask_id, so model reasoning/output/tool_call
        // all go into the subtask; subagent_view renders floating panel content accordingly.
        //
        // Timing constraint: must be after root CreateTask + UserQuery persistence, before model
        // stream. Otherwise the conversation can't find the root task / user query reference pair.
        let mut current_task_id = if lrc_should_spawn_subagent {
            task_id.clone()
        } else {
            target_task_id.clone()
        };
        if lrc_should_spawn_subagent {
            let Some(command_id) = lrc_command_id.clone() else {
                log::warn!("[byop] LRC spawn requested without command_id");
                yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                    "BYOP LRC spawn requested without command_id"
                ))));
                return;
            };
            let subtask_id = Uuid::new_v4().to_string();
            let tool_call_id = Uuid::new_v4().to_string();
            log::info!(
                "[byop] LRC tag-in: spawning cli subagent subtask={subtask_id} \
                 command_id={command_id} parent={task_id}"
            );

            let subagent_tool = api::message::tool_call::Tool::Subagent(
                api::message::tool_call::Subagent {
                    task_id: subtask_id.clone(),
                    payload: String::new(),
                    metadata: Some(
                        api::message::tool_call::subagent::Metadata::Cli(
                            api::message::tool_call::subagent::CliSubagent {
                                command_id,
                            },
                        ),
                    ),
                },
            );
            let subagent_msg = make_tool_call_message(
                &task_id,
                &request_id,
                &tool_call_id,
                subagent_tool,
            );
            // a) Attach the subagent tool_call to root.messages for new_subtask to look up SubagentParams.
            yield Ok(make_add_messages_event(&task_id, vec![subagent_msg]));
            // b) Create a subtask with parent_task_id; the conversation detects non-empty
            //    parent_id → takes the `Task::new_subtask` path, auto-binding SubagentParams.
            yield Ok(create_subtask_event(&subtask_id, &task_id));

            // c) Waz A1: also copy the current turn's UserQuery to the subtask, initializing
            //    the subtask's exchange.output.messages. Otherwise CLISubagentView rendering
            //    finds the subtask's exchanges output empty, and the floating panel permanently
            //    shows only a 49.6-height empty dialog with no visible content.
            //    The upstream cloud has a complete ClientAction sequence filling exchange.output
            //    for the cli subagent task; BYOP client self-management requires explicit injection.
            //
            //    Only copy this turn's UserQuery (`pending_user_queries`); don't touch the root's
            //    copy (root retains the user query reference to avoid exchange.input being empty
            //    which would confuse the state machine).
            //    Subsequent model chunks use `current_task_id = subtask_id`, appending after this starting point.
            if !pending_user_queries.is_empty() {
                let mut subtask_messages: Vec<api::Message> = Vec::new();
                for (q, imgs) in &pending_user_queries {
                    subtask_messages.push(make_user_query_message(
                        &subtask_id,
                        &request_id,
                        q.clone(),
                        imgs,
                    ));
                }
                yield Ok(make_add_messages_event(&subtask_id, subtask_messages));
            }

            // Subsequent chunk emit switches to subtask.
            current_task_id = subtask_id;
        }

        log::info!("[byop] opening stream: model={model_id}");
        let mut sdk_stream = match client
            .exec_chat_stream(&model_id, chat_req, Some(&chat_opts))
            .await
        {
            Ok(resp) => {
                log::info!("[byop] stream opened OK (HTTP request accepted)");
                resp.stream
            }
            Err(e) => {
                let mapped = map_genai_error(e);
                log::error!("[byop] open stream failed: {mapped:#}");
                yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                    "BYOP open stream failed: {mapped}"
                ))));
                return;
            }
        };

        // Streaming state: text / reasoning each get their message id generated when the first
        // chunk arrives; subsequent chunks use AppendToMessageContent for incremental appending.
        let mut text_msg_id: Option<String> = None;
        let mut reasoning_msg_id: Option<String> = None;
        // tool_calls accumulated by call_id — genai's streamed ToolCallChunk already carries
        // a complete ToolCall (behavior since 0.4.0), but the same call_id may appear across
        // multiple chunks with incremental args. We accumulate by id in a HashMap and emit
        // them all at stream end.
        let mut tool_bufs: HashMap<String, ToolCall> = HashMap::new();
        let mut tool_order: Vec<String> = Vec::new();
        // call_id → message id of the first-frame placeholder ToolCall message.
        // When the first ToolCallChunk arrives and can be parsed, we immediately emit a
        // placeholder card (so the UI can show "calling tool X" feedback before stream End);
        // at stream end, we update_message in-place to the final args.
        // call_ids not in this map (first-frame parse failure / web tools) use the old path
        // of emitting all at once after End.
        let mut tool_msg_ids: HashMap<String, String> = HashMap::new();
        // call_id → timestamp of last update_message incremental refresh.
        // For long-args tools (create_or_edit_document, long grep query), args accumulate across
        // multiple chunks; throttle to ≥200ms between reparse + update, making it feel as
        // continuous as text streaming rather than freezing on the first frame until End.
        let mut tool_last_update: HashMap<String, Instant> = HashMap::new();
        // Incremental refresh throttle threshold: consecutive chunks closer than this won't
        // trigger update_message, avoiding frequent UI relayout.
        // Note: the SDK stream awaits each ChatStreamEvent independently; when multiple tools
        // arrive concurrently they're already sequential, so batch-emitting in the same tick
        // has little benefit at this layer. The real jitter reduction comes from throttling;
        // this comment is a reminder not to blindly introduce batching later.
        const TOOL_ARGS_UPDATE_THROTTLE_MS: u64 = 200;
        // Diagnostics: count each stream event type, log at INFO level at stream end.
        // Used to debug "silent message disappearance" — if chunk_count=0 and tool_count=0,
        // it means the upstream returned empty content.
        let mut start_count: u32 = 0;
        let mut chunk_count: u32 = 0;
        let mut chunk_bytes: usize = 0;
        let mut reasoning_count: u32 = 0;
        let mut reasoning_bytes: usize = 0;
        let mut tool_chunk_count: u32 = 0;
        let mut end_count: u32 = 0;
        let mut other_count: u32 = 0;
        // Accumulated token usage for this turn. genai carries captured_usage (Option<Usage>)
        // in the ChatStreamEvent::End event; its prompt_tokens represents the entire history
        // for this turn (both Anthropic and OpenAI count by "full request prompt"),
        // completion_tokens is the model output. The sum divided by context_window gives
        // the "context utilization rate", semantically consistent with warp's own server path.
        let mut captured_prompt_tokens: i32 = 0;
        let mut captured_completion_tokens: i32 = 0;
        // P0-6 prompt cache hit rate monitoring: assemble cache_read / cache_create fields
        // returned by Anthropic / OpenAI / Gemini from genai `Usage.prompt_tokens_details`.
        // See stream End handling logic. DeepSeek / Ollama don't use cache fields;
        // their values remain 0.
        let mut captured_cache_read_tokens: i32 = 0;
        let mut captured_cache_create_tokens: i32 = 0;

        while let Some(item) = sdk_stream.next().await {
            let event = match item {
                Ok(ev) => ev,
                Err(e) => {
                    let mapped = map_genai_error(e);
                    let err_text = format!("{mapped:#}");
                    log::error!("[byop] stream chunk error: {err_text}");
                    log::error!("[byop-diag] full_request_json_on_error={diag_body_json}");
                    // Parse "column N" from the error message, dump diag_body_json ±200 char
                    // context around that position + byte hex.
                    if let Some(col) = err_text
                        .split("column ")
                        .nth(1)
                        .and_then(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<usize>().ok())
                    {
                        let body = &diag_body_json;
                        let byte_len = body.len();
                        let start = col.saturating_sub(200).min(byte_len);
                        let end = (col + 200).min(byte_len);
                        let context = body.get(start..end).unwrap_or("(slice failed: not on char boundary)");
                        log::error!(
                            "[byop] error column={col} diag_body_len={byte_len} context[{start}..{end}]={context:?}"
                        );
                        let hex_start = col.saturating_sub(20).min(byte_len);
                        let hex_end = (col + 20).min(byte_len);
                        if let Some(slice) = body.as_bytes().get(hex_start..hex_end) {
                            log::error!("[byop] error bytes[{hex_start}..{hex_end}] hex={slice:02x?}");
                        }
                    }
                    yield Err(Arc::new(AIApiError::Other(anyhow::anyhow!(
                        "BYOP stream error: {mapped}"
                    ))));
                    return;
                }
            };

            match event {
                ChatStreamEvent::Start => {
                    // Unit event; UI already shows thinking via StreamInit, this is a no-op
                    start_count += 1;
                }
                ChatStreamEvent::Chunk(c) if !c.content.is_empty() => {
                    chunk_count += 1;
                    chunk_bytes += c.content.len();
                    if let Some(id) = text_msg_id.clone() {
                        yield Ok(make_append_event(&current_task_id, &id, AppendKind::Text(c.content)));
                    } else {
                        let new_id = Uuid::new_v4().to_string();
                        let mut msg = make_agent_output_message(&current_task_id, &request_id, c.content);
                        msg.id = new_id.clone();
                        text_msg_id = Some(new_id);
                        yield Ok(make_add_messages_event(&current_task_id, vec![msg]));
                    }
                }
                ChatStreamEvent::Chunk(_) => {}
                ChatStreamEvent::ReasoningChunk(c) if !c.content.is_empty() => {
                    reasoning_count += 1;
                    reasoning_bytes += c.content.len();
                    // Runtime latch: this (api_type, model_id) has sent a reasoning chunk →
                    // mark to force echo reasoning_content from the next turn onward, covering
                    // any domestic/third-party thinking model not in the INTERLEAVED_RULES
                    // static table (aligned with opencode's data-driven approach: use stream
                    // detection instead of external catalog).
                    super::reasoning::note_reasoning_seen(api_type, &model_id);
                    if let Some(id) = reasoning_msg_id.clone() {
                        yield Ok(make_append_event(&current_task_id, &id, AppendKind::Reasoning(c.content)));
                    } else {
                        let new_id = Uuid::new_v4().to_string();
                        let mut msg = make_reasoning_message(&current_task_id, &request_id, c.content);
                        msg.id = new_id.clone();
                        reasoning_msg_id = Some(new_id);
                        yield Ok(make_add_messages_event(&current_task_id, vec![msg]));
                    }
                }
                ChatStreamEvent::ReasoningChunk(_) => {}
                ChatStreamEvent::ToolCallChunk(tc) => {
                    tool_chunk_count += 1;
                    let mut call = tc.tool_call;
                    // Very few providers (self-hosted ollama proxies, etc.) don't send call_id; fall back to local uuid.
                    if call.call_id.is_empty() {
                        call.call_id = Uuid::new_v4().to_string();
                    }
                    // First time seeing this call_id → immediately push a placeholder ToolCall
                    // message to pending_placeholders, so the UI shows a "calling tool X" card
                    // before stream End.
                    // When multiple tools arrive in the same tick: batch emit once via
                    // add_messages before this loop iteration ends, reducing view tree relayout.
                    // Already in the map (placeholder sent) and new args chunk arrives →
                    // throttle ≥200ms reparse + update_message incremental refresh;
                    // long-args tools (create_or_edit_document, long grep, etc.) feel continuous.
                    // Web tools (webfetch/websearch) use their own loading frame pipeline
                    // (L2102 region); skipped here to avoid duplicate cards.
                    // todowrite goes through the BYOP todo interceptor, synthesizing
                    // Message::UpdateTodos to trigger the chip; also skipped here to avoid
                    // showing a meaningless "calling todowrite" card.
                    if call.fn_name != tools::webfetch::TOOL_NAME
                        && call.fn_name != tools::websearch::TOOL_NAME
                        && call.fn_name != tools::todowrite::TOOL_NAME
                    {
                        if let Some(msg_id) = tool_msg_ids.get(&call.call_id).cloned() {
                            // Placeholder already emitted → throttled incremental refresh.
                            let now = Instant::now();
                            let last = tool_last_update.get(&call.call_id).copied();
                            let elapsed_ok = last
                                .map(|t| now.duration_since(t).as_millis() as u64 >= TOOL_ARGS_UPDATE_THROTTLE_MS)
                                .unwrap_or(true);
                            if elapsed_ok {
                                if let Ok(parsed) =
                                    parse_incoming_tool_call(&call, mcp_context.as_ref(), cwd.as_deref())
                                {
                                    let mut updated = make_tool_call_message(
                                        &current_task_id,
                                        &request_id,
                                        &call.call_id,
                                        parsed,
                                    );
                                    updated.id = msg_id;
                                    tool_last_update.insert(call.call_id.clone(), now);
                                    yield Ok(make_update_message_event(
                                        &current_task_id,
                                        updated,
                                        vec!["tool_call".to_owned()],
                                    ));
                                }
                                // Reparse failed (intermediate state): silently skip, wait for next chunk.
                            }
                        } else if let Ok(parsed) =
                            parse_incoming_tool_call(&call, mcp_context.as_ref(), cwd.as_deref())
                        {
                            // First successful parse → immediately emit placeholder card.
                            // Each chunk before placeholder emit will re-parse (i.e. "retry on every
                            // chunk"), so even if the first frame's args are incomplete, any
                            // subsequent chunk with complete args will immediately trigger
                            // placeholder emit — this is the P1-4 coverage path,
                            // no generic placeholder variant needed.
                            let msg_id = Uuid::new_v4().to_string();
                            let mut placeholder = make_tool_call_message(
                                &current_task_id,
                                &request_id,
                                &call.call_id,
                                parsed,
                            );
                            placeholder.id = msg_id.clone();
                            tool_msg_ids.insert(call.call_id.clone(), msg_id);
                            tool_last_update.insert(
                                call.call_id.clone(),
                                Instant::now(),
                            );
                            yield Ok(make_add_messages_event(
                                &current_task_id,
                                vec![placeholder],
                            ));
                        }
                        // First-frame parse failed (args not yet complete / unknown tool): don't
                        // emit yet; retry on next chunk or use the old path at End to avoid
                        // visual jitter.
                    }
                    // Same call_id across multiple chunks: later arrivals overwrite (genai has merged args).
                    if !tool_bufs.contains_key(&call.call_id) {
                        tool_order.push(call.call_id.clone());
                    }
                    tool_bufs.insert(call.call_id.clone(), call);
                }
                ChatStreamEvent::End(end) => {
                    end_count += 1;
                    // genai >= 0.4.0's captured_content includes tool_calls.
                    // Prefer tool_calls from captured_content (more complete);
                    // otherwise fall back to the streaming-accumulated tool_bufs.
                    if let Some(content) = end.captured_content.as_ref() {
                        let mut captured_order: Vec<String> = Vec::new();
                        for call in content.tool_calls() {
                            if !captured_order.contains(&call.call_id) {
                                captured_order.push(call.call_id.clone());
                            }
                            tool_bufs.insert(call.call_id.clone(), call.clone());
                        }
                        if !captured_order.is_empty() {
                            for call_id in &tool_order {
                                if !captured_order.contains(call_id) {
                                    captured_order.push(call_id.clone());
                                }
                            }
                            tool_order = captured_order;
                        }
                    }
                    if let Some(usage) = end.captured_usage.as_ref() {
                        // Multiple End events: take the max as a safeguard (theoretically a single stream has only one End).
                        if let Some(p) = usage.prompt_tokens {
                            captured_prompt_tokens = captured_prompt_tokens.max(p);
                        }
                        if let Some(c) = usage.completion_tokens {
                            captured_completion_tokens = captured_completion_tokens.max(c);
                        }
                        // P0-6 prompt cache hit rate monitoring: Anthropic / OpenAI / Gemini
                        // respectively return `cache_read_input_tokens` (Anthropic) /
                        // `cached_tokens` (OpenAI) / `cachedContentTokenCount` (Gemini)
                        // in `prompt_tokens_details`. genai has unified them into `cached_tokens`.
                        // Similarly, `cache_creation_tokens` is only provided by Anthropic
                        // (write billing hint).
                        // Multiple End events: take the max, same semantics as prompt/completion.
                        if let Some(details) = usage.prompt_tokens_details.as_ref() {
                            if let Some(r) = details.cached_tokens {
                                captured_cache_read_tokens =
                                    captured_cache_read_tokens.max(r);
                            }
                            if let Some(w) = details.cache_creation_tokens {
                                captured_cache_create_tokens =
                                    captured_cache_create_tokens.max(w);
                            }
                        }
                    }
                }
                _ => {
                    other_count += 1;
                    // ThoughtSignatureChunk etc. not handled for now (Gemini 3 thoughts need
                    // to be passed to subsequent turns; BYOP currently doesn't persist
                    // thought_signatures, accepting the degradation)
                }
            }
        }

        // Stream statistics INFO log. When chunk_count=0 && tool_count=0, upstream returned empty;
        // most likely model_id not recognized / max_tokens missing / Anthropic API-compatible proxy
        // returned 200 but with empty body.
        let total_tools = tool_bufs.len();
        log::info!(
            "[byop] stream stats: start={start_count} chunks={chunk_count} ({chunk_bytes}B) \
             reasoning={reasoning_count} ({reasoning_bytes}B) tool_chunks={tool_chunk_count} \
             ends={end_count} other={other_count} captured_tools={total_tools}"
        );
        // P0-6 prompt cache hit rate log (only printed when the provider returns cache fields).
        // ratio = cache_read / (prompt_tokens.max(1)) represents the fraction of this turn's
        // input that directly hit cache. create > 0 means this turn had a cache write;
        // write cost ≈ 1.25x base (5m) or 2x base (1h). read cost ≈ 0.1x base;
        // long-term, ≥1 reuse pays for the write.
        // Use ratio to verify whether P0 optimization is effective: turns 2+ in the same
        // conversation should show significantly higher ratio.
        //
        // **P2-16**: additionally include a `compaction=` identifier. Compaction itself rewrites
        // history, making the messages prefix inconsistent across the compaction boundary →
        // the first turn after compaction always has a cache miss. Outputting this signal in
        // logs allows later analysis (`script/analyze-prompt-cache.ps1`) to distinguish
        // "normal miss" from "compaction-induced miss", preventing false positives.
        if captured_cache_read_tokens > 0 || captured_cache_create_tokens > 0 {
            let denom = captured_prompt_tokens.max(1);
            let read_ratio = captured_cache_read_tokens as f32 / denom as f32;
            let create_ratio = captured_cache_create_tokens as f32 / denom as f32;
            // Compaction state: none → not enabled / inactive → enabled but no changes this turn /
            // active(count of hidden message ids) → compaction path was taken this turn.
            let compaction_label = match params.compaction_state.as_ref() {
                None => "none".to_owned(),
                Some(s) => {
                    let hidden = s.hidden_message_ids().len();
                    if hidden == 0 {
                        "inactive".to_owned()
                    } else {
                        format!("active(hidden={hidden})")
                    }
                }
            };
            log::info!(
                "[byop-cache] prompt_tokens={captured_prompt_tokens} \
                 cache_read={captured_cache_read_tokens} ({:.1}%) \
                 cache_create={captured_cache_create_tokens} ({:.1}%) \
                 model={model_id} compaction={compaction_label}",
                read_ratio * 100.0,
                create_ratio * 100.0,
            );
        }
        if chunk_count == 0 && reasoning_count == 0 && total_tools == 0 {
            log::warn!(
                "[byop] stream returned 0 content / 0 reasoning / 0 tool_calls — \
                 upstream may have returned an empty response (wrong model_id? missing max_tokens? proxy error?)"
            );
        }

        // Stream ended: emit all accumulated tool_calls at once.
        let mut final_messages: Vec<api::Message> = Vec::new();
        let mut ordered_tool_calls: Vec<ToolCall> = Vec::with_capacity(tool_bufs.len());
        for call_id in tool_order {
            if let Some(call) = tool_bufs.remove(&call_id) {
                ordered_tool_calls.push(call);
            }
        }
        let mut unordered_tool_calls: Vec<ToolCall> = tool_bufs.into_values().collect();
        if !unordered_tool_calls.is_empty() {
            // Under normal paths, both ChunkArgs and End maintain `tool_order` in sync,
            // so `tool_bufs` should be empty at this point. This fallback is only hit when
            // the provider behaves abnormally (e.g. captured_content and ChunkArgs each
            // missing some call_id). Dict-sort ensures OpenAI-compatible `tool_calls[]`
            // order is stable across calls (no cache prefix drift), but this should warn.
            log::warn!(
                "[byop] {} tool_calls fell through to dict-sort fallback — \
                 provider inconsistency between ChunkArgs and captured_content; \
                 call_ids={:?}",
                unordered_tool_calls.len(),
                unordered_tool_calls.iter().map(|t| t.call_id.as_str()).collect::<Vec<_>>(),
            );
        }
        unordered_tool_calls.sort_by(|a, b| a.call_id.cmp(&b.call_id));
        ordered_tool_calls.extend(unordered_tool_calls);
        for call in ordered_tool_calls {
            // Diagnostics: dump the raw tool_call payload actually sent by the model
            // (call_id / fn_name / fn_arguments JSON raw + type annotation),
            // for verifying whether the model follows the schema for input/output params
            // (common issues: bool fields stringified, numbers quoted, nested objects
            // collapsed into strings, etc.).
            // debug level: only shows when RUST_LOG=debug for schema troubleshooting,
            // doesn't pollute INFO normally.
            // info level: retains a short one-liner without args for stream timing visibility.
            log::info!(
                "[byop] tool_call_in: name={} call_id={}",
                call.fn_name,
                call.call_id,
            );
            if log::log_enabled!(log::Level::Debug) {
                let args_repr = if call.fn_arguments.is_string() {
                    format!("string({:?})", call.fn_arguments.as_str().unwrap_or(""))
                } else {
                    format!(
                        "{}({})",
                        match &call.fn_arguments {
                            Value::Object(_) => "object",
                            Value::Array(_) => "array",
                            Value::Bool(_) => "bool",
                            Value::Number(_) => "number",
                            Value::Null => "null",
                            Value::String(_) => "string",
                        },
                        call.fn_arguments
                    )
                };
                log::debug!(
                    "[byop] tool_call_in_args: name={} call_id={} args={}",
                    call.fn_name,
                    call.call_id,
                    args_repr,
                );
            }

            // Waz BYOP todowrite interception: instead of mapping to protobuf executor,
            // synthesize `Message::UpdateTodos` to directly write conversation.todo_lists,
            // triggering chip + popup UI (aligned with server-side
            // ClientAction::AddMessagesToTask::UpdateTodos path).
            // Then append carrier ToolCall + ToolCallResult to unblock the model.
            if call.fn_name == tools::todowrite::TOOL_NAME {
                let args_str = if call.fn_arguments.is_string() {
                    call.fn_arguments.as_str().unwrap_or("").to_owned()
                } else {
                    call.fn_arguments.to_string()
                };

                match tools::todowrite::build_update_todos_messages(
                    &args_str,
                    &current_task_id,
                    &request_id,
                ) {
                    Ok(todo_msgs) if !todo_msgs.is_empty() => {
                        // Directly yield UpdateTodos so the UI updates the chip in real time.
                        // Goes through AddMessagesToTask: the apply_client_action path hits
                        // the Message::UpdateTodos branch → update_todo_list_from_todo_op
                        // → emits BlocklistAIHistoryEvent::UpdatedTodoList; UI refreshes automatically.
                        yield Ok(make_add_messages_event(&current_task_id, todo_msgs));
                        let result_payload =
                            tools::todowrite::success_result_to_json("todo list updated");
                        let result_content = serde_json::to_string(&result_payload)
                            .unwrap_or_else(|_| r#"{"status":"ok"}"#.to_owned());
                        final_messages.push(make_tool_call_carrier_message(
                            &current_task_id,
                            &request_id,
                            &call.call_id,
                            &call.fn_name,
                            &args_str,
                        ));
                        final_messages.push(make_tool_call_result_message(
                            &current_task_id,
                            &request_id,
                            call.call_id.clone(),
                            result_content,
                        ));
                    }
                    Ok(_) => {
                        // Empty todos array: don't emit UpdateTodos, but still need to give
                        // the model a result, otherwise the next chat turn will stall
                        // (model waits for a result matching the tool_call_id).
                        let result_payload = tools::todowrite::success_result_to_json("no todos");
                        let result_content = serde_json::to_string(&result_payload)
                            .unwrap_or_else(|_| r#"{"status":"ok","message":"no todos"}"#.to_owned());
                        final_messages.push(make_tool_call_carrier_message(
                            &current_task_id,
                            &request_id,
                            &call.call_id,
                            &call.fn_name,
                            &args_str,
                        ));
                        final_messages.push(make_tool_call_result_message(
                            &current_task_id,
                            &request_id,
                            call.call_id.clone(),
                            result_content,
                        ));
                    }
                    Err(e) => {
                        // Args parse failed: same as from_args failure, emit error tool_result.
                        log::warn!(
                            "[byop] todowrite args parse failed: call_id={} err={e:#}",
                            call.call_id
                        );
                        let error_payload = tools::todowrite::invalid_arguments_result_to_json(
                            e.to_string(),
                            &args_str,
                        );
                        let error_content = serde_json::to_string(&error_payload)
                            .unwrap_or_else(|_| r#"{"error":"invalid_arguments"}"#.to_owned());
                        final_messages.push(make_tool_call_carrier_message(
                            &current_task_id,
                            &request_id,
                            &call.call_id,
                            &call.fn_name,
                            &args_str,
                        ));
                        final_messages.push(make_tool_call_result_message(
                            &current_task_id,
                            &request_id,
                            call.call_id.clone(),
                            error_content,
                        ));
                    }
                }
                continue;
            }

            // Waz BYOP web tool interception: webfetch / websearch are not mapped to
            // protobuf executor variants; they run local HTTP directly here, synthesizing
            // a (carrier ToolCall, ToolCallResult) message pair, bypassing
            // parse_incoming_tool_call.
            //
            // UI: aligned with cloud mode, emit one `Message::WebSearch` /
            // `Message::WebFetch` status message before and after, triggering inline_action
            // `WebSearchView` / `WebFetchView` rendering: Searching/Fetching loading card →
            // Success (URL list) / Error collapsible card. These two don't go into
            // final_messages but are directly yielded for real-time UI updates;
            // carrier + result still go through final_messages for the next model turn.
            if call.fn_name == tools::webfetch::TOOL_NAME
                || call.fn_name == tools::websearch::TOOL_NAME
            {
                let args_str = if call.fn_arguments.is_string() {
                    call.fn_arguments.as_str().unwrap_or("").to_owned()
                } else {
                    call.fn_arguments.to_string()
                };
                let is_search = call.fn_name == tools::websearch::TOOL_NAME;

                // Pre-parse args to extract query / url for the UI loading card. Even if args
                // parsing fails, we still emit (using empty fields as fallback) to ensure the
                // UI shows at least one loading frame; the subsequent dispatch will still
                // return invalid_arguments → transition to Error card.
                let preview_query = if is_search {
                    serde_json::from_str::<tools::web_runtime::SearchToolArgs>(&args_str)
                        .map(|a| a.query)
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let preview_urls: Vec<String> = if !is_search {
                    serde_json::from_str::<tools::web_runtime::FetchArgs>(&args_str)
                        .map(|a| vec![a.url])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

                // Searching/Fetching loading frame and final Success/Error frame must share
                // the same message.id — `block.rs::handle_web_search_messages` reuses
                // WebSearchView by id; different ids would create two separate cards.
                let web_msg_id = Uuid::new_v4().to_string();
                let mut loading_msg = if is_search {
                    make_web_search_searching_message(
                        &current_task_id,
                        &request_id,
                        preview_query.clone(),
                    )
                } else {
                    make_web_fetch_fetching_message(
                        &current_task_id,
                        &request_id,
                        preview_urls.clone(),
                    )
                };
                loading_msg.id = web_msg_id.clone();
                yield Ok(make_add_messages_event(&current_task_id, vec![loading_msg]));

                let result_json = dispatch_byop_web_tool(&call.fn_name, &args_str).await;

                let mut done_msg = if is_search {
                    make_web_search_status_from_result(
                        &current_task_id,
                        &request_id,
                        &preview_query,
                        &result_json,
                    )
                } else {
                    make_web_fetch_status_from_result(
                        &current_task_id,
                        &request_id,
                        &preview_urls,
                        &result_json,
                    )
                };
                done_msg.id = web_msg_id;
                // The second frame must not use AddMessagesToTask — that would append
                // a second record with the same id to task.messages; `output.rs::WebSearch`
                // rendering branch adds children by message count, resulting in two side-by-side
                // cards. Use UpdateTaskMessage + FieldMask instead: `task::upsert_message`
                // finds the existing message with the same id and applies
                // FieldMaskOperation::update to merge in-place; task.messages still has
                // only one entry → UI shows one card with a set_status transition.
                let mask_path = if is_search { "web_search" } else { "web_fetch" };
                yield Ok(make_update_message_event(
                    &current_task_id,
                    done_msg,
                    vec![mask_path.to_owned()],
                ));

                let result_content = serde_json::to_string(&result_json)
                    .unwrap_or_else(|_| r#"{"status":"serialize_error"}"#.to_owned());
                final_messages.push(make_tool_call_carrier_message(
                    &current_task_id,
                    &request_id,
                    &call.call_id,
                    &call.fn_name,
                    &args_str,
                ));
                final_messages.push(make_tool_call_result_message(
                    &current_task_id,
                    &request_id,
                    call.call_id.clone(),
                    result_content,
                ));
                continue;
            }

            match parse_incoming_tool_call(&call, mcp_context.as_ref(), cwd.as_deref()) {
                Ok(warp_tool) => {
                    // If a placeholder card was already emitted during the ToolCallChunk
                    // phase (same call_id), use update_message to refresh in-place to the
                    // final args (overwriting any late-arriving args delta from chunks).
                    // The placeholder and final frame share the same message.id;
                    // task::upsert_message uses FieldMaskOperation::update;
                    // task.messages still has only one entry → UI refreshes one card
                    // in-place, no duplicate cards.
                    if let Some(msg_id) = tool_msg_ids.get(&call.call_id).cloned() {
                        let mut updated = make_tool_call_message(
                            &current_task_id,
                            &request_id,
                            &call.call_id,
                            warp_tool,
                        );
                        updated.id = msg_id;
                        yield Ok(make_update_message_event(
                            &current_task_id,
                            updated,
                            vec!["tool_call".to_owned()],
                        ));
                    } else {
                        final_messages.push(make_tool_call_message(
                            &current_task_id,
                            &request_id,
                            &call.call_id,
                            warp_tool,
                        ));
                    }
                }
                Err(e) => {
                    // Key: no longer swallow from_args failures as plain text (old impl:
                    // emit AgentOutput). The model believes it called a tool and is waiting
                    // for a result; seeing a Chinese assistant text message, it has no idea
                    // it was an argument type error and cannot correct and retry.
                    // Instead, emit a ToolCall(carrier) + ToolCallResult(error JSON) pair,
                    // so the model sees a standard tool_result error on the next turn and can
                    // conventionally fix args and retry or switch tools.
                    //
                    // The ToolCall's `tool` oneof is left as None (no suitable structured
                    // variant). The original fn_name + args_str are carried via
                    // server_message_data; serialize_outgoing_tool_call's carrier branch
                    // will preferentially restore them.
                    let args_str = if call.fn_arguments.is_string() {
                        call.fn_arguments.as_str().unwrap_or("").to_owned()
                    } else {
                        call.fn_arguments.to_string()
                    };
                    log::warn!(
                        "[byop] tool_call parse failed → emit synthetic error tool_result: \
                         tool={} call_id={} err={e:#}",
                        call.fn_name,
                        call.call_id
                    );
                    let error_payload = serde_json::json!({
                        "error": "invalid_arguments",
                        "detail": e.to_string(),
                        "tool": call.fn_name,
                        "received_args": &args_str,
                        "hint": "Arguments did not match the tool's JSON Schema. \
                                 Re-emit the tool call with corrected types / required fields, \
                                 or pick a different tool.",
                    });
                    let error_content = serde_json::to_string(&error_payload)
                        .unwrap_or_else(|_| r#"{"error":"invalid_arguments"}"#.to_owned());
                    final_messages.push(make_tool_call_carrier_message(
                        &current_task_id,
                        &request_id,
                        &call.call_id,
                        &call.fn_name,
                        &args_str,
                    ));
                    final_messages.push(make_tool_call_result_message(
                        &current_task_id,
                        &request_id,
                        call.call_id.clone(),
                        error_content,
                    ));
                }
            }
        }
        if !final_messages.is_empty() {
            yield Ok(make_add_messages_event(&current_task_id, final_messages));
        }

        // Convert captured token usage into ConversationUsageMetadata.context_window_usage
        // and inject into StreamFinished — controller's handle_response_stream_finished writes
        // it to conversation.conversation_usage_metadata; the footer listens to
        // UpdatedStreamingExchange / AppendedExchange events and refreshes the
        // "X% context remaining" tooltip in real time at the end of each turn.
        let usage_metadata = context_window.and_then(|cw| {
            if cw == 0 || (captured_prompt_tokens == 0 && captured_completion_tokens == 0) {
                return None;
            }
            let used = (captured_prompt_tokens + captured_completion_tokens).max(0) as f32;
            let pct = (used / cw as f32).clamp(0.0, 1.0);
            log::info!(
                "[byop] context usage: prompt={} completion={} window={} → {:.1}%",
                captured_prompt_tokens,
                captured_completion_tokens,
                cw,
                pct * 100.0
            );
            Some(api::response_event::stream_finished::ConversationUsageMetadata {
                context_window_usage: pct,
                summarized: false,
                credits_spent: 0.0,
                #[allow(deprecated)]
                token_usage: Vec::new(),
                tool_usage_metadata: None,
                warp_token_usage: std::collections::HashMap::new(),
                byok_token_usage: std::collections::HashMap::new(),
            })
        });
        yield Ok(make_finished_done(usage_metadata));
    };

    Ok(Box::pin(stream))
}

/// Uses an independent BYOP configuration to send a short non-tool request, having the model
/// generate a conversation title for the first user query.
/// All errors are swallowed (returns Err for upstream to log a warning, without affecting the
/// main flow).
///
/// The implementation delegates to `oneshot::byop_oneshot_streaming_completion`; this function
/// only handles assembling the prompt and sanitizing the output.
///
/// ## Prompt design
///
/// - **system**: see `prompts/tasks/title_system.md`, structured task/rules/examples,
///   covering bilingual (Chinese/English) examples, explicitly forbidding "answering
///   the user's question / refusing / adding quotes".
/// - **user**: wraps the original `user_query` in `<user>...</user>`, prepended with
///   an explicit "Generate a title for this conversation:" to prevent weak models from
///   treating the user text as the primary instruction and directly responding
///   (typical bad case: user="who are you" → model responds "I am Claude" used as title).
/// - **temperature**: 0.3 — opencode title agent uses 0.5; here we're more conservative
///   to reduce off-topic results.
pub(crate) async fn generate_title_via_byop(
    tg: &TitleGenInput,
    user_query: &str,
) -> Result<Option<String>, anyhow::Error> {
    let cfg = super::oneshot::OneshotConfig {
        base_url: tg.base_url.clone(),
        api_key: tg.api_key.clone(),
        model_id: tg.model_id.clone(),
        api_type: tg.api_type,
        reasoning_effort: tg.reasoning_effort,
    };
    let system = include_str!("prompts/tasks/title_system.md");
    let user_prompt = format!(
        "Generate a title for this conversation:\n<user>{}</user>",
        user_query
    );
    let opts = super::oneshot::OneshotOptions {
        max_chars: Some(1000),
        temperature: Some(0.5),
        ..Default::default()
    };
    let raw = super::oneshot::byop_oneshot_completion(&cfg, system, &user_prompt, &opts).await?;
    Ok(sanitize_title(&raw))
}

/// Sanitize title text. Empty string → None (lets upstream skip emit).
///
/// Processing order:
/// 1. Strip `<think>...</think>` / `<reasoning>...</reasoning>` thinking blocks
///    (common prefix from reasoning models).
/// 2. Take the first non-empty line (models often prepend "OK, the title is:" then
///    newline before the actual title).
/// 3. Strip `Title:` / `标题:` / `Thread:` / `Subject:` etc. prefixes (case-insensitive).
/// 4. Strip leading/trailing quotes / backticks (Chinese and English).
/// 5. Remove trailing punctuation.
/// 6. Truncate to 50 characters (by char, protecting CJK); append `…` if exceeded.
fn sanitize_title(raw: &str) -> Option<String> {
    // 1. Strip reasoning tags (may have multiple; DOTALL mode).
    let mut s = raw.to_owned();
    for tag in &["think", "reasoning", "thought", "scratchpad"] {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        while let (Some(start), Some(end_rel)) =
            (s.find(&open), s.find(&close).map(|e| e + close.len()))
        {
            if end_rel <= start {
                break;
            }
            s.replace_range(start..end_rel, "");
        }
    }

    // 2. Take the first non-empty line.
    let first_line = s
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned();
    let mut s = first_line;

    // 3. Strip prefixes (loop to handle double prefixes like "Title: 标题: foo").
    let prefixes = [
        "title:",
        "subject:",
        "thread:",
        "标题:",
        "标题：",
        "主题:",
        "主题：",
    ];
    loop {
        let lower = s.to_lowercase();
        let mut stripped = false;
        for p in &prefixes {
            if lower.starts_with(p) {
                s = s[p.len()..].trim_start().to_owned();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }

    // 4. Strip leading/trailing quotes (Chinese and English).
    let quotes = ['"', '\'', '`', '“', '”', '‘', '’', '《', '》', '「', '」'];
    while let Some(c) = s.chars().next() {
        if quotes.contains(&c) {
            s.remove(0);
        } else {
            break;
        }
    }
    while let Some(c) = s.chars().last() {
        if quotes.contains(&c) {
            let new_len = s.len() - c.len_utf8();
            s.truncate(new_len);
        } else {
            break;
        }
    }

    // 5. Remove trailing punctuation.
    while let Some(c) = s.chars().last() {
        if matches!(
            c,
            '.' | '。' | '!' | '！' | '?' | '？' | ',' | '，' | ';' | '；' | ':' | '：'
        ) {
            let new_len = s.len() - c.len_utf8();
            s.truncate(new_len);
        } else {
            break;
        }
    }

    let s = s.trim().to_owned();
    if s.is_empty() {
        return None;
    }

    // 6. Truncate to 50 characters (by char, protecting CJK). Add ellipsis if too long.
    const MAX_CHARS: usize = 50;
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > MAX_CHARS {
        let mut truncated: String = chars.iter().take(MAX_CHARS - 1).collect();
        truncated.push('…');
        Some(truncated)
    } else {
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// Event construction helpers
// ---------------------------------------------------------------------------

enum AppendKind {
    Reasoning(String),
    Text(String),
}

fn make_add_messages_event(task_id: &str, messages: Vec<api::Message>) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::AddMessagesToTask(
                        api::client_action::AddMessagesToTask {
                            task_id: task_id.to_owned(),
                            messages,
                        },
                    )),
                }],
            },
        )),
    }
}

/// Uses `UpdateTaskMessage` + FieldMask to replace partial fields of an existing message.
/// controller `conversation::Action::UpdateTaskMessage` → `task::upsert_message` →
/// `FieldMaskOperation::update` merges in-place; if the id already exists it won't push
/// a duplicate record.
/// Used for BYOP web tool loading → success/error status transition (see interception branch).
fn make_update_message_event(
    task_id: &str,
    message: api::Message,
    mask_paths: Vec<String>,
) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::UpdateTaskMessage(
                        api::client_action::UpdateTaskMessage {
                            task_id: task_id.to_owned(),
                            message: Some(message),
                            mask: Some(prost_types::FieldMask { paths: mask_paths }),
                        },
                    )),
                }],
            },
        )),
    }
}

fn make_append_event(task_id: &str, message_id: &str, kind: AppendKind) -> api::ResponseEvent {
    let (msg_inner, mask_path) = match kind {
        AppendKind::Reasoning(r) => (
            api::message::Message::AgentReasoning(api::message::AgentReasoning {
                reasoning: r,
                finished_duration: None,
            }),
            "agent_reasoning.reasoning",
        ),
        AppendKind::Text(t) => (
            api::message::Message::AgentOutput(api::message::AgentOutput { text: t }),
            "agent_output.text",
        ),
    };
    let message = api::Message {
        id: message_id.to_owned(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(msg_inner),
        request_id: String::new(),
        timestamp: None,
    };
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::AppendToMessageContent(
                        api::client_action::AppendToMessageContent {
                            task_id: task_id.to_owned(),
                            message: Some(message),
                            mask: Some(prost_types::FieldMask {
                                paths: vec![mask_path.to_owned()],
                            }),
                        },
                    )),
                }],
            },
        )),
    }
}

/// Local dispatcher for BYOP web tools (`webfetch` / `websearch`).
///
/// Does not go through the protobuf executor — directly runs HTTP locally via reqwest,
/// serializing the structured result as a JSON Value for the upstream LLM. Errors are also
/// serialized as `{status:"error", ...}` so the model sees a standard tool_result.
async fn dispatch_byop_web_tool(tool_name: &str, args_str: &str) -> Value {
    use tools::web_runtime;
    // Build an SSRF-protected client for webfetch: custom redirect policy validates each hop target.
    let client = match web_runtime::build_ssrf_safe_client() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[byop] reqwest client build failed: {e:#}");
            return web_runtime::error_to_json(tool_name, &anyhow::anyhow!(e.to_string()));
        }
    };
    if tool_name == tools::webfetch::TOOL_NAME {
        match serde_json::from_str::<web_runtime::FetchArgs>(args_str) {
            Ok(args) => match web_runtime::run_webfetch(&client, args).await {
                Ok(out) => web_runtime::fetch_output_to_json(&out),
                Err(e) => {
                    log::warn!("[byop][webfetch] error: {e:#}");
                    web_runtime::error_to_json(tool_name, &e)
                }
            },
            Err(e) => web_runtime::error_to_json(
                tool_name,
                &anyhow::anyhow!(format!("invalid arguments: {e}")),
            ),
        }
    } else {
        // websearch
        match serde_json::from_str::<web_runtime::SearchToolArgs>(args_str) {
            Ok(args) => {
                let api_key = std::env::var("EXA_API_KEY").ok();
                match web_runtime::run_websearch(&client, args, api_key.as_deref(), None).await {
                    Ok(out) => web_runtime::search_output_to_json(&out),
                    Err(e) => {
                        log::warn!("[byop][websearch] error: {e:#}");
                        web_runtime::error_to_json(tool_name, &e)
                    }
                }
            }
            Err(e) => web_runtime::error_to_json(
                tool_name,
                &anyhow::anyhow!(format!("invalid arguments: {e}")),
            ),
        }
    }
}

fn parse_incoming_tool_call(
    call: &ToolCall,
    mcp_ctx: Option<&crate::ai::agent::MCPContext>,
    cwd: Option<&str>,
) -> anyhow::Result<api::message::tool_call::Tool> {
    // genai ToolCall.fn_arguments is a Value; tools::*'s from_args expects &str,
    // so serialize the Value back to a string before passing (the original protocol is string JSON).
    let args_str = if call.fn_arguments.is_string() {
        call.fn_arguments.as_str().unwrap_or("").to_owned()
    } else {
        call.fn_arguments.to_string()
    };
    if tools::mcp::is_mcp_function(&call.fn_name) {
        return tools::mcp::parse_mcp_tool_call(&call.fn_name, &args_str, mcp_ctx);
    }
    if tools::tmp_ai::is_tmp_function(&call.fn_name) {
        let cwd_val = cwd.unwrap_or("");
        return tools::tmp_ai::parse_tmp_tool_call(&call.fn_name, &args_str, cwd_val)
            .map_err(|e| anyhow::anyhow!("TMP tool call validation failed: {}", e));
    }
    let Some(tool) = tools::lookup(&call.fn_name) else {
        anyhow::bail!("unknown tool name: {}", call.fn_name);
    };
    match (tool.from_args)(&args_str) {
        Ok(t) => Ok(t),
        Err(e) => {
            // First failure: most likely the model serialized bool/number/array as strings.
            // Run a type coerce against the tool's own schema, then retry.
            let schema = (tool.parameters)();
            if let Some(coerced) = tools::coerce::coerce_args_against_schema(&args_str, &schema) {
                match (tool.from_args)(&coerced) {
                    Ok(t) => {
                        log::info!(
                            "[byop] from_args coerced ok: tool={} original_err={e:#}",
                            call.fn_name
                        );
                        return Ok(t);
                    }
                    Err(e2) => {
                        log::warn!(
                            "[byop] from_args failed (after coerce): tool={} err={e2:#} original_err={e:#} coerced_args={coerced} args_str={args_str}",
                            call.fn_name
                        );
                        return Err(e2);
                    }
                }
            }
            // Diagnostics: when parsing fails, print the raw string that from_args received,
            // combined with the upstream [byop] tool_call_in args= line to determine:
            //   1. Whether the model output has wrong types (bool→"true" / number→"1" etc.)
            //   2. Whether genai Value→string conversion has escaping issues
            //   3. Whether fn_arguments as a whole was stringified (should be object but is string)
            log::warn!(
                "[byop] from_args failed: tool={} err={e:#} args_str={args_str}",
                call.fn_name
            );
            Err(e)
        }
    }
}

fn make_reasoning_message(task_id: &str, request_id: &str, reasoning: String) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentReasoning(
            api::message::AgentReasoning {
                reasoning,
                finished_duration: None,
            },
        )),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

fn make_agent_output_message(task_id: &str, request_id: &str, text: String) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput { text },
        )),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

fn make_user_query_message(
    task_id: &str,
    request_id: &str,
    query: String,
    binaries: &[user_context::UserBinary],
) -> api::Message {
    // Waz: write multimodal binary (image / pdf / audio etc.) into `UserQuery.context.images`
    // (InputContext field; the proto Image is actually a `bytes data + string mime_type` generic
    // container, named "images" for historical reasons). UserBinary.data is a base64 string,
    // proto.data is raw bytes, so we decode once here; entries that fail to decode are skipped
    // without blocking the model stream (decode failure means the entry wasn't truly sent
    // upstream this turn anyway — dropping it doesn't affect history consistency).
    let proto_binaries: Vec<api::input_context::Image> = binaries
        .iter()
        .filter_map(|b| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&b.data)
                .ok()
                .map(|bytes| api::input_context::Image {
                    data: bytes,
                    mime_type: b.content_type.clone(),
                })
        })
        .collect();
    let context = if proto_binaries.is_empty() {
        None
    } else {
        Some(api::InputContext {
            images: proto_binaries,
            ..Default::default()
        })
    };
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query,
            context,
            ..Default::default()
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

/// When BYOP intercepts websearch, emit `Message::WebSearch(Searching{query})`; the UI renders
/// a "Searching the web for \"query\"" loading card (`inline_action::web_search`) based on this.
fn make_web_search_searching_message(
    task_id: &str,
    request_id: &str,
    query: String,
) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::WebSearch(api::message::WebSearch {
            status: Some(api::message::web_search::Status {
                r#type: Some(api::message::web_search::status::Type::Searching(
                    api::message::web_search::status::Searching { query },
                )),
            }),
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

/// Extract (url, title) pairs from the exa MCP results string.
///
/// The actual format is a line-based metadata block, with multiple results separated by `---`:
/// ```
/// Title: Announcing Rust 1.95.0 | Rust Blog
/// URL: https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/
/// Published: 2026-04-16T00:00:00.000Z
/// Author: N/A
/// Highlights:
/// ...
/// ---
/// Title: ...
/// ```
/// Scans for `Title: X` and caches the candidate; the first subsequent `URL: Y` is paired as (Y, X) and enqueued, with deduplication.
/// Compatibility fallback: also scans for `[title](url)` markdown link format (in case the exa template switches in the future).
fn extract_search_pages_from_exa_results(s: &str) -> Vec<(String, String)> {
    let mut pages = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Path 1: Title:/URL: line-based format
    let mut current_title: Option<String> = None;
    for line in s.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Title:") {
            current_title = Some(rest.trim().to_owned());
        } else if let Some(rest) = trimmed.strip_prefix("URL:") {
            let url = rest.trim().to_owned();
            let title = current_title.take().unwrap_or_default();
            if (url.starts_with("http://") || url.starts_with("https://"))
                && seen.insert(url.clone())
            {
                pages.push((url, title));
            }
        }
    }

    // Path 2: markdown link `[title](url)` fallback (dedup already in effect, no duplicates)
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(rel_close_text) = s[i + 1..].find("](") {
                let text_end = i + 1 + rel_close_text;
                let url_start = text_end + 2;
                if let Some(rel_close_url) = s[url_start..].find(')') {
                    let url_end = url_start + rel_close_url;
                    let title = s[i + 1..text_end].trim().to_owned();
                    let url = s[url_start..url_end].trim().to_owned();
                    if (url.starts_with("http://") || url.starts_with("https://"))
                        && seen.insert(url.clone())
                    {
                        pages.push((url, title));
                    }
                    i = url_end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    pages
}

/// After BYOP websearch completes, determines Success / Error status based on `result_json`.
///
/// `pages` are extracted by scanning `[title](url)` links from the exa-assembled markdown in `result_json["results"]`.
fn make_web_search_status_from_result(
    task_id: &str,
    request_id: &str,
    query: &str,
    result_json: &Value,
) -> api::Message {
    let is_error = result_json.get("status").and_then(|v| v.as_str()) == Some("error");
    let r#type = if is_error {
        api::message::web_search::status::Type::Error(())
    } else {
        let pages = result_json
            .get("results")
            .and_then(|v| v.as_str())
            .map(extract_search_pages_from_exa_results)
            .unwrap_or_default()
            .into_iter()
            .map(
                |(url, title)| api::message::web_search::status::success::SearchedPage {
                    url,
                    title,
                },
            )
            .collect();
        api::message::web_search::status::Type::Success(api::message::web_search::status::Success {
            query: query.to_owned(),
            pages,
        })
    };
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::WebSearch(api::message::WebSearch {
            status: Some(api::message::web_search::Status {
                r#type: Some(r#type),
            }),
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

/// When BYOP intercepts webfetch, it emits `Message::WebFetch(Fetching{urls})`, and the UI
/// renders a "Fetching N URLs" loading card (`inline_action::web_fetch`) based on this.
fn make_web_fetch_fetching_message(
    task_id: &str,
    request_id: &str,
    urls: Vec<String>,
) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::WebFetch(api::message::WebFetch {
            status: Some(api::message::web_fetch::Status {
                r#type: Some(api::message::web_fetch::status::Type::Fetching(
                    api::message::web_fetch::status::Fetching { urls },
                )),
            }),
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

/// After BYOP webfetch completes, extract `url` + HTTP `status` from the `FetchOutput` JSON
/// to build a Success card; if status="error", use an Error card instead.
fn make_web_fetch_status_from_result(
    task_id: &str,
    request_id: &str,
    fallback_urls: &[String],
    result_json: &Value,
) -> api::Message {
    let is_error = result_json.get("status").and_then(|v| v.as_str()) == Some("error");
    let r#type = if is_error {
        api::message::web_fetch::status::Type::Error(())
    } else {
        let url = result_json
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned())
            .unwrap_or_else(|| fallback_urls.first().cloned().unwrap_or_default());
        // FetchOutput.status is an HTTP status code; 2xx counts as success.
        let success = result_json
            .get("status")
            .and_then(|v| v.as_u64())
            .map(|c| (200..300).contains(&c))
            .unwrap_or(true);
        api::message::web_fetch::status::Type::Success(api::message::web_fetch::status::Success {
            pages: vec![api::message::web_fetch::status::success::FetchedPage {
                url,
                title: String::new(),
                success,
            }],
        })
    };
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::WebFetch(api::message::WebFetch {
            status: Some(api::message::web_fetch::Status {
                r#type: Some(r#type),
            }),
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

fn make_tool_call_result_message(
    task_id: &str,
    request_id: &str,
    tool_call_id: String,
    content: String,
) -> api::Message {
    // ToolCallResult persistence: the warp protobuf `tool_call_result.result` oneof only
    // has structured variants (RunShellCommand / Grep / ReadFiles / ...) with no generic
    // string fallback variant. BYOP already serializes the result to a JSON string in
    // chat_stream, so there's no need to use warp's structured protocol — we store the
    // string directly in the `server_message_data` free-form string field and leave the
    // `result` oneof as None. The next round of build_chat_request must special-case the
    // `Message::ToolCallResult` branch: when result=None, read content from
    // server_message_data (otherwise use tools::serialize_result to deserialize the
    // structured variant).
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: content,
        citations: vec![],
        message: Some(api::message::Message::ToolCallResult(
            api::message::ToolCallResult {
                tool_call_id,
                context: None,
                result: None,
            },
        )),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

/// When BYOP `from_args` parsing fails, emit a placeholder ToolCall as a carrier:
/// the `tool` oneof is left as None (no suitable structured variant), and the original
/// fn_name + args_str are encoded into `server_message_data` as `<fn_name>\n<args_str>`.
/// In the next build_chat_request round, the carrier branch of
/// `serialize_outgoing_tool_call` restores them, ensuring the upstream model sees the
/// same tool_use name / args as the original call (otherwise using
/// "warp_internal_empty" as a placeholder would confuse the model and wouldn't match
/// the immediately following ToolCallResult error context).
fn make_tool_call_carrier_message(
    task_id: &str,
    request_id: &str,
    tool_call_id: &str,
    fn_name: &str,
    args_str: &str,
) -> api::Message {
    let carrier = format!("{}\n{}", fn_name, args_str);
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: carrier,
        citations: vec![],
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: tool_call_id.to_owned(),
            tool: None,
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

fn make_tool_call_message(
    task_id: &str,
    request_id: &str,
    tool_call_id: &str,
    tool: api::message::tool_call::Tool,
) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: tool_call_id.to_owned(),
            tool: Some(tool),
        })),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

fn create_task_event(task_id: &str) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::CreateTask(
                        api::client_action::CreateTask {
                            task: Some(api::Task {
                                id: task_id.to_owned(),
                                description: String::new(),
                                dependencies: None,
                                messages: vec![],
                                summary: String::new(),
                                server_data: String::new(),
                            }),
                        },
                    )),
                }],
            },
        )),
    }
}

/// Construct a `Action::CreateTask` representing a new subtask, with `dependencies.parent_task_id`.
/// When the conversation sees `task.parent_id()` is non-empty in `apply_client_action::CreateTask`,
/// it takes the `Task::new_subtask` path: finds the matching subagent tool_call in parent.messages,
/// extracts `SubagentParams` and attaches it to the subtask, then emits
/// `BlocklistAIHistoryEvent::CreatedSubtask`. The LRC tag-in popup spawn chain depends on this event.
fn create_subtask_event(subtask_id: &str, parent_task_id: &str) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(api::client_action::Action::CreateTask(
                        api::client_action::CreateTask {
                            task: Some(api::Task {
                                id: subtask_id.to_owned(),
                                description: String::new(),
                                dependencies: Some(api::task::Dependencies {
                                    parent_task_id: parent_task_id.to_owned(),
                                }),
                                messages: vec![],
                                summary: String::new(),
                                server_data: String::new(),
                            }),
                        },
                    )),
                }],
            },
        )),
    }
}

fn make_finished_done(
    usage_metadata: Option<api::response_event::stream_finished::ConversationUsageMetadata>,
) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::Finished(
            api::response_event::StreamFinished {
                reason: Some(api::response_event::stream_finished::Reason::Done(
                    api::response_event::stream_finished::Done {},
                )),
                conversation_usage_metadata: usage_metadata,
                token_usage: vec![],
                should_refresh_model_config: false,
                request_cost: None,
            },
        )),
    }
}

#[cfg(test)]
mod assistant_buffer_tests {
    use super::*;
    use genai::chat::{ChatRole, ToolCall};

    fn reasoning_part(msg: &ChatMessage) -> Option<&str> {
        for p in msg.content.parts() {
            if let ContentPart::ReasoningContent(r) = p {
                return Some(r.as_str());
            }
        }
        None
    }

    /// gate=false + real reasoning → **dropped** (fix for zerx-lab/warp #25).
    /// Cerebras / Groq / OpenRouter and other OpenAI-strict providers return 400 on seeing this field.
    #[test]
    fn no_echo_drops_real_reasoning_text() {
        let mut buf = AssistantBuffer::new(false);
        buf.text = Some("Hi".to_string());
        buf.reasoning = Some("internal thought".to_string());
        let mut msgs = Vec::new();
        buf.flush_into(&mut msgs);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, ChatRole::Assistant);
        assert!(
            reasoning_part(&msgs[0]).is_none(),
            "must not echo reasoning"
        );
    }

    /// gate=false + tool_calls + real reasoning → the tool_calls message also must not carry reasoning.
    #[test]
    fn no_echo_drops_reasoning_on_tool_calls_message() {
        let mut buf = AssistantBuffer::new(false);
        buf.text = Some("calling".to_string());
        buf.tool_calls = vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }];
        buf.reasoning = Some("planning".to_string());
        let mut msgs = Vec::new();
        buf.flush_into(&mut msgs);
        assert_eq!(msgs.len(), 2, "text + tool_calls flushed as two separate messages");
        for m in &msgs {
            assert!(
                reasoning_part(m).is_none(),
                "any-msg reasoning must be absent"
            );
        }
    }

    /// gate=true + real reasoning → attach the real value (DeepSeek / Kimi path).
    #[test]
    fn echo_keeps_real_reasoning() {
        let mut buf = AssistantBuffer::new(true);
        buf.text = Some("ok".to_string());
        buf.reasoning = Some("thinking...".to_string());
        let mut msgs = Vec::new();
        buf.flush_into(&mut msgs);
        assert_eq!(msgs.len(), 1);
        assert_eq!(reasoning_part(&msgs[0]), Some("thinking..."));
    }

    /// gate=true + no reasoning → attach a placeholder (to satisfy the "field must be present" validation).
    #[test]
    fn echo_inserts_placeholder_when_empty() {
        let mut buf = AssistantBuffer::new(true);
        buf.text = Some("ok".to_string());
        buf.reasoning = None;
        let mut msgs = Vec::new();
        buf.flush_into(&mut msgs);
        assert_eq!(msgs.len(), 1);
        assert_eq!(reasoning_part(&msgs[0]), Some(REASONING_ECHO_PLACEHOLDER));
    }

    /// gate=true + tool_calls + real reasoning → the text message gets a placeholder,
    /// the tool_calls message gets the real reasoning value.
    #[test]
    fn echo_with_tool_calls_splits_correctly() {
        let mut buf = AssistantBuffer::new(true);
        buf.text = Some("calling".to_string());
        buf.tool_calls = vec![ToolCall {
            call_id: "c1".to_string(),
            fn_name: "echo".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }];
        buf.reasoning = Some("plan".to_string());
        let mut msgs = Vec::new();
        buf.flush_into(&mut msgs);
        assert_eq!(msgs.len(), 2);
        // text message: placeholder
        assert_eq!(reasoning_part(&msgs[0]), Some(REASONING_ECHO_PLACEHOLDER));
        // tool_calls message: real reasoning + contains ToolCall part
        assert_eq!(reasoning_part(&msgs[1]), Some("plan"));
        assert!(
            !msgs[1].content.tool_calls().is_empty(),
            "second message must carry tool_calls"
        );
    }
}

#[cfg(test)]
mod dashscope_thinking_tests {
    use super::*;
    use crate::settings::ReasoningEffortSetting as R;

    const DASHSCOPE_CN: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1/";
    const DASHSCOPE_INTL: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/";

    #[test]
    fn dashscope_qwen3_triggers() {
        assert!(dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "qwen3-235b-a22b",
            R::High
        ));
    }

    #[test]
    fn dashscope_qwq_triggers() {
        assert!(dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_INTL,
            "qwq-32b",
            R::Medium
        ));
    }

    #[test]
    fn dashscope_deepseek_r1_triggers() {
        assert!(dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "deepseek-r1",
            R::High
        ));
    }

    #[test]
    fn dashscope_kimi_k2_thinking_excluded() {
        // opencode note: kimi-k2-thinking has thinking enabled by default, no need to inject again
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "kimi-k2-thinking",
            R::High
        ));
    }

    #[test]
    fn dashscope_off_setting_skips() {
        // Respect the user's explicit choice to disable thinking
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "qwen3-30b",
            R::Off
        ));
    }

    #[test]
    fn dashscope_non_reasoning_model_skips() {
        // Pure chat models like qwen-turbo / qwen2.5 should not be injected
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "qwen-turbo",
            R::High
        ));
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            DASHSCOPE_CN,
            "qwen2.5-72b",
            R::High
        ));
    }

    #[test]
    fn non_dashscope_url_skips() {
        // OpenAI / Cerebras / Groq etc. base_urls that are not DashScope
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            "https://api.openai.com/v1/",
            "qwen3-30b",
            R::High
        ));
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::OpenAi,
            "https://api.cerebras.ai/v1/",
            "qwen3-30b",
            R::High
        ));
    }

    #[test]
    fn non_openai_api_type_skips() {
        // Anthropic / Gemini / DeepSeek api_type does not go through this path
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::Anthropic,
            DASHSCOPE_CN,
            "qwen3-30b",
            R::High
        ));
        assert!(!dashscope_needs_enable_thinking(
            AgentProviderApiType::DeepSeek,
            DASHSCOPE_CN,
            "deepseek-r1",
            R::High
        ));
    }
}

/// Regression tests for "reasoning depth level dispatch" in `build_chat_options`.
///
/// Aligns with Zed `LanguageModelRequest::thinking_allowed=false` handling across providers:
/// **When Off, no provider should allow server-side thinking**. The specific strategy varies:
/// - Anthropic / Gemini: do not send the thinking field (skip `with_reasoning_effort`)
/// - DeepSeek: `extra_body.thinking.type=disabled` (server enables by default, must explicitly disable)
/// - OpenAI / OpenAiResp: `reasoning_effort: "none"` (GPT-5 accepts this)
#[cfg(test)]
mod build_chat_options_off_tests {
    use super::*;
    use crate::settings::ReasoningEffortSetting as R;
    use genai::chat::ReasoningEffort as GE;

    fn opts(api_type: AgentProviderApiType, model: &str, effort: R) -> genai::chat::ChatOptions {
        build_chat_options(
            api_type,
            "https://example.com/v1/",
            model,
            effort,
            vec![],
            None,
        )
    }

    /// `claude-sonnet-4-6` (hits `SUPPORT_ADAPTTIVE_THINK_MODELS`) + Off must **completely
    /// omit** `reasoning_effort`, otherwise the vendor genai adapter will unconditionally
    /// insert `thinking:{type:adaptive}` (`adapter_impl.rs:121-135`).
    #[test]
    fn anthropic_sonnet_4_6_off_skips_reasoning_effort() {
        let o = opts(AgentProviderApiType::Anthropic, "claude-sonnet-4-6", R::Off);
        assert!(
            o.reasoning_effort.is_none(),
            "Anthropic+Off must not pass reasoning_effort to avoid 4.6 forcing adaptive thinking"
        );
        assert!(
            o.extra_body.is_none(),
            "Anthropic+Off should also not inject extra_body"
        );
    }

    /// `claude-opus-4-6` same as above (double-hit on SUPPORT_EFFORT + SUPPORT_ADAPTIVE).
    #[test]
    fn anthropic_opus_4_6_off_skips_reasoning_effort() {
        let o = opts(AgentProviderApiType::Anthropic, "claude-opus-4-6", R::Off);
        assert!(o.reasoning_effort.is_none());
        assert!(o.extra_body.is_none());
    }

    /// `claude-opus-4-7+` + Off: even though it's not in the adaptive list (already OK),
    /// it should still consistently skip.
    #[test]
    fn anthropic_opus_4_7_off_skips_reasoning_effort() {
        let o = opts(AgentProviderApiType::Anthropic, "claude-opus-4-7", R::Off);
        assert!(o.reasoning_effort.is_none());
        assert!(o.extra_body.is_none());
    }

    /// Anthropic + High still goes through the original reasoning_effort path.
    #[test]
    fn anthropic_high_injects_reasoning_effort() {
        let o = opts(AgentProviderApiType::Anthropic, "claude-opus-4-7", R::High);
        assert!(matches!(o.reasoning_effort, Some(GE::High)));
    }

    /// Anthropic + Auto does not send any parameters.
    #[test]
    fn anthropic_auto_skips() {
        let o = opts(AgentProviderApiType::Anthropic, "claude-opus-4-7", R::Auto);
        assert!(o.reasoning_effort.is_none());
    }

    /// Gemini + Off: do not send thinkingConfig.
    #[test]
    fn gemini_off_skips_reasoning_effort() {
        let o = opts(AgentProviderApiType::Gemini, "gemini-2.5-pro", R::Off);
        assert!(o.reasoning_effort.is_none());
        assert!(o.extra_body.is_none());
    }

    /// Gemini + Medium goes through the thinkingBudget path.
    #[test]
    fn gemini_medium_injects_reasoning_effort() {
        let o = opts(AgentProviderApiType::Gemini, "gemini-2.5-pro", R::Medium);
        assert!(matches!(o.reasoning_effort, Some(GE::Medium)));
    }

    /// DeepSeek + Off: must send `extra_body.thinking.type=disabled`,
    /// and **must not** use reasoning_effort=none (server returns 400 unknown variant).
    #[test]
    fn deepseek_off_uses_extra_body_disabled() {
        let o = opts(AgentProviderApiType::DeepSeek, "deepseek-v4-flash", R::Off);
        assert!(
            o.reasoning_effort.is_none(),
            "DeepSeek+Off cannot use reasoning_effort=none"
        );
        let body = o.extra_body.as_ref().expect("extra_body must be set");
        assert_eq!(
            body.pointer("/thinking/type"),
            Some(&serde_json::Value::String("disabled".to_string())),
            "DeepSeek+Off must send thinking.type=disabled"
        );
    }

    /// DeepSeek + High goes through the top-level reasoning_effort field.
    #[test]
    fn deepseek_high_injects_reasoning_effort() {
        let o = opts(AgentProviderApiType::DeepSeek, "deepseek-reasoner", R::High);
        assert!(matches!(o.reasoning_effort, Some(GE::High)));
        assert!(o.extra_body.is_none());
    }

    /// OpenAI (GPT-5) + Off: uses reasoning_effort=none (GPT-5 accepts the `none` level).
    #[test]
    fn openai_gpt5_off_uses_reasoning_effort_none() {
        let o = opts(AgentProviderApiType::OpenAi, "gpt-5", R::Off);
        assert!(
            matches!(o.reasoning_effort, Some(GE::None)),
            "OpenAI+GPT-5+Off should send reasoning_effort=none"
        );
    }

    /// A model that doesn't support reasoning + any non-Auto level: skip (to avoid upstream 400).
    #[test]
    fn anthropic_haiku_3_5_off_skips() {
        let o = opts(
            AgentProviderApiType::Anthropic,
            "claude-3-5-haiku-20241022",
            R::Off,
        );
        assert!(o.reasoning_effort.is_none());
        assert!(o.extra_body.is_none());
    }

    #[test]
    fn openai_gpt4o_off_skips() {
        // gpt-4o is not in the reasoning model list, Off also skips
        let o = opts(AgentProviderApiType::OpenAi, "gpt-4o", R::Off);
        assert!(o.reasoning_effort.is_none());
    }
}

/// **End-to-end cache boundary stability tests**: verify the "byte-level prefix
/// consistency" guarantee required by prompt cache under multi-turn conversation simulation.
/// These tests do not call upstream APIs; they only check the determinism of
/// `apply_caching_anthropic` and `build_chat_options` outputs.
///
/// This is the **minimum bar** for cache hits: if the same input produces inconsistent
/// output across calls, the upstream hash will inevitably differ → 100% miss.
/// Conversely, consistent output does not guarantee a hit.
#[cfg(test)]
mod cache_boundary_stability_tests {
    use super::*;
    use genai::chat::{ChatMessage, ChatRole};

    /// Build a typical multi-turn conversation messages sequence:
    /// system + user_1 + assistant_1 + user_2 + assistant_2 + user_3
    /// (ends with user, consistent with `ensure_ends_with_user` output).
    fn build_three_turn_conversation() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(
                "You are a helpful coding assistant for Waz BYOP.\n\
                 Guidelines: be concise, prefer code over prose.",
            ),
            ChatMessage::user("What is rust borrow checker?"),
            ChatMessage::assistant("It enforces ownership rules at compile time."),
            ChatMessage::user("Show me a code example"),
            ChatMessage::assistant("```rust\nfn main() { let s = String::new(); }\n```"),
            ChatMessage::user("Explain the lifetime in that code"),
        ]
    }

    fn extract_cache_control(msg: &ChatMessage) -> Option<CacheControl> {
        // ChatMessage's cache_control is on `options.cache_control`.
        msg.options.as_ref().and_then(|o| o.cache_control.clone())
    }

    fn cache_signature(msgs: &[ChatMessage]) -> Vec<(usize, ChatRole, Option<CacheControl>)> {
        msgs.iter()
            .enumerate()
            .map(|(i, m)| (i, m.role.clone(), extract_cache_control(m)))
            .collect()
    }

    /// **P0-4 primary acceptance**: apply_caching_anthropic must produce byte-equal
    /// cache marker positions and TTLs across repeated calls with the same input.
    #[test]
    fn apply_caching_anthropic_is_deterministic() {
        let mut a = build_three_turn_conversation();
        let mut b = build_three_turn_conversation();
        apply_caching_anthropic(&mut a);
        apply_caching_anthropic(&mut b);
        assert_eq!(
            cache_signature(&a),
            cache_signature(&b),
            "Same input × multiple calls cache signature must be consistent"
        );
    }

    /// **TTL mixed strategy acceptance**: system uses 1h (static prefix), non-system uses 5m
    /// (conversation tail). The ordering system(1h) → messages(5m) satisfies Anthropic's
    /// sort constraint and is immune to external 5m injection.
    #[test]
    fn anthropic_cache_uses_mixed_ttl() {
        let mut msgs = build_three_turn_conversation();
        apply_caching_anthropic(&mut msgs);
        let tagged: Vec<_> = msgs
            .iter()
            .filter(|m| extract_cache_control(m).is_some())
            .collect();
        assert!(!tagged.is_empty(), "Must tag at least one breakpoint");
        for m in &tagged {
            let cc = extract_cache_control(m).unwrap();
            let expected = if matches!(m.role, ChatRole::System) {
                CacheControl::Ephemeral1h
            } else {
                CacheControl::Ephemeral5m
            };
            assert_eq!(cc, expected, "TTL for role={:?} does not match expected", m.role);
        }
    }

    /// **P0-4 coverage acceptance**: opencode approach: first 2 system + last 2 non-system.
    /// A multi-turn conversation (1 system + 5 non-system) should have 1+2=3 markers.
    #[test]
    fn anthropic_marks_first_2_system_and_last_2_non_system() {
        let mut msgs = build_three_turn_conversation();
        apply_caching_anthropic(&mut msgs);
        let tagged_indices: Vec<usize> = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| extract_cache_control(m).is_some())
            .map(|(i, _)| i)
            .collect();
        // Verify that system(idx=0) and the last 2 non-system(idx=4, idx=5) are all marked.
        assert!(tagged_indices.contains(&0), "First system is not marked");
        assert!(tagged_indices.contains(&4), "Second to last is not marked");
        assert!(tagged_indices.contains(&5), "Last is not marked");
        assert_eq!(
            tagged_indices.len(),
            3,
            "Total 3 breakpoints (1 system + 2 tail)"
        );
    }

    /// **Simulate cache prefix stability across multi-turn conversations**:
    /// turn N's messages = turn N-1's messages + (N-1 round assistant) + (new user).
    /// Cache markers at the beginning (system + intermediate rounds) should not drift
    /// as the conversation grows.
    #[test]
    fn cache_marks_stable_as_conversation_grows() {
        // turn 1
        let mut t1 = vec![ChatMessage::system("sys"), ChatMessage::user("q1")];
        apply_caching_anthropic(&mut t1);
        let sys_t1_cc = extract_cache_control(&t1[0]);

        // turn 2: add assistant_1 + user_2
        let mut t2 = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("q1"),
            ChatMessage::assistant("a1"),
            ChatMessage::user("q2"),
        ];
        apply_caching_anthropic(&mut t2);
        let sys_t2_cc = extract_cache_control(&t2[0]);

        // The first system's cache_control is consistent across turns → upstream hash
        // remains unchanged → subsequent turns will hit the cache.
        assert_eq!(
            sys_t1_cc, sys_t2_cc,
            "First system breakpoint's TTL/position across turns should be consistent"
        );
        // turn 1's user is marked (it's the tail), but turn 2 is no longer marked.
        assert!(extract_cache_control(&t1[1]).is_some());
        assert!(
            extract_cache_control(&t2[1]).is_none(),
            "turn 2's old user is no longer tail"
        );
    }

    /// **build_chat_options output determinism** (same input across calls yields
    /// identical results). The minimum bar for prompt cache hits — bit-level hash consistency.
    #[test]
    fn openai_chat_options_is_deterministic() {
        use crate::settings::ReasoningEffortSetting as R;
        let make = || {
            build_chat_options(
                AgentProviderApiType::OpenAi,
                "https://api.openai.com/v1/",
                "gpt-5-mini",
                R::Auto,
                vec![],
                Some("conv-abc-123"),
            )
        };
        let a = make();
        let b = make();
        assert_eq!(a.prompt_cache_key, b.prompt_cache_key);
        assert_eq!(a.cache_control, b.cache_control);
    }

    /// **opencode-compatible whitelisted providers** (api.openai.com / *.openai.azure.com /
    /// openrouter.ai / api.venice.ai / opencode.ai/zen) → emit prompt_cache_key,
    /// and **never emit cache_control** (corresponds to the prompt_cache_retention field;
    /// the opencode repo does not use this field).
    #[test]
    fn whitelisted_provider_emits_prompt_cache_key_only() {
        use crate::settings::ReasoningEffortSetting as R;
        // 5 representative whitelisted URLs, each covering the api_type=OpenAi branch.
        let whitelisted = [
            "https://api.openai.com/v1/",
            "https://my-resource.openai.azure.com/openai/v1/",
            "https://openrouter.ai/api/v1/",
            "https://api.venice.ai/api/v1/",
            "https://opencode.ai/zen/v1/",
        ];
        for url in whitelisted {
            let opts = build_chat_options(
                AgentProviderApiType::OpenAi,
                url,
                "gpt-5-mini",
                R::Auto,
                vec![],
                Some("conv-1"),
            );
            assert_eq!(
                opts.prompt_cache_key.as_deref(),
                Some("conv-1"),
                "{url}: Whitelisted provider should pass prompt_cache_key=conversation_id"
            );
            assert!(
                opts.cache_control.is_none(),
                "{url}: cache_control is never sent (opencode does not use prompt_cache_retention)"
            );
        }
    }

    /// **#126 regression**: OpenAi api_type but base_url not on the whitelist (OpenCode Go
    /// relay for Kimi / GLM, vLLM, lm-studio, DashScope, Moonshot, Zhipu native, etc.)
    /// → emit neither cache_control nor prompt_cache_key.
    ///
    /// Aligns with opencode `options()` function: apart from the 5 providerIDs
    /// (openai/azure/openrouter/venice/opencode), no provider sets promptCacheKey.
    #[test]
    fn non_whitelisted_provider_emits_nothing() {
        use crate::settings::ReasoningEffortSetting as R;
        // Examples from issue #126 body + user follow-up comments, plus other mainstream
        // OpenAI-compatible relays. No URL should emit any cache field.
        let byop_urls = [
            ("https://opencode.go/v1/", "kimi-k2.6"),
            ("https://opencode.go/v1/", "glm-5.1"),
            ("https://api.moonshot.cn/v1/", "kimi-k2"),
            ("https://open.bigmodel.cn/api/paas/v4/", "glm-4.6"),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1/",
                "qwen-max",
            ),
            ("http://localhost:1234/v1/", "local-model"),
        ];
        for (url, model) in byop_urls {
            let opts = build_chat_options(
                AgentProviderApiType::OpenAi,
                url,
                model,
                R::Auto,
                vec![],
                Some("conv-byop"),
            );
            assert!(
                opts.cache_control.is_none(),
                "{url}: Non-whitelisted should not pass cache_control"
            );
            assert!(
                opts.prompt_cache_key.is_none(),
                "{url}: Non-whitelisted should not pass prompt_cache_key"
            );
        }
    }

    /// OpenAiResp api_type follows the same decision logic (genai openai_resp adapter
    /// serializes the same fields): emit for whitelisted / suppress for non-whitelisted.
    #[test]
    fn openai_resp_follows_same_whitelist() {
        use crate::settings::ReasoningEffortSetting as R;
        let on_whitelist = build_chat_options(
            AgentProviderApiType::OpenAiResp,
            "https://api.openai.com/v1/",
            "gpt-5",
            R::Auto,
            vec![],
            Some("conv-resp"),
        );
        assert_eq!(on_whitelist.prompt_cache_key.as_deref(), Some("conv-resp"));
        assert!(on_whitelist.cache_control.is_none());

        let off_whitelist = build_chat_options(
            AgentProviderApiType::OpenAiResp,
            "https://custom.relay/v1/",
            "gpt-5",
            R::Auto,
            vec![],
            Some("conv-resp"),
        );
        assert!(off_whitelist.prompt_cache_key.is_none());
        assert!(off_whitelist.cache_control.is_none());
    }

    /// **Empty conversation_id skips prompt_cache_key** (avoid cross-session routing mistakes).
    #[test]
    fn openai_empty_conversation_id_skips_cache_key() {
        use crate::settings::ReasoningEffortSetting as R;
        let opts = build_chat_options(
            AgentProviderApiType::OpenAi,
            "https://api.openai.com/v1/",
            "gpt-5",
            R::Auto,
            vec![],
            Some(""),
        );
        assert!(
            opts.prompt_cache_key.is_none(),
            "Empty conversation_id should skip prompt_cache_key"
        );
        assert!(opts.cache_control.is_none(), "cache_control is never sent");
    }

    /// **Anthropic path: build_chat_options does not emit cache_control**
    /// (Anthropic uses per-message caching, not ChatOptions-level).
    #[test]
    fn anthropic_chat_options_no_cache_control() {
        use crate::settings::ReasoningEffortSetting as R;
        let opts = build_chat_options(
            AgentProviderApiType::Anthropic,
            "https://api.anthropic.com/v1/",
            "claude-opus-4-7",
            R::Auto,
            vec![],
            Some("conv-3"),
        );
        assert!(
            opts.cache_control.is_none(),
            "Anthropic's ChatOptions must not contain cache_control (uses per-message)"
        );
        assert!(
            opts.prompt_cache_key.is_none(),
            "Anthropic does not use prompt_cache_key"
        );
    }

    /// **DeepSeek / Gemini / Ollama use server-side implicit caching; do not emit cache_control**.
    #[test]
    fn implicit_cache_providers_no_cache_control() {
        use crate::settings::ReasoningEffortSetting as R;
        for api in [
            AgentProviderApiType::DeepSeek,
            AgentProviderApiType::Gemini,
            AgentProviderApiType::Ollama,
        ] {
            let opts = build_chat_options(
                api,
                "https://example.com/v1/",
                "some-model",
                R::Auto,
                vec![],
                Some("conv"),
            );
            assert!(
                opts.cache_control.is_none(),
                "{api:?} should not pass cache_control"
            );
        }
    }
}

#[cfg(test)]
mod serializer_readiness_tests {
    use super::*;
    use crate::ai::agent::task::TaskId;
    use crate::ai::agent::{AIAgentActionId, AIAgentActionResultType, RequestCommandOutputResult};
    use crate::ai::byop_compaction::state::{CompactionState, CompletedCompaction};
    use crate::ai::byop_readiness::{
        PendingByopToolResultsError, RepairRecord, RepairState, ToolCallKey, ToolCallRef,
        BLOCKED_BYOP_REQUEST_MESSAGE,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn kind() -> RedactedToolKind {
        RedactedToolKind::new("shell")
    }

    fn assistant_calls(task_id: &str, assistant_id: &str, call_ids: &[&str]) -> ProjectionItem {
        ProjectionItem::assistant_tool_calls(
            task_id,
            assistant_id,
            call_ids
                .iter()
                .map(|call_id| ProjectedToolCall::new(task_id, assistant_id, *call_id, kind()))
                .collect(),
        )
    }

    fn result(
        task_id: &str,
        message_id: &str,
        assistant_id: Option<&str>,
        call_id: &str,
        source: ToolResultSource,
    ) -> ProjectionItem {
        ProjectionItem::tool_result(ProjectedToolResult::new(
            task_id,
            message_id,
            assistant_id.map(str::to_owned),
            call_id,
            kind(),
            source,
            TerminalResultKind::Real,
        ))
    }

    fn assert_blocked_category(projection: Vec<ProjectionItem>, category: &str) {
        let error = validate_serializer_readiness_projection(projection).unwrap_err();
        assert!(
            error.to_string().contains(BLOCKED_BYOP_REQUEST_MESSAGE),
            "error should use blocked-request copy for {category}: {error}"
        );
    }

    fn shell_tool() -> api::message::tool_call::Tool {
        use api::message::tool_call::run_shell_command::WaitUntilCompleteValue;

        api::message::tool_call::Tool::RunShellCommand(api::message::tool_call::RunShellCommand {
            command: "echo hi".to_owned(),
            is_read_only: true,
            uses_pager: false,
            is_risky: false,
            citations: vec![],
            wait_until_complete_value: Some(WaitUntilCompleteValue::WaitUntilComplete(true)),
            risk_category: 0,
        })
    }

    fn subagent_tool() -> api::message::tool_call::Tool {
        api::message::tool_call::Tool::Subagent(api::message::tool_call::Subagent {
            task_id: "subtask-1".to_owned(),
            payload: String::new(),
            metadata: Some(api::message::tool_call::subagent::Metadata::Cli(
                api::message::tool_call::subagent::CliSubagent {
                    command_id: "command-1".to_owned(),
                },
            )),
        })
    }

    fn task_with_messages(messages: Vec<api::Message>) -> api::Task {
        api::Task {
            id: "task-1".to_owned(),
            messages,
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }
    }

    fn user_query_input(query: &str) -> AIAgentInput {
        AIAgentInput::UserQuery {
            query: query.to_owned(),
            context: Arc::<[AIAgentContext]>::from([]),
            static_query_type: None,
            referenced_attachments: HashMap::new(),
            user_query_mode: UserQueryMode::default(),
            running_command: None,
            intended_agent: None,
        }
    }

    fn cancelled_action_result_input(call_id: &str) -> AIAgentInput {
        AIAgentInput::ActionResult {
            result: AIAgentActionResult {
                id: AIAgentActionId::from(call_id.to_owned()),
                task_id: TaskId::new("task-1".to_owned()),
                result: AIAgentActionResultType::RequestCommandOutput(
                    RequestCommandOutputResult::CancelledBeforeExecution,
                ),
            },
            context: Arc::<[AIAgentContext]>::from([]),
        }
    }

    fn request_params(messages: Vec<api::Message>, input: Vec<AIAgentInput>) -> RequestParams {
        RequestParams::new_for_test(input, vec![task_with_messages(messages)])
    }

    fn request_params_with_repair(
        messages: Vec<api::Message>,
        input: Vec<AIAgentInput>,
        repair_state: RepairState,
    ) -> RequestParams {
        let mut params = request_params(messages, input);
        params.byop_repair_state = RepairStateStatus::Valid(repair_state);
        params
    }

    fn build_openai_request(params: &RequestParams) -> Result<ChatRequest, ConvertToAPITypeError> {
        build_chat_request(params, false, AgentProviderApiType::OpenAi, "test-model")
    }

    fn assert_request_has_no_repair_placeholder(request: &ChatRequest) {
        assert!(
            !request.messages.iter().any(|message| {
                message
                    .content
                    .tool_responses()
                    .iter()
                    .any(|response| is_placeholder_tool_response_content(&response.content))
            }),
            "normal request body must not emit placeholder tool results"
        );
    }

    fn tool_response_contents(request: &ChatRequest, call_id: &str) -> Vec<String> {
        request
            .messages
            .iter()
            .flat_map(|message| message.content.tool_responses())
            .filter(|response| response.call_id == call_id)
            .map(|response| response.content.clone())
            .collect()
    }

    fn assert_build_request_blocked(params: RequestParams, category: &str) {
        let error = build_openai_request(&params).unwrap_err();
        assert!(
            error.to_string().contains(BLOCKED_BYOP_REQUEST_MESSAGE),
            "error should use blocked-request copy for {category}: {error}"
        );
    }

    #[test]
    fn placeholder_then_cancellation_regression_blocks_waits_or_sends_real_cancellation() {
        let user_message = make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]);
        let tool_call_message = make_tool_call_message("task-1", "req-1", "call-1", shell_tool());
        let assistant_message_id = tool_call_message.id.clone();

        assert_build_request_blocked(
            request_params(
                vec![user_message.clone(), tool_call_message.clone()],
                vec![user_query_input("continue")],
            ),
            "MissingResultWithoutRepairSource",
        );

        let pending_report = classify_byop_controller_readiness_with_live_tool_calls(
            &request_params(
                vec![user_message.clone(), tool_call_message.clone()],
                vec![user_query_input("continue")],
            ),
            vec![LiveToolCall::new(
                ToolCallRef::new(
                    ToolCallKey::new("task-1", assistant_message_id, "call-1"),
                    kind(),
                ),
                LiveToolCallState::Running,
            )],
        );
        assert!(matches!(
            pending_report.state,
            ReadinessState::PendingToolResults { ref tool_calls }
                if tool_calls.len() == 1 && tool_calls[0].key.tool_call_id == "call-1"
        ));
        let ReadinessState::PendingToolResults { tool_calls } = pending_report.state else {
            panic!("expected pending tool results");
        };
        let pending_error = PendingByopToolResultsError::new(tool_calls.len());
        assert_eq!(
            pending_error.category(),
            ReadinessCategory::PendingToolResults
        );
        assert_eq!(pending_error.tool_call_count(), 1);
        assert!(
            !pending_error
                .to_string()
                .contains(BLOCKED_BYOP_REQUEST_MESSAGE),
            "pending wait must remain distinct from blocked user-facing errors"
        );

        let request = build_openai_request(&request_params(
            vec![user_message, tool_call_message],
            vec![
                cancelled_action_result_input("call-1"),
                user_query_input("continue"),
            ],
        ))
        .expect("current cancellation result should serialize as a real tool result");
        let errors = strict_chat_completions_ordering_errors(&request.messages);
        assert!(
            errors.is_empty(),
            "request body ordering errors: {errors:?}"
        );
        assert_request_has_no_repair_placeholder(&request);

        let contents = tool_response_contents(&request, "call-1");
        assert_eq!(contents.len(), 1);
        assert!(
            contents[0].to_ascii_lowercase().contains("cancel"),
            "expected real cancellation content, got {}",
            contents[0]
        );
    }

    #[test]
    fn controller_readiness_requires_cancellation_commit_before_user_boundary() {
        let tool_call_message = make_tool_call_message("task-1", "req-1", "call-1", shell_tool());
        let params = request_params(
            vec![tool_call_message],
            vec![
                cancelled_action_result_input("call-1"),
                user_query_input("continue"),
            ],
        );

        let report = classify_byop_controller_readiness(&params);

        assert!(matches!(
            report.state,
            ReadinessState::NeedsCancellationCommit { ref tool_calls }
                if tool_calls.len() == 1
                    && tool_calls[0].key.tool_call_id == "call-1"
        ));
    }

    #[test]
    fn controller_readiness_ignores_duplicate_current_cancellation_after_persistence() {
        let tool_call_message = make_tool_call_message("task-1", "req-1", "call-1", shell_tool());
        let tool_result_message = make_tool_call_result_message(
            "task-1",
            "req-1",
            "call-1".to_owned(),
            r#"{"status":"cancelled"}"#.to_owned(),
        );
        let params = request_params(
            vec![tool_call_message, tool_result_message],
            vec![
                cancelled_action_result_input("call-1"),
                user_query_input("continue"),
            ],
        );

        let report = classify_byop_controller_readiness(&params);

        assert!(matches!(report.state, ReadinessState::Ready));
    }

    #[test]
    fn controller_readiness_accepts_committed_local_interception_results() {
        let carrier =
            make_tool_call_carrier_message("task-1", "req-1", "call-1", "todowrite", "{}");
        let local_result = make_tool_call_result_message(
            "task-1",
            "req-1",
            "call-1".to_owned(),
            r#"{"_byop_intercepted":true,"status":"ok"}"#.to_owned(),
        );
        let invalid_arguments_result = make_tool_call_result_message(
            "task-1",
            "req-1",
            "call-2".to_owned(),
            r#"{"error":"invalid_arguments","tool":"dummy"}"#.to_owned(),
        );
        let invalid_arguments_carrier =
            make_tool_call_carrier_message("task-1", "req-1", "call-2", "dummy", "{}");
        let params = request_params(
            vec![
                carrier,
                local_result,
                invalid_arguments_carrier,
                invalid_arguments_result,
            ],
            vec![],
        );

        let report = classify_byop_controller_readiness(&params);

        assert!(matches!(report.state, ReadinessState::Ready));
    }

    #[test]
    fn controller_readiness_blocks_unreadable_local_interception_payload() {
        let carrier =
            make_tool_call_carrier_message("task-1", "req-1", "call-1", "todowrite", "{}");
        let unreadable_result = make_tool_call_result_message(
            "task-1",
            "req-1",
            "call-1".to_owned(),
            r#"{"_byop_intercepted":true,"status":"ok""#.to_owned(),
        );
        let params = request_params(vec![carrier, unreadable_result], vec![]);

        let report = classify_byop_controller_readiness(&params);

        assert!(matches!(
            report.state,
            ReadinessState::MissingResultWithoutRepairSource {
                reason: crate::ai::byop_readiness::MissingResultReason::UnreadableLocalInterception,
                ..
            }
        ));
    }

    #[test]
    fn controller_readiness_reports_pending_live_action() {
        let tool_call_message = make_tool_call_message("task-1", "req-1", "call-1", shell_tool());
        let assistant_message_id = tool_call_message.id.clone();
        let params = request_params(vec![tool_call_message], vec![]);

        let report = classify_byop_controller_readiness_with_live_tool_calls(
            &params,
            vec![LiveToolCall::new(
                ToolCallRef::new(
                    ToolCallKey::new("task-1", assistant_message_id, "call-1"),
                    kind(),
                ),
                LiveToolCallState::Running,
            )],
        );

        assert!(matches!(
            report.state,
            ReadinessState::PendingToolResults { ref tool_calls }
                if tool_calls.len() == 1 && tool_calls[0].key.tool_call_id == "call-1"
        ));
    }

    #[test]
    fn normal_flow_missing_result_blocks_before_placeholder_repair() {
        assert_blocked_category(
            vec![assistant_calls("task-1", "assistant-1", &["call-1"])],
            "MissingResultWithoutRepairSource",
        );
    }

    #[test]
    fn duplicate_orphan_and_out_of_order_results_block_serialization() {
        assert_blocked_category(
            vec![
                assistant_calls("task-1", "assistant-1", &["call-1"]),
                result(
                    "task-1",
                    "result-1",
                    Some("assistant-1"),
                    "call-1",
                    ToolResultSource::PersistedHistory,
                ),
                result(
                    "task-1",
                    "result-2",
                    Some("assistant-1"),
                    "call-1",
                    ToolResultSource::PersistedHistory,
                ),
            ],
            "DuplicateToolResults",
        );
        assert_blocked_category(
            vec![result(
                "task-1",
                "result-1",
                Some("assistant-missing"),
                "call-1",
                ToolResultSource::PersistedHistory,
            )],
            "OrphanToolResult",
        );
        assert_blocked_category(
            vec![
                assistant_calls("task-1", "assistant-1", &["call-1"]),
                ProjectionItem::user_boundary("task-1", "user-2"),
                result(
                    "task-1",
                    "result-1",
                    Some("assistant-1"),
                    "call-1",
                    ToolResultSource::PersistedHistory,
                ),
            ],
            "OutOfOrderToolResult",
        );
    }

    #[test]
    fn visible_boundaries_block_pending_tool_groups_but_filtered_messages_do_not() {
        assert_blocked_category(
            vec![
                assistant_calls("task-1", "assistant-1", &["call-1"]),
                ProjectionItem::other_boundary("task-1", "visible-other"),
            ],
            "MissingResultWithoutRepairSource",
        );

        let report = validate_serializer_readiness_projection(vec![
            assistant_calls("task-1", "assistant-1", &["call-1"]),
            result(
                "task-1",
                "result-1",
                Some("assistant-1"),
                "call-1",
                ToolResultSource::PersistedHistory,
            ),
        ])
        .expect("filtered-out messages should not affect readiness");
        assert_eq!(report.state, ReadinessState::Ready);
    }

    #[test]
    fn current_input_action_result_satisfies_serializer_readiness() {
        let report = validate_serializer_readiness_projection(vec![
            assistant_calls("task-1", "assistant-1", &["call-1"]),
            result(
                "task-1",
                "current_input:0:call-1",
                None,
                "call-1",
                ToolResultSource::CurrentInput,
            ),
        ])
        .expect("current input result should satisfy a visible tool call");
        assert_eq!(report.state, ReadinessState::Ready);
    }

    #[test]
    fn accepted_history_repair_is_sendable_but_distinct_from_ready() {
        let repair = RepairRecord::new(
            RepairSource::ForkedHistory,
            ToolCallKey::new("task-1", "assistant-1", "call-1"),
        );
        let report = validate_serializer_readiness_projection_with_repair_state(
            vec![assistant_calls("task-1", "assistant-1", &["call-1"])],
            &RepairStateStatus::Valid(RepairState::new(vec![repair.clone()])),
            &ReadinessDiagnosticContext::new(
                "test-conversation",
                "test-attempt",
                ReadinessTriggerLayer::SerializerValidation,
            ),
        )
        .expect("accepted repair should be sendable");

        assert_eq!(
            report.state,
            ReadinessState::AcceptedHistoryRepair {
                repairs: vec![AcceptedRepair {
                    record: repair,
                    tool_call: ToolCallRef::new(
                        ToolCallKey::new("task-1", "assistant-1", "call-1"),
                        kind(),
                    ),
                }],
            }
        );
    }

    #[test]
    fn invalid_repair_sidecar_does_not_block_ready_projection() {
        let report = validate_serializer_readiness_projection_with_repair_state(
            vec![
                assistant_calls("task-1", "assistant-1", &["call-1"]),
                result(
                    "task-1",
                    "result-1",
                    Some("assistant-1"),
                    "call-1",
                    ToolResultSource::PersistedHistory,
                ),
            ],
            &RepairStateStatus::from_sidecar_json(Some("{not valid json".to_owned())),
            &ReadinessDiagnosticContext::new(
                "test-conversation",
                "test-attempt",
                ReadinessTriggerLayer::SerializerValidation,
            ),
        )
        .expect("valid real results should not need repair authorization");

        assert_eq!(report.state, ReadinessState::Ready);
    }

    fn make_tool_call(call_id: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_owned(),
            fn_name: "dummy".to_owned(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    fn assistant_with_calls(call_ids: &[&str]) -> ChatMessage {
        ChatMessage::from(
            call_ids
                .iter()
                .map(|call_id| make_tool_call(call_id))
                .collect::<Vec<_>>(),
        )
    }

    fn tool_response(call_id: &str) -> ChatMessage {
        ChatMessage::from(ToolResponse::new(call_id.to_owned(), "{}".to_owned()))
    }

    fn strict_chat_completions_ordering_errors(messages: &[ChatMessage]) -> Vec<String> {
        let mut errors = Vec::new();
        let mut pending_call_ids: Vec<String> = Vec::new();
        let mut seen_tool_results: HashSet<String> = HashSet::new();

        for (idx, msg) in messages.iter().enumerate() {
            if !pending_call_ids.is_empty() && msg.role != ChatRole::Tool {
                errors.push(format!(
                    "message {idx} role {:?} appeared before pending tool responses {:?}",
                    msg.role, pending_call_ids
                ));
            }

            if msg.role == ChatRole::Assistant {
                let tool_call_ids: Vec<String> = msg
                    .content
                    .tool_calls()
                    .iter()
                    .map(|tool_call| tool_call.call_id.clone())
                    .collect();
                if !tool_call_ids.is_empty() {
                    pending_call_ids = tool_call_ids;
                }
            } else if msg.role == ChatRole::Tool {
                let responses = msg.content.tool_responses();
                if pending_call_ids.is_empty() {
                    errors.push(format!("message {idx} is an orphan tool response"));
                }
                for response in responses {
                    if !seen_tool_results.insert(response.call_id.clone()) {
                        errors.push(format!("duplicate tool result id {}", response.call_id));
                    }
                    match pending_call_ids.first() {
                        Some(expected_call_id) if expected_call_id == &response.call_id => {
                            pending_call_ids.remove(0);
                        }
                        Some(expected_call_id) => {
                            if let Some(pos) = pending_call_ids
                                .iter()
                                .position(|call_id| call_id == &response.call_id)
                            {
                                errors.push(format!(
                                    "out-of-order tool result id {} expected {}",
                                    response.call_id, expected_call_id
                                ));
                                pending_call_ids.remove(pos);
                            } else {
                                errors.push(format!("orphan tool result id {}", response.call_id));
                            }
                        }
                        None => {
                            errors.push(format!("orphan tool result id {}", response.call_id));
                        }
                    }
                }
            }
        }

        if !pending_call_ids.is_empty() {
            errors.push(format!(
                "request ended with pending tool responses {:?}",
                pending_call_ids
            ));
        }

        errors
    }

    #[test]
    fn strict_request_body_checker_accepts_ordered_tool_responses() {
        let messages = vec![
            ChatMessage::user("hi"),
            assistant_with_calls(&["a", "b"]),
            tool_response("a"),
            tool_response("b"),
            ChatMessage::user("continue"),
        ];

        assert!(strict_chat_completions_ordering_errors(&messages).is_empty());
    }

    #[test]
    fn strict_request_body_checker_rejects_orphans_duplicates_and_early_boundaries() {
        let orphan = vec![ChatMessage::user("hi"), tool_response("a")];
        assert!(strict_chat_completions_ordering_errors(&orphan)
            .iter()
            .any(|error| error.contains("orphan")));

        let duplicate = vec![
            ChatMessage::user("hi"),
            assistant_with_calls(&["a"]),
            tool_response("a"),
            tool_response("a"),
        ];
        assert!(strict_chat_completions_ordering_errors(&duplicate)
            .iter()
            .any(|error| error.contains("duplicate")));

        let early_boundary = vec![
            ChatMessage::user("hi"),
            assistant_with_calls(&["a"]),
            ChatMessage::user("too soon"),
            tool_response("a"),
        ];
        assert!(strict_chat_completions_ordering_errors(&early_boundary)
            .iter()
            .any(|error| error.contains("before pending")));

        let out_of_order = vec![
            ChatMessage::user("hi"),
            assistant_with_calls(&["a", "b"]),
            tool_response("b"),
            tool_response("a"),
        ];
        assert!(strict_chat_completions_ordering_errors(&out_of_order)
            .iter()
            .any(|error| error.contains("out-of-order")));
    }

    #[test]
    fn build_chat_request_body_rejects_missing_duplicate_orphan_and_out_of_order_history() {
        assert_build_request_blocked(
            request_params(
                vec![
                    make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                    make_tool_call_message("task-1", "req-1", "call-1", shell_tool()),
                ],
                vec![],
            ),
            "MissingResultWithoutRepairSource",
        );

        assert_build_request_blocked(
            request_params(
                vec![
                    make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                    make_tool_call_message("task-1", "req-1", "call-1", shell_tool()),
                    make_tool_call_result_message(
                        "task-1",
                        "req-1",
                        "call-1".to_owned(),
                        r#"{"status":"ok"}"#.to_owned(),
                    ),
                    make_tool_call_result_message(
                        "task-1",
                        "req-1",
                        "call-1".to_owned(),
                        r#"{"status":"ok-again"}"#.to_owned(),
                    ),
                ],
                vec![],
            ),
            "DuplicateToolResults",
        );

        assert_build_request_blocked(
            request_params(
                vec![
                    make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                    make_tool_call_result_message(
                        "task-1",
                        "req-1",
                        "call-1".to_owned(),
                        r#"{"status":"orphan"}"#.to_owned(),
                    ),
                ],
                vec![],
            ),
            "OrphanToolResult",
        );

        assert_build_request_blocked(
            request_params(
                vec![
                    make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                    make_tool_call_message("task-1", "req-1", "call-1", shell_tool()),
                    make_user_query_message("task-1", "req-2", "too soon".to_owned(), &[]),
                    make_tool_call_result_message(
                        "task-1",
                        "req-2",
                        "call-1".to_owned(),
                        r#"{"status":"late"}"#.to_owned(),
                    ),
                ],
                vec![],
            ),
            "OutOfOrderToolResult",
        );
    }

    #[test]
    fn build_chat_request_body_accepts_current_input_tool_result() {
        let params = request_params(
            vec![
                make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                make_tool_call_message("task-1", "req-1", "call-1", shell_tool()),
            ],
            vec![
                cancelled_action_result_input("call-1"),
                user_query_input("continue"),
            ],
        );

        let request = build_openai_request(&params).expect("current input result should serialize");
        let errors = strict_chat_completions_ordering_errors(&request.messages);
        assert!(
            errors.is_empty(),
            "request body ordering errors: {errors:?}"
        );
        assert_request_has_no_repair_placeholder(&request);
    }

    #[test]
    fn build_chat_request_body_ignores_filtered_subagent_tool_call_result() {
        let params = request_params(
            vec![
                make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                make_tool_call_message("task-1", "req-1", "subagent-call-1", subagent_tool()),
                make_tool_call_result_message(
                    "task-1",
                    "req-1",
                    "subagent-call-1".to_owned(),
                    r#"{"status":"spawned"}"#.to_owned(),
                ),
            ],
            vec![],
        );

        let request =
            build_openai_request(&params).expect("filtered subagent result should not block");
        let errors = strict_chat_completions_ordering_errors(&request.messages);
        assert!(
            errors.is_empty(),
            "request body ordering errors: {errors:?}"
        );
        assert!(
            request
                .messages
                .iter()
                .flat_map(|message| message.content.tool_responses())
                .all(|response| response.call_id != "subagent-call-1"),
            "filtered subagent ToolCallResult must not be sent outbound"
        );
    }

    #[test]
    fn build_chat_request_body_emits_structured_repair_placeholder_only_for_accepted_repair() {
        let tool_call_message = make_tool_call_message("task-1", "req-1", "call-1", shell_tool());
        let assistant_message_id = tool_call_message.id.clone();
        let repair = RepairRecord::new(
            RepairSource::ForkedHistory,
            ToolCallKey::new("task-1", assistant_message_id, "call-1"),
        );
        let params = request_params_with_repair(
            vec![
                make_user_query_message("task-1", "req-1", "hi".to_owned(), &[]),
                tool_call_message,
            ],
            vec![],
            RepairState::new(vec![repair]),
        );

        let request = build_openai_request(&params).expect("accepted repair should serialize");
        let response = request
            .messages
            .iter()
            .flat_map(|message| message.content.tool_responses())
            .find(|response| response.call_id == "call-1")
            .expect("repair placeholder response should be present");
        let payload: serde_json::Value =
            serde_json::from_str(&response.content).expect("placeholder should be JSON");
        let object = payload
            .as_object()
            .expect("placeholder should be an object");

        assert_eq!(
            object.keys().cloned().collect::<HashSet<_>>(),
            HashSet::from([
                "status".to_string(),
                "reason".to_string(),
                "note".to_string()
            ])
        );
        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["reason"], "forked_history_repair");
        assert_eq!(
            payload["note"],
            "tool result was unavailable in repaired conversation history"
        );
        assert!(!response.content.contains("(tool result not preserved)"));
        assert!(
            params.tasks[0].messages.iter().all(|message| !matches!(
                message.message,
                Some(api::message::Message::ToolCallResult(_))
            )),
            "accepted repair placeholders must remain outbound-only"
        );
    }

    #[test]
    fn accepted_repair_log_summary_includes_source_counts_and_redacted_keys() {
        let repairs = vec![
            AcceptedRepair {
                record: RepairRecord::new(
                    RepairSource::ForkedHistory,
                    ToolCallKey::new("task-1", "assistant-1", "call-1"),
                ),
                tool_call: ToolCallRef::new(
                    ToolCallKey::new("task-1", "assistant-1", "call-1"),
                    kind(),
                ),
            },
            AcceptedRepair {
                record: RepairRecord::new(
                    RepairSource::RestoredLegacyHistory,
                    ToolCallKey::new("task-1", "assistant-2", "call-2"),
                ),
                tool_call: ToolCallRef::new(
                    ToolCallKey::new("task-1", "assistant-2", "call-2"),
                    RedactedToolKind::new("local_interception:webfetch"),
                ),
            },
        ];
        let context = ReadinessDiagnosticContext::new(
            "conversation-1",
            "attempt-1",
            ReadinessTriggerLayer::SerializerValidation,
        );

        let message = accepted_history_repair_log_message(&repairs, &context);

        assert!(message.contains("serializer accepted history repair"));
        assert!(message.contains("records=2"));
        assert!(message.contains("category=AcceptedHistoryRepair"));
        assert!(message.contains("forked_history=1"));
        assert!(message.contains("restored_legacy_history=1"));
        assert!(message.contains("conversation_id=conversation-1"));
        assert!(message.contains("trigger_layer=serializer_validation"));
        assert!(message.contains("request_attempt_id=attempt-1"));
        assert!(message.contains("task_id=task-1"));
        assert!(message.contains("assistant_tool_call_message_id=assistant-1"));
        assert!(message.contains("tool_call_id=call-1"));
        assert!(message.contains("redacted_tool_kind=local_interception:webfetch"));
        assert!(!message.contains(REPAIR_PLACEHOLDER_NOTE));
        assert!(!message.contains("secret user prompt"));
        assert!(!message.contains("raw tool arguments"));
        assert!(!message.contains("raw tool output"));
        assert!(!message.contains("raw local interception payload"));
    }

    #[test]
    fn build_chat_request_body_honors_compaction_filtering_boundaries() {
        let hidden_user = make_user_query_message("task-1", "req-1", "hidden".to_owned(), &[]);
        let hidden_call = make_tool_call_message("task-1", "req-1", "hidden-call", shell_tool());
        let summary_user = make_user_query_message("task-1", "req-2", "/compact".to_owned(), &[]);
        let summary_assistant =
            make_agent_output_message("task-1", "req-2", "redacted summary".to_owned());
        let visible_user = make_user_query_message("task-1", "req-3", "visible".to_owned(), &[]);

        let mut compaction_state = CompactionState::default();
        compaction_state.push_completed(CompletedCompaction {
            user_msg_id: summary_user.id.clone(),
            assistant_msg_id: summary_assistant.id.clone(),
            head_message_ids: vec![hidden_user.id.clone(), hidden_call.id.clone()],
            tail_start_id: Some(visible_user.id.clone()),
            summary_text: Some("redacted summary".to_owned()),
            auto: false,
            overflow: false,
        });

        let mut params = request_params(
            vec![
                hidden_user.clone(),
                hidden_call.clone(),
                summary_user.clone(),
                summary_assistant.clone(),
                visible_user.clone(),
            ],
            vec![],
        );
        params.compaction_state = Some(compaction_state.clone());

        let request = build_openai_request(&params)
            .expect("hidden historical tool-call gap should not block");
        let errors = strict_chat_completions_ordering_errors(&request.messages);
        assert!(
            errors.is_empty(),
            "request body ordering errors: {errors:?}"
        );
        assert_request_has_no_repair_placeholder(&request);

        let visible_call = make_tool_call_message("task-1", "req-3", "visible-call", shell_tool());
        let mut params = request_params(
            vec![
                hidden_user,
                hidden_call,
                summary_user,
                summary_assistant,
                visible_user,
                visible_call,
            ],
            vec![],
        );
        params.compaction_state = Some(compaction_state);

        assert_build_request_blocked(params, "MissingResultWithoutRepairSource");
    }
}

/// **Accepted history repair outbound fix behavior verification**:
///
/// This module only covers outbound repairs explicitly authorized by `RepairRecord`.
/// Normal-flow missing results must first be blocked by readiness checks, and cannot
/// come through here for placeholder insertion.
#[cfg(test)]
mod accepted_history_repair_tests {
    use super::*;
    use crate::ai::byop_readiness::{RepairRecord, ToolCallKey, ToolCallRef};
    use genai::chat::{ChatMessage, ChatRole, ToolCall};

    fn make_tool_call(call_id: &str) -> ToolCall {
        ToolCall {
            call_id: call_id.to_owned(),
            fn_name: "dummy".to_owned(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        }
    }

    fn assistant_with_calls(call_ids: &[&str]) -> ChatMessage {
        let calls: Vec<ToolCall> = call_ids.iter().map(|cid| make_tool_call(cid)).collect();
        ChatMessage::from(calls)
    }

    fn tool_response(call_id: &str, content: &str) -> ChatMessage {
        ChatMessage::from(ToolResponse::new(call_id.to_owned(), content.to_owned()))
    }

    /// Flatten all ToolResponses in a single Tool message into (call_id, content) pairs
    /// for easy assertion.
    fn responses_of(msg: &ChatMessage) -> Vec<(String, String)> {
        msg.content
            .tool_responses()
            .iter()
            .map(|r| (r.call_id.clone(), r.content.clone()))
            .collect()
    }

    fn accepted_repair(key: ToolCallKey, source: RepairSource) -> AcceptedRepair {
        AcceptedRepair {
            record: RepairRecord::new(source, key.clone()),
            tool_call: ToolCallRef::new(key, RedactedToolKind::new("shell")),
        }
    }

    fn outbound_groups_for_messages(messages: &[ChatMessage]) -> Vec<OutboundAssistantToolGroup> {
        let mut groups = Vec::new();
        let mut assistant_group_number = 0;
        for (message_index, message) in messages.iter().enumerate() {
            if message.role != ChatRole::Assistant || message.content.tool_calls().is_empty() {
                continue;
            }

            assistant_group_number += 1;
            let assistant_message_id = format!("assistant-{assistant_group_number}");
            groups.push(OutboundAssistantToolGroup {
                message_index,
                tool_call_keys: message
                    .content
                    .tool_calls()
                    .iter()
                    .map(|tool_call| {
                        ToolCallKey::new("task-1", &assistant_message_id, &tool_call.call_id)
                    })
                    .collect(),
            });
        }

        groups
    }

    fn repairs_for_groups(groups: &[OutboundAssistantToolGroup]) -> Vec<AcceptedRepair> {
        groups
            .iter()
            .flat_map(|group| {
                group
                    .tool_call_keys
                    .iter()
                    .cloned()
                    .map(|key| accepted_repair(key, RepairSource::ForkedHistory))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn repair_messages(messages: &mut Vec<ChatMessage>) {
        let groups = outbound_groups_for_messages(messages);
        let repairs = repairs_for_groups(&groups);
        repair_tool_call_pairs_for_accepted_history_gaps(messages, &repairs, &groups)
            .expect("repair helper expects all gaps to be authorized in tests");
    }

    fn assert_structured_repair_payload(content: &str, reason: &str) {
        let payload: serde_json::Value =
            serde_json::from_str(content).expect("repair placeholder should be JSON");
        assert_eq!(payload["status"], "unavailable");
        assert_eq!(payload["reason"], reason);
        assert_eq!(
            payload["note"],
            "tool result was unavailable in repaired conversation history"
        );
    }

    /// Normal push path: [user, asst(a,b), tool_a, tool_b] → merged into a single
    /// bundled message, no placeholders inserted.
    #[test]
    fn normal_push_path_merges_adjacent_tool_messages_without_placeholder() {
        let mut msgs = vec![
            ChatMessage::user("hi"),
            assistant_with_calls(&["a", "b"]),
            tool_response("a", "resp_a"),
            tool_response("b", "resp_b"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3, "Two adjacent Tools merged into one");
        assert_eq!(msgs[0].role, ChatRole::User);
        assert_eq!(msgs[1].role, ChatRole::Assistant);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![
                ("a".to_owned(), "resp_a".to_owned()),
                ("b".to_owned(), "resp_b".to_owned()),
            ],
            "bundled response order must match Assistant.tool_calls"
        );
    }

    /// Fork truncation scenario A: Assistant has tool_calls but **all** ToolCallResults
    /// are missing → insert a single all-placeholder Tool message after the Assistant.
    #[test]
    fn fork_truncated_missing_all_tool_responses_inserts_placeholders() {
        let mut msgs = vec![ChatMessage::user("q"), assistant_with_calls(&["a", "b"])];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3, "Must append a Tool message after Assistant");
        assert_eq!(msgs[2].role, ChatRole::Tool);
        let responses = responses_of(&msgs[2]);
        assert_eq!(
            responses.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            vec!["a".to_owned(), "b".to_owned()]
        );
        for (_, content) in &responses {
            assert_structured_repair_payload(content, "forked_history_repair");
        }
    }

    /// Fork truncation scenario B: Assistant has (a, b) but only tool_a is retained →
    /// b gets a placeholder, order is still reorganized per Assistant.tool_calls
    /// as (real_a, placeholder_b).
    #[test]
    fn fork_truncated_partial_tool_responses_fills_missing_only() {
        let mut msgs = vec![
            ChatMessage::user("q"),
            assistant_with_calls(&["a", "b"]),
            tool_response("a", "real_a"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        let responses = responses_of(&msgs[2]);
        assert_eq!(responses[0], ("a".to_owned(), "real_a".to_owned()));
        assert_eq!(responses[1].0, "b".to_owned());
        assert_structured_repair_payload(&responses[1].1, "forked_history_repair");
    }

    /// Orphan ToolResponse (call_id not in Assistant.tool_calls) is dropped and won't
    /// pollute the output.
    #[test]
    fn orphan_tool_response_with_unknown_call_id_is_dropped() {
        let mut msgs = vec![
            ChatMessage::user("q"),
            assistant_with_calls(&["a"]),
            tool_response("a", "real_a"),
            tool_response("z", "orphan_z"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3, "Two adjacent Tools merged, orphan z discarded");
        let responses = responses_of(&msgs[2]);
        assert_eq!(
            responses,
            vec![("a".to_owned(), "real_a".to_owned())],
            "Only keep call_id recognized by Assistant"
        );
    }

    /// Assistant.tool_calls order is (a, b) but existing Tool order is (b, a) →
    /// bundled output is reordered to (real_a, real_b), aligning with Anthropic tool_use order.
    #[test]
    fn out_of_order_tool_responses_are_reordered_per_assistant_calls() {
        let mut msgs = vec![
            ChatMessage::user("q"),
            assistant_with_calls(&["a", "b"]),
            tool_response("b", "real_b"),
            tool_response("a", "real_a"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![
                ("a".to_owned(), "real_a".to_owned()),
                ("b".to_owned(), "real_b".to_owned()),
            ]
        );
    }

    /// When the user interrupts/continues, completed tool results may be persisted later
    /// than a new UserQuery. The outbound request must move the result back after the
    /// corresponding Assistant, otherwise the upstream will see an orphan ToolResponse.
    #[test]
    fn late_tool_response_after_user_query_is_moved_back_to_tool_call() {
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a"]),
            ChatMessage::user("interrupt"),
            tool_response("a", "real_a"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, ChatRole::User);
        assert_eq!(msgs[1].role, ChatRole::Assistant);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![("a".to_owned(), "real_a".to_owned())]
        );
        assert_eq!(msgs[3].role, ChatRole::User);
    }

    /// Two real ToolResponses with the same call_id in a row (duplicate persistence /
    /// manual retry scenarios): the later real value should overwrite the earlier one —
    /// maintaining the same "last insert wins" semantics as the old implementation.
    #[test]
    fn real_tool_response_is_replaced_by_later_real_tool_response() {
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a"]),
            tool_response("a", "real_a_v1"),
            tool_response("a", "real_a_v2"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![("a".to_owned(), "real_a_v2".to_owned())],
            "Multiple real responses for same call_id, later ones win"
        );
    }

    /// A placeholder must not overwrite an existing real result.
    /// This scenario occurs when: a previous round inserted a placeholder as a stand-in →
    /// the current round receives the real result → then encounters another placeholder
    /// with the same call_id (e.g., after a fork splice). Must ensure the real value is preserved.
    #[test]
    fn placeholder_does_not_overwrite_existing_real_response() {
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a"]),
            tool_response("a", "real_a"),
            tool_response("a", "(tool execution result not preserved)"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![("a".to_owned(), "real_a".to_owned())],
            "placeholder cannot overwrite real value"
        );
    }

    /// Polluted history may contain both a placeholder and a late real result;
    /// the real result should overwrite the placeholder.
    #[test]
    fn placeholder_is_replaced_by_late_real_tool_response() {
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a"]),
            tool_response("a", "(tool execution result not preserved)"),
            ChatMessage::user("interrupt"),
            tool_response("a", "real_a"),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![("a".to_owned(), "real_a".to_owned())]
        );
        assert_eq!(msgs[3].role, ChatRole::User);
    }

    /// Multiple Assistant tool_calls groups are each handled independently:
    /// each Assistant + Tool segment does not affect the others.
    #[test]
    fn multiple_assistant_tool_call_groups_handled_independently() {
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a", "b"]),
            tool_response("a", "real_a"),
            tool_response("b", "real_b"),
            ChatMessage::user("q2"),
            assistant_with_calls(&["c"]),
            tool_response("c", "real_c"),
        ];
        repair_messages(&mut msgs);

        // Expected result: user, asst(a,b), bundled(a,b), user, asst(c), bundled(c)
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![
                ("a".to_owned(), "real_a".to_owned()),
                ("b".to_owned(), "real_b".to_owned()),
            ]
        );
        assert_eq!(msgs[5].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[5]),
            vec![("c".to_owned(), "real_c".to_owned())]
        );
    }

    /// An Assistant message without tool_calls is left untouched — no extra Tool
    /// message is appended.
    #[test]
    fn assistant_without_tool_calls_is_untouched() {
        let mut msgs = vec![
            ChatMessage::user("q"),
            ChatMessage::assistant("plain reply"),
        ];
        let before = msgs.len();
        repair_messages(&mut msgs);
        assert_eq!(msgs.len(), before);
        assert_eq!(msgs[1].role, ChatRole::Assistant);
        assert!(msgs[1].content.tool_calls().is_empty());
    }

    #[test]
    fn accepted_repair_placeholder_is_authorized_by_full_tool_call_key() {
        let first_key = ToolCallKey::new("task-1", "assistant-1", "dup");
        let second_key = ToolCallKey::new("task-2", "assistant-2", "dup");
        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["dup"]),
            ChatMessage::user("q2"),
            assistant_with_calls(&["dup"]),
            tool_response("dup", "real_second"),
        ];
        let groups = vec![
            OutboundAssistantToolGroup {
                message_index: 1,
                tool_call_keys: vec![first_key.clone()],
            },
            OutboundAssistantToolGroup {
                message_index: 3,
                tool_call_keys: vec![second_key],
            },
        ];
        let repairs = vec![accepted_repair(first_key, RepairSource::ForkedHistory)];

        repair_tool_call_pairs_for_accepted_history_gaps(&mut msgs, &repairs, &groups)
            .expect("second_key has a real response, so repair must succeed");

        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[2].role, ChatRole::Tool);
        let first_responses = responses_of(&msgs[2]);
        assert_eq!(first_responses[0].0, "dup");
        assert_structured_repair_payload(&first_responses[0].1, "forked_history_repair");
        assert_eq!(msgs[5].role, ChatRole::Tool);
        assert_eq!(
            responses_of(&msgs[5]),
            vec![("dup".to_owned(), "real_second".to_owned())],
            "Real result for duplicate call_id must stay after its own Assistant group"
        );
    }

    #[test]
    fn unavailable_json_with_non_repair_reason_is_not_placeholder() {
        let real_unavailable =
            r#"{"status":"unavailable","reason":"service_down","note":"try later"}"#;
        assert!(!is_placeholder_tool_response_content(real_unavailable));
        assert!(!is_placeholder_tool_response_content(
            r#"{"status":"unavailable","reason":"forked_history_repair","note":"tool result was unavailable in repaired conversation history","extra":true}"#,
        ));
        assert!(is_placeholder_tool_response_content(
            &repair_placeholder_content(RepairSource::ForkedHistory)
        ));

        let mut msgs = vec![
            ChatMessage::user("q1"),
            assistant_with_calls(&["a"]),
            tool_response("a", real_unavailable),
            tool_response(
                "a",
                &repair_placeholder_content(RepairSource::ForkedHistory),
            ),
        ];
        repair_messages(&mut msgs);

        assert_eq!(msgs.len(), 3);
        assert_eq!(
            responses_of(&msgs[2]),
            vec![("a".to_owned(), real_unavailable.to_owned())],
            "Real unavailable JSON cannot be overwritten by repair placeholder"
        );
    }
}

// ---------------------------------------------------------------------------
// Test helpers: for use by `cache_stability_tests` within the same crate.
// ---------------------------------------------------------------------------

/// Test-only wrapper: allows other test modules within the same crate to call the
/// otherwise file-private `serialize_outgoing_tool_call`. Only exposed under
/// `cfg(test)`, does not affect the production API surface.
#[cfg(test)]
pub(super) fn serialize_outgoing_tool_call_for_test(
    tc: &api::message::ToolCall,
    mcp_ctx: Option<&crate::ai::agent::MCPContext>,
    server_message_data: &str,
) -> (String, Value) {
    serialize_outgoing_tool_call(tc, mcp_ctx, server_message_data)
}

/// Issue #94 regression test: when `build_chat_request` collects messages across
/// multiple tasks, historical user messages must not be reordered to the end,
/// nor duplicated due to LRC subagent copies.
#[cfg(test)]
mod issue_94_task_linearization_tests {
    use super::*;
    use crate::test_util::ai_agent_tasks::{
        create_api_subtask, create_api_task, create_message, create_subagent_tool_call_message,
    };

    /// Construct a UserQuery message with a `request_id`.
    fn user_query_msg(id: &str, task_id: &str, request_id: &str, query: &str) -> api::Message {
        api::Message {
            id: id.to_string(),
            task_id: task_id.to_string(),
            server_message_data: String::new(),
            citations: vec![],
            message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                query: query.to_string(),
                ..Default::default()
            })),
            request_id: request_id.to_string(),
            timestamp: None,
        }
    }

    /// Reproduce the Issue #94 01.json / 02.json scenario: the user sent only one
    /// message, but after LRC CLI subagent derivation, this UserQuery exists in both
    /// the root task and the subtask.
    ///
    /// - root: [UserQuery, AgentOutput, Subagent ToolCall→subtask]
    /// - subtask: [UserQuery (copy with the same request_id+query), AgentOutput]
    fn issue_94_tasks() -> (api::Task, api::Task) {
        const REQ: &str = "req-1";
        const QUERY: &str = "Help me set up a dns(doh) split routing service on this server";
        let root = create_api_task(
            "root",
            vec![
                user_query_msg("m1", "root", REQ, QUERY),
                create_message("m2", "root"),
                create_subagent_tool_call_message("m3", "root", "sub-1", None),
            ],
        );
        let subtask = create_api_subtask(
            "sub-1",
            "root",
            vec![
                user_query_msg("s1", "sub-1", REQ, QUERY),
                create_message("s2", "sub-1"),
            ],
        );
        (root, subtask)
    }

    fn count_user_queries(msgs: &[&api::Message]) -> usize {
        msgs.iter()
            .filter(|m| matches!(&m.message, Some(api::message::Message::UserQuery(_))))
            .count()
    }

    fn message_ids(msgs: &[&api::Message]) -> Vec<String> {
        msgs.iter().map(|m| m.id.clone()).collect()
    }

    /// Reproduce: the old implementation `params.tasks.iter().flat_map(|t| t.messages.iter())`
    /// naively concatenated — (1) UserQuery appeared twice; (2) result order depended on
    /// task input order (`compute_active_tasks` collects via HashMap::into_values, which
    /// has non-deterministic ordering).
    #[test]
    fn naive_flat_map_reproduces_issue_94() {
        let (root, subtask) = issue_94_tasks();

        let naive = |tasks: &[api::Task]| -> Vec<String> {
            tasks
                .iter()
                .flat_map(|t| t.messages.iter())
                .map(|m| m.id.clone())
                .collect()
        };

        let root_first = naive(&[root.clone(), subtask.clone()]);
        let subtask_first = naive(&[subtask.clone(), root.clone()]);

        // (1) UserQuery was concatenated twice.
        let root_first_refs: Vec<&api::Message> = [&root, &subtask]
            .iter()
            .flat_map(|t| t.messages.iter())
            .collect();
        assert_eq!(
            count_user_queries(&root_first_refs),
            2,
            "Naive concatenation makes same user query appear twice —— this is exactly Issue #94 bug"
        );

        // (2) Order drifts with task input order — when subtask comes first,
        // historical user(m1) is pushed to the end.
        assert_ne!(
            root_first, subtask_first,
            "Naive concatenation result depends on task order, non-deterministic"
        );
        assert_eq!(
            subtask_first.last().map(String::as_str),
            Some("m3"),
            "When subtask is ahead, root's messages move back"
        );
        assert!(
            subtask_first.iter().position(|id| id == "s1").unwrap()
                < subtask_first.iter().position(|id| id == "m1").unwrap(),
            "subtask's UserQuery copy(s1) placed before root original(m1)"
        );
    }

    /// Fix verification: `collect_linearized_task_messages` output is independent of
    /// task input order, UserQuery is deduplicated to a single instance, and the overall
    /// order follows root→subtask DFS linear sequence.
    #[test]
    fn linearized_collection_is_deterministic_and_deduped() {
        let (root, subtask) = issue_94_tasks();

        let root_first = vec![root.clone(), subtask.clone()];
        let subtask_first = vec![subtask.clone(), root.clone()];

        let a = collect_linearized_task_messages(&root_first);
        let b = collect_linearized_task_messages(&subtask_first);

        // Independent of input order.
        assert_eq!(
            message_ids(&a),
            message_ids(&b),
            "Result must be independent of params.tasks input order"
        );

        // UserQuery dedup: the LRC-copied subtask copy (s1) is discarded.
        assert_eq!(
            count_user_queries(&a),
            1,
            "Duplicate UserQuery must be deduplicated into one"
        );

        // DFS linear order: root's messages come first, drilling into subtask
        // upon encountering a Subagent ToolCall. s1 is deduped, so expected: [m1, m2, m3, s2].
        assert_eq!(message_ids(&a), vec!["m1", "m2", "m3", "s2"]);

        // The surviving user query is the root original, positioned at the start of the sequence.
        assert_eq!(a.first().map(|m| m.id.as_str()), Some("m1"));
    }

    /// Normal single-task conversations (no subagent) are unaffected: messages are
    /// returned as-is, in their original order.
    #[test]
    fn single_task_conversation_unchanged() {
        let root = create_api_task(
            "root",
            vec![
                user_query_msg("m1", "root", "req-1", "Hello"),
                create_message("m2", "root"),
            ],
        );
        let out = collect_linearized_task_messages(std::slice::from_ref(&root));
        assert_eq!(message_ids(&out), vec!["m1", "m2"]);
    }

    /// Different user turns with the same query text are kept as long as `request_id`
    /// differs — they won't be incorrectly deleted.
    #[test]
    fn distinct_turns_with_same_text_are_kept() {
        let root = create_api_task(
            "root",
            vec![
                user_query_msg("m1", "root", "req-1", "Continue"),
                create_message("m2", "root"),
                user_query_msg("m3", "root", "req-2", "Continue"),
            ],
        );
        let out = collect_linearized_task_messages(std::slice::from_ref(&root));
        assert_eq!(
            count_user_queries(&out),
            2,
            "User messages from two turns with different request_id must be kept"
        );
        assert_eq!(message_ids(&out), vec!["m1", "m2", "m3"]);
    }
}
