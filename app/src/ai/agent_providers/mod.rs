//! Custom Agent provider support.
//!
//! This module is responsible for:
//! - Securely store each Provider's `api_key` to the OS keychain (secure_storage),
//!   The Provider metadata (name/base_url/model list) takes the normal settings.toml.
//! - Call `${base_url}/models` through `OpenAiCompatibleClient`
//!   Fetch the list of models available upstream (for use by the UI "Fetch models" button).
//!
//! The second phase will implement the `AiProvider` trait based on this set of configurations,
//! Offload Agent's multi-agent calls to the local Provider.

pub mod active_ai;
pub mod attachment_caps;
pub mod chat_stream;
pub mod llm_id;
pub mod models_dev;
pub mod oneshot;
pub mod openai_compatible;
pub mod prompt_renderer;
pub mod reasoning;
pub mod secrets;
pub mod tools;
pub mod user_context;

#[cfg(test)]
mod cache_stability_tests;

// Current external usage points:
// - `fetch_openai_compatible_models`: FetchAgentProviderModels handler in ai_page.rs
// - `AgentProviderSecrets`: multiple handlers in ai_page.rs and lib.rs registration points
// Other symbols (`OpenAiCompatibleError`/`OpenAiCompatibleModel`/`AgentProviderSecretsEvent`)
// Still accessible through full paths such as `crate::ai::agent_providers::openai_compatible::*`,
// No more re-export here to avoid `unused_imports` warnings.
pub use openai_compatible::fetch_openai_compatible_models;
pub use secrets::AgentProviderSecrets;

// ---------------------------------------------------------------------------
// LLMInfo synthesis: Convert the agent_providers configured in settings into a form usable by picker
// ---------------------------------------------------------------------------

use std::collections::HashMap;

use settings::Setting;
use warpui::{AppContext, SingletonEntity};

use crate::ai::llms::{
    AvailableLLMs, DisableReason, LLMContextWindow, LLMInfo, LLMProvider, LLMUsageMetadata,
    ModelsByFeature,
};
use crate::settings::{AISettings, AgentProvider};

/// Synthesizes a list of LLMInfo of all legal (provider, model) pairs for the given provider.
///
/// "Legal" = provider has non-empty base_url + at least 1 model.
/// **API key optional**: Local non-authentication provider (ollama / lm-studio / vllm, etc.) is allowed to be left blank.
/// When the key is missing, the model will still be exposed to picker; the request will still be sent at runtime, but without `Authorization`.
/// Illegal providers (without base_url or no model) will be ignored as a whole, and the models under them will not be displayed in the picker.
/// In this way, users can intuitively see "which providers are not filled in → do not appear".
fn build_byop_llm_infos(app: &AppContext) -> Vec<LLMInfo> {
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let mut out = Vec::new();

    for provider in providers {
        if provider.base_url.trim().is_empty() {
            continue;
        }
        if provider.models.is_empty() {
            continue;
        }

        let provider_label = if provider.name.trim().is_empty() {
            provider.id.clone()
        } else {
            provider.name.clone()
        };

        for model in &provider.models {
            if model.id.trim().is_empty() {
                continue;
            }
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            // The final capability of three-layer priority resolution: the user can force the switch of the three-state chip in settings →
            // models.dev catalog inference → substring fallback.
            // This function is also the same one used when chat_stream decides to block ContentPart::Binary,
            // UI display and runtime behavior are always consistent.
            let resolved_caps =
                attachment_caps::resolve_for_model(&provider.id, provider.api_type, model);
            let vision_supported = resolved_caps.images;
            out.push(LLMInfo {
                display_name: format!("{provider_label} / {display_name}"),
                base_model_name: format!("{provider_label} / {display_name}"),
                id: llm_id::encode(&provider.id, &model.id),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                vision_supported,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                discount_percentage: None,
                context_window: LLMContextWindow::default(),
            });
        }
    }

    out
}

/// Placeholder entry: When the user is not configured with any legal provider, the picker must have at least 1 entry
/// (`AvailableLLMs::new` rejects empty lists). The entry is grayed out with `DisableReason::Unavailable`,
/// If it cannot be selected, the user is prompted to configure it in settings.
fn placeholder_llm_info() -> LLMInfo {
    LLMInfo {
        display_name: "Custom provider not configured - go to Settings -> AI to add".to_owned(),
        base_model_name: "Not Configured".to_owned(),
        id: ai::LLMId::from("byop-placeholder"),
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 1,
            credit_multiplier: None,
        },
        description: None,
        disable_reason: Some(DisableReason::Unavailable),
        vision_supported: false,
        spec: None,
        provider: LLMProvider::Unknown,
        host_configs: HashMap::new(),
        discount_percentage: None,
        context_window: LLMContextWindow::default(),
    }
}

/// Constructs a `ModelsByFeature` populated entirely with BYOP models.
/// 4 features (agent_mode / coding / cli_agent / computer_use) use the same model collection —
/// Custom providers do not differentiate between capabilities, and all models can be used as any feature.
pub fn build_byop_models_by_feature(app: &AppContext) -> ModelsByFeature {
    let mut choices = build_byop_llm_infos(app);
    if choices.is_empty() {
        choices.push(placeholder_llm_info());
    }

    let default_id = choices[0].id.clone();
    let make = || {
        AvailableLLMs::new(default_id.clone(), choices.clone(), None)
            .expect("choices is non-empty by construction")
    };

    ModelsByFeature {
        agent_mode: make(),
        coding: make(),
        cli_agent: Some(make()),
        computer_use: Some(make()),
    }
}

/// Given a BYOP `LLMId`, retrieve `(provider, api_key, model_id)` from `AISettings` and secrets.
/// Returns `None` if any information is missing (controller callers should map to an `InvalidApiKey` error).
pub fn lookup_byop(app: &AppContext, id: &ai::LLMId) -> Option<(AgentProvider, String, String)> {
    let (provider_id, model_id) = llm_id::decode(id)?;
    let providers = AISettings::as_ref(app).agent_providers.value().clone();
    let provider = providers.into_iter().find(|p| p.id == provider_id)?;
    // API key is optional: if there is no key, an empty string will be returned, and the downstream build_client will pass it to genai
    // `AuthData::from_single("")` - does not come with `Authorization`, adapts to local non-authentication services such as ollama.
    let api_key = AgentProviderSecrets::as_ref(app)
        .get(&provider_id)
        .map(str::to_owned)
        .unwrap_or_default();
    Some((provider, api_key, model_id))
}
