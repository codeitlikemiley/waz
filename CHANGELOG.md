# Changelog

This document records key changes in each release of Waz. Only functional commits are included, and internal rolling tags such as dev / stable are omitted.
## [Unreleased]

- **AI / BYOP**: port opencode `applyCaching`, enable prompt caching; `write_to_long_running_shell_command` refuses to embed LF in line mode; BYOP LRC monitor fallback changes to silent subtask; `cancel_execution` 50ms window sender leak repair (#134 follow-up, #137)- **Cloud stripping Phase 1–2**: Add `cloud-disabled` channel predicate; clean up billing/pricing, referral/reward, cloud sharing dialog UI; unsubscribe from RTC UpdateManager; retire notebook/folder sync queue- **Platform**: Fix the panic when Spotlight/Finder/Launchpad starts macOS; `run_shell_command` stdout falls back to command grid- **Infrastructure**:`.gitattributes` forced LF; added stale bot and Claude Code GitHub workflow- **Editor**: Code/Markdown viewer adds syntax highlighting for 15 new languages ​​(Dart, Zig, SCSS, R, Julia, OCaml, Erlang, Nix, Groovy, Solidity, GraphQL, Protobuf, Clojure, Elm, CMake)
## [v2026.05.06.preview] — 2026-05-06

- **AI**
  - Integrate DeepSeek CLI agent to improve LSP installation reliability  - LSP changed to global `enabled_lsp_servers` setting, removed `/index` command and codebase indexing runtime  - `/plan` Real replica Plan Mode (system prompt + tool hard guardrail)  - Agent dynamic tool whitelist, `persist_conversations` setting, auto-approve under `ask_user_question` always ask  - BYOP support provider extra headers- **repair**  - `apply_file_diffs` schema changed from `const` to `enum` to adapt to Gemini  - Root cause of SSE lag - genai gzip is turned off by default + workflow split  - Planned folder notebooks are created immediately without a cloud environment- **Brand**: logo and icon use white background; BYOP mode hides credits/billing UI
## [v2026.05.04.preview] — 2026-05-04

- **SSH Manager**: Data layer + persistence + keychain implementation; UI/UX complete access (Panel + Central Pane + Drag + Fold + Connect + Command Palette)- **AI**: Distinguish model "no suggestion" output and improve the prompt system; BYOP historical multi-modal extension to PDF/audio, opencode style ERROR replacement; UserQuery.context.images full link keep alive- **UI**: The title bar search box can hide the switch; the key setting editing state and the shortcut key badge contrast repair- **i18n**: The remaining main interfaces have fixed copywriting in Chinese; `/model` is bound to `alt-shift-/` by default- **Fix**: Anthropic adapter has 1M context beta header by default; BYOP ToolCall’s first frame is the emit placeholder card; OpenAI-strict provider prohibits returning `reasoning_content`- **Infrastructure**: CI fix `.deb` build and enable PR testing
## [v2026.05.03.preview(.2/.3/.4)] — 2026-05-03

- **Upstream synchronization**: merge a large number of warp-upstream commits (tab cross-window drag and drop, shell script recognition, IME cursor, remote server initialization reconstruction, SSH remote-server automatic upgrade, cross-window tab drag, etc.); create rerere + `waz-ours` merge driver; add blacklist document- **AI/BYOP**: tool parameter type-mismatched output coerce layer; suspicious backslash scan tightening to eliminate ls/diff false positives- **i18n**: Chinese internationalization completion (settings panel, etc.)- **Website**: GitHub address is unified to `zerx-lab/warp`; horizontal overflow repair on mobile terminal- **Fix**: Windows taskbar ICO is aligned with the upstream format; NLD in terminal defaults to true to restore Chinese input to automatically enter AI
## [v2026.05.02.preview] — 2026-05-02

- **AI / BYOP**
  - Complete the session compression closed loop - `byop_compaction` module, settings persistence, auto prune, overflow transparent transmission, 1:1 opencode replication  - Migrating reasoning effort from provider settings to input box picker  - Multi-modal attachment capabilities access BYOP path  - Native BYOP webfetch/websearch integration with Exa  - Select the system prompt template according to the model identification and add multiple templates- **Privacy/Cloud Stripping**  - Physically delete P4 and easily strip dead code (anonymous_id / EXPERIMENT_ID_HEADER / settings synchronization / app_focus)  - Cut off closed source telemetry, Sentry, anonymous_id, and Settings to synchronize four outgoing links  - Three privacy switches with default values ​​true → false  - `cloud_conversations` Two waves of cleanup (UI / Privacy / FeatureFlag / AIClient / cargo feature)- **Refactor**: Remove blocklist artificial intelligence response scores and buried points; remove `agent_attribution` and Oz changelog toggle- **CI**: Zhou Jianji changed to officially release and standardize tags
## [v2026.05.01.preview] — 2026-05-01

- **Cloud Stripping**: Physically delete 6 cloud LLM tool + child_agent + orchestration; Physically delete the share modal three-piece set and billing denied modal; change the website to a monochrome logo- **AI**
  - Workflow Autofill access to BYOP one-shot  - BYOP LRC continues to inject context in subsequent rounds + sanitize enhancement + control key token  - Chat flow adds remote login session prompts and inference postback  - genai error mapping refined to Stream / Other variants  - chat stream adapter, fix ToolCall None handling- **Platform**: `warpui_core` avoids repeated scanning of system fonts; the synchronization command unconditionally disables pager and uses `PAGER=cat` instead to retain the real exit code- **Website**: Full site components and i18n reconstruction, Tailwind and global style synchronization
## [v2026.04.30.oss] — 2026-04-30

- **CI**:CHANNEL `preview` → `oss`, fix Windows / macOS build failure- **Refactor**: Delete cloud_mode residual code and settings
## [v2026.04.30.preview] — 2026-04-30

The first preview version of the Waz community branch.
- **Brand and Positioning**: Waz name change + logo remake + community branch README- **BYOP**
  - `async-openai` → `genai`, supports explicit binding of 5 native protocols  - Providers subpage + models.dev data source + quickly add search box  - Streamlined prompt template- **Decentralized Cleanup**: Remove `UseComputer` / `RequestComputerUse` tools, Drive `Create team` / `Join team` entrance, referral related code- **i18n**: Fluent infrastructure + 12 settings_view file translations; ai/features/teams three-page i18n completion- **Website**: Added BYOP landing page (Astro + Tailwind, bilingual in Chinese and English); responsive optimization- **AI**: CJK input classification, reasoning splitting, BYOP tool_call diagnosis, LRC tag-in synthetic virtual subagent + floating window spawn link- **CI**:Release explicitly declares `contents: write` permission modification 403
[Unreleased]: https://github.com/zerx-lab/warp/compare/v2026.05.06.preview...HEAD
[v2026.05.06.preview]: https://github.com/zerx-lab/warp/compare/v2026.05.04.preview...v2026.05.06.preview
[v2026.05.04.preview]: https://github.com/zerx-lab/warp/compare/v2026.05.03.preview.4...v2026.05.04.preview
[v2026.05.03.preview(.2/.3/.4)]: https://github.com/zerx-lab/warp/compare/v2026.05.02.preview...v2026.05.03.preview.4
[v2026.05.02.preview]: https://github.com/zerx-lab/warp/compare/v2026.05.01.preview...v2026.05.02.preview
[v2026.05.01.preview]: https://github.com/zerx-lab/warp/compare/v2026.04.30.oss...v2026.05.01.preview
[v2026.04.30.oss]: https://github.com/zerx-lab/warp/compare/v2026.04.30.preview...v2026.04.30.oss
[v2026.04.30.preview]: https://github.com/zerx-lab/warp/releases/tag/v2026.04.30.preview
