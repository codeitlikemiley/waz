use super::bytes_look_like_shell_prompt;

fn matches(input: &str) -> bool {
    bytes_look_like_shell_prompt(input.as_bytes())
}

#[test]
fn matches_dollar_prompt() {
    assert!(matches("user@host:~$ "));
    assert!(matches("$ "));
}

#[test]
fn matches_hash_root_prompt() {
    assert!(matches("root@host:~# "));
    assert!(matches("# "));
}

#[test]
fn matches_powershell_prompt() {
    assert!(matches("PS C:\\Users\\u> "));
    assert!(matches("> "));
}

#[test]
fn matches_powerline_prompts() {
    assert!(matches("❯ "));
    assert!(matches("▶ "));
    assert!(matches("» "));
    assert!(matches("λ "));
    assert!(matches("→ "));
}

#[test]
fn does_not_match_partial_prompt_chars() {
    // Missing spaces do not count as prompt
    assert!(!matches("$"));
    assert!(!matches("#"));
    assert!(!matches(">"));
    assert!(!matches("❯"));
}

#[test]
fn does_not_match_random_output() {
    assert!(!matches("hello world"));
    assert!(!matches("error: connection refused\n"));
}

#[test]
fn matches_with_long_preceding_output() {
    // tail only looks at 256 bytes, and there is 1KB of output in front, as long as the prompt is at the end, it still hits
    let mut s = "x".repeat(1024);
    s.push_str("$ ");
    assert!(matches(&s));
}

#[test]
fn does_not_match_quoted_prompt_in_middle() {
    // Prompt characters appearing in positions other than the end should not be accidentally hit.
    assert!(!matches("$ foo"));
    assert!(!matches("# comment"));
}
