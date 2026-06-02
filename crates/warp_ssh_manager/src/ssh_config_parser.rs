//! `~/.ssh/config` -> `SshConfigCandidate` parser and one-time loader.
//!
//! Design and boundary details are in `specs/gh-110-ssh-config-import/{PRODUCT,TECH}.md` (corresponding to GitHub
//! issue #110): only 5 fields are supported (`Host` / `HostName` / `User` / `Port` /
//! `IdentityFile`), wildcards / negated `Host` are skipped, `Match` blocks are ignored, `Include` only
//! warns and does not recurse, and invalid `Port` returns `None` instead of silently falling back to 22.
//!
//! The parser is a pure function (`parse_ssh_config(&str) -> Vec<_>`), untouched by IO, env, or tokio,
//! driven by literals in unit tests. `load_candidates()` is the top-level IO wrapper. The returned
//! `LoadResult` separates the "path" and "result", so the UI can inform the user which path was
//! actually attempted even in NotFound / Error scenarios.

use std::path::PathBuf;

/// An importable candidate, originating from a valid `Host` block in `~/.ssh/config`.
///
/// Fields are a subset of OpenSSH `ssh_config` — the minimum set chosen by PRODUCT.md decisions I/J/K.
/// `alias` is the literal alias on the `Host` line, used as the `host` field when imported to `SshServerInfo`.
/// This ensures that when starting `ssh` from Waz, OpenSSH can still apply advanced directives (`ProxyJump`, etc.)
/// corresponding to this alias in `~/.ssh/config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshConfigCandidate {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
}

/// Parses the body of an `ssh_config` file, returning an ordered list of candidates.
///
/// Ordering follows the appearance order of `Host` blocks in the file; a line like `Host a b c` expands into 3 candidates
/// sharing the same body. See Section 4 (F-L) of `PRODUCT.md` for specific boundary rules.
pub fn parse_ssh_config(content: &str) -> Vec<SshConfigCandidate> {
    let mut out = Vec::new();
    let mut state = ParseState::Outside;

    for line in content.lines() {
        // Everything after `#` inline is treated as a comment and truncated. OpenSSH's actual semantics for `#` outside/inside
        // quotes have edge case variations, but none of the 5 fields within the scope of the PRODUCT.md decision
        // will contain `#` in reasonable inputs. Naive truncation is correct for user expectations.
        let no_comment = match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        };
        let trimmed = no_comment.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let keyword = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("").trim();

        if keyword.eq_ignore_ascii_case("Host") {
            flush(&mut state, &mut out);
            let aliases = parse_host_aliases(value);
            state = if aliases.is_empty() {
                // The entire line is a wildcard / negated pattern — do not open a new block, but "consume" subsequent
                // field lines so they do not leak into the next valid Host. The InMatch state happens to have the semantics
                // of "discard until the next Host", which is reused here.
                ParseState::InMatch
            } else {
                ParseState::InHost {
                    aliases,
                    body: BodyFields::default(),
                }
            };
        } else if keyword.eq_ignore_ascii_case("Match") {
            // PRODUCT.md decision H: The Match block is ignored as a whole, sharing the same InMatch
            // path as a "fully wildcard Host".
            flush(&mut state, &mut out);
            state = ParseState::InMatch;
        } else if keyword.eq_ignore_ascii_case("Include") {
            // PRODUCT.md decision F: The MVP does not recurse, only warns. The state remains unchanged, and subsequent
            // lines still belong to the current Host block (if any) — this is consistent with OpenSSH's Include semantics
            // (Include does not terminate the current Host context).
            log::warn!(
                "Include directive in ssh_config is not supported by importer; \
                 hosts in `{value}` will not be imported"
            );
        } else if let ParseState::InHost { body, .. } = &mut state {
            apply_body_field(body, keyword, value);
        }
        // Other keywords under InMatch / Outside: ignored.
    }

    flush(&mut state, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

enum ParseState {
    /// Have not encountered any Host / Match yet.
    Outside,
    /// Currently inside a valid Host block. `aliases` are the remaining aliases after removing wildcards.
    InHost {
        aliases: Vec<String>,
        body: BodyFields,
    },
    /// Currently inside an ignored block (`Match` or a fully wildcarded `Host`), consuming fields until
    /// the next `Host` or EOF.
    InMatch,
}

#[derive(Default, Clone)]
struct BodyFields {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
}

fn flush(state: &mut ParseState, out: &mut Vec<SshConfigCandidate>) {
    let prev = std::mem::replace(state, ParseState::Outside);
    if let ParseState::InHost { aliases, body } = prev {
        for alias in aliases {
            out.push(SshConfigCandidate {
                alias,
                hostname: body.hostname.clone(),
                user: body.user.clone(),
                port: body.port,
                identity_file: body.identity_file.clone(),
            });
        }
    }
}

/// Parses a line like `Host a *.prod b !bad` into `["a", "b"]`.
fn parse_host_aliases(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|tok| !tok.contains('*') && !tok.contains('?') && !tok.contains('!'))
        .map(|s| s.to_string())
        .collect()
}

