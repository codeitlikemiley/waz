//! TMP + AI Resolve Engine.
//!
//! Combines TMP schemas (with resolved data sources) and AI to produce
//! grounded, non-hallucinated commands from natural language queries.

use crate::config::Config;
use crate::context::RuntimeContext;
use crate::generate::{load_all_schemas_with_context, resolve_data_sources_pub_ctx};
use crate::tui::app::CommandEntry;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Detect the best TMP tool to use, checking in priority order:
/// 1. Query keyword match (user explicitly mentions a tool name)
/// 2. CWD project file match (e.g., Cargo.toml → cargo)
pub fn detect_best_tool(query: &str, cwd: &str) -> Option<String> {
    detect_best_tool_with_context(query, cwd, None)
}

pub fn detect_best_tool_with_context(
    query: &str,
    cwd: &str,
    context: Option<&RuntimeContext>,
) -> Option<String> {
    // Priority 1: keyword match from query
    if let Some(tool) = detect_tool_from_query(query) {
        return Some(tool);
    }

    // Priority 2: CWD-based project file detection
    detect_project_tool(cwd, context)
}

/// Scan the query for mentions of available TMP schema tool names.
/// Checks: 1) exact tool name, 2) custom keywords from schema meta, 3) hardcoded aliases.
fn detect_tool_from_query(query: &str) -> Option<String> {
    let available = list_available_schemas();
    if available.is_empty() {
        return None;
    }

    let query_lower = query.to_lowercase();
    let words: Vec<&str> = query_lower.split_whitespace().collect();

    // Check exact tool name match first (highest confidence)
    for tool in &available {
        if words.contains(&tool.as_str()) {
            return Some(tool.clone());
        }
    }

    // Check custom keywords from schema meta
    for tool in &available {
        if let Some(keywords) = load_schema_keywords(tool) {
            for kw in &keywords {
                let kw_lower = kw.to_lowercase();
                if words.contains(&kw_lower.as_str()) {
                    return Some(tool.clone());
                }
            }
        }
    }

    // Fallback: hardcoded aliases for common tools
    let aliases: &[(&str, &str)] = &[
        ("postgres", "psql"),
        ("postgresql", "psql"),
        ("node", "npm"),
        ("nodejs", "npm"),
        ("yarn", "npm"),
        ("pnpm", "npm"),
        ("rust", "cargo"),
        ("rustc", "cargo"),
        ("homebrew", "brew"),
        ("python", "pip"),
        ("python3", "pip"),
        ("pip3", "pip"),
        ("golang", "go"),
        ("kubectl", "kubernetes"),
        ("k8s", "kubernetes"),
    ];

    for (alias, target) in aliases {
        if words.contains(alias) && available.contains(&target.to_string()) {
            return Some(target.to_string());
        }
    }

    None
}

/// Load just the keywords from a schema's meta (lightweight — doesn't parse commands).
fn load_schema_keywords(tool: &str) -> Option<Vec<String>> {
    let path = crate::generate::schemas_dir().join(format!("{}.json", tool));
    let content = std::fs::read_to_string(&path).ok()?;
    let schema: crate::tui::app::SchemaFile = serde_json::from_str(&content).ok()?;
    if schema.meta.keywords.is_empty() {
        None
    } else {
        Some(schema.meta.keywords)
    }
}

/// List all available schema tool names (just filenames, no loading).
fn list_available_schemas() -> Vec<String> {
    let dir = crate::generate::schemas_dir();
    let mut tools = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    tools.push(stem.to_string());
                }
            }
        }
    }
    tools
}

/// Detect the most relevant tool based on project files in CWD.
/// Returns None if no known project files are found.
pub fn detect_project_tool(cwd: &str, context: Option<&RuntimeContext>) -> Option<String> {
    let available = list_available_schemas();
    detect_project_tool_with_available(cwd, context, &available)
}

