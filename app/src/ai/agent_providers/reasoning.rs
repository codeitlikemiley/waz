//! Heuristic determination of model reasoning (thinking chain) ability.
//!
//! Background: Each adapter in genai 0.6 does not perform capability gate on the model internally——
//! As long as `ChatOptions::reasoning_effort` is non-empty, the thinking parameter will still be injected.
//! This pair of models does not support reasoning (claude-3-5-haiku/gpt-4o/gemini-1.5-pro)
//! This will cause the upstream API to directly respond with 400, so the client must make its own decision.
//!
//! The determination strategy follows the "hardcoding + substring matching" of opencode `provider/transform.ts::variants()`:
//! The model id filled in by BYOP users is an arbitrary string and cannot rely on registry metadata. It can only match the naming convention.
//!
//! refer to:
//! - SUPPORT_EFFORT_MODELS / SUPPORT_ADAPTTIVE_THINK_MODELS for genai 0.6 anthropic adapter
//! - anthropicAdaptiveEfforts / OPENAI_EFFORTS list for opencode v5
//! - List of thinking-mode models in official documents of each provider

use crate::settings::{AgentProviderApiType, ReasoningEffortSetting};
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

/// Returns a list of actually available reasoning effort gears for the specified (api_type, model_id).
///
/// List is empty → picker is hidden entirely (reasoning is not supported or client cannot be injected reliably).
/// First item in the list → the recommended default file for this model (the initial value when the picker first appears).
/// The last term is always [`ReasoningEffortSetting::Off`], which means "clearly turn off thinking" (for models that support effort
/// A `none` file will be issued, and the thinking field will be skipped for the budget series).
///
/// The design refers to opencode `provider/transform.ts::variants()` - each gear is hard-coded.
/// Not from models.dev. models.dev only gives "whether reasoning is supported" Boolean, and the specific gear is built in by the client.
pub fn model_reasoning_variants(
    api_type: AgentProviderApiType,
    model_id: &str,
) -> Vec<ReasoningEffortSetting> {
    use ReasoningEffortSetting as R;
    let id = strip_effort_suffix(&model_id.to_ascii_lowercase()).to_string();

    match api_type {
        AgentProviderApiType::Anthropic => {
            if is_opus_4_7_or_higher(&id) {
                // Opus 4.7+: adaptive thinking + xhigh + max (genai has been adapted)
                return vec![R::High, R::Low, R::Medium, R::XHigh, R::Max, R::Off];
            }
            if id.contains("claude-opus-4-6") || id.contains("claude-sonnet-4-6") {
                // Department 4.6: adaptive thinking + max
                return vec![R::High, R::Low, R::Medium, R::Max, R::Off];
            }
            if is_anthropic_reasoning_model(&id) {
                // 4.5 / 3.7-sonnet and other legacy budget, no max
                return vec![R::High, R::Low, R::Medium, R::Off];
            }
            vec![]
        }
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp => {
            if id.contains("gpt-5") || id.contains("codex") {
                // GPT-5 / codex: minimal + xhigh are available
                return vec![R::Medium, R::Minimal, R::Low, R::High, R::XHigh, R::Off];
            }
            if is_openai_reasoning_model(&id) {
                // o-series: low/medium/high only
                return vec![R::Medium, R::Low, R::High, R::Off];
            }
            vec![]
        }
        AgentProviderApiType::Gemini => {
            if is_gemini_reasoning_model(&id) {
                // genai 0.6 sends the thinkingBudget value uniformly, 2.5/3.x does not distinguish between gears
                return vec![R::Medium, R::Low, R::High, R::Off];
            }
            vec![]
        }
        // DeepSeek thinking-mode model (deepseek-reasoner/v4/thinking/r1).
        // Waz local fork (`lib/rust-genai`) relaxes the injection conditions of adapter_shared.rs,
        // Let the `reasoning_effort` top-level field be issued according to the DeepSeek thinking_mode document.
        //
        // The Ollama backend model id is arbitrary and should be left blank conservatively.
        AgentProviderApiType::DeepSeek => {
            if is_deepseek_thinking_model(&id) {
                // DeepSeek’s official thinking depth only has two levels: high / max (low/medium/xhigh
                // Even if the server-side deserializer accepts it, it is only the alias of the same file, and the picker does not expose redundant items).
                // Off file "Close Thinking": local fork genai already supports ChatOptions::extra_body,
                // chat_stream is redirected when DeepSeek+Off
                // `extra_body = {"thinking": {"type": "disabled"}}` Top-level merge.
                vec![R::High, R::Max, R::Off]
            } else {
                vec![]
            }
        }
        AgentProviderApiType::Ollama => vec![],
    }
}

/// The recommended default file for this model (the initial value when picker first appears); None indicates that the model does not support reasoning.
pub fn default_reasoning_for(
    api_type: AgentProviderApiType,
    model_id: &str,
) -> Option<ReasoningEffortSetting> {
    model_reasoning_variants(api_type, model_id)
        .first()
        .copied()
}

