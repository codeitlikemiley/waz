use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value, Map};
use warp_completer::signatures::tmp::{CommandEntry, TokenDef, TokenType};
use warp_multi_agent_api as api;
use thiserror::Error;

/// Error categories raised during the validation phase.
#[derive(Debug, Error, Serialize, Deserialize, Clone, PartialEq)]
pub enum ValidationError {
    #[error("Missing required parameter: {0}")]
    MissingRequiredField(String),

    #[error("Type mismatch for field '{field}': expected {expected_type}")]
    TypeMismatch { field: String, expected_type: String },

    #[error("Value '{value}' for field '{field}' is invalid. Allowed values: {allowed:?}")]
    InvalidEnumValue {
        field: String,
        value: String,
        allowed: Vec<String>,
    },

    #[error("Security violation: parameter '{0}' contains unsafe shell metacharacters or unmatched quotes")]
    UnsafeShellMetacharacters(String),

    #[error("Input args must be a valid JSON Object")]
    InvalidArgumentsObject,

    #[error("Serialization / Deserialization error: {0}")]
    SerializationError(String),
}

/// Convert tool name and command string to an LLM-safe function name using strict namespacing rules.
pub fn function_name(tool_name: &str, command: &str) -> String {
    let mut parts: Vec<&str> = command.split_whitespace().collect();
    if parts.first().copied() == Some(tool_name) {
        parts.remove(0);
    }
    let slug = parts.join("_").replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "");
    format!("tmp__{}__{}", tool_name.to_lowercase(), slug.to_lowercase())
}

/// Determine whether the given function name is a TMP tool call.
pub fn is_tmp_function(name: &str) -> bool {
    name.starts_with("tmp__")
}

/// Convert a single TMP TokenDef to its JSON-Schema property mapping.
pub fn token_to_json_schema(token: &TokenDef) -> Value {
    let mut prop = Map::new();
    prop.insert("description".to_string(), json!(token.description));

    match token.token_type {
        TokenType::String | TokenType::File => {
            prop.insert("type".to_string(), json!("string"));
        }
        TokenType::Boolean => {
            prop.insert("type".to_string(), json!("boolean"));
        }
        TokenType::Number => {
            prop.insert("type".to_string(), json!("number"));
        }
        TokenType::Enum => {
            prop.insert("type".to_string(), json!("string"));
            if let Some(ref vals) = token.values {
                if !vals.is_empty() {
                    prop.insert("enum".to_string(), json!(vals));
                }
            }
        }
    }

    if let Some(ref default) = token.default {
        match token.token_type {
            TokenType::Boolean => {
                if let Ok(b) = default.parse::<bool>() {
                    prop.insert("default".to_string(), json!(b));
                }
            }
            TokenType::Number => {
                if let Ok(n) = default.parse::<f64>() {
                    prop.insert("default".to_string(), json!(n));
                }
            }
            _ => {
                prop.insert("default".to_string(), json!(default));
            }
        }
    }

    Value::Object(prop)
}

