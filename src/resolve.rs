//! TMP + AI Resolve Engine.
//!
//! Combines TMP schemas (with resolved data sources) and AI to produce
//! grounded, non-hallucinated commands from natural language queries.

use crate::config::Config;
use crate::context::RuntimeContext;
use crate::generate::{load_all_schemas_with_context, resolve_data_sources_pub_ctx};
use crate::tui::app::{assemble_command, App, CommandEntry};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How a tool was chosen for a natural-language query.
///
/// `Query` is a lock (the user named the tool). `Project` is only a ranking
/// hint — cargo in a Rust repo must not hide git/brew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolMatch {
    Query(String),
    Project(String),
}

impl ToolMatch {
    pub fn name(&self) -> &str {
        match self {
            Self::Query(s) | Self::Project(s) => s,
        }
    }

    /// Pass to `resolve` as `tool_filter` only when the query named the tool.
    pub fn filter_tool(&self) -> Option<&str> {
        match self {
            Self::Query(s) => Some(s.as_str()),
            Self::Project(_) => None,
        }
    }

    pub fn prefer_tool(&self) -> Option<&str> {
        match self {
            Self::Project(s) => Some(s.as_str()),
            Self::Query(_) => None,
        }
    }
}

pub fn detect_tool_match(
    query: &str,
    cwd: &str,
    context: Option<&RuntimeContext>,
) -> Option<ToolMatch> {
    let available = list_available_schemas();
    if let Some(tool) = detect_tool_from_query_with_available(query, &available) {
        return Some(ToolMatch::Query(tool));
    }
    detect_project_tool_with_available(cwd, context, &available).map(ToolMatch::Project)
}

