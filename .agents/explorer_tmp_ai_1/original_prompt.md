## 2026-05-31T10:06:30Z
You are the Explorer subagent (TMP AI Integration Explorer) working in directory `/Volumes/goldcoders/zap/.agents/explorer_tmp_ai_1`.

Investigate the Tool Metadata Protocol (TMP) schema parsing, resolvers, and its current integrations in the Warp codebase, and draft a detailed research report on how to integrate TMP with AI Agents in an MCP-aligned way.

Your investigation must focus on the following:
1. Analyze `/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp.rs`:
   - Understand the struct definitions: SchemaFile, SchemaMeta, CommandEntry, TokenDef, TokenType, DataSource.
   - Trace how resolvers work (`resolve_data_sources`, `resolve_builtin`, `git_resolve_status_files`, etc.).
   - Understand how commands are parsed and values extracted (`extract_token_values`, `build_assembled_command`, `find_matching_tmp_command`).
   - Look at `get_active_tmp_prompt` to see how TMP schemas are formatted for the agent prompt.

2. Analyze `/Volumes/goldcoders/zap/app/src/ai/agent_providers/prompt_renderer.rs` and `/Volumes/goldcoders/zap/app/src/ai/agent_providers/tools/`:
   - Understand how system prompts render `tmp_context`.
   - Look at `mcp.rs` and other files to understand the tool schema definition structure, parameter validation, and execution format.
   - Research the Model Context Protocol (MCP) tool schema spec (which utilizes JSON-Schema definitions for inputs).

3. Propose a detailed mapping & architectural design for:
   - R1: Translating TMP command schemas (e.g. CommandEntry + TokenDef) into JSON-Schema / MCP tool schemas (mapping types like String, Boolean, Enum, File, Number to JSON Schema type/format/enum definitions, and handling required/optional fields).
   - R2: A Rust-level validation & execution framework (defining traits and structs like `TmpCommandExecutor`, `TmpCommandValidator`, and flow of validation checks before executing, including security gating).
   - R3: Workspace-level custom schema discovery (how the agent scans `.waz/schemas/*.json` or `.warp/tmp/*.json` in the workspace root, loads them, and resolves dynamic data sources).

Please write a comprehensive research and architecture report named `research_report.md` in your working directory `/Volumes/goldcoders/zap/.agents/explorer_tmp_ai_1/research_report.md`.
Deliver the handoff report in `/Volumes/goldcoders/zap/.agents/explorer_tmp_ai_1/handoff.md` and notify me when done.
