# Technical Specification Review Report: Tool Metadata Protocol (TMP) Integration

This report presents a dual-perspective review (Quality Review and Adversarial Review) of the technical specification `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`.

---

# part 1: Quality Review

## Review Summary

**Verdict**: APPROVE

The technical specification is highly detailed, complete, and directly addresses all three requirements:
- **R1 (AI Agent Integration & Schema Translation Design)**: Provides a concrete namespace convention (`tmp__<tool_name>__<command_slug>`), clear mapping from `TokenType` primitives to JSON-Schema types, and a comprehensive example transformation (`cargo build`).
- **R2 (Validation and Execution Framework Design)**: Details Rust traits (`TmpCommandValidator`, `TmpCommandExecutor`), structured `ValidationError` enum variants, and clear integration points within the `crates/ai` tool registration and routing loop.
- **R3 (Workspace-Level Custom Schema Discovery & Trust Gating)**: Formulates a robust scanning paradigm, trust boundary segregation (allowing safe built-in resolvers, while gating external command resolvers behind a trust registry), and specifies the interactive UI authorization flow.

The executive summary successfully justifies shifting from raw markdown-injected prompts to structured MCP-aligned tool schemas (enumerating advantages in tokens, safety, reliability, and UI transparency). Furthermore, all terminology used aligns perfectly with the structs and types implemented in `crates/warp_completer/src/signatures/tmp.rs`.

---

## Findings

### [Minor] Finding 1: Unescaped Shell Metacharacters list
- **What**: The list of unsafe shell metacharacters in Section 4.2 (`is_parameter_safe`) is slightly incomplete.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Line 250
- **Why**: Characters like parentheses `(`, `)`, brace expansions `{`, `}`, quotes `"`, `'`, and wildcards `*`, `?` are not part of the `unsafe_chars` array. While quotes are partly handled by the parser, parentheses can be used for subshell execution (e.g. in some shells or tool contexts), and wildcards can lead to unexpected path expansion during shell execution.
- **Suggestion**: Expand the sanity checks to either strictly quote all arguments using a robust crate (e.g. `shell-words` or `shell-escape`) during command assembly, or expand the list of forbidden characters to include glob/subshell operators.

### [Minor] Finding 2: Standard JSON-Schema Format Compatibility
- **What**: The File TokenType mapping specifies `"format": "path"`.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Line 100
- **Why**: `"format": "path"` is not a standard JSON-Schema format string (like `date-time`, `email`, etc.). While it functions well as an internal hint for Warp's LLM client to render filepath pickers, it may cause warnings if validated against strict JSON-Schema meta-schemas.
- **Suggestion**: Explicitly document this as a Warp-specific custom schema format extension.

---

## Verified Claims

- **Claim 1**: Terminology aligns with `crates/warp_completer/src/signatures/tmp.rs`.
  - *Verification Method*: Viewed `crates/warp_completer/src/signatures/tmp.rs`. Confirmed that `SchemaFile`, `CommandEntry`, `TokenDef`, `DataSource`, and `TokenType` (including its variants `String`, `Boolean`, `Enum`, `File`, `Number`) match the spec mapping and descriptions exactly.
  - *Result*: **PASS**

- **Claim 2**: Trust Gating blocks dynamic command-line resolvers in untrusted workspaces.
  - *Verification Method*: Inspected Section 5.2. Verified that `resolve_data_sources_secure` checks `is_workspace_trusted` before executing `execute_external_shell_resolver`, falling back safely.
  - *Result*: **PASS**

- **Claim 3**: Executive Summary defines advantages of structured tools over prompt injection.
  - *Verification Method*: Confirmed Section 1 defines benefits across Token Efficiency, Reliable Tool Calling, Rigid Validation & Sanitization, and Transparent Execution.
  - *Result*: **PASS**

---

## Coverage Gaps

- **Tool Exit Code & Error Feedback Loop** — *Risk level*: **Medium** — *Recommendation*: Investigate.
  - The spec covers validation failure routing back to the LLM, but does not detail how shell execution failures (e.g. the command runs but returns exit code `1` with stderr output) are formatted and returned to the LLM. It is recommended to define a structured error payload for runtime failures so the agent can understand and self-correct.