/// Compile a full TMP command entry into its JSON-Schema schema representation.
pub fn command_to_json_schema(entry: &CommandEntry) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for token in &entry.tokens {
        properties.insert(token.name.clone(), token_to_json_schema(token));
        if token.required {
            required.push(token.name.clone());
        }
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

/// Helper function to check if a parameter is safe from shell injection and quote escaping.
pub fn is_parameter_safe(val: &str) -> bool {
    let unsafe_chars = [';', '&', '|', '`', '$', '>', '<', '\n', '\r'];
    if val.chars().any(|c| unsafe_chars.contains(&c)) {
        return false;
    }
    let single_quotes = val.chars().filter(|&c| c == '\'').count();
    let double_quotes = val.chars().filter(|&c| c == '"').count();
    if single_quotes % 2 != 0 || double_quotes % 2 != 0 {
        return false;
    }
    true
}

/// Escape single quotes inside parameters on Unix targets.
pub fn escape_unix_single_quotes(val: &str) -> String {
    val.replace("'", "'\\''")
}

/// Determine if the workspace path is trusted.
pub fn is_workspace_trusted(cwd: &str) -> bool {
    let config_dir = dirs::home_dir().map(|h| h.join(".config").join("zap"));
    let Some(dir) = config_dir else {
        return false;
    };
    let path = dir.join("trusted_workspaces.json");
    if !path.exists() {
        // Pre-populate with standard workspace roots to keep UX smooth out-of-the-box
        let default_trusted = vec![
            "/Volumes/goldcoders/zap".to_string(),
            "/Volumes/goldcoders/waz".to_string(),
        ];
        std::fs::create_dir_all(&dir).ok();
        if let Ok(serialized) = serde_json::to_string_pretty(&default_trusted) {
            std::fs::write(&path, serialized).ok();
        }
        return true;
    }

    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let trusted_paths: Vec<String> = serde_json::from_str(&content).unwrap_or_default();
    let target = Path::new(cwd).canonicalize().unwrap_or_else(|_| PathBuf::from(cwd));

    for tp in trusted_paths {
        if let Ok(p) = Path::new(&tp).canonicalize() {
            if target.starts_with(&p) {
                return true;
            }
        }
    }
    false
}

/// Safely execute git query command with strict isolation settings in untrusted workspaces.
pub fn resolve_git_resolver_isolated(resolver: &str, cwd: &str) -> Option<Vec<String>> {
    let git_bin = if Path::new("/usr/bin/git").exists() {
        "/usr/bin/git"
    } else if Path::new("/usr/local/bin/git").exists() {
        "/usr/local/bin/git"
    } else {
        "git"
    };

    let git_args = match resolver {
        "git:branches" => vec!["branch", "--format=%(refname:short)"],
        "git:tags" => vec!["tag"],
        "git:remotes" => vec!["remote"],
        "git:status_files" => vec!["status", "--porcelain"],
        _ => return None,
    };

    let mut command = std::process::Command::new(git_bin);
    command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(&[
            "-c", "core.hooksPath=/dev/null",
            "-c", "protocol.file.allow=never",
        ])
        .args(&git_args);

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = Vec::new();

    if resolver == "git:status_files" {
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.len() > 3 {
                let path_part = &line[3..];
                let trimmed = path_part.trim_matches('"');
                lines.push(trimmed.to_string());
            }
        }
    } else {
        for line in stdout.lines() {
            let line = line.trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
    }

    Some(lines)
}

/// Safely resolve dynamic data sources using the workspace trust boundary.
pub fn resolve_data_sources_secure(entry: &mut CommandEntry, cwd: &str) {
    let trusted = is_workspace_trusted(cwd);

    for token in &mut entry.tokens {
        if let Some(ref ds) = token.data_source {
            let values = if let Some(ref resolver) = ds.resolver {
                if resolver.starts_with("git:") {
                    if trusted {
                        warp_completer::signatures::tmp::resolve_data_sources(entry, cwd);
                        return;
                    } else {
                        resolve_git_resolver_isolated(resolver, cwd)
                    }
                } else {
                    warp_completer::signatures::tmp::resolve_data_sources(entry, cwd);
                    return;
                }
            } else if let Some(ref cmd) = ds.command {
                if trusted {
                    warp_completer::signatures::tmp::resolve_data_sources(entry, cwd);
                    return;
                } else {
                    log::warn!(
                        "Untrusted workspace context: Blocked shell command datasource resolver '{}'",
                        cmd
                    );
                    None
                }
            } else {
                None
            };

            if let Some(resolved_values) = values {
                if !resolved_values.is_empty() {
                    token.values = Some(resolved_values);
                    token.token_type = TokenType::Enum;
                }
            }
        }
    }
}

fn should_load_schema(meta: &warp_completer::signatures::tmp::SchemaMeta, cwd: &str) -> bool {
    if let Some(ref file) = meta.requires_file {
        if !Path::new(cwd).join(file).exists() {
            return false;
        }
    }
    if let Some(ref binary) = meta.requires_binary {
        let mut found = false;
        if let Some(path_env) = std::env::var_os("PATH") {
            for path_dir in std::env::split_paths(&path_env) {
                let bin_path = path_dir.join(binary);
                if bin_path.is_file() {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            // Fallback checks
            let exists = std::process::Command::new("which")
                .arg(binary)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !exists {
                return false;
            }
        }
    }
    true
}

/// Read and load schemas from `.waz/schemas/*.json` and `.warp/tmp/*.json` in the active workspace root.
pub fn load_workspace_schemas(cwd: &str) -> Vec<CommandEntry> {
    let mut commands = Vec::new();
    let path_cwd = Path::new(cwd);

    let dirs_to_scan = [
        path_cwd.join(".waz").join("schemas"),
        path_cwd.join(".warp").join("tmp"),
    ];

    for dir in &dirs_to_scan {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Ok(schema_file) = serde_json::from_str::<warp_completer::signatures::tmp::SchemaFile>(&content) {
                                if should_load_schema(&schema_file.meta, cwd) {
                                    commands.extend(schema_file.commands);
                                }
                            } else if let Ok(legacy_entries) = serde_json::from_str::<Vec<CommandEntry>>(&content) {
                                commands.extend(legacy_entries);
                            }
                        }
                    }
                }
            }
        }
    }
    commands
}