/// Opus 4.7 and above (`claude-opus-4-7` / `claude-opus-5-0` ...).
/// The same semantics as the `is_opus_4_7_or_higher` regex of the genai anthropic adapter.
fn is_opus_4_7_or_higher(model_name: &str) -> bool {
    static RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"claude-opus-(\d+)-(\d+)").ok());
    let Some(re) = re.as_ref() else {
        return false;
    };
    let Some(caps) = re.captures(model_name) else {
        return false;
    };
    let major = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
    let minor = caps.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
    matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (4, 7))
}

/// Determine whether the specified (api_type, model_name) combination supports reasoning (thinking chain).
///
/// Inject `reasoning_effort` into genai only if `true` is returned, otherwise send it as is
/// Normal chat requests, avoid injecting thinking into old models (such as claude-3-5-haiku / gpt-4o)
/// Parameter rejected by upstream.
///
/// The naming convention is based on each company's model ID style (all converted to lowercase and then substring matched):
/// - **Anthropic**:`claude-opus-4` / `claude-sonnet-4` / `claude-haiku-4` /
///   `claude-3-7-sonnet` (extended thinking starting point) and newer versions
/// - **OpenAI / OpenAIResp**:`o1` / `o3` / `o4` series, `gpt-5`, `codex`
/// - **Gemini**:`gemini-2.5*` / `gemini-3*`(2.5 and above, all 3.x systems)
/// - **DeepSeek**:`deepseek-reasoner` / `deepseek-r1` / `deepseek-v4*` /
///   `deepseek-thinking` (Official two levels: high / max and `reasoning_effort` top-level fields,
///   Off: `extra_body.thinking.type=disabled` (turn off thinking)
/// - **Ollama**: Take the OpenAI compatible path, the backend model id is uncontrollable, **conservatively returns `false`**
///   (If the user is really running the thinking model, he can explicitly adjust the settings in Settings and then relax later)
pub fn model_supports_reasoning(api_type: AgentProviderApiType, model_id: &str) -> bool {
    !model_reasoning_variants(api_type, model_id).is_empty()
}

fn strip_effort_suffix(id: &str) -> &str {
    if let Some((prefix, last)) = id.rsplit_once('-') {
        if matches!(
            last,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "zero"
        ) {
            return prefix;
        }
    }
    id
}

fn is_anthropic_reasoning_model(id: &str) -> bool {
    // claude-3-7-sonnet is the starting point for extended thinking (released on 2025-02).
    if id.contains("claude-3-7-sonnet") {
        return true;
    }
    // claude-opus-4* / claude-sonnet-4* / claude-haiku-4* are supported by all series.
    // Compatible with `4.5` / `4-5` / `4_5` three dot styles.
    let four_series = ["claude-opus-4", "claude-sonnet-4", "claude-haiku-4"];
    if four_series.iter().any(|prefix| id.contains(prefix)) {
        return true;
    }
    false
}

fn is_openai_reasoning_model(id: &str) -> bool {
    // o-series reasoning models (o1/o1-mini/o1-pro/o3/o3-mini/o4/o4-mini).
    // Note that `o1-mini` is excluded in opencode azure case, but OpenAI officially accepts reasoning_effort,
    // This is retained according to the upstream OpenAI behavior.
    let o_series_prefixes = ["o1", "o3", "o4"];
    for prefix in o_series_prefixes {
        if id == prefix
            || id.starts_with(&format!("{prefix}-"))
            || id.starts_with(&format!("{prefix}_"))
        {
            return true;
        }
    }
    // GPT-5 series (full series of reasoning) + codex variants (gpt-5-codex / codex-* / o*-codex, etc.).
    if id.contains("gpt-5") || id.contains("codex") {
        return true;
    }
    false
}

fn is_deepseek_thinking_model(id: &str) -> bool {
    // DeepSeek thinking-mode model name convention: reasoner/r1/v4*/*-thinking.
    // `deepseek-v4` substring covers `deepseek-v4-flash` and other subsequent variants.
    id.contains("deepseek-reasoner")
        || id.contains("deepseek-v4")
        || id.contains("deepseek-thinking")
        || id.contains("deepseek-r1")
}

fn is_gemini_reasoning_model(id: &str) -> bool {
    // gemini-2.5-* starting thinking mode (flash-thinking-exp / pro / pro-thinking).
    // gemini-3.* full series (opencode differentiates on levels 3 / 3.1).
    if id.contains("gemini-2.5") || id.contains("gemini-3") {
        return true;
    }
    // Historical thinking exp channel (2.0 flash-thinking-exp also counts).
    if id.contains("thinking") {
        return true;
    }
    false
}

