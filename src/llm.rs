use crate::config::{Config, ProviderConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::time::Duration;

/// Rotation state persisted between invocations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RotationState {
    /// Index of the last provider used (for round-robin)
    provider_index: usize,
    /// Per-provider key index (for key rotation within a provider)
    key_indices: std::collections::HashMap<String, usize>,
}

impl RotationState {
    fn load() -> Self {
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

    fn save(&self) {
        let path = Config::rotation_state_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, serde_json::to_string(self).unwrap_or_default());
    }

    fn next_key_for(&mut self, provider_name: &str, num_keys: usize) -> usize {
        if num_keys == 0 {
            return 0;
        }
        let idx = self.key_indices.entry(provider_name.to_string()).or_insert(0);
        let current = *idx;
        *idx = (current + 1) % num_keys;
        current
    }
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
        if provider.keys.is_empty() && provider.name != "ollama" {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        if let Some(result) = call_provider(provider, key_idx, prompt, llm.timeout_secs) {
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

    // Try starting from current index, wrapping around
    for i in 0..ordered.len() {
        let idx = (start + i) % ordered.len();
        let provider = &ordered[idx];
        if provider.keys.is_empty() && provider.name != "ollama" {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        if let Some(result) = call_provider(provider, key_idx, prompt, llm.timeout_secs) {
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
    if provider.keys.is_empty() && provider.name != "ollama" {
        return None;
    }
    let key_idx = state.next_key_for(&provider.name, provider.keys.len());
    call_provider(provider, key_idx, prompt, llm.timeout_secs)
}

/// Get providers sorted by the configured order.
fn get_ordered_providers(llm: &crate::config::LlmConfig) -> Vec<&ProviderConfig> {
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

/// Dispatch to the right API format based on provider name.
fn call_provider(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    timeout: u64,
) -> Option<String> {
    match provider.name.as_str() {
        "gemini" => call_gemini(provider, key_idx, prompt, timeout),
        "ollama" => call_ollama(provider, prompt, timeout),
        // All others use OpenAI-compatible format
        _ => call_openai_compatible(provider, key_idx, prompt, timeout),
    }
}

// ── API Calls ──────────────────────────────────────────────────────

fn call_gemini(
    provider: &ProviderConfig,
    key_idx: usize,
    prompt: &str,
    timeout: u64,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let url = format!(
        "{}/models/{}:generateContent?key={}",
        provider.base_url, provider.model, key
    );

    let body = json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {
            "temperature": 0.1,
            "maxOutputTokens": 100,
            "stopSequences": ["\n"]
        }
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(timeout))
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
    timeout: u64,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let url = format!("{}/chat/completions", provider.base_url);

    let body = json!({
        "model": provider.model,
        "messages": [
            {
                "role": "system",
                "content": "You are a shell command predictor. Respond with ONLY the predicted command, nothing else. No explanation, no quotes, no markdown."
            },
            {
                "role": "user",
                "content": prompt
            }
        ],
        "temperature": 0.1,
        "max_tokens": 100
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(timeout))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
}

fn call_ollama(
    provider: &ProviderConfig,
    prompt: &str,
    timeout: u64,
) -> Option<String> {
    let url = format!("{}/api/generate", provider.base_url);
    let body = json!({
        "model": provider.model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.1,
            "num_predict": 100,
            "stop": ["\n"]
        }
    });

    let resp = ureq::post(&url)
        .timeout(Duration::from_secs(timeout))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
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
- Just the raw shell command on a single line",
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
        assert_eq!(clean_response("\"cargo build\"", None), Some("cargo build".into()));
        assert_eq!(clean_response("$ npm install", None), Some("npm install".into()));
        assert_eq!(clean_response("", None), None);
    }

    #[test]
    fn test_clean_response_with_prefix() {
        assert_eq!(clean_response("git push", Some("git")), Some("git push".into()));
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
                    base_url: "".into(),
                    keys: vec!["k1".into()],
                    model: "".into(),
                },
                ProviderConfig {
                    name: "qwen".into(),
                    base_url: "".into(),
                    keys: vec!["k2".into()],
                    model: "".into(),
                },
                ProviderConfig {
                    name: "glm".into(),
                    base_url: "".into(),
                    keys: vec!["k3".into()],
                    model: "".into(),
                },
            ],
            ..Default::default()
        };

        let ordered = get_ordered_providers(&llm);
        assert_eq!(ordered[0].name, "qwen");
        assert_eq!(ordered[1].name, "gemini");
        assert_eq!(ordered[2].name, "glm"); // not in order, appended
    }
}
