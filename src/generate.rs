//! AI-powered TMP schema generator.
//!
//! Runs `<tool> --help` recursively, sends output to an LLM, and
//! saves the resulting schema as JSON to `~/.config/waz/schemas/`.

use crate::config::{Config, ProviderDefaults};
use crate::llm;
use crate::tui::app::{CommandEntry, TokenType};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Directory where generated schemas are stored.
pub fn schemas_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
        .join("waz")
        .join("schemas");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Check if a schema already exists for the given tool.
pub fn schema_exists(tool: &str) -> bool {
    schemas_dir().join(format!("{}.json", tool)).exists()
}

/// Load all JSON schemas from the schemas directory.
pub fn load_all_schemas() -> Vec<CommandEntry> {
    let dir = schemas_dir();
    let mut commands = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return commands,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match serde_json::from_str::<Vec<CommandEntry>>(&content) {
                    Ok(mut entries) => {
                        // Resolve data_source fields to populate dynamic values
                        for entry in &mut entries {
                            resolve_data_sources(entry);
                        }
                        commands.extend(entries);
                    }
                    Err(e) => {
                        eprintln!("Warning: failed to parse schema {}: {}", path.display(), e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to read schema {}: {}", path.display(), e);
            }
        }
    }

    commands
}

/// Resolve any `data_source` fields in tokens by running the specified command.
fn resolve_data_sources(entry: &mut CommandEntry) {
    for token in &mut entry.tokens {
        if let Some(ref ds) = token.data_source {
            let output = Command::new("sh")
                .args(["-c", &ds.command])
                .output();

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let values: Vec<String> = match ds.parse.as_str() {
                    "words" => stdout.split_whitespace().map(|s| s.to_string()).collect(),
                    _ => stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                };

                if !values.is_empty() {
                    token.values = Some(values);
                    token.token_type = TokenType::Enum;
                }
            }
        }
    }
}

/// Generate a TMP schema for a CLI tool using AI.
///
/// 1. Runs `<tool> --help` and subcommand help recursively
/// 2. Sends to LLM with a structured prompt
/// 3. Parses response into Vec<CommandEntry>
/// 4. Saves to ~/.config/waz/schemas/<tool>.json
pub fn generate_schema(config: &Config, tool: &str) -> Result<Vec<CommandEntry>, String> {
    // Step 1: Check tool exists
    let which = Command::new("which").arg(tool).output();
    match which {
        Ok(out) if out.status.success() => {},
        _ => return Err(format!("'{}' not found on PATH", tool)),
    }

    eprintln!("🔍 Detecting {} commands...", tool);

    // Step 2: Gather help text
    let mut help_texts = Vec::new();

    // Main help
    let main_help = run_help(tool, &[]);
    if main_help.is_empty() {
        return Err(format!("'{}' --help produced no output", tool));
    }
    eprintln!("   Running: {} --help", tool);
    help_texts.push(format!("=== {} --help ===\n{}", tool, main_help));

    // Extract subcommands from the main help and run --help on each
    let subcommands = extract_subcommands(&main_help);
    let max_subcommands = 20; // Cap to avoid excessive API calls
    for (i, sub) in subcommands.iter().take(max_subcommands).enumerate() {
        eprintln!("   Running: {} {} --help ({}/{})", tool, sub, i + 1, subcommands.len().min(max_subcommands));
        let sub_help = run_help(tool, &[sub.as_str()]);
        if !sub_help.is_empty() {
            help_texts.push(format!("=== {} {} --help ===\n{}", tool, sub, sub_help));
        }
    }

    eprintln!("\n🤖 Generating schema with AI...");

    // Step 3: Build prompt and call LLM
    let help_combined = help_texts.join("\n\n");
    // Truncate if too long (keep last portion which has subcommands)
    let help_truncated = if help_combined.len() > 12000 {
        &help_combined[help_combined.len() - 12000..]
    } else {
        &help_combined
    };

    let prompt = build_generate_prompt(tool, help_truncated);
    let response = call_llm_for_schema(config, &prompt)?;

    // Step 4: Parse response
    let commands = parse_schema_response(tool, &response)?;

    // Step 5: Save to file
    let schema_path = schemas_dir().join(format!("{}.json", tool));
    let json = serde_json::to_string_pretty(&commands)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&schema_path, &json)
        .map_err(|e| format!("Failed to write schema: {}", e))?;

    eprintln!("   Found {} commands with {} tokens",
        commands.len(),
        commands.iter().map(|c| c.tokens.len()).sum::<usize>()
    );
    eprintln!("\n✅ Saved to {}", schema_path.display());
    eprintln!("   Next time you type /{} in the TUI, these commands will load.", tool);

    Ok(commands)
}

