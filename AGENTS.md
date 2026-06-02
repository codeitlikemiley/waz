# AGENTS.md

> This document serves as a navigation guide for AI/automated agents working in this repository. It summarizes the overall architecture of the repository, the responsibilities of each crate in the Cargo workspace, the boundaries of each submodule under the `app/` main binary, and the engineering conventions that must be adhered to before making modifications.
>
> It is a companion document to `WARP.md`: `WARP.md` is the developer manual (commands, style, processes), while this document is the **code map**. Read `WARP.md` first, then use this document to locate the correct crate/module.

---

## 1. Repository Overview

Warp is a Rust-centric **agentic terminal / development environment**: built on a custom UI framework (WarpUI), it integrates terminal emulation, AI agents, cloud sync (Drive), code review, completion, Notebook, settings, IPC, and other capabilities.

Top-level directories:

| Directory | Purpose |
|------|------|
| `app/` | Main binary crate (`warp`), assembling all subsystems, UI, database migrations, and platform glue layer |
| `crates/` | 67 workspace members, library crates split by responsibility |
| `command-signatures-v2/` | Independent subproject (excluded when running nextest with `--exclude`) |
| `script/` | Cross-platform bootstrap, build, and presubmit scripts |
| `resources/` | Fonts, icons, shell integration scripts, shaders, and other runtime resources |
| `docker/` | Containerized build configurations |
| `specs/` | Product/Technical spec documents |
| `.agents/skills`, `.claude/skills` | Skill descriptions for agent workflows (PR creation, error fixing, feature gating/rollout, etc.) |
| `.warp/`, `.config/`, `.cargo/`, `.vscode/` | Configurations for various tools |

Build System: Cargo workspace, `resolver = "2"`. `default-members` is intentionally restricted to the subset that is frequently compiled/tested (see `Cargo.toml`). `serve-wasm` and `integration` are excluded from `default-members` by default.

License Split:
- `crates/warpui` and `crates/warpui_core` → MIT
- Others → AGPL-3.0-only

---

## 2. Top-Level Architectural Layers

There are roughly 4 layers from bottom to top. When adding new code or locating a bug, first determine which layer the change belongs to, and **never introduce circular or downward-pointing dependencies across layers**.

```
app/  (Main binary: assembly, entry points, platform glue, persistence migrations, UI view root)
  ↑
Product domain crates: ai / computer_use / vim / onboarding /
                      warp_completer / lsp / languages / code-review …
  ↑
Framework crates: warpui / warpui_core / warpui_extras / editor /
            ui_components / sum_tree / syntax_tree
  ↑
Infrastructure crates: warp_core / warp_util / http_client /
                websocket / ipc / jsonrpc / persistence / graphql /
                managed_secrets / virtual_fs / watcher / asset_cache …
```

Key Architectural Patterns (see `WARP.md` for details):

1. **Entity-Handle System**: `App` globally owns all view/model entities. Views reference each other via `ViewHandle<T>` rather than direct ownership.
2. **Element / Action**: The UI is composed of a declarative Element tree + Action event system (Flutter-style).
3. **Cross-Platform**: Native implementation for macOS / Windows / Linux + WASM target. Platform-specific code is isolated using `#[cfg(...)]`.
4. **AI Integration**: Agent Mode and context index. Code is concentrated in `app/src/ai` (389 files) and `crates/ai`.
5. **Cloud Sync**: `Drive` enables object synchronization across multiple devices. See `app/src/drive` and `crates/warp_files`.
6. **Feature Flag**: Runtime rollout/gating takes precedence over `#[cfg]`. The enum is defined in `crates/warp_core/src/features.rs`.

---

## 3. `crates/` Overview

The table below lists all 67 crates grouped by topic. Each row contains only a **one-sentence description of its responsibility**; to view implementation details, open `crates/<name>/src/lib.rs` directly (many crates have `//!` module documentation at the top of `lib.rs`).

### 3.1 UI Framework / View Layer

