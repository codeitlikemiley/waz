//! BYOP one-shot non-streaming completion adaptation layer.
//!
//! For the "active AI" sub-link (prompt suggestions / NLD predict / relevant files /
//! Session title generation, etc.): You need to send a short request to get a piece of text, **no tool calling,
//! No need to stream or persist to task.messages**.
//!
//! Differences from `chat_stream::generate_byop_output` (main conversation stream):
//! - Use `Client::exec_chat` (non-streaming) here, and take `ChatResponse::first_text()` at once.
//! - Do not connect `RequestParams` / `ResponseEvent` / `task_store`, pure string in and string out.
//! - reasoning is disabled by default (active AI should not trigger thinking chains — waste of tokens + slow),
//!   Injected by capability gate only if `OneshotOptions.allow_reasoning = true`.
//!
//! Model selection is determined by the caller: `resolve_active_ai_oneshot()` puts `active_ai_model`
//! (profile fallback to base_model) decoded as BYOP `OneshotConfig`,
//! Decoding failed (BYOP is not configured / the model is not in the BYOP encoding space) → return `None`,
//! Caller silent no-op.

use anyhow::Context as _;
use futures::StreamExt;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use warpui::{AppContext, EntityId, SingletonEntity as _};

use super::chat_stream;
use crate::ai::llms::LLMPreferences;
use crate::settings::{AgentProviderApiType, ReasoningEffortSetting};

/// Provider/model information required for BYOP one-shot requests.
#[derive(Debug, Clone)]
pub struct OneshotConfig {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub api_type: AgentProviderApiType,
    pub reasoning_effort: ReasoningEffortSetting,
}

/// Optional parameter for one-shot calls.
#[derive(Debug, Clone, Default)]
pub struct OneshotOptions {
    /// User message character truncation upper limit (by char, protected CJK). `None` = default 8000.
    pub max_chars: Option<usize>,
    /// Temperature (genai `ChatOptions::temperature`), `None` = provider default.
    pub temperature: Option<f32>,
    /// Whether to require JSON output (OpenAI compatible provider uses response_format).
    /// Note: Unsupported adapters will ignore this parameter, and the system prompt word needs to require JSON itself.
    pub response_format_json: bool,
    /// Whether to allow triggering of reasoning. Default is `false` (active AI is a low-latency lightweight call).
    pub allow_reasoning: bool,
}

const DEFAULT_MAX_CHARS: usize = 8000;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}

fn build_oneshot_request(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> (ChatRequest, ChatOptions) {
    let mut chat_opts = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_usage(true);
    if let Some(t) = opts.temperature {
        chat_opts = chat_opts.with_temperature(t.into());
    }
    if opts.response_format_json {
        chat_opts = chat_opts.with_response_format(genai::chat::ChatResponseFormat::JsonMode);
    }
    if opts.allow_reasoning {
        if let Some(effort) = cfg.reasoning_effort.to_genai() {
            if super::reasoning::model_supports_reasoning(cfg.api_type, &cfg.model_id) {
                chat_opts = chat_opts.with_reasoning_effort(effort);
            }
        }
    }

    let max_chars = opts.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
    let user_truncated = truncate_chars(user, max_chars);

    let chat_req = ChatRequest::from_messages(vec![ChatMessage::user(user_truncated)])
        .with_system(system.to_owned());

    (chat_req, chat_opts)
}

/// Send a BYOP non-streaming chat completion, returning the plain text of the model reply.
///
/// Error handling is determined by the caller - here only propagate `anyhow::Error`, no logging is done.
pub async fn byop_oneshot_completion(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> anyhow::Result<String> {
    let client = chat_stream::build_client(cfg.api_type, cfg.base_url.clone(), cfg.api_key.clone());
    let (chat_req, chat_opts) = build_oneshot_request(cfg, system, user, opts);

    let resp = client
        .exec_chat(&cfg.model_id, chat_req, Some(&chat_opts))
        .await
        .with_context(|| format!("byop oneshot exec_chat failed (model={})", cfg.model_id))?;

    Ok(resp.first_text().unwrap_or("").to_owned())
}

/// Send a BYOP streaming chat completion, aggregate all text chunks and return.
///
/// For use by OpenAI Responses compatible agents that only accept `stream=true`. The caller still gets the complete
/// string, so you can continue to reuse the one-shot title cleaning/JSON parsing logic.
pub async fn byop_oneshot_streaming_completion(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> anyhow::Result<String> {
    let client = chat_stream::build_client(cfg.api_type, cfg.base_url.clone(), cfg.api_key.clone());
    let (chat_req, chat_opts) = build_oneshot_request(cfg, system, user, opts);
    let mut resp = client
        .exec_chat_stream(&cfg.model_id, chat_req, Some(&chat_opts))
        .await
        .with_context(|| {
            format!(
                "byop oneshot exec_chat_stream failed (model={})",
                cfg.model_id
            )
        })?
        .stream;

    let mut text = String::new();
    while let Some(event) = resp.next().await {
        match event.with_context(|| {
            format!(
                "byop oneshot exec_chat_stream event failed (model={})",
                cfg.model_id
            )
        })? {
            ChatStreamEvent::Chunk(chunk) => {
                text.push_str(&chunk.content);
            }
            ChatStreamEvent::Start
            | ChatStreamEvent::ReasoningChunk(_)
            | ChatStreamEvent::ThoughtSignatureChunk(_)
            | ChatStreamEvent::ToolCallChunk(_)
            | ChatStreamEvent::End(_) => {}
        }
    }

    Ok(text)
}

/// Parse the `active_ai_model` of the current active profile (fallback to `base_model`),
/// If the decoding is a legal BYOP encoding → return `OneshotConfig`, otherwise `None` (caller silent no-op).
pub fn resolve_active_ai_oneshot(
    app: &AppContext,
    terminal_view_id: Option<EntityId>,
) -> Option<OneshotConfig> {
    let llm_prefs = LLMPreferences::as_ref(app);
    let id = llm_prefs
        .get_active_ai_model(app, terminal_view_id)
        .id
        .clone();
    let (provider, api_key, model_id) = super::lookup_byop(app, &id)?;
    let reasoning_effort =
        llm_prefs.get_reasoning_effort(terminal_view_id, provider.api_type, &model_id);
    Some(OneshotConfig {
        base_url: provider.base_url,
        api_key,
        model_id,
        api_type: provider.api_type,
        reasoning_effort,
    })
}

/// Parse the `next_command_model` of the current active profile (fallback to `base_model`),
/// If the decoding is a legal BYOP encoding → return `OneshotConfig`, otherwise `None`.
pub fn resolve_next_command_oneshot(
    app: &AppContext,
    terminal_view_id: Option<EntityId>,
) -> Option<OneshotConfig> {
    let llm_prefs = LLMPreferences::as_ref(app);
    let id = llm_prefs
        .get_active_next_command_model(app, terminal_view_id)
        .id
        .clone();
    let (provider, api_key, model_id) = super::lookup_byop(app, &id)?;
    let reasoning_effort =
        llm_prefs.get_reasoning_effort(terminal_view_id, provider.api_type, &model_id);
    Some(OneshotConfig {
        base_url: provider.base_url,
        api_key,
        model_id,
        api_type: provider.api_type,
        reasoning_effort,
    })
}