/// Align opencode `model.capabilities.interleaved.field`(`provider/provider.ts:1182-1187`,
/// `provider/transform.ts:217-249`): Some thinking-mode models require historical reasoning to be
/// The field name is linked back to the assistant message.
///
/// The two legal values ​​​​of opencode are `"reasoning_content"` and `"reasoning_details"`:
/// - `reasoning_content`: Most domestic OpenAI compatible thinking models (DeepSeek/Kimi/MiMo/Qwen3/
///   Top-level string field used by GLM-thinking/MiniMax/Hunyuan/Ernie/Doubao...).
/// - `reasoning_details`: array form of aggregation providers such as OpenRouter; genai 0.6 OpenAI adapter
///   Not supported yet (can only hoist the top-level `reasoning_content` string) - reserved as enum placeholder,
///   Degenerate on hit serialization by `ReasoningContent` (enough to cover most compatible endpoints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReasoningInterleavedField {
    /// Top-level `reasoning_content` string field.
    ReasoningContent,
    /// The top-level `reasoning_details` array field (reserved, the current serialization path takes fallback).
    ReasoningDetails,
}

/// Domestic/third-party OpenAI compatible thinking model model_id substring matching table.
///
/// Design the `capabilities.interleaved` data field of opencode `models.dev` - each item
/// The thinking model explicitly declares the field in the catalog, and the client looks up the model table to determine the return form.
/// Warp does not have an external catalog. The table is hard-coded and can be changed to configurable override later.
///
/// Rule: **If the lowercase model_id substring contains needle, it is a hit**. The order is irrelevant (short strings and long strings do not cover each other,
/// The first one to hit is enough). During maintenance, you only need to add a row to the table without changing the control flow.
const INTERLEAVED_RULES: &[(&str, ReasoningInterleavedField)] = {
    use ReasoningInterleavedField::ReasoningContent as RC;
    &[
        // DeepSeek's full range of thinking (users often configure the official OpenAI compatible endpoint as OpenAi api_type)
        ("deepseek-reasoner", RC),
        ("deepseek-v4", RC),
        ("deepseek-r1", RC),
        ("deepseek-thinking", RC),
        // Moonshot Kimi Series
        ("kimi", RC),
        ("moonshot", RC),
        // Xiaomi MiMo (error issue source: `mimo-v2.5-pro`)
        ("mimo", RC),
        // Ali Qwen thinking / QwQ (DashScope OpenAI compatible endpoint + enable_thinking)
        ("qwen3", RC),
        ("qwq", RC),
        // GLM thinking (z.ai / Zhipu open platform)
        ("zai-glm", RC),
        ("glm-4.5-thinking", RC),
        ("glm-4.6-thinking", RC),
        ("glm-4.7", RC),
        // MiniMax M1 thinking
        ("minimax-m1", RC),
        // Tencent Hunyuan T1 thinking
        ("hunyuan-t1", RC),
        // Baidu Wenxin X1/thinking
        ("ernie-x1", RC),
        ("ernie-thinking", RC),
        // Step thinking
        ("step-r-mini", RC),
        ("step-thinking", RC),
        // Byte Beanbag thinking
        ("doubao-thinking", RC),
        ("doubao-1-5-thinking", RC),
        // Zero One Yi thinking
        ("yi-thinking", RC),
    ]
};

/// Runtime latch collection: records which (api_type, model_id) have been sent in a certain stream
/// `ReasoningChunk` - that is, "the endpoint server knows the reasoning_content field"
/// Precision heuristic signals.
///
/// This is the key difference from opencode: opencode uses `models.dev` to statically declare an external catalog
/// `capabilities.interleaved`, warp does not have catalog, use stream detection instead - posted by reasoning
/// The endpoint of chunk must be reasoning_content,**Cerebras / Groq / OpenRouter
/// / Together AI / SambaNova** and other strict providers that do not send this chunk will never be latch.
/// Automatically avoid false hang 400s like zerx-lab/warp #25.
///
/// Signals are only retained in memory across stream/turn, and are cleared when the process restarts (see reasoning chunk next time
/// will re-latch). Only meaningful for OpenAi / OpenAiResp api_type - DeepSeek entire
/// The adapter defaults to echo; Anthropic / Gemini go their own way thinking blocks / thought
/// signatures, even if the reasoning chunk is streamed, the top-level `reasoning_content` field is not required.
static REASONING_ECHO_LATCH: OnceLock<RwLock<HashSet<(AgentProviderApiType, String)>>> =
    OnceLock::new();

fn latch_set() -> &'static RwLock<HashSet<(AgentProviderApiType, String)>> {
    REASONING_ECHO_LATCH.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Called when stream receives `ReasoningChunk`, mark (api_type, lowercased model_id) as
