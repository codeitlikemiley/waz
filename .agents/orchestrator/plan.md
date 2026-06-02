# Plan - TMP AI Agent Integration Specification

This plan outlines the milestones and tasks required to research, design, and compile the technical specification for integrating Tool Metadata Protocol (TMP) command schemas and resolvers with AI Agents in the Warp ecosystem.

## Milestones

### Milestone 1: Decompose design specs & create Project plan
- **Goal**: Establish scope, milestones, and initial layout for the research and specification.
- **Tasks**:
  1. Define required specification topics based on requirements R1, R2, and R3.
  2. Map out Mermaid diagram content (system flow, translation flow).
- **Verification**: Updated briefing, progress, and plan.

### Milestone 2: Research and document MCP Translation & Agent Integration (R1)
- **Goal**: Design the mapping between TMP schema structures and Model Context Protocol (MCP) or JSON-Schema.
- **Tasks**:
  1. Analyze `TokenDef`, `TokenType`, `DataSource`, and `resolve_data_sources` in `tmp.rs`.
  2. Formulate schema translation rules from TMP tokens/types to MCP tool arguments (JSON-Schema format).
  3. Write details on how MCP-capable agents consume these schemas to validate inputs, discover tools, and provide completions.
- **Verification**: Research findings documented.

### Milestone 3: Research and document Validation & Execution Framework (R2)
- **Goal**: Design the Rust-level interface and execution/validation hooks.
- **Tasks**:
  1. Define Rust traits and structs for command execution (`TmpCommandExecutor`), argument validation (`TmpValidator`), and error types.
  2. Map out execution flows, sandbox gating (pre-execution check), and integration into `crates/ai` (MCP tool execution) and `crates/warp_completer`.
- **Verification**: Rust draft structures and execution flows completed.

### Milestone 4: Research and document Workspace-Level Custom Schema Discovery (R3)
- **Goal**: Design the discovery mechanism for developer-defined schemas.
- **Tasks**:
  1. Define directory conventions for local workspace schemas (e.g. `./.waz/schemas/*.json`, `./.warp/tmp/*.json`).
  2. Describe loading, validation, and security constraints for project-specific custom scripts/tools.
- **Verification**: Discovery rules and schema security guidelines completed.

### Milestone 5: Compile first draft of specs/tmp_ai_integration.md
- **Goal**: Generate the comprehensive technical specification file.
- **Tasks**:
  1. Combine R1, R2, and R3 designs.
  2. Include Mermaid diagrams for system architecture flow.
  3. Write example JSON schema transformations (TMP to MCP/JSON-Schema).
  4. Save the file to `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`.
- **Verification**: File exists at path.

### Milestone 6: Review, refine, and verify specs/tmp_ai_integration.md
- **Goal**: Ensure correctness, completeness, and formatting compliance.
- **Tasks**:
  1. Verify specification has Mermaid diagrams, example schemas, draft Rust traits, and clear discovery guides.
  2. Check spelling and Markdown rendering.
- **Verification**: Verification and final sign-off.
