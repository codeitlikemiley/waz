use std::path::{Path, PathBuf};

/// Extract a suggested command from command output text.
///
/// Scans the output for common patterns where tools suggest follow-up commands:
/// - `Run 'command'` / `Run "command"` / `Run \`command\``
/// - `run: command` / `Next, run: command`
/// - `Try 'command'` / `Execute: command`
/// - Emoji markers like `👉 Run 'command'`
/// - Prompt-style lines: `$ command` or `> command`
pub fn extract_hint(output: &str) -> Option<String> {
    let mut last = None;
    for line in output.lines() {
        if let Some(cmd) = extract_from_line(line) {
            last = Some(cmd);
        }
    }
    last
}

fn extract_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let stripped = strip_leading_decorations(trimmed);

    if let Some(cmd) = extract_prompt_command(stripped) {
        return sanitize_command(&cmd);
    }
    if let Some(cmd) = extract_verb_command(stripped) {
        return sanitize_command(&cmd);
    }
    if let Some(cmd) = extract_mid_line_verb(stripped) {
        return sanitize_command(&cmd);
    }

    None
}

/// Strip leading emojis, bullets, and other decorations while keeping
/// prompt markers (`$`) and quotes intact.
fn strip_leading_decorations(s: &str) -> &str {
    s.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && !"$`'\"./~".contains(c))
}

fn extract_prompt_command(s: &str) -> Option<String> {
    for prefix in ["$ ", "$> ", "> ", "% "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let cmd = rest.trim();
            if looks_like_command(cmd) {
                return Some(cmd.to_string());
            }
        }
    }
    None
}

fn extract_verb_command(s: &str) -> Option<String> {
    let s = strip_leading_phrases(s);
    let lower = s.to_ascii_lowercase();

    for verb in ["run", "try", "execute", "type", "use"] {
        if let Some(rest) = strip_verb_prefix(s, &lower, verb) {
            if let Some(cmd) = command_after_verb(rest) {
                return Some(cmd);
            }
        }
    }
    None
}

fn strip_leading_phrases(s: &str) -> &str {
    let lower = s.to_ascii_lowercase();
    for phrase in [
        "next, ",
        "then, ",
        "please ",
        "you can now ",
        "you can ",
        "now ",
        "to get started, ",
        "to get started ",
        "what's next: ",
        "whats next: ",
    ] {
        if lower.starts_with(phrase) {
            return &s[phrase.len()..];
        }
    }
    s
}

fn strip_verb_prefix<'a>(s: &'a str, lower: &str, verb: &str) -> Option<&'a str> {
    if !lower.starts_with(verb) {
        return None;
    }
    let rest = &s[verb.len()..];
    if rest.is_empty()
        || rest.starts_with(|c: char| c.is_whitespace() || c == ':' || c == '-' || c == ',')
    {
        Some(rest)
    } else {
        None
    }
}

fn command_after_verb(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let (rest, had_separator) = if let Some(r) = trimmed.strip_prefix(':') {
        (r.trim_start(), true)
    } else if let Some(r) = trimmed.strip_prefix('-') {
        (r.trim_start(), true)
    } else if let Some(r) = trimmed.strip_prefix(',') {
        (r.trim_start(), true)
    } else {
        (trimmed, false)
    };
    let rest = skip_filler(rest);

    if let Some(quoted) = extract_first_quoted(rest) {
        if looks_like_command(quoted) {
            return Some(quoted.to_string());
        }
    }

    // Unquoted text is only a command when the tool used `run:` / `try:` style.
    // Otherwise "Run tests to verify" would become the command `tests`.
    if had_separator && !rest.is_empty() && looks_like_command(rest) {
        Some(rest.trim_end().to_string())
    } else {
        None
    }
}

fn skip_filler(s: &str) -> &str {
    let lower = s.to_ascii_lowercase();
    for filler in ["this command ", "the following ", "the command ", "this "] {
        if lower.starts_with(filler) {
            return &s[filler.len()..];
        }
    }
    s
}

fn extract_mid_line_verb(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    for marker in [
        " run:",
        " run ",
        " run'",
        " run\"",
        " run`",
        " try:",
        " try ",
        " try'",
        " try\"",
        " execute:",
        " or run",
    ] {
        if let Some(pos) = lower.find(marker) {
            let slice = &s[pos..];
            let slice = slice.trim_start_matches(|c: char| !c.is_ascii_alphabetic());
            if let Some(cmd) = extract_verb_command(slice) {
                return Some(cmd);
            }
        }
    }
    None
}

fn extract_first_quoted(s: &str) -> Option<&str> {
    for delim in ['\'', '"', '`'] {
        if let Some(start) = s.find(delim) {
            let rest = &s[start + delim.len_utf8()..];
            if let Some(end) = rest.find(delim) {
                if end > 0 {
                    return Some(&rest[..end]);
                }
            }
        }
    }
    None
}