/// Applies a field on the body of the current Host block. **First occurrence wins** (matches OpenSSH semantics).
fn apply_body_field(body: &mut BodyFields, keyword: &str, value: &str) {
    if keyword.eq_ignore_ascii_case("HostName") {
        if body.hostname.is_none() {
            body.hostname = Some(value.to_string());
        }
    } else if keyword.eq_ignore_ascii_case("User") {
        if body.user.is_none() {
            body.user = Some(value.to_string());
        }
    } else if keyword.eq_ignore_ascii_case("Port") {
        // Note: The first "declaration" wins, not the first "valid" one — but because we fill in None when Port parsing
        // fails (PRODUCT.md decision K), the first-wins "already declared" state is equivalent to "value is not None" here.
        // For simplicity, guard with is_none.
        if body.port.is_none() {
            body.port = value.parse::<u16>().ok();
        }
    } else if keyword.eq_ignore_ascii_case("IdentityFile") && body.identity_file.is_none() {
        let unquoted = strip_surrounding_quotes(value);
        body.identity_file = Some(expand_tilde(unquoted));
    }
    // Remaining keywords: ignored (MVP only supports 5 fields).
}

fn strip_surrounding_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// Cross-platform default `~/.ssh/config` path for the current user.
///
/// Returns `None` if the home directory cannot be found (rare).
pub fn default_ssh_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("config"))
}

/// Parsing result and its source path, used by the UI for error / empty state presentation.
#[derive(Debug)]
pub struct LoadResult {
    /// The path actually attempted to read. `None` means the home directory could not be obtained.
    pub path: Option<PathBuf>,
    pub outcome: LoadOutcome,
}

#[derive(Debug)]
pub enum LoadOutcome {
    /// File successfully read and parsed (the list might be empty).
    Loaded(Vec<SshConfigCandidate>),
    /// The path does not exist — clean state, the UI displays a "not found" prompt instead of an error.
    NotFound,
    /// IO error (permission, encoding, disk, etc.). `String` is a human-readable message for the user.
    Error(String),
}

/// Loads `~/.ssh/config` from the default path at once, returning the path + result.
///
/// Designed as synchronous + panic-free: the UI calls this once when the panel is first opened. A typical config is <10KB,
/// so synchronous IO is fast enough. If file read fails due to non-existence / permission issues, it goes to `NotFound` / `Error`
/// respectively, without throwing errors upwards.
pub fn load_candidates() -> LoadResult {
    match default_ssh_config_path() {
        Some(p) => load_candidates_from(&p),
        None => LoadResult {
            path: None,
            outcome: LoadOutcome::Error("Could not determine home directory".into()),
        },
    }
}

