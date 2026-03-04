use crate::config::Config;
use serde_json::json;
use std::time::Duration;

/// A suggested command from the AI with metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SuggestedCommand {
    pub cmd: String,
    pub desc: String,
    #[serde(default)]
    pub placeholders: Vec<String>,
}

/// Structured AI response with explanation and suggested commands.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StructuredResponse {
    pub explanation: String,
    pub commands: Vec<SuggestedCommand>,
}

/// Result from asking the LLM a natural language question (legacy).
#[derive(Debug)]
pub struct AskResult {
    pub response: String,
    pub suggested_command: Option<String>,
}

/// Ask the LLM and return a structured response with commands array.
pub fn ask_structured(
    config: &Config,
    query: &str,
    cwd: &str,
    recent_commands: &[String],
) -> Option<StructuredResponse> {
    let prompt = build_structured_prompt(query, cwd, recent_commands);
    let raw = call_ask_llm(config, &prompt)?;

    // Try JSON parsing first
    if let Some(parsed) = parse_structured_json(&raw) {
        return Some(parsed);
    }

    // Fallback: parse text format (extract $ commands)
    Some(fallback_parse(&raw))
}

/// Ask the LLM a natural language question (legacy text format).
pub fn ask(config: &Config, query: &str, cwd: &str, recent_commands: &[String]) -> Option<AskResult> {
    let prompt = build_ask_prompt(query, cwd, recent_commands);
    let response = call_ask_llm(config, &prompt)?;
    let suggested_command = extract_command(&response);
    Some(AskResult {
        response,
        suggested_command,
    })
}

fn build_structured_prompt(query: &str, cwd: &str, recent_commands: &[String]) -> String {
    let history = if recent_commands.is_empty() {
        String::new()
    } else {
        let cmds: Vec<String> = recent_commands
            .iter()
            .enumerate()
            .map(|(i, cmd)| format!("{}. {}", i + 1, cmd))
            .collect();
        format!("\nRecent commands:\n{}", cmds.join("\n"))
    };

    format!(
        r#"You are a shell assistant. The user typed a query into their terminal.

Working directory: {}{}

User query: "{}"

Respond ONLY with valid JSON (no markdown, no backticks, no extra text) in this exact format:
{{
  "explanation": "Brief explanation of what to do",
  "commands": [
    {{"cmd": "the_command --with <placeholder>", "desc": "short description"}}
  ]
}}

Rules:
- For variable parts use angle-bracket placeholders: <filename>, <search_term>, <package_name>
- Include 1-4 relevant command variations
- Keep explanation to 1-2 sentences
- Each command should be a single runnable shell command
- If the query is a factual question with NO commands, use an empty commands array"#,
        cwd, history, query
    )
}

/// Try to parse the LLM response as structured JSON.
fn parse_structured_json(raw: &str) -> Option<StructuredResponse> {
    // Strip markdown code fences if present
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let mut resp: StructuredResponse = serde_json::from_str(cleaned).ok()?;

    // Auto-detect placeholders from angle brackets in each command
    for cmd in &mut resp.commands {
        cmd.placeholders = extract_placeholders(&cmd.cmd);
    }

    Some(resp)
}

/// Extract <placeholder> names from a command string.
fn extract_placeholders(cmd: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut rest = cmd;
    while let Some(start) = rest.find('<') {
        if let Some(end) = rest[start..].find('>') {
            let name = &rest[start + 1..start + end];
            if !name.is_empty() && !placeholders.contains(&name.to_string()) {
                placeholders.push(name.to_string());
            }
            rest = &rest[start + end + 1..];
        } else {
            break;
        }
    }
    placeholders
}

/// Fallback: parse plain text response into StructuredResponse.
fn fallback_parse(raw: &str) -> StructuredResponse {
    let mut explanation_lines = Vec::new();
    let mut commands = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(cmd_str) = trimmed.strip_prefix("$ ") {
            let cmd_str = cmd_str.trim().to_string();
            let placeholders = extract_placeholders(&cmd_str);
            commands.push(SuggestedCommand {
                cmd: cmd_str,
                desc: String::new(),
                placeholders,
            });
        } else if !trimmed.is_empty() {
            explanation_lines.push(trimmed.to_string());
        }
    }

    StructuredResponse {
        explanation: explanation_lines.join("\n"),
        commands,
    }
}

fn build_ask_prompt(query: &str, cwd: &str, recent_commands: &[String]) -> String {
    let history = if recent_commands.is_empty() {
        String::new()
    } else {
        let cmds: Vec<String> = recent_commands
            .iter()
            .enumerate()
            .map(|(i, cmd)| format!("{}. {}", i + 1, cmd))
            .collect();
        format!("\nRecent commands:\n{}", cmds.join("\n"))
    };

    format!(
        "You are a helpful shell assistant. The user typed a natural language query into their terminal.

Working directory: {}{}

User query: \"{}\"

Rules:
- If the query asks HOW to do something, provide the exact shell command(s) to run
- If the query asks a factual question, provide a concise answer
- If suggesting commands, wrap each command in a line starting with exactly `$ ` (dollar-space)
- For variable parts in commands, ALWAYS use angle-bracket placeholders like <filename>, <search_term>, <package_name> — never use quoted example values like \"search_term\" or bare words like filename
- Keep responses short and terminal-friendly (no long paragraphs)
- Use plain text, no markdown formatting",
        cwd, history, query
    )
}

