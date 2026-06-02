## 2026-05-31T10:08:06Z
You are the Worker subagent (TMP AI Integration Worker) working in directory `/Volumes/goldcoders/zap/.agents/worker_tmp_spec_1`.

Your mission is to compile the final comprehensive technical specification and implementation plan file at `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`.

You must incorporate all the findings and structure detailed in the explorer's research report located at `/Volumes/goldcoders/zap/.agents/explorer_tmp_ai_1/research_report.md`. Make sure to read that file to get all details.

The technical specification file `specs/tmp_ai_integration.md` must include:
1. **Title**: Tool Metadata Protocol (TMP) Integration with AI Agents.
2. **Executive Summary**: Rationale, advantages of structured JSON-Schema/MCP tools over plain text prompt injection (token efficiency, execution safety, reliability).
3. **Architecture Overview**: Complete system architecture flow diagrams (using Mermaid syntax) illustrating the integration:
   - Schema translation (TMP CommandEntry -> JSON-Schema).
   - Validation & execution loop (Agent invokes dynamic tool -> validation stage -> command assembly -> shell execution).
4. **Schema Translation Design (R1)**:
   - Detailed mapping rules for TokenDef `TokenType` primitives (`String`, `Boolean`, `Enum`, `File`, `Number`) to JSON-Schema.
   - Example schema transformation showing a standard TMP command definition in JSON (e.g., git commit or cargo build) and its translated MCP tool schema definition.
5. **Validation and Execution Framework (R2)**:
   - Proposed Rust interfaces/traits (`TmpCommandValidator`, `TmpCommandExecutor`, error enums).
   - Flow descriptions of safety checks: type matching, enum bound check, shell injection character scanning (e.g. `;`, `&`, `|`, etc.), security gating (dangerous commands prompt user approval).
   - Detailed integration points within `crates/ai` and `crates/warp_completer`.
6. **Workspace-Level Schema Discovery & Security (R3)**:
   - Directory scanning paths: `.waz/schemas/*.json` and `.warp/tmp/*.json` in active workspace.
   - Trust boundary gating logic: Dynamic data source resolvers that run shell commands (via `command`) are disabled if the workspace is untrusted; built-in resolvers (like `cargo:*`, `git:*`, `npm:*`) are always allowed.
   - Flow of workspace trust state management.

Please write the file to `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` (creating the parent directory if it does not exist). Ensure it is extremely well-formatted, professional, clear, and comprehensive.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A Forensic Auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Once complete, write your handoff report to `/Volumes/goldcoders/zap/.agents/worker_tmp_spec_1/handoff.md` and notify me.