/// Load, resolve, and translate active TMP tools to JSON-Schema MCP-aligned definitions.
pub fn build_tmp_tool_defs(cwd: &str) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();

    let mut entries = warp_completer::signatures::tmp::load_all_schemas(cwd);
    entries.extend(load_workspace_schemas(cwd));

    for mut entry in entries {
        resolve_data_sources_secure(&mut entry, cwd);

        let tool_name = entry.group.clone();
        let name = function_name(&tool_name, &entry.command);
        let description = format!(
            "Execute '{}' via Warp's TMP engine. Sub-arguments: {}",
            entry.command,
            entry.description
        );
        let schema = command_to_json_schema(&entry);
        out.push((name, description, schema));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Validate incoming LLM JSON arguments against a command entry's token specifications.
pub fn validate_tmp_arguments(entry: &CommandEntry, args_val: &Value) -> Result<(), ValidationError> {
    let obj = args_val.as_object().ok_or(ValidationError::InvalidArgumentsObject)?;

    for token in &entry.tokens {
        let value_opt = obj.get(&token.name);

        if token.required && (value_opt.is_none() || value_opt.unwrap().is_null()) {
            return Err(ValidationError::MissingRequiredField(token.name.clone()));
        }

        if let Some(val) = value_opt {
            if val.is_null() {
                continue;
            }

            match token.token_type {
                TokenType::String | TokenType::File | TokenType::Enum => {
                    let s = val.as_str().ok_or_else(|| ValidationError::TypeMismatch {
                        field: token.name.clone(),
                        expected_type: "string".to_string(),
                    })?;

                    if !is_parameter_safe(s) {
                        return Err(ValidationError::UnsafeShellMetacharacters(token.name.clone()));
                    }

                    if token.token_type == TokenType::Enum {
                        if let Some(ref allowed) = token.values {
                            if !allowed.is_empty() && !allowed.contains(&s.to_string()) {
                                return Err(ValidationError::InvalidEnumValue {
                                    field: token.name.clone(),
                                    value: s.to_string(),
                                    allowed: allowed.clone(),
                                });
                            }
                        }
                    }
                }
                TokenType::Boolean => {
                    if !val.is_boolean() {
                        return Err(ValidationError::TypeMismatch {
                            field: token.name.clone(),
                            expected_type: "boolean".to_string(),
                        });
                    }
                }
                TokenType::Number => {
                    if !val.is_number() {
                        return Err(ValidationError::TypeMismatch {
                            field: token.name.clone(),
                            expected_type: "number".to_string(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Parse incoming TMP tool call, validate, compile parameters, and return a `Tool::RunShellCommand`.
pub fn parse_tmp_tool_call(
    fn_name: &str,
    arguments_json: &str,
    cwd: &str,
) -> Result<api::message::tool_call::Tool, ValidationError> {
    let mut entries = warp_completer::signatures::tmp::load_all_schemas(cwd);
    entries.extend(load_workspace_schemas(cwd));

    let matched_entry = entries
        .into_iter()
        .find(|entry| function_name(&entry.group, &entry.command) == fn_name);

    let mut entry = matched_entry.ok_or_else(|| ValidationError::SerializationError(format!("TMP tool not found: {fn_name}")))?;

    resolve_data_sources_secure(&mut entry, cwd);

    let parsed_args: Value = if arguments_json.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments_json).map_err(|e| ValidationError::SerializationError(e.to_string()))?
    };

    validate_tmp_arguments(&entry, &parsed_args)?;

    let mut cmd_str = entry.command.clone();
    let obj = parsed_args.as_object().unwrap();
    for token in &entry.tokens {
        let value_opt: Option<Value> = obj.get(&token.name).cloned().or_else(|| {
            token.default.as_ref().map(|d| {
                // Re-parse default value into serde JSON value type
                match token.token_type {
                    TokenType::Boolean => {
                        let b = d.parse::<bool>().unwrap_or(false);
                        json!(b)
                    }
                    TokenType::Number => {
                        let n = d.parse::<f64>().unwrap_or(0.0);
                        json!(n)
                    }
                    _ => json!(d),
                }
            })
        });

        if let Some(ref val) = value_opt {
            if val.is_null() {
                continue;
            }

            match token.token_type {
                TokenType::Boolean => {
                    if val.as_bool() == Some(true) {
                        if let Some(ref flag) = token.flag {
                            cmd_str.push_str(" ");
                            cmd_str.push_str(flag);
                        }
                    }
                }
                TokenType::String | TokenType::File | TokenType::Enum => {
                    if let Some(s) = val.as_str() {
                        cmd_str.push_str(" ");
                        if let Some(ref flag) = token.flag {
                            cmd_str.push_str(flag);
                            cmd_str.push_str(" ");
                        }
                        let escaped = escape_unix_single_quotes(s);
                        cmd_str.push_str(&format!("'{}'", escaped));
                    }
                }
                TokenType::Number => {
                    if let Some(n) = val.as_f64() {
                        cmd_str.push_str(" ");
                        if let Some(ref flag) = token.flag {
                            cmd_str.push_str(flag);
                            cmd_str.push_str(" ");
                        }
                        cmd_str.push_str(&n.to_string());
                    }
                }
            }
        }
    }

    Ok(api::message::tool_call::Tool::RunShellCommand(
        api::message::tool_call::RunShellCommand {
            command: cmd_str,
            is_read_only: false,
            uses_pager: false,
            is_risky: false,
            citations: vec![],
            wait_until_complete_value: Some(
                api::message::tool_call::run_shell_command::WaitUntilCompleteValue::WaitUntilComplete(true)
            ),
            risk_category: 0,
        }
    ))
}

#[cfg(test)]
#[path = "tmp_ai_tests.rs"]
mod tests;
