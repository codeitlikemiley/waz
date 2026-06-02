# BYOP Placeholder Tool Results Local Issues

This file records `$to-issues` confirmed local implementation issues. They are not published to GitHub.
Source artifacts:

- `CONTEXT.md`
- `specs/byop-placeholder-tool-results/PRODUCT.md`
- `specs/byop-placeholder-tool-results/TECH.md`
- `specs/byop-placeholder-tool-results/ADR.md`

## BYOP-PR-1: BYOP Request Readiness core classifier
Type: AFK

## What to build

Create the core BYOP request-readiness classifier as a pure implementation slice. It should normalize the BYOP Chat-visible conversation projection into safe metadata, classify tool-call readiness without mutating controller or serializer state, and preserve the accepted terminology around Terminal Tool Result, Placeholder Tool Result, Repair Record, and Request Readiness.

This slice must not wire controller preflight or serializer dispatch behavior yet. Its output is a focused, independently testable module that later slices can call.

## Acceptance criteria

- [x] A `byop_readiness` module exposes normalized projection types that carry task ID, message ID, assistant tool-call message ID, tool call ID, redacted tool kind, result source, and projected message kind.
- [x] The projection and classifier do not carry raw user prompt text, tool arguments, tool output, raw carrier payloads, or raw local interception payloads.
- [x] The classifier covers `Ready`, `AcceptedHistoryRepair`, `PendingToolResults`, `NeedsCancellationCommit`, `DuplicateToolResults`, `OrphanToolResult`, `OutOfOrderToolResult`, `MissingResultWithoutRepairSource`, and non-blocking stale repair-record ignored behavior.
- [x] Repair matching requires task ID, assistant tool-call message ID, and tool call ID; tool-call ID alone is insufficient.
- [x] Unit tests cover complete, pending, cancelled, duplicate, orphan, out-of-order, corrupted, compacted, current-input, structured-error, local-interception, unreadable-local-interception, and accepted-repair groups.
- [x] This slice does not create or persist Repair Records and does not insert Placeholder Tool Results.

## Blocked by

None - can start immediately.

## BYOP-PR-2: Serializer blocks common process missing Tool Result
Type: AFK

## What to build

Apply BYOP request readiness at the serializer boundary so normal request flow cannot send Placeholder Tool Results for missing, duplicate, orphaned, or out-of-order tool results. Validation must run over the final outbound BYOP Chat projection after compaction/filtering and current request input have been accounted for.

This slice keeps explicit history repair unavailable until the repair sidecar slice adds durable Repair Records.

## Acceptance criteria

- [x] BYOP Chat request serialization runs readiness validation before provider `ChatMessage` construction or sanitizer repair.
- [x] Normal-flow missing tool results are blocked before placeholder insertion.
- [x] Duplicate, orphan, and out-of-order tool results are blocked rather than reordered or silently deduplicated.
- [x] Serializer validation treats visible unknown or readiness-irrelevant outbound messages as ordering boundaries, while filtered-out messages do not affect readiness.
- [x] Current `RequestParams.input` action results can satisfy readiness when they are valid visible Terminal Tool Results.
- [x] Request-body tests prove strict Chat Completions ordering: no orphan tool responses, no duplicate tool result IDs, and no user/assistant message before required tool responses.

## Blocked by

- BYOP-PR-1

## BYOP-PR-3: Repair Sidecar and Accepted History Repair
Type: AFK

## What to build

Add durable BYOP repair metadata and allow Placeholder Tool Results only for explicit accepted History Repair. Repair state should load, validate, and persist as an app-layer sidecar while persistence only stores opaque optional JSON. Accepted repair placeholders are outbound-only structured JSON and must not be written back as conversation history.

## Acceptance criteria