/// Run `<tool> [args...] --help` and return stdout+stderr.
fn run_help(tool: &str, args: &[&str]) -> String {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    cmd.arg("--help");

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!("{}{}", stdout, stderr)
        }
        Err(_) => String::new(),
    }
}

/// Extract subcommand names from help text.
fn extract_subcommands(help: &str) -> Vec<String> {
    let mut subs = Vec::new();
    let mut in_commands = false;

    for line in help.lines() {
        let trimmed = line.trim();

        // Detect command section headers
        if trimmed.to_lowercase().contains("commands:")
            || trimmed.to_lowercase().contains("subcommands:")
            || trimmed.to_lowercase() == "commands"
        {
            in_commands = true;
            continue;
        }

        // End of command section (blank line or new section)
        if in_commands {
            if trimmed.is_empty() {
                // Could be end of section, but allow one blank line
                continue;
            }
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') && trimmed.ends_with(':') {
                in_commands = false;
                continue;
            }

            // Extract first word as subcommand name
            let first_word = trimmed.split_whitespace().next().unwrap_or("");
            // Skip help, version, and meta entries
            if !first_word.is_empty()
                && first_word != "help"
                && first_word != "version"
                && !first_word.starts_with('-')
                && !first_word.starts_with('[')
                && first_word.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                subs.push(first_word.to_string());
            }
        }
    }

    subs
}

/// Build the LLM prompt for schema generation.
fn build_generate_prompt(tool: &str, help_text: &str) -> String {
    format!(
        r#"You are a CLI tool analyzer. Given the help output of '{}', generate a JSON array of command entries.

Each entry must have this exact format:
{{
  "command": "{} <subcommand>",
  "description": "Short description",
  "group": "{}",
  "tokens": [
    {{
      "name": "param_name",
      "description": "Short description",
      "required": true/false,
      "token_type": "String" or "Boolean" or "Enum" or "File" or "Number",
      "default": null or "default_value",
      "values": null or ["option1", "option2"],
      "flag": "--flag-name" or null,
      "data_source": null
    }}
  ]
}}

Rules:
- Include 5-15 of the MOST COMMONLY USED subcommands (not all)
- For each subcommand, include 1-4 of the most important flags/options
- Use token_type "Boolean" for flags like --verbose, --force
- Use token_type "Enum" when there are known choices
- Use token_type "File" for file/path arguments
- Use token_type "Number" for numeric values
- Set "flag" to the CLI flag (e.g. "--verbose", "-n")
- Set "flag" to null for positional arguments
- For data_source: if the values can be dynamically resolved (e.g. installed packages),
  set it to {{"command": "shell command to list values", "parse": "lines"}}
- Output ONLY the JSON array, no markdown, no explanation

Help output:
{}

JSON:"#,
        tool, tool, tool, help_text
    )
}

/// Call the LLM to generate schema JSON.
fn call_llm_for_schema(config: &Config, prompt: &str) -> Result<String, String> {
    let mut state = llm::load_rotation_state();
    let providers = llm::get_ordered_providers_pub(&config.llm);

    if providers.is_empty() {
        return Err("No LLM provider configured. Set GEMINI_API_KEY or configure ~/.config/waz/config.toml".to_string());
    }

    for provider in &providers {
        if provider.keys.is_empty() {
            continue;
        }

        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        let key = match provider.keys.get(key_idx) {
            Some(k) => k,
            None => continue,
        };

        let result = match provider.name.as_str() {
            "gemini" => call_gemini_long(provider, key, prompt),
            _ => call_openai_long(provider, key, prompt),
        };

        if let Some(response) = result {
            state.save();
            return Ok(response);
        }
    }

    state.save();
    Err("All LLM providers failed. Check your API keys.".to_string())
}