/// "Need to return reasoning_content". Next round [`model_reasoning_interleaved`] /
/// [`model_requires_reasoning_echo`] When querying, return `Some(ReasoningContent)` /
/// `true`, regardless of whether it is in the static [`INTERLEAVED_RULES`] table.
///
/// Only the OpenAi / OpenAiResp api_type is actually written (other api_types have already had native
/// reasoning channel, latch has no profit and will pollute the set); the remaining paths return quickly.
pub fn note_reasoning_seen(api_type: AgentProviderApiType, model_id: &str) {
    if !matches!(
        api_type,
        AgentProviderApiType::OpenAi | AgentProviderApiType::OpenAiResp
    ) {
        return;
    }
    let key = (api_type, model_id.to_ascii_lowercase());
    if let Ok(s) = latch_set().read() {
        if s.contains(&key) {
            return;
        }
    }
    if let Ok(mut s) = latch_set().write() {
        s.insert(key);
    }
}

fn latch_contains(api_type: AgentProviderApiType, model_id_lower: &str) -> bool {
    latch_set()
        .read()
        .map(|s| s.contains(&(api_type, model_id_lower.to_string())))
        .unwrap_or(false)
}

/// For testing: clear latch. Production code should not call this.
#[cfg(test)]
fn reset_reasoning_latch() {
    if let Ok(mut s) = latch_set().write() {
        s.clear();
    }
}

/// Look up the table to get the reasoning interleaved field that the model should use; `None` means that the endpoint should not be returned
/// `reasoning_content` - Even if the stream receives real reasoning, it will be discarded during playback to avoid being
/// **Cerebras/Groq/OpenRouter/Together AI/SambaNova/OpenAI official** etc.
/// Strict schema providers reject with 400 `wrong_api_format`.
///
/// Align the `capabilities.interleaved` semantics of opencode `provider/transform.ts:217-249`,
/// Enhanced to a two-stage decision-making (precision first → recall rate):
///
/// 1. **Runtime latch** (accurate): This (api_type, model_id) has been posted in the history stream
///    `ReasoningChunk` → The endpoint server must recognize the reasoning_content field →
///    Return `Some(ReasoningContent)`. Override any domestic / outside the [`INTERLEAVED_RULES`] table
///    Third-party thinking model, no need to maintain whitelist.
/// 2. **static hint** (cold start): fallback to check [`INTERLEAVED_RULES`] substring table when latch misses
///    With api_type default value:
///    - **DeepSeek api_type**: The entire adapter is DeepSeek exclusive, full model echo
///      (Same as opencode default `apiID.includes("deepseek") → { field: "reasoning_content" }`)
///    - **OpenAI / OpenAiResp**: move list, covering domestic mainstream thinking models
///    - **Anthropic / Gemini / Ollama**:`None`(Anthropic walks thinking blocks,
///      Gemini uses thought signatures, and Ollama uses native reasoning; neither requires this echo)
pub fn model_reasoning_interleaved(
    api_type: AgentProviderApiType,
    model_id: &str,
) -> Option<ReasoningInterleavedField> {
    use AgentProviderApiType as T;
    let id = model_id.to_ascii_lowercase();
    // (1) Runtime latch - the echo is locked after the reasoning chunk was sent to the stream in the last round
    if matches!(api_type, T::OpenAi | T::OpenAiResp) && latch_contains(api_type, &id) {
        return Some(ReasoningInterleavedField::ReasoningContent);
    }
    // (2) Static hint - cold start / first round (not yet streamed) cover
    match api_type {
        T::DeepSeek => Some(ReasoningInterleavedField::ReasoningContent),
        T::OpenAi | T::OpenAiResp => INTERLEAVED_RULES
            .iter()
            .find(|(needle, _)| id.contains(needle))
            .map(|(_, f)| *f),
        T::Anthropic | T::Gemini | T::Ollama => None,
    }
}

