use crate::config::{Config, ProviderConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::time::Duration;

/// Rotation state persisted between invocations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RotationState {
    /// Index of the last provider used (for round-robin)
    provider_index: usize,
    /// Per-provider key index (for key rotation within a provider)
    key_indices: std::collections::HashMap<String, usize>,
}

impl RotationState {
    pub fn load() -> Self {
        let path = Config::rotation_state_path();
        if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let path = Config::rotation_state_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, serde_json::to_string(self).unwrap_or_default());
    }

    pub fn next_key_for(&mut self, provider_name: &str, num_keys: usize) -> usize {
        if num_keys == 0 {
            return 0;
        }
        let idx = self
            .key_indices
            .entry(provider_name.to_string())
            .or_insert(0);
        let current = *idx;
        *idx = (current + 1) % num_keys;
        current
    }
}

/// Public helper to load rotation state.
#[allow(dead_code)]
pub fn load_rotation_state() -> RotationState {
    RotationState::load()
}

/// Tier 3: LLM-based command prediction with multi-provider rotation.
pub fn predict_with_llm(
    config: &Config,
    recent_commands: &[String],
    cwd: &str,
    prefix: Option<&str>,
) -> Option<String> {
    let llm = &config.llm;

    if llm.providers.is_empty() {
        return None;
    }

    let prompt = build_prompt(recent_commands, cwd, prefix);
    let mut state = RotationState::load();

    let result = match llm.strategy.as_str() {
        "round-robin" => call_round_robin(llm, &prompt, &mut state),
        "single" => call_single(llm, &prompt, &mut state),
        _ => call_fallback(llm, &prompt, &mut state), // "fallback" is default
    };

    state.save();

    result.and_then(|r| clean_response(&r, prefix))
}