fn detect_project_tool_with_available(
    cwd: &str,
    context: Option<&RuntimeContext>,
    available: &[String],
) -> Option<String> {
    let p = Path::new(cwd);

    // Only return tools that actually have schemas
    let check = |tool: &str| -> Option<String> {
        if available.iter().any(|t| t == tool) {
            Some(tool.to_string())
        } else {
            None
        }
    };

    if let Some(context) = context {
        if context.file_kind == "single_file_script" {
            let rust_script_available = available.iter().any(|t| t == "rust-script");
            let cargo_script_available = available.iter().any(|t| t == "cargo-script");

            if let Some(engine) = context.script_engine.as_deref() {
                if engine == "rust-script" {
                    if rust_script_available {
                        return Some("rust-script".to_string());
                    }
                    if cargo_script_available {
                        return Some("cargo-script".to_string());
                    }
                } else if cargo_script_available {
                    return Some("cargo-script".to_string());
                } else if rust_script_available {
                    return Some("rust-script".to_string());
                }
            } else if cargo_script_available {
                return Some("cargo-script".to_string());
            } else if rust_script_available {
                return Some("rust-script".to_string());
            }
        }

        if context.file_kind == "cargo_project" && available.iter().any(|t| t == "cargo") {
            return Some("cargo".to_string());
        }

        if let Some(root) = context.project_root.as_deref() {
            let root_path = Path::new(root);
            if root_path.join("Cargo.toml").exists() && available.iter().any(|t| t == "cargo") {
                return Some("cargo".to_string());
            }
        }
    }

    if p.join("Cargo.toml").exists() {
        if let Some(t) = check("cargo") { return Some(t); }
    }
    if p.join("package.json").exists() {
        if let Some(t) = check("npm") { return Some(t); }
    }
    if p.join("go.mod").exists() {
        if let Some(t) = check("go") { return Some(t); }
    }
    if p.join("Gemfile").exists() {
        if let Some(t) = check("bundler") { return Some(t); }
    }
    if p.join("pyproject.toml").exists() || p.join("setup.py").exists() {
        if let Some(t) = check("python") { return Some(t); }
    }
    if p.join(".git").exists() {
        if let Some(t) = check("git") { return Some(t); }
    }
    None
}

/// A filled token with its source information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenFill {
    pub name: String,
    pub value: String,
    pub source: String,
}

/// Result of resolving a natural language query against TMP schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveResult {
    pub command: String,
    pub tool: String,
    pub explanation: String,
    pub confidence: String,
    pub tokens_filled: Vec<TokenFill>,
}

/// Resolve a natural language query into a grounded command using TMP schemas.
///
/// 1. Loads all available schemas (filtered by CWD context)
/// 2. Resolves data sources (cargo:packages, git:branches, etc.)
/// 3. Builds a schema-aware prompt with real values
/// 4. Calls LLM to pick the best command and fill tokens
pub fn resolve(
    config: &Config,
    query: &str,
    cwd: &str,
    tool_filter: Option<&str>,
) -> Result<ResolveResult, String> {
    resolve_with_context(config, query, cwd, tool_filter, None)
}

pub fn resolve_with_context(
    config: &Config,
    query: &str,
    cwd: &str,
    tool_filter: Option<&str>,
    context: Option<&RuntimeContext>,
) -> Result<ResolveResult, String> {
    // Step 1: Load and filter schemas
    let mut commands = load_all_schemas_with_context(cwd, context);

    if commands.is_empty() {
        return Err("No TMP schemas available. Run `waz generate <tool>` first.".to_string());
    }

    // Filter by tool if specified
    if let Some(tool) = tool_filter {
        commands.retain(|c| c.group.to_lowercase() == tool.to_lowercase());
        if commands.is_empty() {
            return Err(format!(
                "No schema found for '{}'. Run `waz generate {}` first.",
                tool, tool
            ));
        }
    }

    // Step 2: Resolve data sources for all commands
    for cmd in &mut commands {
        resolve_data_sources_pub_ctx(cmd, cwd, context);
    }

    // Step 3: Build schema-aware prompt
    let prompt = build_resolve_prompt(query, cwd, &commands);

    // Step 4: Call LLM
    let raw = call_resolve_llm(config, &prompt)
        .ok_or_else(|| "Failed to get LLM response. Check your API keys.".to_string())?;

    // Step 5: Parse response
    parse_resolve_response(&raw)
}