- [x] `RepairState`, `RepairRecord`, `RepairSource`, and `RepairStateStatus` support version `1`, missing sidecar as valid empty state, invalid JSON as invalid status, and unsupported/missing versions as unsupported status.
- [x] `AgentConversationData` roundtrips the optional repair sidecar, and legacy payloads without the field still deserialize.
- [x] `AIConversation` loads, preserves, and saves valid or invalid repair sidecar state without collapsing invalid state to empty.
- [x] `RequestParams` carries a read-only repair-state snapshot for BYOP serializer validation.
- [x] Accepted history repair remains distinct from ordinary `Ready` in validation, diagnostics, and tests.
- [x] The broad sanitizer is renamed or extracted so only accepted history repair can emit placeholders.
- [x] Repair placeholders are structured JSON containing only `status`, `reason`, and `note`, with `status = "unavailable"` and stable snake_case reason values.
- [x] Repair placeholders are not persisted as task messages or conversation history.

## Blocked by

- BYOP-PR-1
- BYOP-PR-2

## BYOP-PR-4: User Continuation cancel path submit real Cancellation Result
Type: AFK

## What to build

Add controller preflight behavior for the user-continuation case where a prior Model Tool Call must be cancelled before the next BYOP request is serialized. The controller should commit a real Cancellation Result while it owns mutable conversation/action state, persist it, rebuild request parameters, and rerun readiness.

## Acceptance criteria

- [x] Controller preflight detects `NeedsCancellationCommit` from readiness and commits a Cancellation Result keyed to the original tool call.
- [x] Cancellation-result commit is idempotent by tool call ID and avoids duplicate persisted or outbound tool results.
- [x] Cancellation-result persistence must succeed before BYOP dispatch continues.
- [x] If cancellation persistence fails, BYOP dispatch is blocked and does not proceed with in-memory-only state or current-input fallback.
- [x] After successful persistence, old `RequestParams` are discarded, `RequestParams` are rebuilt from updated conversation state, and readiness is rerun.
- [x] Corrupted or unexplained history from this path does not open a provider request and does not schedule retry or auto-resume.

## Blocked by

- BYOP-PR-1
- BYOP-PR-2

## BYOP-PR-5: Finished Result Drain and Auto-Resume Readiness
Type: AFK

## What to build

Extend controller preflight to drain finished action results and coordinate pending/live action state with BYOP readiness. Auto-resume and locally intercepted tool paths should only proceed once the relevant Terminal Tool Result is represented in the outbound projection as real, compacted, structured-error, or local-interception result.

## Acceptance criteria

- [x] Finished action results needed for readiness are persisted before BYOP dispatch.
- [x] Finished-result persistence failure blocks BYOP dispatch and cannot be bypassed by reintroducing the result through current `RequestParams.input`.
- [x] Pending tool results do not open a provider request and remain in the existing wait or auto-resume path.
- [x] Live action matching requires current conversation, task ID, and tool call or action ID so another task cannot satisfy the missing result.
- [x] Successful drain or cancellation persistence causes `RequestParams` rebuild and avoids projecting both current-input and persisted-history copies of the same result.
- [x] Committed local interception results, including structured invalid-arguments results, satisfy readiness; unreadable local interception payloads block without logging raw payloads.
- [x] The readiness rerun loop is bounded to the accepted initial maximum and blocks with non-sensitive diagnostics if no progress is made.

## Blocked by

- BYOP-PR-1
- BYOP-PR-2
- BYOP-PR-4

## BYOP-PR-6: Fork/Legacy Restore Create Repair Records
Type: AFK

## What to build

Create Repair Records only at explicit history transformation boundaries: fork creation and legacy restore/conversion. The records should authorize only retained Model Tool Calls whose matching Terminal Tool Results were intentionally unavailable due to that transformation.

## Acceptance criteria

- [x] Fork creation records `ForkedHistory` repair only for retained assistant tool calls whose matching results were intentionally omitted by the fork operation.
- [x] Fork creation does not convert retained orphan tool results into repair records.
- [ ] Legacy restore/conversion records `RestoredLegacyHistory` repair only for gaps proven by conversion-time input or documented legacy format behavior.
- [x] Restored conversations with unexplained missing tool results are treated as corrupted persisted history by default.
- [x] Repair records survive conversation serialization and remain available after fork routing tokens are cleared.
- [x] Repair records authorize only exact task ID, assistant tool-call message ID, and tool call ID matches.

Review note: no current legacy restore/conversion input in this checkout proves a missing result came from documented legacy behavior, so this slice intentionally does not create `RestoredLegacyHistory` records from unexplained restored gaps.

