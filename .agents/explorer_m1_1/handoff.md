# Handoff Report — TMP Completer Investigation

## 1. Observation
- The completions engine defines Tool Metadata Protocol (TMP) schema handling inside `crates/warp_completer/src/signatures/tmp.rs`.
- The data source configuration for a token is parsed into `DataSource` structure:
  ```rust
  pub struct DataSource {
      pub command: Option<String>,
      pub resolver: Option<String>,
      pub parse: String,
  }
  ```
- Built-in resolvers are routed and invoked via `resolve_builtin(resolver: &str, cwd: &str)` (lines 196-211 of `tmp.rs`).
- Platform separation is handled using conditional compilation flags `#[cfg(not(target_family = "wasm"))]` and `#[cfg(target_family = "wasm")]`. The command execution wrapper `command::blocking::Command` is imported on non-wasm targets (Cargo.toml dependencies lines 48-52).
- Example git resolver implementation for branches (`git_resolve_branches` at lines 443-456 of `tmp.rs`) uses `command::blocking::Command::new("git")` with the `current_dir(cwd)` set to target working directory.

## 2. Logic Chain
- To implement `git:status_files`, we must match the existing structure:
  - Add a pattern match for `(Some("git"), Some("status_files"))` in `resolve_builtin` to dispatch it.
  - Implement a new helper function `git_resolve_status_files(cwd: &str)` under both target configs.
  - Under `#[cfg(not(target_family = "wasm"))]`, execute `git status --porcelain` in `cwd`.
  - Parse output line-by-line:
    - If status code is `??`, it's untracked.
    - If status code contains `M`, it's modified.
    - If status code contains `R`, it's renamed, with `old -> new` format in path.
  - Trim any double-quotes git outputs around paths (using `.trim_matches('"')`).
  - Use `BTreeSet` to deduplicate and sort files, converting to `Vec<String>`.
  - Under `#[cfg(target_family = "wasm")]`, return `None` immediately.

## 3. Caveats
- `git status --porcelain` version 1 is stable and ignores most user settings (e.g. `status.short` and `status.branch`), but certain settings or git configurations could change paths formatting (e.g. `core.quotepath`). The proposed quote-trimming logic is sufficient for basic quotes but does not unescape octal character sequences.
- Submodules status or ignored files might not be returned depending on repository status, which matches git's default behavior.

## 4. Conclusion
- A new resolver `git:status_files` can be safely implemented in `crates/warp_completer/src/signatures/tmp.rs` using the proposed pattern, matching the registration and target-gating conventions already present in the workspace.

## 5. Verification Method
- **Inspection**: Confirm registration in `resolve_builtin` and implementation matches formatting and imports of existing resolvers in `/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp.rs`.
- **Unit Testing**: Add a test in `/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp_tests.rs` similar to `test_resolve_command_data_source` that executes `git:status_files` on a mock git repository.
- **Project Test Execution**: Run the completions tests using:
  ```bash
  cargo test -p warp_completer --lib signatures::tmp::tests
  ```