| Crate | Responsibility |
|-------|------|
| `warpui_core` | WarpUI framework core (MIT): `App` / `Entity` / `ViewHandle` / `AppContext` and other infrastructure |
| `warpui` | WarpUI high-level components, Element tree, layout, rendering pipeline (MIT) |
| `warpui_extras` | Optional extensions of WarpUI, not all features are enabled by default |
| `ui_components` | High-level component library reused across views (buttons, inputs, lists, modals, etc.) |
| `editor` (`warp_editor`) | Text editor: buffers, selections, cursors, keymaps, undo stack |
| `sum_tree` | Persistent balanced B-tree, core data structure for editor / Notebook / large lists |
| `syntax_tree` | Tree-sitter wrapper and syntax highlighting support |
| `markdown_parser` | Markdown parsing (used for AI messages, document views, Notebooks, etc.) |
| `vim` | Vim mode keybindings and operational semantics |
| `voice_input` | Voice input support |

### 3.2 Terminal

| Crate | Responsibility |
|-------|------|
| `warp_terminal` | Terminal emulation core: PTY management, ANSI/VT parsing, grid, scrolling, shell integration hooks |
| `input_classifier` | Terminal input intent classification (pure commands / natural language / AI prompt) |
| `natural_language_detection` | Natural language detection (in conjunction with `input_classifier`) |

### 3.3 AI / Agent

| Crate | Responsibility |
|-------|------|
| `ai` | AI model client, prompt orchestration, agent protocols, tool invocation framework |
| `computer_use` | Rust-side implementation of "Computer Use" tool capabilities (screenshot, click, typing, etc.) |
| `command-signatures-v2` | Command signatures v2 (command classification metadata for AI); independent project, excluded from the main workspace test suite |
| `onboarding` | Data and state for the new user onboarding process |

### 3.4 Network / Protocols / IPC

| Crate | Responsibility |
|-------|------|
| `http_client` | Unified HTTP client wrapper for the workspace |
| `http_server` | Embedded HTTP server (local RPC, login callbacks, etc.) |
| `websocket` | Shared WebSocket abstraction for native and WASM, compatible with `graphql_ws_client` |
| `ipc` | General-purpose typed IPC request/response protocols (inter-process) |
| `jsonrpc` | JSON-RPC implementation |
| `lsp` | Language Server Protocol client implementation |
| `remote_server` | Server-side logic under remote sshd mode |
| `serve-wasm` | Auxiliary server to host WASM build artifacts (excluded from compilation by default) |
| `firebase` | Firebase client tools (Crash reporting, analytics channels, etc.) |

### 3.5 Persistence / Files / Resources

| Crate | Responsibility |
|-------|------|
| `persistence` | Diesel + SQLite persistence foundation; **migrations are in `app/migrations/`, schema in `app/src/persistence/schema.rs`** |
| `warp_files` | Synchronizable file objects like Drive files, Workflows, Notebooks, etc. |
| `virtual_fs` | Abstract file system (identical interface for test mock and production real FS) |
| `repo_metadata` | Repository metadata: file tree construction, `.gitignore` processing, file system watching |
| `watcher` | File system watcher (wrapper around `notify`) |
| `asset_cache` | Asset disk/memory cache |
| `asset_macro` | Asset reference macros such as `bundled!` / `theme!` |
| `managed_secrets` / `managed_secrets_wasm` | Keychain / DPAPI / Linux Keyring abstraction + WASM proxy |

### 3.6 Configuration / Settings

| Crate | Responsibility |
|-------|------|
| `settings` | Settings storage and change distribution |
| `settings_value` | `SettingsValue` trait: controls TOML serialization semantics |
| `settings_value_derive` | `#[derive(SettingsValue)]` procedural macro (converts enum variants to snake_case, etc.) |
| `warp_features` | High-level Feature Flag API (consumer-side) |
| `channel_versions` | Release channels (stable/preview/dogfood) and version comparison |

### 3.7 Commands / Completions / Languages

| Crate | Responsibility |
|-------|------|
| `command` | Safe wrapper for cross-platform process spawning, **specifically handling the `no_window` flag on Windows**; all newly spawned child processes must use this |
| `warp_completer` | Completion engine (supports `--features v2`) |
| `languages` | Language/extension/Tree-sitter grammar registration |
| `warp_ripgrep` | Thin ripgrep wrapper for `warp_cli` |
| `warp_cli` | CLI subcommands parsing within the binary (`warp <subcmd>`) |
| `fuzzy_match` | Fuzzy matching + glob-style wildcard matching, used for path searches and command palette |

### 3.8 Platform / System Services