## Blocked by

- BYOP-PR-3

## BYOP-PR-7: Blocked Request Error and Readiness Logs
Type: AFK

## What to build

Add user-visible blocked-request behavior and non-sensitive readiness diagnostics. Corrupted or unexplained history should be reported as a local no-send condition, not as a provider rejection, and should not automatically retry or resume. Logs should distinguish accepted repair, pending live work, cancellation commit, and corrupted history without exposing sensitive content.

## Acceptance criteria

- [x] Corrupted or unexplained readiness failures map to `RenderableAIError::Other` with `will_attempt_resume = false`.
- [x] Blocked-request copy states that Waz did not send the BYOP request because a previous tool call is missing its recorded result.
- [x] The initial blocked-request UI does not expose automatic retry or ordinary retry behavior.
- [x] Manual continuation still passes through readiness and cannot bypass the same blocked state.
- [x] Diagnostics include readiness category, conversation ID, task ID, assistant tool-call message ID, tool call ID, redacted tool kind, trigger layer, and a non-persistent request-attempt ID where available.
- [x] Diagnostics omit tool arguments, tool output, raw user prompt text, raw carrier payloads, and raw local interception payloads.
- [x] Repeated blocked diagnostics are coalesced within one request attempt and summarize suppressed counts without sensitive content.
- [x] Accepted history repair logs one summarized `info` diagnostic per request with record count and repair-source counts.

## Blocked by

- BYOP-PR-2
- BYOP-PR-3
- BYOP-PR-4
- BYOP-PR-5

## BYOP-PR-8: Placeholder-Then-Cancellation Returns and Strict Ordering Harness
Type: AFK

## What to build

Add end-to-end regression coverage for the observed placeholder-then-cancellation shape using synthetic, minimal, redacted fixtures. The test harness should verify that normal BYOP requests never emit placeholder tool results for live or unexplained gaps, while accepted repair remains provider-compatible and outbound-only.

## Acceptance criteria

- [x] A synthetic regression fixture reproduces the structure where one request previously would have contained `(tool execution result not retained)` before a later request saw the real cancellation result.- [x] The same fixture now proves the immediate BYOP request is blocked, waits, or contains a real Cancellation Result, depending on controller state, but never sends a normal-flow Placeholder Tool Result.
- [x] Strict Chat Completions ordering helper validates generated request bodies for orphan tools, duplicate tool results, and pending tool calls before user/assistant messages.
- [x] Tests cover accepted repair placeholders, including structured JSON semantics and outbound-only behavior.
- [x] Tests cover compaction/filtering boundaries so hidden historical gaps do not block while visible assistant tool calls still require a result or repair record.
- [x] Log-focused assertions verify diagnostic categories and redaction for the observed shape.
- [x] Project-required validation is documented for the implementation branch, at minimum `cargo check`.

Validation:

- 2026-05-18: `cargo test -p warp serializer_readiness_tests --lib`
- 2026-05-18: `cargo test -p warp byop_readiness --lib`
- 2026-05-18: `cargo check -p warp --lib`
- 2026-05-18: `git diff --check -- app\src\ai\agent_providers\chat_stream.rs app\src\ai\byop_readiness\mod_test.rs specs\byop-placeholder-tool-results\ISSUES.md`
- 2026-05-19 follow-up review fixes: `cargo test -p warp serializer_readiness_tests --lib`
- 2026-05-19 follow-up review fixes: `cargo test -p warp byop_readiness --lib`
- 2026-05-19 follow-up review fixes: `cargo check -p warp --lib`
- 2026-05-19 follow-up review fixes: `git diff --check -- app\src\ai\agent_providers\chat_stream.rs app\src\ai\byop_readiness\mod.rs app\src\ai\byop_readiness\mod_test.rs app\src\ai\blocklist\controller.rs specs\byop-placeholder-tool-results\ISSUES.md`

## Blocked by

- BYOP-PR-2
- BYOP-PR-3
- BYOP-PR-4
- BYOP-PR-5
- BYOP-PR-6
- BYOP-PR-7
