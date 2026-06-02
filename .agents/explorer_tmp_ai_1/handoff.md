# Handoff Report: TMP AI Integration Research & Architecture Design

## 1. Observation
We have systematically investigated the Tool Metadata Protocol (TMP) schema parsing, resolvers, and its current integrations in the Warp codebase, and drafted a detailed research report on how to integrate TMP with AI Agents in an MCP-aligned way.

Direct observations include:
1. **`/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp.rs`**:
   - Struct definitions:
     - `SchemaFile` (lines 6-10): `pub struct SchemaFile { pub meta: SchemaMeta, pub commands: Vec<CommandEntry> }`
     - `SchemaMeta` (lines 12-38): contains metadata fields like `tool`, `requires_file`, `requires_binary`, and `keywords`.
     - `CommandEntry` (lines 43-51): represents a subcommand signature pattern.
     - `TokenDef` (lines 54-68): represents parameter properties like `name`, `description`, `required`, `token_type`, `default`, `values`, `flag`, and `data_source`.
     - `TokenType` (lines 82-89): enum mapping `String`, `Boolean`, `Enum`, `File`, `Number`.
     - `DataSource` (lines 70-78): represents dynamic choices resolved via `resolver` or shell `command`.
   - Data source resolution: `resolve_data_sources` (lines 155-174) resolves built-ins via `resolve_builtin` (lines 196-212) or shell commands via `run_data_source_command` (lines 176-189).
   - Command parsing & assembly: `extract_token_values` (lines 771-940) parses buffer command arguments and values into tokens; `build_assembled_command` (lines 669-726) compiles a list of values back to a single shell command; `find_matching_tmp_command` (lines 942-983) locates active subcommands.
   - Prompt compilation: `get_active_tmp_prompt` (lines 548-667) gathers active schemas matching user search query, resolves their dynamic choices, and formats them into a Markdown text block.

2. **`/Volumes/goldcoders/zap/app/src/ai/agent_providers/prompt_renderer.rs`**:
   - `render_system` function calls the TMP prompt generator:
     ```rust
     let tmp_context = prompt_ctx.cwd.as_ref().and_then(|cwd| {
         warp_completer::signatures::tmp::get_active_tmp_prompt(cwd, query)
     });
     prompt_ctx.tmp_context = tmp_context;
     ```
   - In `/Volumes/goldcoders/zap/app/src/ai/agent_providers/prompts/partials/footer.j2` (lines 11-13):
     ```jinja
     {%- if tmp_context %}
     {{ tmp_context }}
     {%- endif %}
     ```
     This appends the formatted TMP Markdown directly to the system prompt footer.

3. **`/Volumes/goldcoders/zap/app/src/ai/agent_providers/tools/mcp.rs`**:
   - `build_mcp_tool_defs` (lines 79-140) translates dynamic MCP tool schema definitions lexicographically.
   - `parse_mcp_tool_call` (lines 145-185) handles mapping sanitized tool names back to `api::message::tool_call::Tool::CallMcpTool` and translates JSON structures to `prost_types::Struct`.

---

## 2. Logic Chain
1. *Prompt Bloat & Execution Inefficiency*: Based on observations in `prompt_renderer.rs` and `footer.j2`, TMP schemas are currently formatted into plain text Markdown and injected into the agent system prompt. The model reads this text, manually structures a command string, and runs it via `run_shell_command` tool.
2. *MCP Alignment*: Based on observations in `mcp.rs` and the `OpenAiTool` struct in `tools/mod.rs`, Warp has a framework for passing structured JSON schemas (parameters) to the model. The model makes structured tool calls containing typed JSON arguments, which are parsed locally.
3. *R1 (Translation to JSON-Schema)*: Therefore, we can translate each `CommandEntry` and its `TokenDef`s into a JSON-Schema representation. Positional and flagged tokens map to schema properties; required flags map to the `required` schema array; and token types like String, Boolean, Enum, File, and Number map to their respective JSON-Schema primitives.
4. *R2 (Validation & Execution)*: When the LLM executes the structured tool call, we run type and security validations (e.g., checking for shell injection characters like `;`, `&`, `|`, etc.). Then we assemble the command line string using `build_assembled_command` and delegate execution to the existing trusted terminal/shell executor (`api::message::tool_call::Tool::RunShellCommand`).
5. *R3 (Workspace Discovery & Security)*: To support custom schemas in `.waz/schemas/*.json` or `.warp/tmp/*.json`, we scan the workspace root. However, since custom workspace schemas could execute arbitrary shell commands inside `data_source.command` during prompt generation, we must enforce a trust boundary: shell-command-based datasource resolvers are blocked unless the user has explicitly trusted the workspace.

---

## 3. Caveats
- No code was implemented in the source directories as this is a read-only investigation.
- Shell command execution during dynamic resolver resolution is assumed to run on the user's local operating system using `sh -c` (from `tmp.rs`). In containerized or remote SSH environments, this would execute inside the remote shell sandbox.

---

## 4. Conclusion
Integrating TMP schemas with AI agents in an MCP-aligned way is highly feasible. It requires translating `CommandEntry` and `TokenDef`s to JSON-Schema declarations (R1), validating inputs and delegating command assembly to the shell executor (R2), and implementing workspace custom schema scanning with strict trust-gating to prevent remote code execution (R3). This design has been fully detailed in `research_report.md`.

---

## 5. Verification Method
1. **Report Verification**: Read `/Volumes/goldcoders/zap/.agents/explorer_tmp_ai_1/research_report.md` to review the architectural blueprint.
2. **Current System Tests**: Run the existing test suite to ensure no regressions:
   ```bash
   cargo nextest run -p warp_completer --lib signatures::tmp::tests
   ```
