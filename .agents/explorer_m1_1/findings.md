# Tool Metadata Protocol (TMP) Completer Investigation

This report details how data source resolvers are defined, registered, and executed in the Warp Tool Metadata Protocol (TMP) completions engine, and outlines a plan for implementing the new `git:status_files` resolver.

---

## 1. TMP Resolver System Architecture

The TMP completions engine dynamically resolves values for command arguments defined in tool schemas. The resolution pipeline is defined in `crates/warp_completer/src/signatures/tmp.rs`.

### Structures & Metadata Design
The schema structures are modeled as follows:

1. **`DataSource`**: Holds metadata for where to fetch token completion options:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   pub struct DataSource {
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub command: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub resolver: Option<String>,
       #[serde(default = "default_parse_mode")]
       pub parse: String,
   }
   ```
2. **`TokenDef`**: Defines a single argument token, referencing an optional `DataSource`:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   pub struct TokenDef {
       pub name: String,
       pub description: String,
       #[serde(default)]
       pub required: bool,
       pub token_type: TokenType,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub default: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub values: Option<Vec<String>>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub flag: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub data_source: Option<DataSource>,
   }
   ```

### Execution Lifecycle
When completions are generated or schemas are matched (e.g. inside `get_active_tmp_prompt` or `find_matching_tmp_command`), the engine calls **`resolve_data_sources(entry: &mut CommandEntry, cwd: &str)`**.
1. It iterates through all tokens of a command.
2. If a token has a `data_source` with a defined `resolver` string, it calls `resolve_builtin(resolver, cwd)`.
3. If the resolver returns options (as `Some(Vec<String>)`), these are populated into `token.values` and the token type is updated to `TokenType::Enum`.

---

## 2. Resolver Registration and Routing

Resolvers are registered using static pattern matching on the resolver identifier within **`resolve_builtin`** in `tmp.rs`:

```rust
fn resolve_builtin(resolver: &str, cwd: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = resolver.splitn(3, ':').collect();
    match (parts.get(0).copied(), parts.get(1).copied()) {
        (Some("cargo"), Some("bins")) => cargo_resolve_bins(cwd),
        (Some("cargo"), Some("examples")) => cargo_resolve_examples(cwd),
        (Some("cargo"), Some("packages")) => cargo_resolve_packages(cwd),
        (Some("cargo"), Some("features")) => cargo_resolve_features(cwd),
        (Some("cargo"), Some("profiles")) => cargo_resolve_profiles(cwd),
        (Some("cargo"), Some("tests")) => cargo_resolve_tests(cwd),
        (Some("cargo"), Some("benches")) => cargo_resolve_benches(cwd),
        (Some("git"), Some("branches")) => git_resolve_branches(cwd),
        (Some("git"), Some("remotes")) => git_resolve_remotes(cwd),
        (Some("npm"), Some("scripts")) => npm_resolve_scripts(cwd),
        _ => None,
    }
}
```

Functions executing shell commands or binary tools (like `git`) are gated using target-family conditional compilation:
- **`#[cfg(not(target_family = "wasm"))]`**: Executes the actual command using `command::blocking::Command` inside the specified directory (`cwd`).
- **`#[cfg(target_family = "wasm")]`**: Stubs out the resolver to immediately return `None`, preventing compilation/execution errors on WASM targets where process spawning is unsupported.

---

## 3. Implementation Plan for `git:status_files`

To implement the `git:status_files` resolver, we will:
1. Register `(Some("git"), Some("status_files"))` in `resolve_builtin` matching.
2. Define `git_resolve_status_files(cwd: &str) -> Option<Vec<String>>` with WASM and non-WASM implementations.

### Proposed Code Changes (Diff Patch format)

```rust
// In crates/warp_completer/src/signatures/tmp.rs:

// 1. Add registration in resolve_builtin:
     match (parts.get(0).copied(), parts.get(1).copied()) {
         ...
         (Some("git"), Some("branches")) => git_resolve_branches(cwd),
         (Some("git"), Some("remotes")) => git_resolve_remotes(cwd),
+        (Some("git"), Some("status_files")) => git_resolve_status_files(cwd),
         (Some("npm"), Some("scripts")) => npm_resolve_scripts(cwd),
         _ => None,
     }

// 2. Add the resolver functions at the bottom of the Git resolvers section:

#[cfg(not(target_family = "wasm"))]
fn git_resolve_status_files(cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = BTreeSet::new();

    let clean_path = |p: &str| p.trim_matches('"').to_string();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }

        let index_status = line.chars().nth(0);
        let worktree_status = line.chars().nth(1);
        let path_part = &line[3..];

        if index_status == Some('?') && worktree_status == Some('?') {
            // Untracked files (??)
            files.insert(clean_path(path_part));
        } else if index_status == Some('M') || worktree_status == Some('M') {
            // Modified files (M)
            files.insert(clean_path(path_part));
        } else if index_status == Some('R') || worktree_status == Some('R') {
            // Renamed files (R  old -> new)
            if let Some((old, new)) = path_part.split_once(" -> ") {
                files.insert(clean_path(old));
                files.insert(clean_path(new));
            } else {
                files.insert(clean_path(path_part));
            }
        }
    }

    if files.is_empty() {
        None
    } else {
        Some(files.into_iter().collect())
    }
}

#[cfg(target_family = "wasm")]
fn git_resolve_status_files(_cwd: &str) -> Option<Vec<String>> {
    None
}
```

### Parsing Mechanics
- **Untracked (`??`)**: `index_status == '?' && worktree_status == '?'`.
- **Modified (`M`)**: Checked using `index_status == 'M' || worktree_status == 'M'` to capture all index and worktree modification states (e.g. ` M`, `M `, `MM`, `AM`, `RM`).
- **Renamed (`R`)**: Identifies renamed files by looking for `R` in status codes, splitting the path part by ` -> `, and adding both the original and new file paths for maximal completion relevance.
- **Double-quotes stripping**: Raw paths containing special/non-ASCII characters are enclosed in double-quotes by git's porcelain output. The helper `clean_path` removes them.
- **Deduplication & Sorting**: Using `BTreeSet` ensures paths are deduplicated (important if a path appears multiple times or is renamed) and naturally sorted before being converted back into a `Vec<String>`.