/// Complete a partial natural language sentence (for inline ghost text).
/// Returns only the CONTINUATION, not the full sentence.
pub fn complete_sentence(config: &Config, partial: &str) -> Option<String> {
    let prompt = format!(
        "Complete this text as if the user is typing a question or request in a terminal. \
Return ONLY the remaining words to finish the sentence. Do NOT repeat what the user already typed. \
Keep it short (max 5-8 words to finish the thought).

User is typing: \"{}\"
Completion (just the remaining words):",
        partial
    );

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

        // Short timeout (2s) for inline completion — it's ghost text, not blocking
        let result = match provider.name.as_str() {
            "gemini" => call_gemini_ask(provider, key_idx, &prompt, 2),
            "ollama" => call_ollama_ask(provider, &prompt, 2),
            _ => call_openai_ask(provider, key_idx, &prompt, 2),
        };

        if let Some(r) = result {
            state.save();
            // Clean up the response — remove quotes, trim, ensure it doesn't repeat the input
            let cleaned = r.trim().trim_matches('"').trim_matches('\'').trim();
            if cleaned.is_empty() {
                continue;
            }
            // If the LLM repeated the input, strip it
            let completion = if cleaned.to_lowercase().starts_with(&partial.to_lowercase()) {
                cleaned[partial.len()..].trim_start().to_string()
            } else {
                cleaned.to_string()
            };
            if completion.is_empty() {
                continue;
            }
            return Some(completion);
        }
    }

    state.save();
    None
}

/// Call the LLM for an ask query (uses longer timeout since user is waiting).
fn call_ask_llm(config: &Config, prompt: &str) -> Option<String> {
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

        // Use longer timeout for ask (10s) since user is waiting for a response
        let result = match provider.name.as_str() {
            "gemini" => call_gemini_ask(provider, key_idx, prompt, 10),
            "ollama" => call_ollama_ask(provider, prompt, 10),
            _ => call_openai_ask(provider, key_idx, prompt, 10),
        };

        if let Some(r) = result {
            state.save();
            return Some(r);
        }
    }

    state.save();
    None
}

fn call_gemini_ask(
    provider: &crate::config::ProviderConfig,
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
            "temperature": 0.3,
            "maxOutputTokens": 500
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

fn call_openai_ask(
    provider: &crate::config::ProviderConfig,
    key_idx: usize,
    prompt: &str,
    timeout: u64,
) -> Option<String> {
    let key = provider.keys.get(key_idx)?;
    let url = format!("{}/chat/completions", provider.base_url);

    let body = json!({
        "model": provider.model,
        "messages": [
            {"role": "system", "content": "You are a helpful shell assistant. Keep responses short and terminal-friendly."},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.3,
        "max_tokens": 500
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

fn call_ollama_ask(
    provider: &crate::config::ProviderConfig,
    prompt: &str,
    timeout: u64,
) -> Option<String> {
    let url = format!("{}/api/generate", provider.base_url);
    let body = json!({
        "model": provider.model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": 0.3,
            "num_predict": 500
        }
    });

    let resp = ureq::post(&url)
        .timeout(Duration::from_secs(timeout))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["response"].as_str().map(|s| s.trim().to_string())
}

/// Extract the first suggested command from the LLM response.
/// Looks for lines starting with `$ `, backtick-wrapped commands, etc.
fn extract_command(response: &str) -> Option<String> {
    // Look for `$ command` pattern
    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(cmd) = trimmed.strip_prefix("$ ") {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                return Some(cmd.to_string());
            }
        }
    }

    // Look for inline backtick-wrapped commands: `command args`
    // Find the first `...` that looks like a command
    if let Some(start) = response.find('`') {
        if !response[start..].starts_with("```") {
            if let Some(end) = response[start + 1..].find('`') {
                let cmd = response[start + 1..start + 1 + end].trim();
                if !cmd.is_empty() && cmd.split_whitespace().count() >= 1 {
                    return Some(cmd.to_string());
                }
            }
        }
    }

    None
}

/// Check if input looks like natural language rather than a mistyped command.
pub fn is_natural_language(input: &str) -> bool {
    let words: Vec<&str> = input.split_whitespace().collect();

    // 3+ words is almost certainly natural language
    if words.len() >= 3 {
        return true;
    }

    // 2 words starting with a question/action word
    if words.len() == 2 {
        let first = words[0].to_lowercase();
        let nl_starters = [
            "how", "what", "why", "where", "when", "which", "who",
            "can", "do", "does", "is", "are", "show", "list", "find",
            "create", "make", "delete", "remove", "install", "update",
            "uninstall", "upgrade", "check", "search", "open", "close",
            "start", "stop", "restart", "kill", "run", "build", "deploy",
            "explain", "describe", "compare", "convert", "generate",
            "download", "upload", "compress", "extract", "mount",
        ];
        return nl_starters.contains(&first.as_str());
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_command_dollar() {
        let resp = "To install Rust, run:\n$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh";
        assert_eq!(
            extract_command(resp),
            Some("curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh".into())
        );
    }

    #[test]
    fn test_extract_command_backtick() {
        let resp = "You can check with `rustc --version`";
        assert_eq!(extract_command(resp), Some("rustc --version".into()));
    }

    #[test]
    fn test_extract_command_none() {
        let resp = "Rust is a systems programming language.";
        assert_eq!(extract_command(resp), None);
    }

    #[test]
    fn test_is_natural_language() {
        assert!(is_natural_language("how to install rust"));
        assert!(is_natural_language("whats my ip address"));
        assert!(is_natural_language("delete all docker containers"));
        assert!(is_natural_language("show disk usage"));
        assert!(is_natural_language("find large files"));
        assert!(!is_natural_language("gti")); // typo, 1 word
        assert!(!is_natural_language("htop")); // just a command name
    }
}