/// Determine whether the specified (api_type, model_id) needs to be returned on each assistant message
/// `reasoning_content` field (including empty string placeholders). Equivalent to [`model_reasoning_interleaved`]
/// `.is_some()`, retain the old name for compatibility with existing call sites.
///
/// Background: `deepseek-v4-flash` / `mimo-v2.5-pro` and other new generation thinking-mode models
/// Server-side validation has been tightened from "assistants containing only tool_calls must have reasoning_content" to
/// "Every assistant under thinking-mode must have reasoning_content, if it is missing, it will be 400
/// `The reasoning_content in the thinking mode must be passed back to the API`"。
/// genai 0.6 serialization layer (`adapter_shared.rs:368-373`) only echoes existing ones
/// `ContentPart::ReasoningContent`, **will not automatically fill the gap**, so the client layer must be forced to hang up
/// Placeholder fields (empty strings are also acceptable - genai inserts as is, the server only verifies the existence of the fields).
pub fn model_requires_reasoning_echo(api_type: AgentProviderApiType, model_id: &str) -> bool {
    model_reasoning_interleaved(api_type, model_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_supported() {
        let t = AgentProviderApiType::Anthropic;
        assert!(model_supports_reasoning(t, "claude-opus-4-5"));
        assert!(model_supports_reasoning(t, "claude-sonnet-4-6"));
        assert!(model_supports_reasoning(t, "claude-opus-4-7"));
        assert!(model_supports_reasoning(t, "claude-3-7-sonnet-20250219"));
        // The suffix does not affect the judgment
        assert!(model_supports_reasoning(t, "claude-sonnet-4-5-high"));
        assert!(model_supports_reasoning(t, "claude-opus-4-7-max"));
    }

    #[test]
    fn anthropic_unsupported() {
        let t = AgentProviderApiType::Anthropic;
        assert!(!model_supports_reasoning(t, "claude-3-5-haiku-20241022"));
        assert!(!model_supports_reasoning(t, "claude-3-5-sonnet-20241022"));
        assert!(!model_supports_reasoning(t, "claude-3-opus-20240229"));
        assert!(!model_supports_reasoning(t, "claude-2.1"));
    }

    #[test]
    fn openai_supported() {
        let t = AgentProviderApiType::OpenAi;
        assert!(model_supports_reasoning(t, "o1"));
        assert!(model_supports_reasoning(t, "o1-mini"));
        assert!(model_supports_reasoning(t, "o3-mini"));
        assert!(model_supports_reasoning(t, "o4-mini"));
        assert!(model_supports_reasoning(t, "gpt-5"));
        assert!(model_supports_reasoning(t, "gpt-5-codex"));
        assert!(model_supports_reasoning(t, "gpt-5-codex-high"));
    }

    #[test]
    fn openai_unsupported() {
        let t = AgentProviderApiType::OpenAi;
        assert!(!model_supports_reasoning(t, "gpt-4o"));
        assert!(!model_supports_reasoning(t, "gpt-4-turbo"));
        assert!(!model_supports_reasoning(t, "gpt-3.5-turbo"));
    }

    #[test]
    fn gemini_supported() {
        let t = AgentProviderApiType::Gemini;
        assert!(model_supports_reasoning(t, "gemini-2.5-pro"));
        assert!(model_supports_reasoning(t, "gemini-2.5-flash"));
        assert!(model_supports_reasoning(t, "gemini-3-pro"));
        assert!(model_supports_reasoning(t, "gemini-2.0-flash-thinking-exp"));
    }

    #[test]
    fn gemini_unsupported() {
        let t = AgentProviderApiType::Gemini;
        assert!(!model_supports_reasoning(t, "gemini-1.5-pro"));
        assert!(!model_supports_reasoning(t, "gemini-1.5-flash"));
        assert!(!model_supports_reasoning(t, "gemini-2.0-flash"));
    }

    #[test]
    fn deepseek_thinking_models_supported() {
        let t = AgentProviderApiType::DeepSeek;
        assert!(model_supports_reasoning(t, "deepseek-reasoner"));
        assert!(model_supports_reasoning(t, "deepseek-v4"));
        assert!(model_supports_reasoning(t, "deepseek-v4-flash"));
        assert!(model_supports_reasoning(t, "deepseek-thinking"));
        assert!(model_supports_reasoning(t, "deepseek-r1"));
        // Ordinary chat model does not include thinking
        assert!(!model_supports_reasoning(t, "deepseek-chat"));
        assert!(!model_supports_reasoning(t, "deepseek-coder"));
    }

    #[test]
    fn ollama_always_false() {
        assert!(!model_supports_reasoning(
            AgentProviderApiType::Ollama,
            "qwq-32b"
        ));
    }

    #[test]
    fn requires_reasoning_echo_deepseek() {
        // DeepSeek api_type is always echo, no model is chosen.
        assert!(model_requires_reasoning_echo(
            AgentProviderApiType::DeepSeek,
            "deepseek-v4-flash"
        ));
        assert!(model_requires_reasoning_echo(
            AgentProviderApiType::DeepSeek,
            "deepseek-chat"
        ));
        assert!(model_requires_reasoning_echo(
            AgentProviderApiType::DeepSeek,
            "deepseek-reasoner"
        ));
    }

    #[test]
    fn requires_reasoning_echo_kimi_via_openai() {
        let t = AgentProviderApiType::OpenAi;
        assert!(model_requires_reasoning_echo(t, "kimi-k2-thinking"));
        assert!(model_requires_reasoning_echo(t, "moonshot-v1-32k"));
        assert!(model_requires_reasoning_echo(
            AgentProviderApiType::OpenAiResp,
            "Kimi-Latest"
        ));
        // Normal OpenAI model does not echo
        assert!(!model_requires_reasoning_echo(t, "gpt-5"));
        assert!(!model_requires_reasoning_echo(t, "o3-mini"));
    }

    #[test]
    fn requires_reasoning_echo_deepseek_via_openai() {
        // DeepSeek's official endpoint is OpenAI-compatible, and users often match it to OpenAI api_type.
        // BYOP provider. The thinking model must return echo `reasoning_content`, otherwise 400.
        let t = AgentProviderApiType::OpenAi;
        assert!(model_requires_reasoning_echo(t, "deepseek-v4-flash"));
        assert!(model_requires_reasoning_echo(t, "deepseek-v4"));
        assert!(model_requires_reasoning_echo(t, "deepseek-reasoner"));
        assert!(model_requires_reasoning_echo(t, "deepseek-r1"));
        assert!(model_requires_reasoning_echo(t, "deepseek-thinking"));
        // Case insensitive
        assert!(model_requires_reasoning_echo(t, "DeepSeek-V4-Flash"));
        // OpenAiResp origin
        assert!(model_requires_reasoning_echo(
            AgentProviderApiType::OpenAiResp,
            "deepseek-r1"
        ));
        // Non-thinking DeepSeek model (deepseek-chat / deepseek-coder) goes to OpenAI
        // When the path is compatible, thinking-mode verification is not performed and no echo is required.
        assert!(!model_requires_reasoning_echo(t, "deepseek-chat"));
        assert!(!model_requires_reasoning_echo(t, "deepseek-coder"));
    }

    #[test]
    fn opus_4_7_variants_have_xhigh_and_max() {
        let v =
            model_reasoning_variants(AgentProviderApiType::Anthropic, "claude-opus-4-7-20260101");
        assert!(v.contains(&ReasoningEffortSetting::XHigh));
        assert!(v.contains(&ReasoningEffortSetting::Max));
        assert_eq!(v.first().copied(), Some(ReasoningEffortSetting::High));
        assert_eq!(v.last().copied(), Some(ReasoningEffortSetting::Off));
    }

    #[test]
    fn opus_5_0_variants_treated_as_4_7_plus() {
        let v = model_reasoning_variants(AgentProviderApiType::Anthropic, "claude-opus-5-0");
        assert!(v.contains(&ReasoningEffortSetting::XHigh));
        assert!(v.contains(&ReasoningEffortSetting::Max));
    }

    #[test]
    fn sonnet_4_6_variants_have_max_no_xhigh() {
        let v = model_reasoning_variants(AgentProviderApiType::Anthropic, "claude-sonnet-4-6");
        assert!(v.contains(&ReasoningEffortSetting::Max));
        assert!(!v.contains(&ReasoningEffortSetting::XHigh));
    }

    #[test]
    fn sonnet_4_5_variants_legacy_no_max_no_xhigh() {
        let v = model_reasoning_variants(AgentProviderApiType::Anthropic, "claude-sonnet-4-5");
        assert!(!v.contains(&ReasoningEffortSetting::Max));
        assert!(!v.contains(&ReasoningEffortSetting::XHigh));
        assert!(v.contains(&ReasoningEffortSetting::High));
    }

    #[test]
    fn claude_3_5_haiku_variants_empty() {
        let v =
            model_reasoning_variants(AgentProviderApiType::Anthropic, "claude-3-5-haiku-20241022");
        assert!(v.is_empty());
    }

    #[test]
    fn gpt_5_variants_have_minimal_and_xhigh() {
        let v = model_reasoning_variants(AgentProviderApiType::OpenAi, "gpt-5");
        assert!(v.contains(&ReasoningEffortSetting::Minimal));
        assert!(v.contains(&ReasoningEffortSetting::XHigh));
        assert_eq!(v.first().copied(), Some(ReasoningEffortSetting::Medium));
    }

    #[test]
    fn o3_variants_no_minimal_no_xhigh() {
        let v = model_reasoning_variants(AgentProviderApiType::OpenAi, "o3-mini");
        assert!(!v.contains(&ReasoningEffortSetting::Minimal));
        assert!(!v.contains(&ReasoningEffortSetting::XHigh));
        assert!(v.contains(&ReasoningEffortSetting::High));
    }

    #[test]
    fn gpt_4o_variants_empty() {
        let v = model_reasoning_variants(AgentProviderApiType::OpenAi, "gpt-4o");
        assert!(v.is_empty());
    }

    #[test]
    fn gemini_2_5_variants_three_levels() {
        let v = model_reasoning_variants(AgentProviderApiType::Gemini, "gemini-2.5-pro");
        assert_eq!(v.len(), 4); // Medium, Low, High, Off
        assert!(v.contains(&ReasoningEffortSetting::Off));
    }

    #[test]
    fn gemini_1_5_variants_empty() {
        let v = model_reasoning_variants(AgentProviderApiType::Gemini, "gemini-1.5-pro");
        assert!(v.is_empty());
    }

    #[test]
    fn deepseek_thinking_variants_two_levels_plus_off() {
        let v = model_reasoning_variants(AgentProviderApiType::DeepSeek, "deepseek-reasoner");
        // DeepSeek official: only high / max two gears + Off
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], ReasoningEffortSetting::High);
        assert_eq!(v[1], ReasoningEffortSetting::Max);
        assert_eq!(v[2], ReasoningEffortSetting::Off);
        // Redundant aliases should not be exposed
        assert!(!v.contains(&ReasoningEffortSetting::Medium));
        assert!(!v.contains(&ReasoningEffortSetting::Low));
        assert!(!v.contains(&ReasoningEffortSetting::XHigh));
    }

    #[test]
    fn deepseek_chat_variants_empty() {
        assert!(
            model_reasoning_variants(AgentProviderApiType::DeepSeek, "deepseek-chat").is_empty()
        );
    }

    #[test]
    fn ollama_variants_empty() {
        assert!(model_reasoning_variants(AgentProviderApiType::Ollama, "qwq-32b").is_empty());
    }

    #[test]
    fn default_reasoning_for_consistency() {
        // default should be equal to the first item in the variants list
        assert_eq!(
            default_reasoning_for(AgentProviderApiType::Anthropic, "claude-opus-4-7"),
            Some(ReasoningEffortSetting::High)
        );
        assert_eq!(
            default_reasoning_for(AgentProviderApiType::OpenAi, "gpt-5"),
            Some(ReasoningEffortSetting::Medium)
        );
        assert_eq!(
            default_reasoning_for(AgentProviderApiType::OpenAi, "gpt-4o"),
            None
        );
    }

    #[test]
    fn supports_reasoning_consistent_with_variants() {
        // Single source: supports == !variants.is_empty()
        for (t, m) in [
            (AgentProviderApiType::Anthropic, "claude-opus-4-7"),
            (AgentProviderApiType::Anthropic, "claude-3-5-haiku"),
            (AgentProviderApiType::OpenAi, "gpt-5"),
            (AgentProviderApiType::OpenAi, "gpt-4o"),
            (AgentProviderApiType::Gemini, "gemini-2.5-pro"),
            (AgentProviderApiType::Gemini, "gemini-1.5-pro"),
            (AgentProviderApiType::DeepSeek, "deepseek-reasoner"),
        ] {
            assert_eq!(
                model_supports_reasoning(t, m),
                !model_reasoning_variants(t, m).is_empty(),
                "{t:?}/{m}"
            );
        }
    }

    #[test]
    fn requires_reasoning_echo_domestic_thinking_models() {
        // Domestic OpenAI compatible thinking models must echo `reasoning_content`,
        // Otherwise the server 400 `The reasoning_content in the thinking mode must be passed back`.
        // The test hits under OpenAi api_type (the most common BYOP configuration for users).
        let t = AgentProviderApiType::OpenAi;
        // Xiaomi MiMo (this issue trigger model)
        assert!(model_requires_reasoning_echo(t, "mimo-v2.5-pro"));
        assert!(model_requires_reasoning_echo(t, "mimo-vl-7b"));
        // Ali Qwen3 thinking / QwQ
        assert!(model_requires_reasoning_echo(
            t,
            "qwen3-235b-a22b-thinking-2507"
        ));
        assert!(model_requires_reasoning_echo(t, "qwq-32b-preview"));
        // GLM thinking
        assert!(model_requires_reasoning_echo(t, "zai-glm-4.7"));
        assert!(model_requires_reasoning_echo(t, "glm-4.6-thinking"));
        assert!(model_requires_reasoning_echo(t, "glm-4.5-thinking"));
        // MiniMax / Hunyuan / Wen Xin / Step / Bean Bag / Yi
        assert!(model_requires_reasoning_echo(t, "minimax-m1-80k"));
        assert!(model_requires_reasoning_echo(t, "hunyuan-t1-latest"));
        assert!(model_requires_reasoning_echo(t, "ernie-x1-turbo-32k"));
        assert!(model_requires_reasoning_echo(t, "step-r-mini"));
        assert!(model_requires_reasoning_echo(t, "doubao-1-5-thinking-pro"));
        assert!(model_requires_reasoning_echo(t, "yi-thinking-v1"));
        // OpenAiResp origin
        let r = AgentProviderApiType::OpenAiResp;
        assert!(model_requires_reasoning_echo(r, "MiMo-V2.5-Pro"));
        assert!(model_requires_reasoning_echo(r, "Qwen3-Coder-Thinking"));
    }

    #[test]
    fn reasoning_interleaved_field_for_domestic_models() {
        // model_reasoning_interleaved must return ReasoningContent (currently all INTERLEAVED_RULES
        // They are all ReasoningContent; ReasoningDetails is a reserved enum placeholder).
        let t = AgentProviderApiType::OpenAi;
        assert_eq!(
            model_reasoning_interleaved(t, "mimo-v2.5-pro"),
            Some(ReasoningInterleavedField::ReasoningContent)
        );
        assert_eq!(
            model_reasoning_interleaved(t, "deepseek-v4-flash"),
            Some(ReasoningInterleavedField::ReasoningContent)
        );
        // All models of DeepSeek api_type (including non-thinking chat / coder) return ReasoningContent —
        // adapter is exclusive to DeepSeek, and opencode `apiID.includes("deepseek") →
        // { field: "reasoning_content" }` Default alignment.
        let d = AgentProviderApiType::DeepSeek;
        assert_eq!(
            model_reasoning_interleaved(d, "deepseek-chat"),
            Some(ReasoningInterleavedField::ReasoningContent)
        );
        // Undeclared model / non-OpenAI system → None
        assert_eq!(model_reasoning_interleaved(t, "gpt-5"), None);
        assert_eq!(model_reasoning_interleaved(t, "gpt-4o"), None);
        assert_eq!(
            model_reasoning_interleaved(AgentProviderApiType::Anthropic, "claude-opus-4-7"),
            None
        );
        assert_eq!(
            model_reasoning_interleaved(AgentProviderApiType::Gemini, "gemini-2.5-pro"),
            None
        );
        assert_eq!(
            model_reasoning_interleaved(AgentProviderApiType::Ollama, "qwq-32b"),
            None
        );
    }

    #[test]
    fn requires_reasoning_echo_strict_providers_excluded() {
        // OpenAI official / Anthropic / Gemini / Common OpenAI model → not hanging reasoning_content,
        // Avoid Cerebras / Groq / OpenRouter etc. strict OpenAI provider 400 `wrong_api_format`
        // (zerx-lab/warp #25)。
        let t = AgentProviderApiType::OpenAi;
        assert!(!model_requires_reasoning_echo(t, "gpt-5"));
        assert!(!model_requires_reasoning_echo(t, "gpt-4o"));
        assert!(!model_requires_reasoning_echo(t, "o3-mini"));
        // Any BYOP model that has neither a known thinking substring nor a DeepSeek api_type in its name
        assert!(!model_requires_reasoning_echo(t, "llama-3.3-70b-instruct"));
        assert!(!model_requires_reasoning_echo(t, "mistral-large-2407"));
    }

    #[test]
    fn runtime_latch_overrides_static_table() {
        // Any domestic/third-party thinking model not included in INTERLEAVED_RULES,
        // Once the stream has sent the reasoning chunk → it will automatically echo from the next round.
        // Use a deliberately "non-existent" model id to verify that the latch actually works.
        let t = AgentProviderApiType::OpenAi;
        let exotic = "totally-new-thinking-model-2099";
        reset_reasoning_latch();
        assert!(
            !model_requires_reasoning_echo(t, exotic),
            "Non-whitelisted models should not echo before latching"
        );
        note_reasoning_seen(t, exotic);
        assert!(
            model_requires_reasoning_echo(t, exotic),
            "Must echo after latching"
        );
        assert_eq!(
            model_reasoning_interleaved(t, exotic),
            Some(ReasoningInterleavedField::ReasoningContent)
        );
        // Case insensitive
        assert!(model_requires_reasoning_echo(
            t,
            "Totally-New-Thinking-Model-2099"
        ));
        // OpenAiResp and OpenAi are independent keys - but the same endpoint category should latch each
        let r = AgentProviderApiType::OpenAiResp;
        assert!(
            !model_requires_reasoning_echo(r, exotic),
            "Other api_type does not interfere"
        );
        note_reasoning_seen(r, exotic);
        assert!(model_requires_reasoning_echo(r, exotic));
        reset_reasoning_latch();
    }

    #[test]
    fn runtime_latch_never_writes_for_strict_api_types() {
        // Anthropic / Gemini / Ollama each use the original reasoning channel, even if someone mistune it
        // note_reasoning_seen cannot pollute latch (otherwise model_id will be shared across api_type
        // It may be accidentally hit in the OpenAi path - we use (api_type, id) composite key, which is inherently isolated.
        // But semantic extra insurance: these api_types are not included in the latch).
        reset_reasoning_latch();
        for at in [
            AgentProviderApiType::Anthropic,
            AgentProviderApiType::Gemini,
            AgentProviderApiType::Ollama,
            AgentProviderApiType::DeepSeek,
        ] {
            note_reasoning_seen(at, "some-model");
        }
        // Any OpenAi/OpenAiResp query should not be hit by these noises
        assert!(!model_requires_reasoning_echo(
            AgentProviderApiType::OpenAi,
            "some-model"
        ));
        assert!(!model_requires_reasoning_echo(
            AgentProviderApiType::OpenAiResp,
            "some-model"
        ));
        reset_reasoning_latch();
    }

    #[test]
    fn requires_reasoning_echo_others_false() {
        assert!(!model_requires_reasoning_echo(
            AgentProviderApiType::Anthropic,
            "claude-opus-4-7"
        ));
        assert!(!model_requires_reasoning_echo(
            AgentProviderApiType::Gemini,
            "gemini-2.5-pro"
        ));
        assert!(!model_requires_reasoning_echo(
            AgentProviderApiType::Ollama,
            "qwq-32b"
        ));
    }
}