| Crate | Responsibility |
|-------|------|
| `app-installation-detection` | Detect installed apps on the system (used for launcher linkage) |
| `prevent_sleep` | Prevent sleep mode (during long tasks / AI Agent execution) |
| `isolation_platform` | Compatibility layer running inside sandboxes like Docker / GitHub Actions |
| `node_runtime` | Automatically install/manage Node.js and npm (macOS/Linux/Windows × multi-architecture) |
| `warp_js` | Helper abstraction to manipulate JavaScript values/functions on the Rust side |

### 3.9 Common Utilities / Communication

| Crate | Responsibility |
|-------|------|
| `warp_core` | The lowest-level "core" of the workspace: platform abstractions, the `FeatureFlag` enum in `features.rs`, and `DOGFOOD/PREVIEW/RELEASE_FLAGS` |
| `warp_util` | General utility functions shared across multiple crates |
| `warp_logging` | Unified entry point for logging configuration |
| `simple_logger` | Simple async file logger for stderr-only processes like `remote_server` |
| `warp_web_event_bus` | Web-side event bus (for embedded web views) |
| `field_mask` | gRPC/Proto style FieldMask utility |
| `string-offset` | Basic offset types (byte/char/utf16) |
| `handlebars` | Handlebars template engine wrapper |
| `integration` | Integration testing framework, used exclusively for tests |

> Naming Nuisance: the package name for `crates/editor` is `warp_editor`; `crates/isolation_platform` is `warp_isolation_platform`; `crates/managed_secrets` is `warp_managed_secrets`; `crates/virtual_fs` is `virtual-fs` (hyphenated); `crates/string-offset` is `string-offset` (hyphenated).

---

## 4. `app/` Submodule Navigation

`app/src/` contains 60+ product domain directories, with each directory roughly corresponding to a product feature line. Below they are grouped by theme, with the approximate number of `.rs` files in parentheses to estimate the module volume:

### 4.1 Startup / Assembly / Global
- `bin/` (7) — Multiple binary entry points (main program, companion tools).
- `lib.rs` / `app_state.rs` / `app_state_tests.rs` — Application state root.
- `app_menus.rs`, `app_services/`, `app_id_test.rs`
- `appearance.rs`, `gpu_state.rs`, `font_fallback.rs`, `global_resource_handles.rs`
- `dynamic_libraries.rs`, `alloc.rs`, `tracing.rs`, `profiling.rs`
- `crash_recovery.rs`, `crash_reporting/` (4)
- `features.rs` — Consumption of `warp_core::FeatureFlag` within `app/`; when adding a new flag, it usually needs to be hooked up in both places.
- `channel.rs`, `download_method.rs`, `autoupdate/` (8)

### 4.2 Terminal
- `terminal/` (427) — Core: shell processes, PTY, grid, blocks, shell integration, command execution, and I/O pipeline.
- `default_terminal/` (2) — Default terminal startup logic.
- `shell_indicator.rs`, `prefix.rs` / `prefix_test.rs` (command prefix parsing), `vim_registers.rs`

### 4.3 AI / Agent
- `ai/` (389) — Contains Agent UI, conversation models, Agent management, tools/MCP, Cloud Agent, Plan/Diff views, artifacts, blocklists, execution profiles, etc. **This is the largest subtree in the repository**. Before making changes, first grep for specific sub-topics (`agent_*`, `conversation_*`, `cloud_agent_*`, `mcp`, `tool_*`) within this directory.
- `ai_assistant/` (9) — Old AI assistant entry point/adapter.
- `chip_configurator/`, `context_chips/` (22) — Agent context chip selection/construction.
- `coding_entrypoints/` (5), `coding_panel_enablement_state.rs`
- `prompt/` (2), `tips/` (3), `voice/` (2), `completer/` (3)

### 4.4 Editor / Code / Review
- `editor/` (38) — Main editor integration.
- `code/` (52) — Code views, diffs, and navigation.
- `code_review/` (36) — Code Review flow.
- `notebooks/` (30), `workflows/` (22)

### 4.5 Search
- `search/` (172) — Multi-target search (files, commands, Agent history, etc.).
- `search_bar.rs`

### 4.6 Server Communication / Drive / Sync
- `server/` (55) — HTTP/WS interaction with the Warp backend (corresponds to the local development mode `with_local_server`).
- `drive/` (45) — Cloud object sync entry point.
- `cloud_object/` (12) — Cloud object abstraction layer (workflows, notebooks, etc.).
- `remote_server/` (5) — Client-side glue for connecting to remote mode sshd.

