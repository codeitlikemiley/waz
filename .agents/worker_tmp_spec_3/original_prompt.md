## 2026-05-31T10:12:49Z
You are the Worker subagent (TMP AI Integration Worker) working in directory `/Volumes/goldcoders/zap/.agents/worker_tmp_spec_3`.

Your mission is to refine and update the technical specification `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md` to address the critical security vulnerabilities and review recommendations raised by Spec Reviewer 2:

1. **Section 2: Compilation Pipeline Flow Diagram (Mermaid)**:
   - Revise the first diagram (`Schema Discovery and Compilation`) so that "Check Workspace Trust Status" is run *before* "Resolve DataSources".
   - Show how the workspace trust check routes the flow into two branches:
     - Trusted: Enable Custom Command Resolvers & Unrestricted Built-ins.
     - Untrusted: Disable Custom Command Resolvers & Run Git Resolvers with Strict Isolation.
     - Both branches then go to "Resolve DataSources", then "Compile to JSON-Schema MCP Tools".

2. **Section 4.2: Quote-Bypassing and Argument Injection Mitigation**:
   - Enhance the quote escaping section in Section 4.2. Add a clear description of the argument injection threat via unmatched single/double quotes (e.g. `main' --orphan 'evil_branch`).
   - Specify that the validation layer must check for unmatched single/double quotes, and either reject the input entirely or escape quotes using shell-appropriate escaping (e.g., replacing `'` with `'\''` on Unix).

3. **Section 5: Workspace-Level Schema Discovery & Security**:
   - Update Section 5.2 and 5.3 to explicitly cover the security vulnerability of built-in git resolvers in untrusted workspaces (RCE via `.git/config` hooks/aliases, and PATH hijacking).
   - Document a concrete **Git Resolver Isolation Strategy**:
     - **Environment Isolation**: Set `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_SYSTEM=/dev/null` on spawned processes to bypass system/global git configs.
     - **Hook Disabling**: Pass `-c core.hooksPath=/dev/null` to git.
     - **Protocol Restrictions**: Pass `-c protocol.file.allow=never` to restrict submodule/file protocol exploits.
     - **Absolute PATH Resolution**: Ensure we resolve `git` using a trusted absolute path (e.g., `/usr/bin/git` or `/usr/local/bin/git`) or search path that excludes the current workspace root to prevent PATH hijacking.
   - Refine the Rust code snippet for `resolve_data_sources_secure` to show:
     - Checking if `resolver` is git-related (e.g. starts with `"git:"`).
     - Gating git resolvers behind the workspace trust boundary so that if `!is_workspace_trusted`, we execute the git resolver using `resolve_git_resolver_isolated(resolver, cwd)` instead of the standard built-in resolver.
     - Provide a helper draft function `resolve_git_resolver_isolated(resolver: &str, cwd: &str)` in the Rust code block to illustrate how a secure `Command` is spawned with absolute path and git environment overrides (setting `GIT_CONFIG_NOSYSTEM`, etc.).

Please read the file at `/Volumes/goldcoders/zap/specs/tmp_ai_integration.md`, make these changes with high precision and clarity, and write the updated contents back to the file. Ensure the Markdown format is clean and professional.

MANDATORY INTEGRITY WARNING:
DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A Forensic Auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Once complete, write your handoff report to `/Volumes/goldcoders/zap/.agents/worker_tmp_spec_3/handoff.md` and notify me.
