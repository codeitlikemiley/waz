/// Tokenize a shell command, respecting simple single/double quotes.
pub fn tokenize(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;

    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }

    if !cur.is_empty() {
        tokens.push(cur);
    }

    tokens
}

/// Binary + subcommand identity used for sequence matching.
///
/// `git commit -m "foo"` and `git commit -m "bar"` share the key `git commit`.
/// Tools without subcommands keep the full command so `cd src` stays distinct
/// from `cd tests`.
pub fn sequence_key(command: &str) -> String {
    let tokens = tokenize(command);
    if tokens.is_empty() {
        return command.trim().to_string();
    }

    let bin = tokens[0].as_str();
    if is_hub_bin(bin) || !has_subcommands(bin) {
        return tokens.join(" ");
    }

    let max = max_key_tokens(bin);
    let mut key = Vec::with_capacity(max);
    key.push(bin.to_string());

    for token in tokens.iter().skip(1) {
        if token.starts_with('+') {
            // cargo +nightly test
            key.push(token.clone());
            continue;
        }
        if token.starts_with('-') || looks_like_value(token) {
            break;
        }
        key.push(token.clone());
        if key.iter().filter(|t| !t.starts_with('+')).count() >= max {
            break;
        }
    }

    key.join(" ")
}

pub fn is_hub_command(command: &str) -> bool {
    tokenize(command)
        .first()
        .map(|bin| is_hub_bin(bin))
        .unwrap_or(false)
}

pub fn matches_prefix(command: &str, prefix: Option<&str>) -> bool {
    match prefix {
        Some(prefix) if !prefix.is_empty() => command.starts_with(prefix),
        _ => true,
    }
}

pub fn prefix_is_exact(command: &str, prefix: Option<&str>) -> bool {
    match prefix {
        Some(prefix) if !prefix.is_empty() => command == prefix,
        _ => false,
    }
}

/// Quote an argument for reuse in a suggested command.
pub fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=".contains(c))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// `LIKE` pattern that matches `key` followed by more arguments.
pub fn like_prefix_pattern(key: &str) -> String {
    let mut escaped = String::with_capacity(key.len() + 2);
    for c in key.chars() {
        if c == '\\' || c == '%' || c == '_' {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push(' ');
    escaped.push('%');
    escaped
}

fn is_hub_bin(bin: &str) -> bool {
    matches!(
        bin,
        "ls" | "ll"
            | "la"
            | "pwd"
            | "clear"
            | "reset"
            | "history"
            | "echo"
            | "true"
            | "false"
            | "date"
            | "whoami"
            | "hostname"
            | "uname"
            | "which"
            | "type"
            | "cat"
            | "less"
            | "more"
            | "head"
            | "tail"
            | "waz"
            | ":"
    )
}

fn has_subcommands(bin: &str) -> bool {
    matches!(
        bin,
        "git"
            | "cargo"
            | "npm"
            | "npx"
            | "pnpm"
            | "yarn"
            | "bun"
            | "bunx"
            | "docker"
            | "podman"
            | "kubectl"
            | "helm"
            | "terraform"
            | "aws"
            | "gcloud"
            | "az"
            | "gh"
            | "poetry"
            | "pip"
            | "pip3"
            | "uv"
            | "rustup"
            | "brew"
            | "apt"
            | "apt-get"
            | "dnf"
            | "pacman"
            | "systemctl"
            | "tmux"
            | "flutter"
            | "dart"
            | "go"
            | "make"
            | "cmake"
            | "just"
            | "nix"
            | "deno"
            | "hugo"
            | "wrangler"
            | "prisma"
            | "compose"
    )
}

fn max_key_tokens(bin: &str) -> usize {
    match bin {
        "npm" | "pnpm" | "yarn" | "bun" | "npx" | "bunx" => 3,
        "docker" | "podman" | "kubectl" => 3,
        _ => 2,
    }
}

fn looks_like_value(token: &str) -> bool {
    token.starts_with('.')
        || token.starts_with('/')
        || token.starts_with('~')
        || token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("git@")
        || token.contains('/')
        || token.contains('\\')
        || token.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_key_strips_git_commit_messages() {
        assert_eq!(sequence_key("git commit -m 'msg'"), "git commit");
        assert_eq!(sequence_key("git commit -m \"fix\""), "git commit");
        assert_eq!(sequence_key("git push origin main"), "git push");
    }

    #[test]
    fn sequence_key_keeps_package_scripts() {
        assert_eq!(sequence_key("npm run build --watch"), "npm run build");
        assert_eq!(sequence_key("docker compose up -d"), "docker compose up");
    }

    #[test]
    fn sequence_key_keeps_cd_paths() {
        assert_eq!(sequence_key("cd src"), "cd src");
        assert_eq!(sequence_key("mkdir -p foo/bar"), "mkdir -p foo/bar");
    }

    #[test]
    fn tokenize_respects_quotes() {
        assert_eq!(tokenize("mkdir \"foo bar\""), vec!["mkdir", "foo bar"]);
        assert_eq!(
            tokenize("git commit -m 'a b'"),
            vec!["git", "commit", "-m", "a b"]
        );
    }

    #[test]
    fn like_prefix_escapes_wildcards() {
        assert_eq!(like_prefix_pattern("git commit"), "git commit %");
        assert_eq!(like_prefix_pattern("100%"), "100\\% %");
    }
}
