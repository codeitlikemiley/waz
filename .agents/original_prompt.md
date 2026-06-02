## 2026-05-30T14:36:39Z

Improve path suggestion resolution and confirm selection behavior in the Tool Metadata Protocol (TMP) Form Panel for git commands and file completions.

Working directory: /Volumes/goldcoders/zap
Integrity mode: development

## Requirements

### R1. Implement Git Status File Resolver
Implement a new built-in TMP data source resolver `git:status_files` in `crates/warp_completer/src/signatures/tmp.rs`. This resolver must:
- Execute `git status --porcelain` in the current working directory (`cwd`).
- Parse the output line-by-line: extract the relative file paths of modified (`M`), untracked (`??`), or renamed (`R  old -> new`) files.
- Return the list of files as a vector of strings. On WASM target, it should return `None`.

### R2. Update `git add` Curated Schema
Update `git.json` schema (in both `/Volumes/goldcoders/waz/schemas/curated/git.json` and the active user config path `~/.config/zap/schemas/git.json`) to:
- Change the `path` token definition of the `git add` command to have `token_type` `"Enum"`.
- Set `"default"` to `"."`.
- Add `"data_source": { "resolver": "git:status_files" }`.

### R3. Fix Tab & Suggestion Confirmation inside TmpFormPanel
Ensure that when the completions menu is open (whether it is an Enum list like branches/status files, or standard path completions), pressing Tab or Arrow Down/Up and confirming suggestion correctly updates the active token value in the TMP state and modifies the editor buffer text.

## Verification Plan

### Automated Tests
- Run input unit tests in the `warp` package to verify `test_tmp_path_completions` and any new tests:
  ```bash
  cargo test --package warp --lib -- terminal::input::tests::
  ```
- Run completer tests in `warp_completer`:
  ```bash
  cargo test --package warp_completer --lib
  ```

## Acceptance Criteria

### Git Status Suggestion Correctness
- [ ] Running completions on `git add` in the TMP Form Panel returns all modified/untracked files from `git status --porcelain`.
- [ ] Hitting Tab or Arrow keys cycles through these suggested files.
- [ ] Selecting/confirming a suggestion inserts the chosen filename into the input buffer (e.g. `git add example.txt`) and updates the preview.

## 2026-05-31T10:05:20Z

Research and compile a comprehensive implementation plan and architectural specification detailing how the Tool Metadata Protocol (TMP) command schemas and resolvers can be leveraged by AI Agents in the Warp ecosystem to validate, discover, and execute commands.

Save the final design specification file as a new technical specification under `specs/tmp_ai_integration.md` in the `/Volumes/goldcoders/zap` workspace.

Working directory: /Volumes/goldcoders/zap
Integrity mode: development

## Requirements

### R1. AI Agent Integration & Schema Translation Design (MCP-Aligned)
Analyze the current TMP schemas and design a translation layer to convert them into Model Context Protocol (MCP) tool schemas or JSON-Schema definitions. Describe how standard MCP-capable AI agents can consume these schemas to dynamically discover available terminal tools, validate user input parameters, and offer dynamic completions in terminal/agent interfaces.

### R2. Validation and Execution Framework Design (Rust-level)
Design a Rust-level execution and validation framework where AI agents invoke terminal commands using structured JSON arguments matching TMP definitions. The framework should compile the structured JSON back into command strings or execute them directly via a secure validation sandbox gatekeeper. Provide concrete Rust traits/struct draft definitions, error types, and execution hook flows showing integration into the `crates/ai` and `crates/warp_completer` modules.

### R3. Workspace-Level Custom Schema Discovery
Define a mechanism for the AI Agent to automatically scan, discover, and load custom developer-defined TMP schemas (e.g. `./.waz/schemas/*.json` or `./.warp/tmp/*.json`) within the active workspace root. Provide design guidelines on how AI agents read these local files to dynamically support project-specific terminal tools/scripts.

## Acceptance Criteria

### Technical Specification Document
- [ ] A detailed architectural design document saved to `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`.
- [ ] Includes complete system architecture flow diagrams (using Mermaid) illustrating schema translation, validation, and command execution.
- [ ] Includes draft Rust interfaces/traits demonstrating how the completer's TMP parsing code communicates with the agent's tool execution engine.
- [ ] Provides example schema transformations from a standard TMP tool JSON schema to an MCP tool schema.