### 4.7 Settings / User Config / Themes / Onboarding
- `settings/` (46), `settings_view/` (63)
- `user_config/` (6), `themes/` (11), `appearance.rs`
- `experiments/` (7), `tab_configs/` (15), `launch_configs/` (4)
- `tips/`, `banner/` (3), `quit_warning/` (1), `wasm_nux_dialog.rs`, `referral_theme_status.rs`

### 4.8 Authentication / Billing / Usage
- `auth/` (22) — Login, tokens, and SSO.
- `billing/` (3), `pricing/` (1), `usage/` (1), `reward_view.rs`

### 4.9 Persistence
- `persistence/` (9) — Diesel migrations assembly, `schema.rs` (generated by Diesel), and migration runners.
- Migration files are located in the top-level `migrations/` directory of the repository (managed by Diesel CLI).

### 4.10 Platform / System Integration
- `platform/` (2), `system/` (3) / `system.rs`
- `login_item/` (3), `antivirus/` (3), `network.rs`
- `external_secrets/` (1), `env_vars/` (14)
- `keyboard.rs` / `keyboard_test.rs`, `safe_triangle.rs` / `safe_triangle_tests.rs` (menu hover safe triangle)

### 4.11 View Root / Panels / Common UI
- `root_view.rs` / `root_view_tests.rs`
- `pane_group/` (35) — Split-pane and block layouts.
- `tab.rs`, `command_palette.rs`, `modal.rs`, `menu.rs` / `menu_test.rs`
- `palette.rs`, `notification.rs`, `resource_center/` (10)
- `view_components/` (20), `ui_components/` (14)
- `workspace/` (54), `workspaces/` (10), `voltron.rs` (multi-window/multi-workspace coordination)
- `session_management.rs`, `undo_close/` (3), `word_block_editor.rs`
- `suggestions/` (2), `input_suggestions.rs` / `input_suggestions_test.rs`
- `plugin/` (21) — Plugin system integration.
- `uri/` (7) — `warp://` URL handling.
- `debug_dump.rs`, `debounce.rs`, `interval_timer.rs`, `throttle.rs`
- `linear.rs`, `resource_limits.rs`, `warp_managed_paths_watcher.rs`
- `preview_config_migration.rs` / `preview_config_migration_tests.rs`
- `window_settings.rs`, `projects.rs`

### 4.12 Test Infrastructure
- `integration_testing/` (79) — End-to-end integration testing support.
- `test_util/` (6) — Common utilities for unit tests.

---

## 5. Engineering Discipline (Hard Constraints for Agents)

> These are compiled based on `WARP.md` and custom project rules; validation requirements for agents in this document are subject to `cargo check`.

### 5.1 Required Reading Conventions
- **Comments and replies must be in English** (User Rule).
- Use the `fff` tool or `rg -n "<keyword>" <path>` for searching/grepping within the git index; `read_file` is strictly for images/binary files.
- Before submitting a PR / pushing a new commit, you **only** need to pass: `cargo check`.
- Changes must be precise: **every line of modification must be traceable to a user request**. Do not casually "improve" unrelated code, comments, or formatting.
- Simplicity first: do not introduce abstractions, configurations, error handling, or redundant features for single-point usage.
- Explain alternative options and expose uncertainties, rather than making choices for the user silently.
- worktree path: .worktrees/<worktree_name>/

### 5.2 Rust Style (Excerpted from `WARP.md`)
- Do not write redundant type annotations for closure parameters.
- Unify `use` statements at the top; do not write long fully-qualified paths, except within `#[cfg]` branches.
- Context parameters must be named `ctx` and placed last; if there is also a closure parameter, place the closure last.
- Unused parameters must be **deleted directly** rather than prefixed with `_`. Update caller sites accordingly.
- Use inline format arguments in macros like `println!` / `format!` (`"{x}"` instead of `"{}", x`) to satisfy the `uninlined_format_args` lint.
- `match` statements **must not use the `_` wildcard** (unless absolutely necessary); maintain exhaustive matching.
- Do not delete or modify existing comments due to unrelated modifications.

