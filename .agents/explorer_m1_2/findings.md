# Findings — git.json Schemas and git:status_files Resolver Plan

## Executive Summary
This report analyzes the `git.json` curated and user configuration schemas and details the plan to update the `git add` command schema to support the new `git:status_files` resolver. The investigation verifies the exact paths of the files, provides concrete line-level information, and outlines the codebase additions needed to implement the backend resolver.

---

## 1. Schema File Verification

The investigation verified the existence and content of two schema files:

### Curated Workspace Schema File
- **Verified Path**: `/Volumes/goldcoders/waz/schemas/curated/git.json`
- **Lines containing `git add`**: Lines 20-28
- **Current Token Definition**: Line 26

### User Config Schema File
- **Verified Path**: `/Users/uriah/.config/zap/schemas/git.json` (resolving the active user config path `~/.config/zap/schemas/git.json`)
- **Lines containing `git add`**: Lines 20-28
- **Current Token Definition**: Line 26

---

## 2. Current `git add` Command Structure

In both files, the `git add` command schema is defined as:

```json
    {
      "command": "git add",
      "description": "Stage files for commit",
      "group": "git",
      "verified": true,
      "tokens": [
        { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "String", "default": ".", "values": null, "flag": null }
      ]
    },
```

### Analysis of current token:
* **`name`**: `"path"` (The positional token name)
* **`token_type`**: `"String"` (Currently generic string, not auto-completed)
* **`default`**: `"."` (Default to stage all files)
* **`values`**: `null` (No pre-determined set of values)
* **`flag`**: `null` (Positional argument, no flag prefix)

---

## 3. Planned Changes to Schema Files

To support the new `git:status_files` resolver, the `"path"` token schema must be updated.

### Exact Modifications

#### 1. In `/Volumes/goldcoders/waz/schemas/curated/git.json`
* **Target Line**: Line 26
* **Before**:
  ```json
  { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "String", "default": ".", "values": null, "flag": null }
  ```
* **After**:
  ```json
  { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "Enum", "default": ".", "values": [], "flag": null, "data_source": { "resolver": "git:status_files" } }
  ```

#### 2. In `/Users/uriah/.config/zap/schemas/git.json`
* **Target Line**: Line 26
* **Before**:
  ```json
  { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "String", "default": ".", "values": null, "flag": null }
  ```
* **After**:
  ```json
  { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "Enum", "default": ".", "values": [], "flag": null, "data_source": { "resolver": "git:status_files" } }
  ```

### Justification of Token Field Changes
* **`token_type`**: Changed from `"String"` to `"Enum"` to signal that waz/zap should render a list of selectable items.
* **`default`**: Stays `"."` as specified.
* **`values`**: Changed from `null` to `[]` to be consistent with all other resolver-driven Enum tokens in `git.json`.
* **`data_source`**: Added `{ "resolver": "git:status_files" }` to bind the new resolver to the token.

---

## 4. Implementation Plan for `git:status_files` Resolver

To ensure the new resolver works at runtime, the following backend changes must be implemented in the `waz` crate:

### Codebase Target: `/Volumes/goldcoders/waz/src/generate.rs`

#### 1. Registration in `resolve_builtin`
Inside the `resolve_builtin` function, map `git:status_files` to a new helper function:

```rust
        (Some("git"), Some("branches"), _) => git_resolve_branches(cwd),
        (Some("git"), Some("remotes"), _) => git_resolve_remotes(cwd),
        (Some("git"), Some("status_files"), _) => git_resolve_status_files(cwd), // ADD THIS LINE
```

#### 2. Implementation of `git_resolve_status_files`
Add the helper function in the Git resolvers section of the file:

```rust
/// Git: resolve modified/untracked/staged file names using git status.
fn git_resolve_status_files(cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout.lines()
        .filter_map(|s| {
            if s.len() > 3 {
                let path_part = s[3..].trim();
                // Handle renamed files output: "R  old -> new"
                if let Some(pos) = path_part.find(" -> ") {
                    Some(path_part[pos + 4..].to_string())
                } else {
                    Some(path_part.to_string())
                }
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    if files.is_empty() { None } else { Some(files) }
}
```

---

## 5. Verification & Testing

Following changes, validity can be verified by running the `waz` test suite:
1. `cargo test --workspace` or specifically `cargo test -p waz`
2. Manually testing the resolver output via the `waz` CLI or integration tests in TUI context if available.