/// Build a prompt that includes TMP schemas with resolved data source values.
fn build_resolve_prompt(query: &str, cwd: &str, commands: &[CommandEntry]) -> String {
    let mut schema_text = String::new();

    for (i, cmd) in commands.iter().enumerate() {
        schema_text.push_str(&format!("\n{}. `{}`", i + 1, cmd.command));
        if !cmd.description.is_empty() {
            schema_text.push_str(&format!(" — {}", cmd.description));
        }
        schema_text.push('\n');

        for token in &cmd.tokens {
            let required = if token.required { " (REQUIRED)" } else { "" };
            let flag_str = match &token.flag {
                Some(f) => format!(" flag: {}", f),
                None => " (positional)".to_string(),
            };

            schema_text.push_str(&format!(
                "   - {}:{}{} — {}",
                token.name, flag_str, required, token.description
            ));

            // Show default if set
            if let Some(default) = &token.default {
                schema_text.push_str(&format!(" [default: {}]", default));
            }

            // Show resolved values (the key innovation — real data, not guesses)
            if let Some(values) = &token.values {
                if !values.is_empty() {
                    let display: Vec<&str> = values.iter().take(20).map(|s| s.as_str()).collect();
                    schema_text.push_str(&format!("\n     valid values: {:?}", display));
                    if values.len() > 20 {
                        schema_text.push_str(&format!(" ... ({} total)", values.len()));
                    }
                }
            }
            schema_text.push('\n');
        }
    }

    format!(
        r#"You are a CLI command resolver. Given TMP schemas with REAL resolved data source values, pick the BEST matching command and fill its tokens.

Working directory: {}

Available commands with their tokens:
{}

User query: "{}"

CRITICAL RULES:
- Pick the SINGLE best matching command from the schemas above
- Fill tokens using ONLY the valid values shown (if values are listed)
- For tokens without listed values, use reasonable values from the query
- If a token is optional and the query doesn't mention it, omit it
- If the query doesn't match ANY available command, set confidence to "none"

Respond ONLY with valid JSON (no markdown, no backticks):
{{
  "command": "the full command with tokens filled",
  "tool": "the tool group name",
  "explanation": "brief explanation of what this command does",
  "confidence": "high" or "medium" or "low" or "none",
  "tokens_filled": [
    {{"name": "token_name", "value": "filled_value", "source": "how this value was determined"}}
  ]
}}"#,
        cwd, schema_text, query
    )
}

/// Parse the LLM response into a ResolveResult.
fn parse_resolve_response(raw: &str) -> Result<ResolveResult, String> {
    let trimmed = raw.trim();

    // Strip markdown code fences if present
    let json_str = if trimmed.starts_with("```") {
        let after_open = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };
        let before_close = after_open.trim();
        before_close.strip_suffix("```").unwrap_or(before_close).trim()
    } else {
        trimmed
    };

    serde_json::from_str::<ResolveResult>(json_str).map_err(|e| {
        format!(
            "Failed to parse resolve response: {}\n\nRaw: {}",
            e,
            &json_str[..json_str.len().min(300)]
        )
    })
}

/// Call the LLM for resolve queries (reuses ask module's LLM infrastructure).
fn call_resolve_llm(config: &Config, prompt: &str) -> Option<String> {
    let llm = &config.llm;
    if llm.providers.is_empty() {
        return None;
    }

    let mut state = crate::llm::load_rotation_state();
    let ordered = crate::llm::get_ordered_providers_pub(llm);

    for provider in &ordered {
        if provider.keys.is_empty() && provider.name != "ollama" {
            continue;
        }
        let key_idx = state.next_key_for(&provider.name, provider.keys.len());

        let result = match provider.name.as_str() {
            "gemini" => call_gemini_resolve(provider, key_idx, prompt),
            "ollama" => call_ollama_resolve(provider, prompt),
            _ => call_openai_resolve(provider, key_idx, prompt),
        };

        if let Some(r) = result {
            state.save();
            return Some(r);
        }
    }

    state.save();
    None
}