- **Workspace Path Canonicalization** — *Risk level*: **Low** — *Recommendation*: Accept risk.
  - In the Workspace Trust Registry, workspace paths should be canonicalized to avoid trust bypass via symbolic links or case-insensitive path mismatches (especially on macOS).

---

## Unverified Items

- **Warp UI confirmation dialog styling**
  - *Reason*: Out of scope for this technical integration backend specification.

---

# part 2: Adversarial Review

## Challenge Summary

**Overall risk assessment**: MEDIUM

While the validation layers are structurally sound, executing shell commands constructed from LLM inputs introduces inherent risks, particularly related to the environment in which commands run and potential edge cases in shell/argument construction.

---

## Challenges

### [High] Challenge 1: Shell Execution PATH hijacking during branch resolution
- **Assumption challenged**: The spec assumes that built-in resolvers (like `git:branches`, which executes `git` via `Command::new("git")`) are always safe to run, even in untrusted workspaces.
- **Attack scenario**: If a user opens an untrusted repository that contains a malicious executable named `git` in the root folder, and the workspace processes or parent environment prepends the workspace folder to `PATH`, running the built-in `git_resolve_branches` will execute the *local malicious* `git` binary instead of the system `git` binary.
- **Blast radius**: Remote code execution (RCE) on opening an untrusted workspace containing custom schemas, completely bypassing the Trust Boundary.
- **Mitigation**: Ensure that the command execution environment for built-in resolvers is initialized with a sanitized, safe `PATH` that does not include the workspace directory, or invoke system binaries using their absolute paths (e.g., `/usr/bin/git` or resolved via a secure environment manager).

### [Medium] Challenge 2: Argument injection via word splitting
- **Assumption challenged**: The spec assumes shell character scanning is sufficient to prevent command injection when assembling command strings.
- **Attack scenario**: If a parameter value does not contain any banned metacharacters (e.g. no `;`, `&`, `|`) but contains spaces, and is parsed as a single argument, it could be split into multiple arguments by the shell if the formatting template does not wrap it in quotes.
  - *Example*: In `git checkout <branch>`, if `<branch>` receives `main --orphan new-branch`, the assembled command becomes `git checkout main --orphan new-branch`. This is not a shell injection (no arbitrary commands were run), but it changes the operation flags, potentially leading to unintended destructive states.
- **Blast radius**: Unexpected tool behavior or destructive actions without user approval.
- **Mitigation**: The command builder must automatically escape and quote every substituted token, or use direct argument arrays `args: &[String]` for spawn executions instead of building raw shell strings whenever possible.

### [Low] Challenge 3: Resolver DoS (OOM / CPU exhaustion)
- **Assumption challenged**: Built-in resolvers parsing files (like `package.json` or `Cargo.toml`) can run unconditionally in untrusted workspaces.
- **Attack scenario**: An attacker commits a massive or recursively structured `package.json` designed to cause deep recursion or high allocation sizes in JSON parsing libraries, causing Warp to crash/hang on folder load.
- **Blast radius**: Denial of Service (DoS) of Warp workspace.
- **Mitigation**: Impose a strict file size ceiling (e.g. 5 MB) on metadata files read by resolvers before passing them to serializers.

---

## Stress Test Results

- **Scenario**: LLM attempts tool call with command injection string: `tmp__git__checkout(branch="main; rm -rf /")`.
  - *Expected Behavior*: `TmpCommandValidator` flags `;` as unsafe metacharacter.
  - *Predicted Behavior*: The validator blocks the execution, raising `ValidationError::UnsafeShellMetacharacters`. (**PASS**)

- **Scenario**: Workspace has custom resolver executing `data_source.command = "curl attacker.com"` in `.waz/schemas/config.json`.
  - *Expected Behavior*: Silent loading of schema, but `is_workspace_trusted` is false, so resolver command execution is blocked.
  - *Predicted Behavior*: Logs a warning and blocks command resolver execution; built-in resolvers continue to run normally. (**PASS**)

---

## Unchallenged Areas

- **WASM compilation specifics**: Specific WASM-safe mock implementations are out of scope as command execution is primarily run natively inside the terminal PTY.