### 5.3 Terminal Model Lock (High Priority!)
- Calling `TerminalModel::lock()` is highly prone to deadlocks (which manifests as UI freezing / beachballing on macOS).
- Before adding `model.lock()`, you must verify that no upper level in the call stack already holds the lock. Try to pass the locked reference down the call stack instead of re-locking.
- Minimize the lock scope, and do not call functions that might re-acquire the lock while holding it.

### 5.4 Feature Flag
- Add: add a variant to the `FeatureFlag` enum in `crates/warp_core/src/features.rs`; add it to `DOGFOOD_FLAGS` / `PREVIEW_FLAGS` / `RELEASE_FLAGS` as needed.
- Usage: **Prefer** runtime checks using `FeatureFlag::Xxx.is_enabled()` over `#[cfg(...)]`; use `cfg` only when it cannot compile without it (due to platform or optional dependencies).
- Wrap the entire product feature section rather than adding it to every single invocation point; **clean up flags and dead branches** after stabilization.
- The UI entry point and the code path must be controlled by the same flag.

### 5.5 Database
- ORM: Diesel + SQLite.
- Adding or modifying schemas must go through migrations: add a new directory under `migrations/` (`up.sql` / `down.sql`). Do not manually edit `app/src/persistence/schema.rs` (which is generated by `diesel print-schema`).

### 5.6 Testing
- Use `cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2`.
- Place unit tests in `${filename}_tests.rs` or `mod_test.rs`. At the end of the original file, use:

  ```rust
  #[cfg(test)]
  #[path = "filename_tests.rs"]
  mod tests;
  ```

- For integration tests, use the framework in `crates/integration`; examples are located in `app/src/integration_testing/`.

### 5.7 Cross-Process Commands
- Do not directly use `std::process::Command::new(...)` (which popups a window especially on Windows); unify process spawning via `crates/command`.

### 5.8 Sub-agents / Multi-agents
- Split large tasks into parallel sub-tasks with **non-overlapping write scopes**; information gathering tasks can also run in parallel.
- Execute simple tasks directly without over-splitting.

---

## 6. Quick Reference for Common Entry Points

| Goal | Entry Point / Start Point |
|---------|------|
| Modify terminal grid / shell integration | `crates/warp_terminal/src/`, coupled with `app/src/terminal/` |
| Modify Agent UI / Conversations | grep by sub-theme (`agent_*` / `conversation_*`) within `app/src/ai/` |
| Modify command completion | `crates/warp_completer/` (note the `--features v2` flag) |
| Modify AI models / tool invocation protocols | `crates/ai/` |
| Add new settings items | `crates/settings_value*`, `crates/settings`; UI is in `app/src/settings_view/` |
| Add Feature Flag | `crates/warp_core/src/features.rs` + usage sites |
| Modify cloud sync objects | `crates/warp_files` + `app/src/drive/` + `app/src/cloud_object/` |
| Modify persistence schema | add migrations in `migrations/` + `crates/persistence` |
| Add new binary tools | `app/src/bin/` |
| Platform-specific code | use `#[cfg(target_os = "...")]`; UI platform glue is in `app/src/platform/` |
| Vim Mode | `crates/vim` + `app/src/vim_registers.rs` |
| Navigation / View | `app/src/notebooks/`, `app/src/workflows/`, `crates/warp_files` |
| Cross-platform process spawning | `crates/command` |
| File search / watching | `crates/repo_metadata`, `crates/watcher`, `crates/warp_ripgrep` |

---

## 7. Pre-modification Checklist

Before modifying the code, ask yourself once:

1. Which layer / crate / `app/src/<submodule>` does this change belong to? Will the modifications cross layer boundaries?
2. Do you need to add dependencies? If existing workspace dependencies can be reused, prioritize reusing them in `Cargo.toml` under `[workspace.dependencies]`.
3. Is this a product feature? Does it need to be wrapped in a Feature Flag?
4. Does it involve the terminal model? Does the current call stack already hold the `TerminalModel` lock?
5. Does it involve child processes? Does it use `crates/command`?
6. Does it involve persistence? Is a database migration required?
7. Have you written the corresponding `${file}_tests.rs`?
8. Is `cargo check` green?
9. Can every line of your modifications be mapped directly to the user request? Should any casual "micro-refactoring" be rolled back?

Go through all 9 items above before delivering/submitting.