fn call_gemini_resolve(
    provider: &crate::config::ProviderConfig,
    key_idx: usize,
    prompt: &str,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        provider.model, key
    );
    let body = serde_json::json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {"temperature": 0.1, "maxOutputTokens": 1024}
    });

    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
}

fn call_openai_resolve(
    provider: &crate::config::ProviderConfig,
    key_idx: usize,
    prompt: &str,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let base = if provider.base_url.is_empty() {
        "https://api.openai.com/v1"
    } else {
        &provider.base_url
    };
    let url = format!("{}/chat/completions", base);

    let body = serde_json::json!({
        "model": provider.model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": 1024
    });

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(15))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
}

fn call_ollama_resolve(
    provider: &crate::config::ProviderConfig,
    prompt: &str,
) -> Option<String> {
    let base = if provider.base_url.is_empty() {
        "http://localhost:11434"
    } else {
        &provider.base_url
    };
    let url = format!("{}/api/generate", base);

    let body = serde_json::json!({
        "model": provider.model,
        "prompt": prompt,
        "stream": false,
        "options": {"temperature": 0.1}
    });

    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["response"].as_str().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RuntimeContext;
    use crate::tui::app::{CommandEntry, TokenDef, TokenType};

    #[test]
    fn test_build_resolve_prompt_includes_values() {
        let commands = vec![CommandEntry {
            command: "cargo run".to_string(),
            description: "Run a binary".to_string(),
            group: "cargo".to_string(),
            verified: false,
            tokens: vec![TokenDef {
                name: "package".to_string(),
                description: "Package to run".to_string(),
                required: false,
                token_type: TokenType::String,
                default: None,
                values: Some(vec!["backend".to_string(), "cli".to_string()]),
                flag: Some("--package".to_string()),
                data_source: None,
            }],
        }];

        let prompt = build_resolve_prompt("run backend", "/test", &commands);
        assert!(prompt.contains("cargo run"));
        assert!(prompt.contains("backend"));
        assert!(prompt.contains("cli"));
        assert!(prompt.contains("valid values"));
    }

    #[test]
    fn test_parse_resolve_response() {
        let json = r#"{"command": "cargo run --package backend", "tool": "cargo", "explanation": "Run the backend", "confidence": "high", "tokens_filled": [{"name": "package", "value": "backend", "source": "Cargo.toml"}]}"#;
        let result = parse_resolve_response(json).unwrap();
        assert_eq!(result.command, "cargo run --package backend");
        assert_eq!(result.tool, "cargo");
        assert_eq!(result.confidence, "high");
        assert_eq!(result.tokens_filled.len(), 1);
        assert_eq!(result.tokens_filled[0].value, "backend");
    }

    #[test]
    fn test_parse_resolve_response_with_fences() {
        let json = "```json\n{\"command\": \"git checkout dev\", \"tool\": \"git\", \"explanation\": \"Switch\", \"confidence\": \"high\", \"tokens_filled\": []}\n```";
        let result = parse_resolve_response(json).unwrap();
        assert_eq!(result.command, "git checkout dev");
    }

    #[test]
    fn test_detect_best_tool_uses_script_context() {
        let context = RuntimeContext {
            cwd: "/tmp".to_string(),
            file_kind: "single_file_script".to_string(),
            script_engine: Some("rust-script".to_string()),
            ..RuntimeContext::default()
        };

        let tool = detect_project_tool_with_available(
            "/tmp",
            Some(&context),
            &["rust-script".to_string(), "cargo-script".to_string()],
        );
        assert_eq!(tool.as_deref(), Some("rust-script"));
    }
}
