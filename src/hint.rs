use std::path::PathBuf;

/// Extract a suggested command from command output text.
///
/// Scans the output for common patterns where tools suggest follow-up commands:
/// - `Run 'command'` or `Run "command"`
/// - `run: command`
/// - `Try 'command'` / `Try "command"`
/// - `Execute: command`
/// - Emoji markers like `👉 Run 'command'`
/// - `Next, run: command`
/// - Lines starting with `$` or `>` after suggestion context
pub fn extract_hint(output: &str) -> Option<String> {
    // Process lines in reverse — the last suggestion in output is most relevant
    for line in output.lines().rev() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Pattern: Run/Try/Execute 'command' or "command"
        if let Some(cmd) = extract_quoted_command(trimmed, "run") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_quoted_command(trimmed, "Run") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_quoted_command(trimmed, "try") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_quoted_command(trimmed, "Try") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_quoted_command(trimmed, "execute") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_quoted_command(trimmed, "Execute") {
            return Some(cmd);
        }

        // Pattern: "run: command" or "Run: command"
        if let Some(cmd) = extract_colon_command(trimmed, "run") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_colon_command(trimmed, "Run") {
            return Some(cmd);
        }
        if let Some(cmd) = extract_colon_command(trimmed, "Next, run") {
            return Some(cmd);
        }

        // Pattern: "👉 Run 'cmd'" or emoji + "Run 'cmd'" (strip leading emojis/symbols)
        let stripped = trimmed
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '\'' && c != '"');
        if stripped != trimmed {
            if let Some(cmd) = extract_quoted_command(stripped, "Run") {
                return Some(cmd);
            }
            if let Some(cmd) = extract_quoted_command(stripped, "run") {
                return Some(cmd);
            }
        }

        // Pattern: keyword appears mid-line, e.g. "... or run: command" / "... or Run 'command'"
        for keyword in &["run:", "Run:", "run '", "Run '", "run \"", "Run \""] {
            if let Some(pos) = trimmed.find(keyword) {
                let mid = &trimmed[pos..];
                // Try colon pattern first
                if let Some(cmd) = extract_colon_command(mid, "run")
                    .or_else(|| extract_colon_command(mid, "Run"))
                {
                    return Some(cmd);
                }
                // Try quoted pattern
                if let Some(cmd) = extract_quoted_command(mid, "run")
                    .or_else(|| extract_quoted_command(mid, "Run"))
                {
                    return Some(cmd);
                }
            }
        }
    }

    None
}

/// Extract command from patterns like: `Run 'git push'` or `Run "git push"`
fn extract_quoted_command(line: &str, keyword: &str) -> Option<String> {
    // Find keyword followed by optional punctuation then a quote
    let rest = line.strip_prefix(keyword)?;
    let rest = rest.trim_start();

    // Allow optional colon or dash after keyword
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let rest = rest.trim_start();

    // Extract quoted content
    if let Some(cmd) = extract_between(rest, '\'', '\'') {
        let cmd = cmd.trim().to_string();
        if !cmd.is_empty() && looks_like_command(&cmd) {
            return Some(cmd);
        }
    }
    if let Some(cmd) = extract_between(rest, '"', '"') {
        let cmd = cmd.trim().to_string();
        if !cmd.is_empty() && looks_like_command(&cmd) {
            return Some(cmd);
        }
    }
    // Also handle backtick-quoted commands
    if let Some(cmd) = extract_between(rest, '`', '`') {
        let cmd = cmd.trim().to_string();
        if !cmd.is_empty() && looks_like_command(&cmd) {
            return Some(cmd);
        }
    }

    None
}

/// Extract command from patterns like: `run: git push`
fn extract_colon_command(line: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(keyword)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start();

    if rest.is_empty() {
        return None;
    }

    // The rest of the line is the command
    let cmd = rest.trim_end().to_string();
    if looks_like_command(&cmd) {
        Some(cmd)
    } else {
        None
    }
}

/// Extract text between two delimiter characters.
fn extract_between(s: &str, open: char, close: char) -> Option<&str> {
    let start = s.find(open)? + open.len_utf8();
    let rest = &s[start..];
    let end = rest.rfind(close)?;
    if end == 0 {
        return None;
    }
    Some(&rest[..end])
}

/// Heuristic: does this string look like a shell command?
fn looks_like_command(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must start with a word char, path char, or backtick
    let first = s.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() && first != '.' && first != '/' && first != '~' && first != '`' {
        return false;
    }
    // Should not be a full sentence (too many words without special chars)
    // Commands typically have flags, pipes, etc., or are short
    let word_count = s.split_whitespace().count();
    if word_count > 15 {
        return false; // probably a sentence, not a command
    }
    true
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
    let path = hint_file_path();
    std::fs::write(&path, cmd).ok();
}

/// Read and consume the hint file (one-shot: read then delete).
pub fn consume_hint() -> Option<String> {
    let path = hint_file_path();
    let content = std::fs::read_to_string(&path).ok()?;
    std::fs::remove_file(&path).ok();
    let cmd = content.trim().to_string();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_single_quoted() {
        let output = "✅ Published waz@0.1.1\n👉 Run 'git push && git push --tags' to push";
        assert_eq!(extract_hint(output), Some("git push && git push --tags".to_string()));
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
        assert_eq!(extract_hint(output), Some("docker compose up -d".to_string()));
    }

    #[test]
    fn test_no_hint() {
        let output = "Hello world\nEverything is fine\n";
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
        assert_eq!(extract_hint(output), Some("`cargo fmt`".to_string()));
    }

    #[test]
    fn test_run_with_subshell() {
        let output = "✅ waz installed\n👉 Open a new terminal tab or run: source <(waz init zsh)";
        // 'source' starts with 's' which is alphanumeric, so it should match
        assert_eq!(extract_hint(output), Some("source <(waz init zsh)".to_string()));
    }
}