fn sanitize_command(cmd: &str) -> Option<String> {
    let cmd = unwrap_quotes(cmd.trim());
    let cmd = cmd
        .trim()
        .trim_start_matches("$ ")
        .trim_end_matches(['.', ';'])
        .trim();
    if looks_like_command(cmd) {
        Some(cmd.to_string())
    } else {
        None
    }
}

fn unwrap_quotes(s: &str) -> &str {
    for delim in ['\'', '"', '`'] {
        if s.len() >= 2 && s.starts_with(delim) && s.ends_with(delim) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Heuristic: does this string look like a shell command?
fn looks_like_command(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 400 {
        return false;
    }

    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() && !"./~$".contains(first) {
        return false;
    }

    // Sentences almost always contain period-space; commands almost never do.
    if s.contains(". ") {
        return false;
    }

    let word_count = s.split_whitespace().count();
    if word_count > 12 {
        return false;
    }

    let first_word = s
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.');
    let first_word = first_word.to_ascii_lowercase();
    !matches!(
        first_word.as_str(),
        "the"
            | "this"
            | "that"
            | "these"
            | "your"
            | "please"
            | "it"
            | "to"
            | "a"
            | "an"
            | "for"
            | "if"
            | "when"
            | "you"
            | "we"
    )
}

/// Get the path for the hint file.
pub fn hint_file_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("waz");
    std::fs::create_dir_all(&data_dir).ok();
    data_dir.join("hint.txt")
}

/// Save a hint command to the hint file.
pub fn save_hint(cmd: &str) {
    save_hint_at(&hint_file_path(), cmd);
}

pub fn save_hint_at(path: &Path, cmd: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, cmd).ok();
}

/// Read the hint file without consuming it.
pub fn peek_hint_at(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let cmd = content.trim().to_string();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

/// Read and consume the hint file (one-shot: read then delete).
pub fn consume_hint_at(path: &Path) -> Option<String> {
    let cmd = peek_hint_at(path)?;
    std::fs::remove_file(path).ok();
    Some(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_run_single_quoted() {
        let output = "✅ Published waz@0.1.1\n👉 Run 'git push && git push --tags' to push";
        assert_eq!(
            extract_hint(output),
            Some("git push && git push --tags".to_string())
        );
    }

    #[test]
    fn test_run_double_quoted() {
        let output = "Done!\nRun \"npm install\" to install dependencies";
        assert_eq!(extract_hint(output), Some("npm install".to_string()));
    }

    #[test]
    fn test_run_colon() {
        let output = "Build complete.\nrun: cargo test";
        assert_eq!(extract_hint(output), Some("cargo test".to_string()));
    }

    #[test]
    fn test_try_quoted() {
        let output = "Error: file not found\nTry 'ls -la' to see files";
        assert_eq!(extract_hint(output), Some("ls -la".to_string()));
    }

    #[test]
    fn test_emoji_prefix() {
        let output = "🚀 Run 'docker compose up -d'";
        assert_eq!(
            extract_hint(output),
            Some("docker compose up -d".to_string())
        );
    }

    #[test]
    fn test_no_hint() {
        let output = "Hello world\nEverything is fine\n";
        assert_eq!(extract_hint(output), None);
    }

    #[test]
    fn unquoted_run_without_colon_is_not_a_command() {
        let output = "Run tests to verify the build.";
        assert_eq!(extract_hint(output), None);
    }

    #[test]
    fn test_last_hint_wins() {
        let output = "Run 'first command'\nRun 'second command'";
        assert_eq!(extract_hint(output), Some("second command".to_string()));
    }

    #[test]
    fn test_backtick_quoted() {
        let output = "Next, run: `cargo fmt`";
        assert_eq!(extract_hint(output), Some("cargo fmt".to_string()));
    }

    #[test]
    fn test_run_with_subshell() {
        let output = "✅ waz installed\n👉 Open a new terminal tab or run: source <(waz init zsh)";
        assert_eq!(
            extract_hint(output),
            Some("source <(waz init zsh)".to_string())
        );
    }

    #[test]
    fn test_prompt_style_dollar() {
        let output = "Installed successfully.\nWhat's next:\n$ npm start";
        assert_eq!(extract_hint(output), Some("npm start".to_string()));
    }

    #[test]
    fn test_to_get_started() {
        let output = "Project created.\nTo get started, run `cargo test`";
        assert_eq!(extract_hint(output), Some("cargo test".to_string()));
    }

    #[test]
    fn peek_does_not_consume_hint() {
        let path = unique_hint_path("peek");
        save_hint_at(&path, "npm start");
        assert_eq!(peek_hint_at(&path).as_deref(), Some("npm start"));
        assert_eq!(peek_hint_at(&path).as_deref(), Some("npm start"));
        assert_eq!(consume_hint_at(&path).as_deref(), Some("npm start"));
        assert_eq!(peek_hint_at(&path), None);
    }

    fn unique_hint_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("waz-hint-{name}-{unique}.txt"))
    }
}