/// Scan the query for mentions of available TMP schema tool names.
/// Checks: 1) exact tool name, 2) custom keywords from schema meta, 3) hardcoded aliases.
fn detect_tool_from_query_with_available(query: &str, available: &[String]) -> Option<String> {
    if available.is_empty() {
        return None;
    }

    let query_lower = query.to_lowercase();
    let words: Vec<&str> = query_lower.split_whitespace().collect();

    // Check exact tool name match first (highest confidence)
    for tool in available {
        if words.contains(&tool.as_str()) {
            return Some(tool.clone());
        }
    }

    // Check custom keywords from schema meta
    for tool in available {
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
        ("commit", "git"),
        ("checkout", "git"),
        ("clone", "git"),
        ("python", "pip"),
        ("python3", "pip"),
        ("pip3", "pip"),
        ("golang", "go"),
        ("kubectl", "kubernetes"),
        ("k8s", "kubernetes"),
    ];

    for (alias, target) in aliases {
        if words.contains(alias) && available.iter().any(|t| t == target) {
            return Some((*target).to_string());
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
        if let Some(t) = check("cargo") {
            return Some(t);
        }
    }
    if p.join("package.json").exists() {
        if let Some(t) = check("npm") {
            return Some(t);
        }
    }
    if p.join("go.mod").exists() {
        if let Some(t) = check("go") {
            return Some(t);
        }
    }
    if p.join("Gemfile").exists() {
        if let Some(t) = check("bundler") {
            return Some(t);
        }
    }
    if p.join("pyproject.toml").exists() || p.join("setup.py").exists() {
        if let Some(t) = check("python") {
            return Some(t);
        }
    }
    if p.join(".git").exists() {
        if let Some(t) = check("git") {
            return Some(t);
        }
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
    /// Schema command name (`cargo run`), not a baked argv.
    pub command: String,
    pub tool: String,
    pub explanation: String,
    pub confidence: String,
    pub tokens_filled: Vec<TokenFill>,
    /// `assemble_command` output. Empty when confidence is `none`.
    #[serde(default)]
    pub argv: String,
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
    resolve_with_context(config, query, cwd, tool_filter, None, None)
}

pub fn resolve_with_context(
    config: &Config,
    query: &str,
    cwd: &str,
    tool_filter: Option<&str>,
    context: Option<&RuntimeContext>,
    prefer_tool: Option<&str>,
) -> Result<ResolveResult, String> {
    // Step 1: Load and filter schemas
    let mut commands = load_all_schemas_with_context(cwd, context);

    if commands.is_empty() {
        return Err("No TMP schemas available. Run `waz generate <tool>` first.".to_string());
    }

    // Filter by tool if specified (query named the tool — not a cwd hint)
    if let Some(tool) = tool_filter {
        commands.retain(|c| c.group.to_lowercase() == tool.to_lowercase());
        if commands.is_empty() {
            return Err(format!(
                "No schema found for '{}'. Run `waz generate {}` first.",
                tool, tool
            ));
        }
    }

    if let Some(prefer) = prefer_tool {
        commands.sort_by(|a, b| {
            let ap = a.group.eq_ignore_ascii_case(prefer);
            let bp = b.group.eq_ignore_ascii_case(prefer);
            bp.cmp(&ap)
        });
    }

    // Step 2: Resolve data sources for all commands
    for cmd in &mut commands {
        resolve_data_sources_pub_ctx(cmd, cwd, context);
    }

    // Step 3: Build schema-aware prompt
    let prompt = build_resolve_prompt(query, cwd, &commands, prefer_tool);

    // Step 4: Call LLM
    let raw = call_resolve_llm(config, &prompt)
        .ok_or_else(|| "Failed to get LLM response. Check your API keys.".to_string())?;

    // Step 5: Parse and ground against schemas
    let parsed = parse_resolve_response(&raw)?;
    Ok(ground_resolve_result(parsed, &commands, context))
}

/// Build a prompt that includes TMP schemas with resolved data source values.
fn build_resolve_prompt(
    query: &str,
    cwd: &str,
    commands: &[CommandEntry],
    prefer_tool: Option<&str>,
) -> String {
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

    let prefer = prefer_tool
        .map(|t| {
            format!(
                "\nIf the query is about running/building/testing this project, prefer `{t}` commands. If it names another tool (git, brew, npm, …), pick that tool.\n"
            )
        })
        .unwrap_or_default();

    format!(
        r#"You are a CLI command resolver. Given TMP schemas with REAL resolved data source values, pick the BEST matching command and fill its tokens.

Working directory: {}
{}
Available commands with their tokens:
{}

User query: "{}"

CRITICAL RULES:
- Pick the SINGLE best matching command from the schemas above
- `command` MUST be the schema command name only (e.g. `cargo run`, `git commit`) — never flags or argv
- Fill tokens using ONLY the valid values shown (if values are listed)
- For tokens without listed values, use reasonable values from the query
- If a token is optional and the query doesn't mention it, omit it
- If the query doesn't match ANY available command, set confidence to "none"

Respond ONLY with valid JSON (no markdown, no backticks):
{{
  "command": "cargo run",
  "tool": "the tool group name",
  "explanation": "brief explanation of what this command does",
  "confidence": "high" or "medium" or "low" or "none",
  "tokens_filled": [
    {{"name": "token_name", "value": "filled_value", "source": "how this value was determined"}}
  ]
}}"#,
        cwd, prefer, schema_text, query
    )
}

fn find_schema_command<'a>(commands: &'a [CommandEntry], name: &str) -> Option<&'a CommandEntry> {
    let name = name.trim();
    if let Some(c) = commands.iter().find(|c| c.command == name) {
        return Some(c);
    }
    let mut best: Option<&CommandEntry> = None;
    for c in commands {
        if name == c.command || name.starts_with(&format!("{} ", c.command)) {
            if best
                .map(|b| c.command.len() > b.command.len())
                .unwrap_or(true)
            {
                best = Some(c);
            }
        }
    }
    best
}

