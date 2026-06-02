# Handoff Report — git:status_files Resolver Planning

## 1. Observation
- The curated `git.json` schema exists at `/Volumes/goldcoders/waz/schemas/curated/git.json`.
- The user config `git.json` schema exists at `/Users/uriah/.config/zap/schemas/git.json`.
- The `git add` command schema is defined identically in both files at lines 20-28:
  ```json
  20:     {
  21:       "command": "git add",
  22:       "description": "Stage files for commit",
  23:       "group": "git",
  24:       "verified": true,
  25:       "tokens": [
  26:         { "name": "path", "description": "Files to stage (. for all)", "required": true, "token_type": "String", "default": ".", "values": null, "flag": null }
  27:       ]
  28:     },
  ```
- The backend resolver registration code is located in `/Volumes/goldcoders/waz/src/generate.rs` within the `resolve_builtin` function (lines 240-268):
  ```rust
  239: /// Resolve a built-in named resolver (e.g. "cargo:bins", "git:branches", "waz:models:gemini").
  240: fn resolve_builtin(
  241:     resolver: &str,
  242:     cwd: &str,
  243:     context: Option<&RuntimeContext>,
  244: ) -> Option<Vec<String>> {
  ```
  And specific git resolvers are registered on lines 256-257:
  ```rust
  256:         (Some("git"), Some("branches"), _) => git_resolve_branches(cwd),
  257:         (Some("git"), Some("remotes"), _) => git_resolve_remotes(cwd),
  ```

---

## 2. Logic Chain
- To integrate the new `git:status_files` resolver for the `path` token of `git add` in both the curated and user configuration schemas:
  - We must change the `token_type` from `"String"` to `"Enum"` so the frontend can treat it as a list of selectable choices.
  - We must retain `"default": "."` as specified.
  - We must change `"values": null` to `"values": []` to be consistent with all other dynamic enum resolvers in the schema (e.g. `git:branches`, `git:remotes`).
  - We must add `"data_source": { "resolver": "git:status_files" }` to bind the token to the status files resolver.
- To implement the backend logic:
  - Add a match arm inside `resolve_builtin` mapping `git:status_files` to a new helper function `git_resolve_status_files(cwd)`.
  - The function `git_resolve_status_files(cwd)` will run `git status --porcelain` to extract path changes, handling possible renames (`old -> new` format) cleanly.

---

## 3. Caveats
- No actual code/JSON files were modified in the codebase during this phase, complying with the read-only boundary constraint.
- Assumes the user configuration directory is `/Users/uriah/.config/zap` rather than `/Users/uriah/.config/waz`, since only the former file existed.
- `git status --porcelain` outputs paths relative to the repository root. Depending on the directory from which `git` is executed and how `waz` handles relative paths, further path canonicalization might be needed.

---

## 4. Conclusion
The proposed updates are precise, highly consistent with existing codebase conventions, and ready for implementation. The target changes are localized to:
- Line 26 of `/Volumes/goldcoders/waz/schemas/curated/git.json`
- Line 26 of `/Users/uriah/.config/zap/schemas/git.json`
- `resolve_builtin` match arms in `/Volumes/goldcoders/waz/src/generate.rs`

---

## 5. Verification Method
1. Inspect the two schema files to verify that line 26 of both files matches the target pattern.
2. Verify that `/Volumes/goldcoders/waz/src/generate.rs` is compiled and tested using `cargo test -p waz`.