/// Same as [`load_candidates`], but allows the caller to explicitly specify the path — primarily for unit tests
/// (tempfile), and reserves an interface for future "custom config path" settings.
pub fn load_candidates_from(path: &std::path::Path) -> LoadResult {
    let outcome = match std::fs::read_to_string(path) {
        Ok(s) => LoadOutcome::Loaded(parse_ssh_config(&s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoadOutcome::NotFound,
        Err(e) => LoadOutcome::Error(format!("{e}")),
    };
    LoadResult {
        path: Some(path.to_path_buf()),
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quick helper constructor for testing, defaulting all to `None` and only filling in fields of interest for the case.
    fn cand(alias: &str) -> SshConfigCandidate {
        SshConfigCandidate {
            alias: alias.into(),
            hostname: None,
            user: None,
            port: None,
            identity_file: None,
        }
    }

    /// The simplest happy path: a Host block with all 5 fields, producing one candidate.
    /// This test drives the minimal "Host block identification + field parsing" mainline; subsequent cases
    /// add state machine branches on top of it.
    #[test]
    fn single_host_with_all_fields() {
        let input = "\
Host prodbox
    HostName prod.example.com
    User alice
    Port 2222
    IdentityFile /home/alice/.ssh/id_ed25519
";
        let got = parse_ssh_config(input);
        assert_eq!(
            got,
            vec![SshConfigCandidate {
                alias: "prodbox".into(),
                hostname: Some("prod.example.com".into()),
                user: Some("alice".into()),
                port: Some(2222),
                identity_file: Some(PathBuf::from("/home/alice/.ssh/id_ed25519")),
            }]
        );
    }

    #[test]
    fn empty_file_produces_no_candidates() {
        assert_eq!(parse_ssh_config(""), vec![]);
    }

    #[test]
    fn comments_only_produces_no_candidates() {
        assert_eq!(parse_ssh_config("# top comment\n# another\n"), vec![]);
    }

    #[test]
    fn host_with_only_alias_has_no_hostname_field() {
        // The importer layer (not in this module) uses `alias` as `server.host`; this only ensures
        // that the parser does not fabricate a hostname.
        assert_eq!(parse_ssh_config("Host foo\n"), vec![cand("foo")]);
    }

    #[test]
    fn multiple_hosts_in_order() {
        let input = "\
Host a
    User x
Host b
    User y
Host c
    User z
";
        let got = parse_ssh_config(input);
        let users: Vec<_> = got
            .iter()
            .map(|c| (c.alias.as_str(), c.user.as_deref()))
            .collect();
        assert_eq!(
            users,
            vec![("a", Some("x")), ("b", Some("y")), ("c", Some("z"))]
        );
    }

    #[test]
    fn wildcard_star_host_skipped() {
        // PRODUCT.md decision G: `Host *.prod` is a template rather than a machine, and is not added to the candidate area.
        let input = "\
Host *.prod
    User root
Host realbox
    User me
";
        let got = parse_ssh_config(input);
        assert_eq!(
            got,
            vec![SshConfigCandidate {
                user: Some("me".into()),
                ..cand("realbox")
            }]
        );
    }

    #[test]
    fn wildcard_question_host_skipped() {
        let input = "\
Host srv?
    User x
";
        assert_eq!(parse_ssh_config(input), vec![]);
    }

    #[test]
    fn negation_host_skipped() {
        let input = "\
Host !bad
    User x
";
        assert_eq!(parse_ssh_config(input), vec![]);
    }

    #[test]
    fn host_with_multiple_aliases_expands_to_separate_candidates() {
        // PRODUCT.md decision L: `Host a b c` share the same body.
        let input = "\
Host a b c
    Port 22
    User shared
";
        let got = parse_ssh_config(input);
        assert_eq!(got.len(), 3);
        for (i, alias) in ["a", "b", "c"].iter().enumerate() {
            assert_eq!(got[i].alias, *alias);
            assert_eq!(got[i].port, Some(22));
            assert_eq!(got[i].user.as_deref(), Some("shared"));
        }
    }

    #[test]
    fn host_with_mixed_aliases_filters_wildcards_keeps_literals() {
        // `Host a *.prod b` -> only export a and b.
        let input = "\
Host a *.prod b
    User shared
";
        let got = parse_ssh_config(input);
        let aliases: Vec<&str> = got.iter().map(|c| c.alias.as_str()).collect();
        assert_eq!(aliases, vec!["a", "b"]);
    }

    #[test]
    fn match_block_ignored_until_next_host() {
        // PRODUCT.md decision H: The `Match` block is ignored as a whole, it should not "contaminate" the body
        // of the previous Host, nor should it be treated as a new candidate.
        let input = "\
Host a
    User u_a
Match user someone
    User SHOULD_NOT_APPEAR
    Port 9999
Host b
    User u_b
";
        let got = parse_ssh_config(input);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].alias, "a");
        assert_eq!(got[0].user.as_deref(), Some("u_a"));
        assert_eq!(got[0].port, None, "Port 9999 of Match block should not leak into a");
        assert_eq!(got[1].alias, "b");
        assert_eq!(got[1].user.as_deref(), Some("u_b"));
    }

    #[test]
    fn match_block_at_eof_does_not_panic() {
        let input = "\
Host a
    User u
Match user x
    User leak
";
        let got = parse_ssh_config(input);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].alias, "a");
        assert_eq!(got[0].user.as_deref(), Some("u"));
    }

    #[test]
    fn include_directive_logged_and_skipped_outside_host() {
        // PRODUCT.md decision F: `Include` does not recurse, only warns, subsequent parsing continues as usual.
        let input = "\
Include ~/.ssh/work/*.conf
Host a
    User u
";
        let got = parse_ssh_config(input);
        assert_eq!(
            got,
            vec![SshConfigCandidate {
                user: Some("u".into()),
                ..cand("a")
            }]
        );
    }

    #[test]
    fn port_invalid_string_yields_none() {
        // PRODUCT.md decision K: Do not silently roll back to 22; the UI displays an empty port to the user.
        let input = "Host a\n    Port not-a-number\n";
        assert_eq!(parse_ssh_config(input)[0].port, None);
    }

    #[test]
    fn port_out_of_u16_range_yields_none() {
        let input = "Host a\n    Port 70000\n";
        assert_eq!(parse_ssh_config(input)[0].port, None);
    }

    #[test]
    fn port_valid_yields_some() {
        let input = "Host a\n    Port 2222\n";
        assert_eq!(parse_ssh_config(input)[0].port, Some(2222));
    }

    #[test]
    fn quoted_identity_file_has_quotes_stripped() {
        // OpenSSH allows paths with spaces to be enclosed in quotes.
        let input = "Host a\n    IdentityFile \"C:\\Users\\Jiaqi Jiang\\.ssh\\id\"\n";
        assert_eq!(
            parse_ssh_config(input)[0].identity_file,
            Some(PathBuf::from("C:\\Users\\Jiaqi Jiang\\.ssh\\id"))
        );
    }

    #[test]
    fn tilde_in_identity_file_expanded_to_home() {
        // ~/x expands to $HOME/x. $HOME varies in different CI environments, so only assert that the prefix is home.
        let input = "Host a\n    IdentityFile ~/keys/id\n";
        let got = parse_ssh_config(input);
        let path = got[0].identity_file.as_ref().expect("IdentityFile set");
        let home = dirs::home_dir().expect("test runner has home dir");
        assert!(
            path.starts_with(&home),
            "expected {path:?} to start with {home:?}"
        );
        assert!(
            path.ends_with("keys/id"),
            "expected {path:?} to end with keys/id"
        );
    }

    #[test]
    fn case_insensitive_keywords() {
        let input = "host a\n    hOsTnAmE example.com\n    user alice\n    PORT 22\n";
        let got = parse_ssh_config(input);
        assert_eq!(
            got,
            vec![SshConfigCandidate {
                alias: "a".into(),
                hostname: Some("example.com".into()),
                user: Some("alice".into()),
                port: Some(22),
                identity_file: None,
            }]
        );
    }

    #[test]
    fn repeated_field_first_wins() {
        // Matches OpenSSH semantics: within the same Host block, for the same field, the first occurrence wins.
        let input = "Host a\n    Port 1\n    Port 2\n    User first\n    User second\n";
        let got = parse_ssh_config(input);
        assert_eq!(got[0].port, Some(1));
        assert_eq!(got[0].user.as_deref(), Some("first"));
    }

    #[test]
    fn inline_trailing_comment_dropped_from_value() {
        // OpenSSH actually has blurry handling boundaries for inline `#`; we take the "conservative" approach:
        // truncate when encountering `#` during whole-line scanning, effective outside quotes.
        let input = "Host a # primary box\n    User alice # admin\n";
        let got = parse_ssh_config(input);
        assert_eq!(got[0].alias, "a");
        assert_eq!(got[0].user.as_deref(), Some("alice"));
    }

    #[test]
    fn leading_indent_tolerated() {
        // OpenSSH allows arbitrary leading whitespace.
        let input = "  Host a\n\t  Port 22\n";
        let got = parse_ssh_config(input);
        assert_eq!(got[0].alias, "a");
        assert_eq!(got[0].port, Some(22));
    }

    // -----------------------------------------------------------------
    // default_ssh_config_path / load_candidates_from / load_candidates
    // -----------------------------------------------------------------

    #[test]
    fn default_path_points_under_home_dot_ssh_config() {
        // Cross-platform: as long as dirs::home_dir() returns a value, the result should be
        // `<home>/.ssh/config`. CI runners always have HOME / USERPROFILE.
        let got = default_ssh_config_path().expect("test runner has home dir");
        let home = dirs::home_dir().expect("test runner has home dir");
        assert!(got.starts_with(&home), "{got:?} should start with {home:?}");
        assert!(got.ends_with("config"));
        assert!(
            got.to_string_lossy()
                .replace('\\', "/")
                .ends_with(".ssh/config"),
            "{got:?} should end with .ssh/config"
        );
    }

    #[test]
    fn load_candidates_from_nonexistent_path_returns_not_found() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let path = tmp.path().join("does_not_exist");
        let res = load_candidates_from(&path);
        assert_eq!(res.path.as_deref(), Some(path.as_path()));
        assert!(
            matches!(res.outcome, LoadOutcome::NotFound),
            "got {:?}",
            res.outcome
        );
    }

    #[test]
    fn load_candidates_from_valid_file_returns_parsed_candidates() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
        writeln!(tmp, "Host a\n    User u\n").expect("write tempfile");
        let res = load_candidates_from(tmp.path());
        match res.outcome {
            LoadOutcome::Loaded(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].alias, "a");
                assert_eq!(v[0].user.as_deref(), Some("u"));
            }
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn load_candidates_from_empty_file_returns_loaded_empty() {
        let tmp = tempfile::NamedTempFile::new().expect("create tempfile");
        let res = load_candidates_from(tmp.path());
        match res.outcome {
            LoadOutcome::Loaded(v) => assert!(v.is_empty()),
            other => panic!("expected Loaded(empty), got {other:?}"),
        }
    }
}