/// Call Gemini with higher token limit for schema generation.
fn call_gemini_long(
    provider: &crate::config::ProviderConfig,
    key: &str,
    prompt: &str,
) -> Option<String> {
    let base_url = if provider.base_url.is_empty() {
        ProviderDefaults::base_url("gemini").to_string()
    } else {
        provider.base_url.clone()
    };
    let model = if provider.model.is_empty() {
        ProviderDefaults::model("gemini").to_string()
    } else {
        provider.model.clone()
    };

    let url = format!(
        "{}/models/{}:generateContent?key={}",
        base_url, model, key
    );

    let body = json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {
            "temperature": 0.2,
            "maxOutputTokens": 4096,
        }
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
}

/// Call OpenAI-compatible API with higher token limit.
fn call_openai_long(
    provider: &crate::config::ProviderConfig,
    key: &str,
    prompt: &str,
) -> Option<String> {
    let base_url = if provider.base_url.is_empty() {
        ProviderDefaults::base_url(&provider.name).to_string()
    } else {
        provider.base_url.clone()
    };
    let model = if provider.model.is_empty() {
        ProviderDefaults::model(&provider.name).to_string()
    } else {
        provider.model.clone()
    };

    let url = format!("{}/chat/completions", base_url);
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": 4096,
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", key))
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
}

/// Parse the LLM response into Vec<CommandEntry>.
fn parse_schema_response(tool: &str, response: &str) -> Result<Vec<CommandEntry>, String> {
    // Strip markdown code fences if present
    let trimmed = response.trim();
    let json_str = if trimmed.starts_with("```") {
        // Remove opening fence (```json or ```)
        let after_open = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };
        // Remove closing fence
        let before_close = after_open.trim();
        before_close.strip_suffix("```").unwrap_or(before_close).trim()
    } else {
        trimmed
    };

    let commands: Vec<CommandEntry> = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse AI response as JSON: {}\n\nRaw response:\n{}", e, &response[..response.len().min(500)]))?;

    if commands.is_empty() {
        return Err(format!("AI generated 0 commands for '{}'", tool));
    }

    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_subcommands() {
        let help = r#"
Usage: brew <command> [options]

Commands:
  install       Install a formula or cask
  uninstall     Uninstall a formula or cask
  search        Search for formulae and casks
  list          List installed formulae and casks
  update        Update Homebrew
  upgrade       Upgrade outdated formulae and casks
  info          Show information about a formula or cask
  help          Show help

Options:
  --version     Show version
"#;
        let subs = extract_subcommands(help);
        assert!(subs.contains(&"install".to_string()));
        assert!(subs.contains(&"search".to_string()));
        assert!(subs.contains(&"upgrade".to_string()));
        assert!(!subs.contains(&"help".to_string()));
    }

    #[test]
    fn test_parse_schema_response() {
        let response = r#"```json
[
  {
    "command": "brew install",
    "description": "Install a formula or cask",
    "group": "brew",
    "tokens": [
      {
        "name": "formula",
        "description": "Formula or cask to install",
        "required": true,
        "token_type": "String",
        "default": null,
        "values": null,
        "flag": null,
        "data_source": null
      }
    ]
  }
]
```"#;
        let commands = parse_schema_response("brew", response).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command, "brew install");
        assert_eq!(commands[0].tokens[0].name, "formula");
    }

    #[test]
    fn test_schemas_dir() {
        let dir = schemas_dir();
        assert!(dir.to_str().unwrap().contains("waz"));
        assert!(dir.to_str().unwrap().contains("schemas"));
    }
}