/// Fallback strategy: try providers in order, skip on failure.
fn call_fallback(
    llm: &crate::config::LlmConfig,
    prompt: &str,
    state: &mut RotationState,
) -> Option<String> {
    // Get providers in the configured order
    let ordered = get_ordered_providers(llm);

    for provider in &ordered {
        if !provider_usable(provider) {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        let opts = CompleteOptions::predict(llm.timeout_secs);
        if let Some(result) = complete_provider(provider, key_idx, prompt, &opts) {
            return Some(result);
        }
    }
    None
}

/// Round-robin strategy: cycle through providers evenly.
fn call_round_robin(
    llm: &crate::config::LlmConfig,
    prompt: &str,
    state: &mut RotationState,
) -> Option<String> {
    let ordered = get_ordered_providers(llm);
    if ordered.is_empty() {
        return None;
    }

    let start = state.provider_index % ordered.len();
    state.provider_index = (start + 1) % ordered.len();

    let opts = CompleteOptions::predict(llm.timeout_secs);
    for i in 0..ordered.len() {
        let idx = (start + i) % ordered.len();
        let provider = &ordered[idx];
        if !provider_usable(provider) {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        if let Some(result) = complete_provider(provider, key_idx, prompt, &opts) {
            return Some(result);
        }
    }
    None
}

/// Single strategy: only use the default provider.
fn call_single(
    llm: &crate::config::LlmConfig,
    prompt: &str,
    state: &mut RotationState,
) -> Option<String> {
    let provider = llm.providers.iter().find(|p| p.name == llm.default)?;
    if !provider_usable(provider) {
        return None;
    }
    let key_idx = state.next_key_for(&provider.name, provider.keys.len());
    complete_provider(
        provider,
        key_idx,
        prompt,
        &CompleteOptions::predict(llm.timeout_secs),
    )
}

/// Get providers sorted by the configured order.
fn get_ordered_providers(llm: &crate::config::LlmConfig) -> Vec<&ProviderConfig> {
    get_ordered_providers_pub(llm)
}

/// Public version for use by the ask module.
pub fn get_ordered_providers_pub(llm: &crate::config::LlmConfig) -> Vec<&ProviderConfig> {
    let mut result: Vec<&ProviderConfig> = Vec::new();

    // Add providers in the configured order
    for name in &llm.order {
        if let Some(p) = llm.providers.iter().find(|p| &p.name == name) {
            result.push(p);
        }
    }

    // Add any remaining providers not in the order list
    for p in &llm.providers {
        if !result.iter().any(|r| r.name == p.name) {
            result.push(p);
        }
    }

    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKind {
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
    Codex,
}

pub fn api_kind(provider: &ProviderConfig) -> ApiKind {
    match provider.api.to_ascii_lowercase().as_str() {
        "openai" | "openai-compatible" | "openai_compatible" => return ApiKind::OpenAi,
        "anthropic" | "claude" => return ApiKind::Anthropic,
        "gemini" | "google" => return ApiKind::Gemini,
        "ollama" => return ApiKind::Ollama,
        "codex" | "chatgpt" | "openai-responses" => return ApiKind::Codex,
        _ => {}
    }
    match crate::config::canonical_name(&provider.name).as_str() {
        "gemini" => ApiKind::Gemini,
        "ollama" => ApiKind::Ollama,
        "anthropic" => ApiKind::Anthropic,
        "codex" => ApiKind::Codex,
        _ => ApiKind::OpenAi,
    }
}

pub fn provider_usable(provider: &ProviderConfig) -> bool {
    match api_kind(provider) {
        ApiKind::Ollama => true,
        ApiKind::Gemini => !provider.keys.is_empty(),
        ApiKind::Anthropic => !provider.keys.is_empty() || crate::oauth::has_provider("anthropic"),
        ApiKind::Codex => crate::oauth::has_provider("codex") || !provider.keys.is_empty(),
        ApiKind::OpenAi => {
            if crate::config::canonical_name(&provider.name) == "grok" {
                return !provider.keys.is_empty() || crate::oauth::has_provider("grok");
            }
            true
        }
    }
}

/// Options for a single completion call.
#[derive(Debug, Clone)]
pub struct CompleteOptions {
    pub system: Option<String>,
    pub temperature: f64,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    pub stop: Vec<String>,
}

impl CompleteOptions {
    pub fn predict(timeout_secs: u64) -> Self {
        Self {
            system: Some(
                "You are a shell command predictor. Respond with ONLY the predicted command, nothing else. No explanation, no quotes, no markdown."
                    .into(),
            ),
            temperature: 0.1,
            max_tokens: 100,
            timeout_secs,
            stop: vec!["\n".into()],
        }
    }

    pub fn ask() -> Self {
        Self {
            system: Some(
                "You are a helpful shell assistant. Keep responses short and terminal-friendly."
                    .into(),
            ),
            temperature: 0.3,
            max_tokens: 500,
            timeout_secs: 10,
            stop: vec![],
        }
    }

    pub fn resolve() -> Self {
        Self {
            system: None,
            temperature: 0.1,
            max_tokens: 1024,
            timeout_secs: 15,
            stop: vec![],
        }
    }

    pub fn generate() -> Self {
        Self {
            system: None,
            temperature: 0.2,
            max_tokens: 4096,
            timeout_secs: 30,
            stop: vec![],
        }
    }
}

/// One successful completion, including which provider served it.
#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub provider: String,
    pub model: String,
}

/// Complete using configured rotation/fallback. Used by ask, resolve, generate, predict.
pub fn complete(config: &Config, prompt: &str, opts: &CompleteOptions) -> Option<String> {
    complete_filtered(config, prompt, opts, None, None)
}

/// Complete, optionally pinning provider and/or model.
pub fn complete_filtered(
    config: &Config,
    prompt: &str,
    opts: &CompleteOptions,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Option<String> {
    complete_with(config, prompt, opts, provider_override, model_override).map(|c| c.text)
}

/// Like `complete_filtered`, but includes the provider/model that answered.
pub fn complete_with(
    config: &Config,
    prompt: &str,
    opts: &CompleteOptions,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Option<Completion> {
    let llm = &config.llm;
    if llm.providers.is_empty() {
        return None;
    }

    let mut state = RotationState::load();
    let mut providers: Vec<ProviderConfig> = get_ordered_providers_pub(llm)
        .into_iter()
        .cloned()
        .collect();

    if let Some(name) = provider_override {
        let want = crate::config::canonical_name(name);
        providers.retain(|p| crate::config::canonical_name(&p.name) == want);
    }

    if let Some(model) = model_override {
        if let Some(p) = providers.first_mut() {
            p.model = model.to_string();
        }
    }

    for provider in &providers {
        if !provider_usable(provider) {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        if let Some(result) = complete_provider(provider, key_idx, prompt, opts) {
            state.save();
            return Some(Completion {
                text: result,
                provider: provider.name.clone(),
                model: provider.model.clone(),
            });
        }
    }

    state.save();
    None
}

fn complete_provider(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    opts: &CompleteOptions,
) -> Option<String> {
    match api_kind(provider) {
        ApiKind::Gemini => call_gemini(provider, key_idx, prompt, opts),
        ApiKind::Ollama => call_ollama(provider, prompt, opts),
        ApiKind::Anthropic => call_anthropic(provider, key_idx, prompt, opts),
        ApiKind::Codex => call_codex_responses(provider, prompt, opts),
        ApiKind::OpenAi => call_openai_compatible(provider, key_idx, prompt, opts),
    }
}

fn user_text(opts: &CompleteOptions, prompt: &str) -> String {
    match &opts.system {
        Some(system) if !system.is_empty() => format!("{system}\n\n{prompt}"),
        _ => prompt.to_string(),
    }
}

fn call_gemini(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    opts: &CompleteOptions,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let url = format!(
        "{}/models/{}:generateContent?key={}",
        provider.base_url.trim_end_matches('/'),
        provider.model,
        key
    );

    let mut gen = json!({
        "temperature": opts.temperature,
        "maxOutputTokens": opts.max_tokens,
    });
    if !opts.stop.is_empty() {
        gen["stopSequences"] = json!(opts.stop);
    }

    let body = json!({
        "contents": [{"parts": [{"text": user_text(opts, prompt)}]}],
        "generationConfig": gen
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(opts.timeout_secs))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
}

fn call_openai_compatible(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    opts: &CompleteOptions,
) -> Option<String> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let mut messages = Vec::new();
    if let Some(system) = &opts.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": prompt}));

    let mut body = json!({
        "model": provider.model,
        "messages": messages,
        "temperature": opts.temperature,
        "max_tokens": opts.max_tokens,
    });
    if !opts.stop.is_empty() {
        body["stop"] = json!(opts.stop);
    }

    let name = crate::config::canonical_name(&provider.name);
    let mut retried = false;
    loop {
        let mut req = ureq::post(&url)
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(opts.timeout_secs));
        if let Some(key) = bearer_for(provider, key_idx, retried) {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        match req.send_json(&body) {
            Ok(resp) => {
                let json: serde_json::Value = resp.into_json().ok()?;
                return json["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.trim().to_string());
            }
            Err(ureq::Error::Status(401, _)) if oauth_name(&name).is_some() && !retried => {
                retried = true;
                continue;
            }
            Err(_) => return None,
        }
    }
}

fn oauth_name(canonical: &str) -> Option<&'static str> {
    match canonical {
        "grok" => Some("grok"),
        "anthropic" => Some("anthropic"),
        "codex" => Some("codex"),
        _ => None,
    }
}

fn bearer_for(
    provider: &ProviderConfig,
    key_idx: usize,
    force_oauth_refresh: bool,
) -> Option<String> {
    if let Some(oauth) = oauth_name(&crate::config::canonical_name(&provider.name)) {
        let token = if force_oauth_refresh {
            crate::oauth::force_refresh_for(oauth)
        } else {
            crate::oauth::access_token_for(oauth)
        };
        if let Some(token) = token {
            return Some(token);
        }
    }
    provider
        .keys
        .get(key_idx)
        .filter(|k| !k.is_empty())
        .cloned()
}

fn call_anthropic(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    opts: &CompleteOptions,
) -> Option<String> {
    let oauth = crate::oauth::has_provider("anthropic");
    let key = bearer_for(provider, key_idx, false).or_else(|| {
        provider
            .keys
            .get(key_idx)
            .filter(|k| !k.is_empty())
            .cloned()
    })?;
    let base = provider.base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    };

    let mut body = json!({
        "model": provider.model,
        "max_tokens": opts.max_tokens,
        "temperature": opts.temperature,
        "messages": [{"role": "user", "content": prompt}],
    });
    if oauth {
        let mut blocks = vec![json!({
            "type": "text",
            "text": crate::oauth::CLAUDE_CODE_SYSTEM_INSTRUCTION
        })];
        if let Some(system) = &opts.system {
            if !system.is_empty() {
                blocks.push(json!({"type": "text", "text": system}));
            }
        }
        body["system"] = json!(blocks);
    } else if let Some(system) = &opts.system {
        body["system"] = json!(system);
    }
    if !opts.stop.is_empty() {
        body["stop_sequences"] = json!(opts.stop);
    }

    let mut retried = false;
    loop {
        let mut req = ureq::post(&url)
            .set("Content-Type", "application/json")
            .set("anthropic-version", "2023-06-01")
            .timeout(Duration::from_secs(opts.timeout_secs));
        let token = if retried {
            bearer_for(provider, key_idx, true).unwrap_or_else(|| key.clone())
        } else {
            key.clone()
        };
        if oauth {
            req = req
                .set("Authorization", &format!("Bearer {token}"))
                .set("anthropic-beta", crate::oauth::ANTHROPIC_OAUTH_BETA);
        } else {
            req = req.set("x-api-key", &token);
        }

        match req.send_json(&body) {
            Ok(resp) => {
                let json: serde_json::Value = resp.into_json().ok()?;
                return json["content"][0]["text"]
                    .as_str()
                    .map(|s| s.trim().to_string());
            }
            Err(ureq::Error::Status(401, _)) if oauth && !retried => {
                retried = true;
                continue;
            }
            Err(_) => return None,
        }
    }
}

fn call_codex_responses(
    provider: &ProviderConfig,
    prompt: &str,
    opts: &CompleteOptions,
) -> Option<String> {
    let mut retried = false;
    loop {
        let token = if retried {
            crate::oauth::force_refresh_for("codex")?
        } else {
            crate::oauth::access_token_for("codex")
                .or_else(|| provider.keys.iter().find(|k| !k.is_empty()).cloned())?
        };
        let account = crate::oauth::account_id_for("codex").unwrap_or_default();
        let url = format!("{}/responses", provider.base_url.trim_end_matches('/'));
        let mut body = json!({
            "model": provider.model,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": prompt}]
            }],
            "stream": false,
            "store": false,
        });
        if let Some(system) = &opts.system {
            if !system.is_empty() {
                body["instructions"] = json!(system);
            }
        }
        let mut req = ureq::post(&url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(opts.timeout_secs));
        if !account.is_empty() {
            req = req.set("ChatGPT-Account-Id", &account);
        }
        match req.send_json(&body) {
            Ok(resp) => {
                let json: serde_json::Value = resp.into_json().ok()?;
                if let Some(text) = json["output_text"].as_str() {
                    if !text.trim().is_empty() {
                        return Some(text.trim().to_string());
                    }
                }
                if let Some(text) = json["output"][0]["content"][0]["text"].as_str() {
                    return Some(text.trim().to_string());
                }
                return None;
            }
            Err(ureq::Error::Status(401, _)) if !retried => {
                retried = true;
                continue;
            }
            Err(_) => return None,
        }
    }
}

fn call_ollama(provider: &ProviderConfig, prompt: &str, opts: &CompleteOptions) -> Option<String> {
    let url = format!("{}/api/generate", provider.base_url.trim_end_matches('/'));
    let mut options = json!({
        "temperature": opts.temperature,
        "num_predict": opts.max_tokens,
    });
    if !opts.stop.is_empty() {
        options["stop"] = json!(opts.stop);
    }
    let body = json!({
        "model": provider.model,
        "prompt": user_text(opts, prompt),
        "stream": false,
        "options": options
    });

    let json: serde_json::Value = ureq::post(&url)
        .timeout(Duration::from_secs(opts.timeout_secs))
        .send_json(&body)
        .ok()?
        .into_json()
        .ok()?;
    json["response"].as_str().map(|s| s.trim().to_string())
}

// ── Prompt & Cleanup ───────────────────────────────────────────────

fn build_prompt(recent_commands: &[String], cwd: &str, prefix: Option<&str>) -> String {
    let history = if recent_commands.is_empty() {
        "No recent commands.".to_string()
    } else {
        recent_commands
            .iter()
            .enumerate()
            .map(|(i, cmd)| format!("{}. {}", i + 1, cmd))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let prefix_hint = match prefix {
        Some(p) if !p.is_empty() => format!("\nThe user has started typing: \"{}\"", p),
        _ => String::new(),
    };

    format!(
        "You are a shell command predictor. Given the user's recent command history and current working directory, predict the single most likely next command they will run.

Working directory: {}
Recent commands:
{}
{}
Rules:
- Respond with ONLY the predicted command, nothing else
- No explanation, no quotes, no markdown
- Just the raw shell command on a single line
- ONLY suggest commands from the recent history list above or very common variants of them
- Do NOT invent flags, options, or arguments the user has not used before
- If unsure, pick the most recently used command from the list",
        cwd, history, prefix_hint
    )
}

fn clean_response(response: &str, prefix: Option<&str>) -> Option<String> {
    let cmd = response
        .lines()
        .next()?
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("$ ")
        .trim();

    if cmd.is_empty() {
        return None;
    }

    if let Some(pfx) = prefix {
        if !pfx.is_empty() && !cmd.starts_with(pfx) {
            return None;
        }
    }

    Some(cmd.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_response() {
        assert_eq!(
            clean_response("git push origin main", None),
            Some("git push origin main".into())
        );
        assert_eq!(clean_response("`git push`", None), Some("git push".into()));
        assert_eq!(
            clean_response("\"cargo build\"", None),
            Some("cargo build".into())
        );
        assert_eq!(
            clean_response("$ npm install", None),
            Some("npm install".into())
        );
        assert_eq!(clean_response("", None), None);
    }

    #[test]
    fn test_clean_response_with_prefix() {
        assert_eq!(
            clean_response("git push", Some("git")),
            Some("git push".into())
        );
        assert_eq!(clean_response("npm install", Some("git")), None);
    }

    #[test]
    fn test_build_prompt() {
        let cmds = vec!["git add .".into(), "git commit -m 'fix'".into()];
        let prompt = build_prompt(&cmds, "/home/user/project", Some("git"));
        assert!(prompt.contains("/home/user/project"));
        assert!(prompt.contains("git add ."));
        assert!(prompt.contains("started typing"));
    }

    #[test]
    fn test_rotation_state() {
        let mut state = RotationState::default();
        assert_eq!(state.next_key_for("gemini", 3), 0);
        assert_eq!(state.next_key_for("gemini", 3), 1);
        assert_eq!(state.next_key_for("gemini", 3), 2);
        assert_eq!(state.next_key_for("gemini", 3), 0); // wraps around
        assert_eq!(state.next_key_for("glm", 2), 0);
        assert_eq!(state.next_key_for("glm", 2), 1);
        assert_eq!(state.next_key_for("glm", 2), 0);
    }

    #[test]
    fn test_get_ordered_providers() {
        use crate::config::{LlmConfig, ProviderConfig};

        let llm = LlmConfig {
            order: vec!["qwen".into(), "gemini".into()],
            providers: vec![
                ProviderConfig {
                    name: "gemini".into(),
                    keys: vec!["k1".into()],
                    ..Default::default()
                },
                ProviderConfig {
                    name: "qwen".into(),
                    keys: vec!["k2".into()],
                    ..Default::default()
                },
                ProviderConfig {
                    name: "glm".into(),
                    keys: vec!["k3".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let ordered = get_ordered_providers(&llm);
        assert_eq!(ordered[0].name, "qwen");
        assert_eq!(ordered[1].name, "gemini");
        assert_eq!(ordered[2].name, "glm"); // not in order, appended
    }

    #[test]
    fn api_kind_infers_from_name_and_explicit_field() {
        let grok = ProviderConfig {
            name: "grok".into(),
            ..Default::default()
        };
        let xai = ProviderConfig {
            name: "xai".into(),
            ..Default::default()
        };
        let claude = ProviderConfig {
            name: "claude".into(),
            ..Default::default()
        };
        let custom = ProviderConfig {
            name: "codex-proxy".into(),
            api: "openai".into(),
            base_url: "http://127.0.0.1:4000/v1".into(),
            ..Default::default()
        };
        let gemini = ProviderConfig {
            name: "work".into(),
            api: "gemini".into(),
            ..Default::default()
        };
        let codex = ProviderConfig {
            name: "codex".into(),
            ..Default::default()
        };
        assert_eq!(api_kind(&grok), ApiKind::OpenAi);
        assert_eq!(api_kind(&xai), ApiKind::OpenAi);
        assert_eq!(api_kind(&claude), ApiKind::Anthropic);
        assert_eq!(api_kind(&codex), ApiKind::Codex);
        assert_eq!(api_kind(&custom), ApiKind::OpenAi);
        assert_eq!(api_kind(&gemini), ApiKind::Gemini);
        let studio = ProviderConfig {
            name: "lmstudio".into(),
            ..Default::default()
        };
        assert_eq!(api_kind(&studio), ApiKind::OpenAi);
        let llama = ProviderConfig {
            name: "llamacpp".into(),
            ..Default::default()
        };
        assert_eq!(api_kind(&llama), ApiKind::OpenAi);
    }
}