fn ground_resolve_result(
    mut parsed: ResolveResult,
    commands: &[CommandEntry],
    context: Option<&RuntimeContext>,
) -> ResolveResult {
    let Some(cmd) = find_schema_command(commands, &parsed.command) else {
        parsed.confidence = "none".into();
        parsed.argv.clear();
        return parsed;
    };
    parsed.command = cmd.command.clone();
    if parsed.tool.is_empty() {
        parsed.tool = cmd.group.clone();
    }
    let mut values = App::default_token_values(cmd, context);
    for fill in &parsed.tokens_filled {
        if let Some(i) = cmd.tokens.iter().position(|t| t.name == fill.name) {
            values[i] = fill.value.clone();
        }
    }
    parsed.argv = assemble_command(cmd, &values);
    parsed
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
        before_close
            .strip_suffix("```")
            .unwrap_or(before_close)
            .trim()
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

fn call_resolve_llm(config: &Config, prompt: &str) -> Option<String> {
    crate::llm::complete(config, prompt, &crate::llm::CompleteOptions::resolve())
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
                repeat: false,
                visible_if: None,
            }],
        }];

        let prompt = build_resolve_prompt("run backend", "/test", &commands, None);
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
    fn ground_resolve_uses_schema_name_and_assemble() {
        let commands = vec![CommandEntry {
            command: "cargo run".into(),
            description: "Run".into(),
            group: "cargo".into(),
            verified: false,
            tokens: vec![TokenDef {
                name: "bin".into(),
                description: "bin".into(),
                required: false,
                token_type: TokenType::Enum,
                default: None,
                values: Some(vec!["waz".into(), "cli".into()]),
                flag: Some("--bin".into()),
                data_source: None,
                repeat: false,
                visible_if: None,
            }],
        }];
        let parsed = ResolveResult {
            command: "cargo run --bin waz".into(),
            tool: "cargo".into(),
            explanation: "run".into(),
            confidence: "high".into(),
            tokens_filled: vec![TokenFill {
                name: "bin".into(),
                value: "waz".into(),
                source: "context".into(),
            }],
            argv: String::new(),
        };
        let grounded = ground_resolve_result(parsed, &commands, None);
        assert_eq!(grounded.command, "cargo run");
        assert_eq!(grounded.argv, "cargo run --bin waz");
        assert_eq!(grounded.confidence, "high");
    }

    #[test]
    fn ground_resolve_unknown_command_is_none() {
        let parsed = ResolveResult {
            command: "rm -rf /".into(),
            tool: "rm".into(),
            explanation: "nope".into(),
            confidence: "high".into(),
            tokens_filled: vec![],
            argv: "rm -rf /".into(),
        };
        let grounded = ground_resolve_result(parsed, &[], None);
        assert_eq!(grounded.confidence, "none");
        assert!(grounded.argv.is_empty());
    }

    #[test]
    fn project_hint_does_not_filter_tool() {
        let m = ToolMatch::Project("cargo".into());
        assert_eq!(m.filter_tool(), None);
        assert_eq!(m.prefer_tool(), Some("cargo"));
        let q = ToolMatch::Query("git".into());
        assert_eq!(q.filter_tool(), Some("git"));
        assert_eq!(q.prefer_tool(), None);
    }

    #[test]
    fn query_names_git_is_lock_not_project_hint() {
        let available = vec!["cargo".into(), "git".into(), "brew".into()];
        assert_eq!(
            detect_tool_from_query_with_available("git commit", &available).as_deref(),
            Some("git")
        );
        assert_eq!(
            detect_tool_from_query_with_available("commit my changes", &available).as_deref(),
            Some("git")
        );
        assert!(
            detect_tool_from_query_with_available("install wget", &available).is_none(),
            "install wget must not lock to cargo"
        );
        let ctx = RuntimeContext {
            file_kind: "cargo_project".into(),
            ..RuntimeContext::default()
        };
        let project = detect_project_tool_with_available("/proj", Some(&ctx), &available);
        assert_eq!(project.as_deref(), Some("cargo"));
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
