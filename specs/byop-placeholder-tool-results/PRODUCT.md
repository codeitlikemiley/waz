# Prevent BYOP Placeholder Tool Results From Reaching Normal Agent Requests

Technical implementation details are tracked in `TECH.md`. The core repair-boundary decision is recorded in `ADR.md`.

## Problem Statement

Waz BYOP Agent can construct a follow-up request before a real asynchronous tool result has been persisted. To keep the OpenAI Chat tool-call sequence valid, the current request sanitizer may insert a placeholder tool result: `(tool execution result is not retained)`.
This avoids upstream protocol rejection, but it gives the model an artificial observation. From the user's perspective, the agent appears to continue normally while reasoning from incomplete or misleading tool output. This can cause repeated actions, weaker follow-up decisions, or confusion in long-running command workflows.

## Solution

Waz should only send real terminal tool states to BYOP providers during normal agent execution. Before a BYOP request is sent, every model-produced tool call that is still relevant to the conversation should have a real terminal result: success, cancellation, or structured error.

Placeholder tool results should remain available as a defensive repair mechanism for explicitly recorded forked or restored legacy history gaps. They should not be produced for ordinary in-flight tool execution, user continuation, auto-resume flow, corrupted persisted history, or compacted tool output.

## User Stories

1. As an Waz user, I want the agent to see real tool results, so that it can continue from accurate context.
2. As a BYOP user, I want Waz to preserve strict OpenAI Chat tool-call ordering, so that strict providers accept my requests.
3. As a DeepSeek BYOP user, I want `assistant(tool_calls) -> tool -> user` ordering to remain valid, so that my requests are not rejected for malformed history.
4. As an agent user, I want cancelled tool execution to be recorded as a real cancellation result, so that the model understands why no normal output was produced.
5. As a user who sends a new prompt while tools are running, I want Waz to drain or cancel prior tool actions before the next request is built, so that history stays coherent.
6. As a user working in a long-running command session, I want the model to receive the latest real PTY tool result, so that it does not act on stale or missing terminal state.
7. As a user using LRC tag-in, I want auto-accepted tools to finish their result persistence before auto-resume, so that the next model turn has complete observations.
8. As a user, I want the agent to avoid retrying work only because it saw a placeholder result, so that it does not repeat commands unnecessarily.
9. As a user, I want placeholders to be rare and diagnosable, so that I can trust normal agent runs are based on real results.
10. As a developer, I want placeholder insertion to be classified by cause, so that I can distinguish expected history repair from a normal-flow bug.
11. As a developer, I want BYOP request readiness to be checked before request serialization, so that protocol balancing is not the only guardrail.
12. As a developer, I want the request sanitizer to remain a final safety net, so that explicitly repairable history still cannot produce invalid provider payloads.
13. As a maintainer, I want a small testable module for tool-call readiness, so that the behavior can be validated without constructing full UI state.
14. As a maintainer, I want deterministic request-body tests, so that future changes do not reintroduce orphan tools, late tool results, or placeholder use in normal paths.
15. As a support engineer, I want logs to identify when a placeholder was inserted and why, so that log-based diagnosis is fast.
16. As a support engineer, I want normal in-flight tool waits to be logged differently from damaged historical state, so that user reports are easier to triage.
17. As a BYOP provider integrator, I want Waz to emit provider-compatible tool result bundles, so that adapters do not need to compensate for malformed conversation state.
18. As a tester, I want regression coverage using the observed placeholder scenario, so that the specific failure shape cannot silently return.
19. As a user of local tool interception, I want intercepted tool results to auto-resume only after they are represented as real tool results, so that the model sees the intended feedback.
20. As an Waz user, I want long conversations to continue without fabricated observations, so that agent quality does not degrade unexpectedly.

## Implementation Decisions

- Build or extract a request-readiness module for BYOP tool-call groups. It should expose a small interface that reports whether a request can be serialized, should wait, should persist cancellation results, or should fall back to history repair.
- The initial readiness result classification should distinguish at least: ready, pending tool results, needs cancellation-result commit, accepted history repair, duplicate tool results, orphan tool result, out-of-order tool result, missing result without repair source, and stale repair record ignored.
- `AcceptedHistoryRepair` should be represented as a distinct sendable readiness state rather than folded into ordinary `Ready`.
- Serializer validation may proceed for `AcceptedHistoryRepair`, but it should carry the accepted repair records forward so outbound placeholder generation, logs, and tests can identify that the request depended on history repair.
- The readiness module should remain a pure classifier. It should return `NeedsCancellationCommit` with enough tool-call metadata for the controller to act, rather than mutating action state or conversation history directly.
- `NeedsCancellationCommit` metadata should include at least conversation ID, task ID, tool call ID or action result ID, assistant tool-call message ID where available, tool kind, and cancellation reason.
- `NeedsCancellationCommit` metadata should not include tool arguments or tool output.
- Core tool-call pairing, ordering classification, and blocking decisions should live in production readiness or serializer-validation code, not only in test helpers.
- Controller preflight should perform synchronous cancellation-result commits while it safely owns mutable action and conversation state, then rerun readiness before BYOP serialization.
- Synchronous cancellation-result commit must succeed in the conversation/request state and persist successfully before serializer validation proceeds.
- If cancellation-result persistence fails, controller preflight should block the BYOP request and surface an error rather than continuing with only in-memory cancellation state.
- The initial implementation should not send a request that depends on a cancellation result that was committed only in memory but failed to persist, because a retry or restart could recreate the missing-result gap.
- Finished action results drained by controller preflight must also persist successfully before BYOP serialization when the current request depends on them to satisfy readiness.
- If finished-result persistence fails, controller preflight should block the BYOP request rather than sending a payload that is valid only in memory.
- After controller preflight successfully persists drained finished results or cancellation results, it should discard the old `RequestParams`, rebuild `RequestParams` from the updated conversation state, and rerun readiness.
- Controller preflight should not patch the old `RequestParams.input` in place after persistence, because that can leave both current-input results and newly persisted history results in the same outbound projection.
- Controller preflight readiness reruns should be bounded and require observable progress on each iteration, such as successfully persisting a drained result, committing a cancellation result, or completing known duplicate deduplication.
- The initial controller preflight readiness loop should use `max_iterations = 3`.
- If controller preflight receives the same actionable readiness state again without progress, it should block BYOP dispatch and log an internal readiness-loop diagnostic rather than looping indefinitely or opening a provider request.
- Readiness-loop diagnostics should include iteration count and final readiness category, but must not include raw message content, tool arguments, tool output, or user prompt text.
- Readiness-loop non-convergence should use a distinct internal diagnostic category such as `ReadinessLoopDidNotConverge`.
- User-visible errors for readiness-loop non-convergence should use the same generalized blocked-request copy as other corrupted or unexplained history states, rather than exposing internal loop state.
- If tool-result persistence succeeds but later BYOP request serialization or dispatch fails, the persisted tool result should not be rolled back.
- A persisted tool result is a real conversation fact. Keeping it allows the next request attempt to continue from consistent history, while rolling it back could recreate a missing-result gap.
- Place the shared readiness implementation in a new `crate::ai::byop_readiness` module rather than inside `agent_providers::chat_stream`.
- The readiness module should be usable by both the blocklist controller preflight and BYOP serializer validation without depending on provider execution internals.
- The readiness module should classify a normalized internal projection rather than directly inspecting provider `ChatMessage` values or raw `api::Message` shapes.
- The normalized projection should preserve readiness metadata such as task ID, message ID, assistant tool-call message ID, tool call ID, redacted tool kind, source location, and whether a result came from persisted history or current `RequestParams.input`.
- The initial normalized projection should cover only the BYOP Chat readiness message kinds needed for tool-call validation, such as user boundary messages, assistant tool-call messages, tool result messages, and other assistant/system boundary messages.
- Unknown or readiness-irrelevant message kinds should preserve ordering boundaries where needed but should not force a full abstraction of every raw `api::Message` variant in the initial implementation.
- Unknown or readiness-irrelevant message kinds should be treated according to final outbound visibility. If they become visible non-tool-response Chat messages in the outbound payload, they are ordering boundaries and should block any pending tool-call group before them.
- If an unknown or readiness-irrelevant raw message kind is fully filtered out before the final outbound projection, it should not affect tool-call group readiness.
- In the normalized projection, `message_id` should identify the current projected item's source message.
- For an assistant tool-call item, `message_id` and `assistant_tool_call_message_id` are the same ID.
- For a tool-result item, `message_id` is the `ToolCallResult` message's own ID, while `assistant_tool_call_message_id` points back to the assistant message that produced the matching tool call when that mapping is known.
- Projection construction may fill a missing tool-result `assistant_tool_call_message_id` by matching the `tool_call_id` to a unique pending assistant tool call in the same task when the order is valid.
- Valid ordering means the tool result appears after the assistant tool-call item in the same task and before any later user message or assistant message.
- All visible tool calls in an assistant tool-call group must be satisfied before the next user or assistant message appears in the outbound projection.
- A tool result that appears before its assistant tool-call item, crosses a later user message, crosses a later assistant message, or attaches to a different tool-call group should be classified as out-of-order or orphaned rather than repaired.
- If the assistant back-reference is ambiguous, crosses an invalid ordering boundary, or cannot be uniquely inferred, projection construction should leave it unknown and readiness should classify the result as orphan, out-of-order, or corrupted as appropriate.
- Repair record authorization must not rely on an ambiguous or non-unique inferred assistant back-reference.
- The normalized projection should distinguish real results from persisted task history and real results from current `RequestParams.input`. Both satisfy readiness, but the distinction is required for diagnostics and duplicate-projection detection.
- The normalized projection should not carry full message content, tool output, tool arguments, or user prompt text.
- If diagnostics need content-related context, the projection may carry safe derived fields such as byte length, token estimate, or non-reversible hash, but not raw content.
- Provider `ChatMessage` construction should remain the final serialization step after readiness and accepted-history repair decisions have already been made.
- Apply request readiness in two layers: a controller-level preflight after `RequestParams` construction and before BYOP dispatch, plus a serializer-level validation before sanitizer repair inside request serialization.
- Controller-level preflight should check normal-flow activity state and the currently relevant task history before BYOP dispatch. It owns pending action, cancellation, and drain decisions.
- Serializer-level validation should check the final outbound BYOP message projection after compaction and filtering have been applied.
- Serializer-level normalized projection should be constructed after compaction/filtering, or should be constructed from inputs that already encode the final post-compaction/filtering outbound visibility.
- Controller preflight may use an un-compacted current task/activity view for pending, drainable, and cancellation decisions, but that view must not replace serializer validation over the final outbound projection.
- Serializer-level validation should evaluate the merged outbound projection that includes both persisted task messages and current `RequestParams.input` action results.
- A real terminal result already present in current request input should satisfy readiness for its matching tool call even if that result has not yet appeared in persisted task history.
- Current `RequestParams.input` action results remain supported for existing valid request-construction paths and transition cases, but they must not become a fallback for results that controller preflight attempted and failed to persist.
- Controller preflight should persist drainable finished results before BYOP serialization whenever it can do so. If that persistence attempt fails, the failed result should not be reintroduced through current `RequestParams.input` to bypass the persistence requirement.
- Current `RequestParams.input` action results that do not yet have persisted message IDs should receive stable diagnostic-only synthetic projection IDs, such as `current_input:{index}:{tool_call_id}`.
- Current-input synthetic projection IDs only need to be stable within a single request construction and serializer-validation pass.
- Current-input synthetic projection IDs should not be treated as stable across requests or app restarts.
- Synthetic projection IDs for current input results should be marked with source `CurrentInput`, should not be written back to conversation history, and should not participate in durable repair-record matching.
- After a current input action result is persisted as a real `ToolCallResult` message, the next projection should use that persisted message ID with source `PersistedHistory`, not the previous synthetic projection ID.
- Request construction should avoid carrying both the old current input result and the newly persisted history result for the same tool call in the same outbound projection.
- Repair-record matching must continue to use task ID, assistant tool-call message ID, and tool call ID, not current-input synthetic projection IDs.
- If the same tool call result appears both in persisted task history and current `RequestParams.input`, request construction should avoid projecting both copies.
- Serializer validation should treat duplicate tool responses in the same outbound projection as duplicate tool results and block, even if one copy came from history and the other from current input.
- Serializer validation should not guess which duplicate result is newer or more authoritative.
- Controller preflight should still prefer to drain and commit finished action results where possible, but serializer validation must not misclassify current input action results as missing.
- Historical tool-call gaps that are fully covered by compaction or filtering and will not appear in the outbound provider payload should not block the current request.
- Compaction or filtering that hides an assistant tool-call message must also hide its corresponding tool results from the outbound projection.
- If compaction or filtering leaves a tool result visible after hiding its assistant tool-call message, serializer validation should block as orphan or corrupted history.
- Compaction or filtering that keeps an assistant tool-call message visible must also keep a real or compacted tool result visible for each visible tool call, unless the missing result is backed by an existing forked-history or restored-legacy repair record that was created before compaction/filtering.
- A pre-existing repair record should participate in serializer validation only when its assistant tool-call message remains visible in the current outbound projection and its matching result is missing.
- If compaction or filtering hides the assistant tool-call message referenced by a repair record, the record should remain unused for the current request and should not be treated as stale or consumed.
- Hidden-call repair records may produce debug diagnostics, but should not trigger cleanup or affect request readiness for the current projection.
- Compaction or filtering should not create new repair-record dependence by hiding a tool result while keeping its assistant tool-call message visible.
- If compaction or filtering hides a tool result and leaves its assistant tool-call message visible without a pre-existing repair record, this is a projection bug and should block rather than fall back to placeholder repair.
- Any assistant tool call that remains in the outbound provider payload must have a real terminal result or an exactly matching repair record before the payload is sent.
- For an assistant message with multiple tool calls, readiness should evaluate each tool call independently. Existing real terminal results should remain real results, while missing tool calls may use placeholder repair only when each missing call has an exactly matching repair record.
- If any visible tool call in a multi-call group lacks both a real terminal result and an exactly matching repair record, the entire outbound BYOP request must be blocked.
- For a sendable multi-tool-call group, serializer output should emit tool responses in the same order as the assistant message's `tool_calls`: use the real terminal result when present, and insert the structured repair placeholder at the corresponding position only for accepted repair records.
- This ordered outbound bundle rule does not authorize arbitrary persisted-history reordering. If persisted real tool results are orphaned, duplicated, or cross a later user or assistant boundary, serializer validation should block instead of reshuffling the history.
- If a visible tool call has multiple persisted tool results, readiness should classify it as corrupted or unexplained history and block request serialization rather than silently selecting the first or last result.
- The controller may deduplicate a known late-cancellation duplicate only while it safely owns mutable conversation state. Serializer validation should receive an already-deduplicated projection or block the request.
- Serializer validation must not reorder persisted `ToolCallResult` messages to repair history. It may only authorize outbound placeholder insertion for accepted repair records.
- If a real `ToolCallResult` appears before its assistant tool call or after a later user message in the outbound projection, serializer validation should block the request as corrupted or unexplained history.
- Controller code may repair known normal-flow ordering defects only while it safely owns mutable conversation state, before serializer validation receives the projected messages.
- The initial controller auto-repair scope should be limited to deterministic normal-flow fixes: draining and committing finished action results, and idempotently deduplicating known late-cancellation duplicates for the same tool call.
- The initial controller auto-repair scope should not reorder arbitrary persisted history, delete unknown duplicate tool results, or move late tool results across a later user turn. Those cases should block as corrupted or unexplained history.
- The controller-level preflight should own normal-flow state handling, including synchronous cancellation-result persistence, waiting for auto-resume prerequisites, or returning a categorized not-ready result without opening a provider request.
- Controller-level preflight must not block the UI thread while waiting for pending tool results, and must not open a BYOP provider request while tool results are still unresolved.
- For normal-flow pending tool results, controller preflight should first drain and persist any finished action results. If relevant actions are still running, it should leave the conversation in its existing waiting or auto-resume path and retry request construction only after those action results are committed.
- Controller preflight may classify a missing result as pending only when the live action model shows a matching running action, or as drainable when a matching finished action result exists but has not yet been committed.
- Controller preflight should match running actions and finished action results within the current conversation, then require task ID plus tool call ID or action ID to match the missing result.
- Controller preflight should not globally match only by action ID or tool call ID, because forked, restored, or multi-task conversations can otherwise drain the wrong action result into the current gap.
- Assistant tool-call message ID should be included in diagnostics for live-action matching decisions, and may be used as an additional guard when that mapping is available.
- If controller preflight finds a missing tool result but there is no running action, no finished result to drain, and no repair record authorizing history repair, it should classify the gap as corrupted or unexplained persisted history rather than continue waiting indefinitely.
- Serializer validation does not have live action-model state, so it should not classify projected gaps as pending. It should treat the final outbound projection as ready, accepted history repair, or corrupted/unexplained history.
- The serializer-level validation should not mutate controller state. It should classify projected message gaps as either accepted history repair or a normal-flow defect before sanitizer placeholder insertion.
- Readiness not-ready results should be handled by category. Pending tool results and missing auto-resume prerequisites should wait without showing a user-visible error. Cancellation-related gaps should first attempt synchronous cancellation-result persistence and only surface an error if persistence fails. Corrupted or unexplained missing results should block the request and surface a user-visible error.
- User-visible readiness errors should reuse `RenderableAIError::Other` with `will_attempt_resume=false` and `waiting_for_network=false`. Detailed readiness categories should remain in the readiness result type, logs, and tests rather than becoming UI error variants.
- UI error mapping may collapse corrupted or unexplained readiness categories into the same `RenderableAIError::Other`, but logs, diagnostics, and tests should preserve the precise readiness category.
- Corrupted or unexplained missing-result messages should state that no BYOP request was sent because a previous tool call is missing its recorded result.
- User-visible blocked-request copy should clearly distinguish local Waz blocking from provider/model errors.
- Blocked-request copy should include both facts: Waz did not send the BYOP request, and a previous tool call is missing a recorded result.
- Corrupted or unexplained history should be treated as a terminal blocked-request state for the current run. It should not trigger automatic resume or retry, because retrying would rebuild the same invalid history and can loop.
- The initial blocked-request UI should not offer an automatic retry or ordinary retry action, because retrying the same damaged history will not make it valid.
- Users may still manually continue the conversation, but the next request must go through the same readiness gate and should remain blocked until the underlying history state is resolved.
- If a user manually continues while the same blocked history state remains unresolved, Waz may show the blocked-request error again so the user receives clear feedback for that attempt.
- Repeated blocked-request diagnostics for the same conversation, task, tool call, and readiness category should be rate-limited or coalesced in logs to avoid high-severity log spam.
- Duplicate blocked-request diagnostics should be keyed by conversation ID, task ID, assistant tool-call message ID, tool call ID, readiness category, and trigger layer.
- The diagnostic coalescing key should not include raw message content, tool arguments, tool output, user prompt text, or raw payloads.
- Diagnostic coalescing should record the first occurrence with full non-sensitive metadata, increment a suppressed count for repeated matching diagnostics, and emit a summary containing `suppressed_count` when the coalescing window ends or the readiness category changes.
- The initial diagnostic coalescing window should be a single request attempt, not a time-based window.
- A later manual continuation is a new request attempt and may produce a new first full non-sensitive diagnostic for the same blocked state.
- Readiness diagnostics should include a non-persistent request-attempt identifier, such as a generated `readiness_attempt_id` or an existing response/request stream ID, to correlate controller preflight and serializer validation logs for one attempt.
- The request-attempt identifier is diagnostic-only. It must not participate in repair matching, readiness identity, or conversation persistence.
- The request-attempt identifier should be logged with readiness diagnostics but should not be part of the duplicate-diagnostic coalescing key, because the coalescing window is already scoped to a single request attempt.
- `AcceptedHistoryRepair` diagnostics should log at `info`, because it is an expected but rare repair path.
- `AcceptedHistoryRepair` should not use blocked-request coalescing because it is sendable rather than blocked.
- If one request uses multiple accepted repair records, log one summarized `info` diagnostic with repair record count and non-sensitive identifiers instead of one log per record.
- The summarized `AcceptedHistoryRepair` diagnostic should include repair-source counts, such as counts for forked-history repair and restored-legacy-history repair.
- `AcceptedHistoryRepair` diagnostics should not include placeholder payload content, tool arguments, tool output, user prompt text, or raw payloads.
- `PendingToolResults` diagnostics should log at `debug` or avoid high-frequency logging, because waiting for live tools is normal control flow.
- `NeedsCancellationCommit` diagnostics should initially log at `debug`, because it is a normal controller preflight action when the controller can commit the cancellation result and rerun readiness.
- Successful cancellation-result commit followed by a ready rerun should not escalate log level.
- Cancellation-result persistence failure, repeated `NeedsCancellationCommit` without progress, or readiness-loop non-convergence should log at `error`.
- `StaleRepairRecordIgnored` diagnostics should log at `debug` when the stale or unused record does not itself block the current request.
- If a stale or irrelevant repair record leaves a visible missing result unauthorized, the blocking readiness category such as `MissingResultWithoutRepairSource` should produce the `error` diagnostic.
- Corrupted or unexplained categories such as `DuplicateToolResults`, `OrphanToolResult`, `OutOfOrderToolResult`, `MissingResultWithoutRepairSource`, invalid repair sidecar, and `ReadinessLoopDidNotConverge` should log at `error`.
- Coalescing summaries should preserve the same non-sensitive key fields and should not include raw content, tool arguments, tool output, user prompt text, or raw payloads.
- A future explicit repair-and-continue action should be a separate user/developer action with its own repair source, confirmation, and diagnostics.
- Pending tool results may enter the waiting or auto-resume path; corrupted or unexplained history must stop and require explicit user or developer intervention.
- Keep the existing sanitizer as a final provider-protocol repair step. It should still handle explicitly recorded forked-history and restored-legacy-history repair sources.
- Rename or extract the existing broad sanitizer entry point so its name reflects accepted history repair, for example `repair_tool_call_pairs_for_accepted_history_gaps`, rather than implying that normal missing results can be sanitized.
- Calls into the repair sanitizer should occur only after readiness and repair-source validation have classified the gap as accepted history repair.
- Placeholder-producing history repair must require an explicit repair source. A missing tool result by itself should not be treated as sufficient evidence for placeholder insertion.
- Accepted repair sources should be represented as enumerable categories such as forked history and restored legacy history.
- The initial repair-source enum should allow only forked history and restored legacy history.
- The initial implementation should not include broad repair-source values such as corrupted history, unknown, or manual repair.
- A future explicit "repair and continue" action should introduce a separate precise source, such as user-approved corrupted-history repair, with its own UI and logging confirmation.
- Repair records should be created only at explicit history transformation points, such as fork creation and legacy restore/conversion.
- Fork creation code should scan the retained forked task/message set after the fork is built, identify tool calls whose matching results were intentionally omitted by the fork operation, and persist `ForkedHistory` repair records for those exact calls.
- Fork creation code should avoid producing orphan tool results where a `ToolCallResult` is retained but the corresponding assistant tool call was removed.
- If retained forked history contains an orphan tool result, it should be treated as a fork projection bug and classified as corrupted or unexplained history. It should not create a repair record or placeholder tool result.
- Repair records should only cover the case where an assistant tool call is retained and its matching tool result is intentionally missing.
- Legacy restore/conversion code should create `RestoredLegacyHistory` repair records only while converting known legacy restored history gaps.
- A known legacy gap requires evidence available during conversion, such as legacy input that retains an assistant tool call while the old format has no recoverable matching tool-result field, or a documented old-version behavior that dropped that result.
- Legacy restore/conversion should not infer a `RestoredLegacyHistory` repair record solely from a missing result observed after conversion.
- If conversion cannot prove the missing result comes from known legacy format behavior, the gap should remain corrupted or unexplained history and block request serialization by default.
- Readiness, serializer validation, and sanitizer code must not lazily create repair records when they discover a missing result. They may only consume existing records or block the request.
- Forked-history repair sources should be persisted with the forked conversation in `conversation_data` JSON, rather than inferred from `forked_from_server_conversation_token`.
- Forked-history repair metadata should be stored as per-missing-tool-call records, not as a conversation-level flag.
- Each forked-history repair record should identify at least the source category, task ID, assistant tool-call message ID, and tool call ID. It may also include fork point or exchange ID metadata for diagnostics.
- A forked-history repair record should authorize placeholder repair only for the recorded missing tool call, not for unrelated future gaps in the same conversation.
- Repair record authorization should require an exact match on task ID, assistant tool-call message ID, and tool call ID. A record that matches only by tool call ID should be treated as stale or irrelevant and should not authorize placeholder insertion.
- Restored-legacy-history repair metadata should also be stored as per-missing-tool-call records, not as a restored-conversation-level flag.
- Restore/conversion code should create restored-legacy repair records only for specific retained tool calls that are known to come from legacy restored history and lack matching tool results.
- If restore/conversion cannot establish a specific restored-legacy repair source for a missing result, the gap should be treated as corrupted persisted history and block serialization by default.
- Persisting forked-history repair sources should not require a SQL migration; it should extend the existing serialized conversation metadata shape, similar to the compaction sidecar.
- Placeholder tool results generated from repair records should be outbound-payload artifacts only. They should not be persisted back into conversation history.
- Placeholder tool results generated for accepted history repair should use a minimal structured JSON payload instead of the legacy bare text `(tool execution results are not retained)`.- The structured repair payload should include `status: "unavailable"`, a `reason` derived from the accepted repair source, and a short generic note such as `tool result was unavailable in repaired conversation history`.
- Structured repair payload `status` should always be the fixed string `"unavailable"`.
- Repair-source-specific information should be represented only in the `reason` field, not by varying `status`.
- Structured repair payload `reason` values should use provider-payload-friendly snake_case strings: `forked_history_repair` for `RepairSource::ForkedHistory` and `restored_legacy_history_repair` for `RepairSource::RestoredLegacyHistory`.
- Internal repair-source enums may use Rust-style names such as `ForkedHistory` and `RestoredLegacyHistory`; outbound JSON reasons should remain stable snake_case strings.
- Structured repair payload `note` should be fixed English machine-facing text and should not be localized.
- Placeholder payload content should not vary by UI locale, so provider/model behavior and tests remain stable.
- Structured repair payloads should contain only `status`, `reason`, and `note`.
- Structured repair payloads should not include the raw tool name. Diagnostics may use a separate redacted tool kind.
- Tests for structured repair payloads should parse JSON and compare the field set and values rather than depending on object field order.
- The structured repair payload shape is an implementation and serialization constraint; it does not need to be promoted into `CONTEXT.md` as domain language.
- Structured repair payloads must not include tool arguments, tool output, user prompt text, or other sensitive content.
- Repair records, not persisted placeholder messages, should provide the durable explanation for why a missing tool result was repaired.
- Repair records should not be consumed after a single outbound request. A record remains valid while its referenced tool call still exists and still lacks a real terminal tool result.
- If a real terminal tool result later appears for a recorded tool call, readiness should ignore or remove the repair record. If the referenced tool call no longer exists, readiness should log a diagnostic and ignore or clean up the stale record.
- Readiness and serializer validation should treat stale repair records as read-only diagnostics: ignore the record for the current authorization decision and log that it is stale or irrelevant.
- Serializer validation must not mutate repair metadata in order to clean stale records.
- Controller code may perform best-effort stale repair-record cleanup when it safely owns mutable `AIConversation` state, and the cleanup should persist with the next normal conversation save rather than becoming a request-sending prerequisite.
- Corrupted persisted history should block request serialization by default. It should log at high severity with conversation, task, and tool call identifiers rather than automatically producing a placeholder.
- A future explicit "repair and continue" user/developer action may opt into corrupted-history placeholder repair, but the initial implementation should not silently continue.
- Compacted tool output should be represented as a compacted tool result, not as placeholder-based history repair. A compacted tool result should satisfy request readiness because it projects an existing real tool result rather than replacing a missing result.
- In normal request flow, readiness failures caused by missing tool results should block BYOP request serialization instead of falling through to sanitizer placeholder insertion.
- Treat placeholder tool results in normal request flow as a defect signal. If a placeholder is inserted, logs should include a reason category and enough metadata to identify the relevant tool call without dumping sensitive output.
- Readiness and serializer diagnostics should include the readiness category, conversation ID, task ID, assistant tool-call message ID, tool call ID, redacted tool name or tool kind, and trigger layer such as controller preflight, serializer validation, or sanitizer repair.
- Readiness and serializer diagnostics should not include tool arguments, tool output, or user prompt text.
- Local interception results should use stable redacted diagnostic tool kinds such as `local_interception:todowrite`, `local_interception:webfetch`, `local_interception:websearch`, and `local_interception:invalid_arguments`.
- Diagnostics for local interception results should not log model-provided raw tool names, tool arguments, or carrier payloads.
- Before sending a user-initiated continuation, drain finished action results and persist them before appending the new user query to the request history.
- If a user continuation cancels an in-flight tool, synchronously commit an explicit cancellation result before request serialization instead of relying on a later executor event or a placeholder.
- Cancellation result persistence must be idempotent by tool call ID, so a late executor cancellation event cannot create duplicate tool results.
- For action-backed tool results, the idempotency key should be the serialized `ToolCallResult.tool_call_id`, which is currently derived from `AIAgentActionResult.id`. Persistence should keep at most one final terminal result for a given conversation, task, and tool call ID.
- Auto-resume should not build a provider request until locally intercepted tool results and asynchronous tool results for the prior turn have been committed to conversation history.
- Locally intercepted BYOP tool results, including `todowrite`, `webfetch`, `websearch`, and `invalid_arguments` carrier results, should count as real terminal tool results once their structured `server_message_data` has been committed.
- `invalid_arguments` carrier results should satisfy request readiness as real structured error results. They represent a final client-side observation for the tool call, not a missing result or placeholder repair.
- If a local interception result's structured `server_message_data` cannot be deserialized or interpreted, readiness should not count it as a real terminal tool result.
- Unreadable local interception payloads should block as corrupted or unexplained history, not fall back to placeholder repair.
- Diagnostics for unreadable local interception payloads should include the stable redacted tool kind, tool call ID, assistant tool-call message ID where available, message ID, and error category, but must not log the raw payload.
- If auto-resume or another normal request path finds an unresolved tool call, it should wait or return a categorized not-ready result rather than emit a provider request.
- Preserve strict provider sequencing in the final payload: assistant tool calls must be followed by corresponding tool responses before any later user or assistant message.
- Do not remove placeholder support entirely. It is still required for defensive repair when explicitly recorded forked-history or restored-legacy-history gaps lack tool results.
- No SQL migration or new persistence table/column is required for the initial implementation.
- The implementation should extend the existing `AgentConversationData` / `conversation_data` JSON metadata with BYOP repair records.
- New repair metadata fields should use serde-compatible defaults, such as `#[serde(default, skip_serializing_if = "Option::is_none")]`, so legacy conversation rows deserialize as having no repair records.
- Define BYOP repair metadata types in `crate::ai::byop_readiness::state`, not in `byop_compaction` and not as domain-owned types inside `crates/persistence`.
- Store repair metadata in `AgentConversationData` as an optional serialized sidecar field such as `byop_repair_state_json: Option<String>`.
- `RepairState` should include an explicit version field, initially version `1`, following the existing compaction sidecar pattern.
- Missing `byop_repair_state_json` in legacy conversation data should deserialize as an empty repair state.
- Invalid or unreadable `byop_repair_state_json` should not be treated as authorization for repair. It should be logged and any affected missing-result gaps should block as corrupted or unexplained history.
- Invalid or unreadable repair sidecar data should block only when the current outbound projection needs repair-record authorization for a visible missing result.
- If all visible tool calls in the current outbound projection have real terminal results, invalid repair sidecar data should not block the request, but should still produce a high-severity diagnostic.
- `AIConversation` should preserve repair sidecar load status explicitly, such as valid empty state, valid state with records, or invalid/unreadable state with an error category.
- Invalid repair sidecar data should not be collapsed into an empty repair state, because serializer validation must distinguish "no repair records" from "repair metadata could not be read."
- The initial repair sidecar load categories should distinguish missing sidecar, invalid JSON, and unsupported version.
- Missing sidecar is a valid empty repair state, not an error. Invalid JSON and unsupported version should not authorize repair.
- The initial reader should support only repair state version `1`.
- A present repair sidecar with no version field should be treated as unsupported version rather than best-effort parsed.
- A present repair sidecar with version greater than `1` should be treated as unsupported version until an explicit migration or reader is implemented.
- Unknown repair state versions must not authorize placeholder repair.
- Repair sidecar load diagnostics should record the load category without logging raw sidecar JSON.
- `AIConversation` should own the deserialized BYOP repair sidecar, following the existing `compaction_state_json` / `compaction_state` pattern.
- `RequestParams` should carry a read-only BYOP repair state snapshot to readiness and serializer validation so request construction does not need to deserialize persistence metadata.
- The current provider scope is the BYOP/OpenAI-compatible Chat serialization path. The implementation should not add branching, compatibility shims, or tests for a nonexistent non-BYOP provider path.
- Do not add a feature flag for request readiness or normal-flow placeholder blocking. This is a BYOP Chat payload correctness and history-consistency fix, not a product rollout switch.
- Debug logging may be added for observability, but there should not be a runtime switch that allows normal request flow to keep sending placeholder tool results.
- The initial implementation should use logs for readiness diagnostics and should not add a new telemetry event.
- Readiness telemetry may be considered later if maintainers need aggregate blocked-rate metrics, but it should be designed separately with its own schema and privacy review.
- Prefer a deep module for readiness classification over adding more special cases inside the request serializer.
- Continue to support compacted tool results intentionally produced by local compaction; those are distinct from placeholder repair for missing tool results.

## Testing Decisions

- Tests should assert externally visible request-message sequences, not internal helper implementation details.
- Add unit tests for the request-readiness module covering complete tool groups, pending tool groups, cancelled tool groups, corrupted history, and compacted tool results.
- Add unit tests for each initial readiness category so category distinctions remain stable even when user-visible error mapping is shared.
- Add unit tests for normalized projection construction, including metadata needed by readiness and diagnostics.
- Add unit tests that the initial normalized projection covers the BYOP Chat readiness message kinds without requiring exhaustive abstraction of every raw `api::Message` variant.
- Add unit tests that visible unknown or readiness-irrelevant message kinds act as ordering boundaries for pending tool-call groups, while fully filtered message kinds do not affect readiness.
- Add unit tests that normalized projection construction omits raw message content, tool output, tool arguments, and user prompt text while retaining safe metadata needed for classification.
- Add unit tests that projection preserves result source, including persisted history versus current `RequestParams.input`, and uses that source to diagnose duplicate projections.
- Add unit tests for normalized projection ID semantics: assistant tool-call items use the assistant message ID for both `message_id` and `assistant_tool_call_message_id`, while tool-result items keep their own `message_id` and reference the assistant message separately when known.
- Add unit tests for strict assistant back-reference inference on tool-result items: unique same-task ordered matches are allowed, ambiguous or invalid-order matches are not used for repair authorization.
- Add unit tests for ordering boundaries: tool results must follow their assistant tool-call group and precede any later user or assistant message in the same task.
- Add tests showing that `AcceptedHistoryRepair` is sendable but remains distinct from ordinary `Ready` in diagnostics and outbound placeholder generation.
- Add tests showing that readiness returns `NeedsCancellationCommit` without mutating state, and controller preflight commits the cancellation result before rerunning readiness.
- Add controller tests showing that cancellation-result persistence failure blocks BYOP dispatch and does not continue with in-memory-only cancellation state.
- Add controller tests showing that finished-result drain persistence failure blocks BYOP dispatch when those results are required for readiness.
- Add controller/request-construction coverage that successful tool-result persistence causes `RequestParams` to be rebuilt from updated conversation state before readiness is rerun.
- Add coverage that controller preflight does not patch old `RequestParams.input` in place and therefore avoids projecting both current-input and newly persisted history results.
- Add controller-level coverage that readiness reruns are bounded, require progress per iteration, and block with diagnostics when the same actionable state repeats without progress.
- Add controller-level coverage that the initial readiness loop stops at `max_iterations = 3` and logs only non-sensitive loop metadata.
- Add controller-level coverage that readiness-loop non-convergence maps to a generalized user-visible blocked-request error while diagnostics preserve `ReadinessLoopDidNotConverge`.
- Add controller/request-construction coverage that persisted tool results are not rolled back when later BYOP serialization or dispatch fails.
- Keep readiness unit tests close to `crate::ai::byop_readiness` so the behavior can be validated without constructing full UI or provider state.
- Add controller-level coverage for preflight behavior before BYOP dispatch, including synchronous cancellation result persistence and auto-resume not-ready handling.
- Add controller-level coverage that pending tool results do not open a BYOP provider request, do not block the UI thread, and resume request construction only after relevant results are committed.
- Add controller-level coverage that missing results are classified as pending or drainable only when the live action model has a matching running action or finished action result.
- Add controller-level coverage that running and finished action matching requires the current conversation plus task ID and tool call or action ID, so another task's result cannot satisfy the current missing result.
- Add controller-level coverage that a missing tool result with no running action, no drainable finished result, and no repair record is classified as corrupted or unexplained history instead of waiting indefinitely.
- Add serializer-level coverage that projected missing results are never classified as pending because serializer validation has no live action-model state.
- Add coverage that pending-result not-ready states wait without rendering an error, while corrupted or unexplained missing-result states produce a user-visible blocked-request error.
- Add coverage that corrupted or unexplained readiness errors map to `RenderableAIError::Other` without introducing a dedicated UI error variant.
- Add UI/error-mapping coverage that blocked-request copy says the BYOP request was not sent and identifies a missing recorded tool result, without implying the provider rejected the request.
- Add coverage that corrupted or unexplained history uses `will_attempt_resume=false`, does not schedule retry or auto-resume, and does not open a BYOP provider request.
- Add UI/error-mapping coverage that blocked-request errors do not expose an automatic retry or ordinary retry action in the initial implementation.
- Add coverage that a later manual continuation still passes through readiness and does not bypass the same blocked history state.
- Add coverage that repeated manual continuations can surface user feedback again while diagnostics for the same blocked state are rate-limited or coalesced.
- Add log-focused coverage that repeated blocked-request diagnostics are coalesced by conversation ID, task ID, assistant tool-call message ID, tool call ID, readiness category, and trigger layer without using sensitive content.
- Add log-focused coverage that coalesced diagnostics report a suppressed count in a non-sensitive summary when the coalescing window ends or the readiness category changes.
- Add log-focused coverage that the initial coalescing window is scoped to one request attempt, and a later manual continuation starts a new coalescing window.
- Add log-focused coverage that readiness diagnostics include a non-persistent request-attempt identifier that correlates preflight and serializer logs without entering repair matching or persistence.
- Add log-focused coverage that the request-attempt identifier is present in diagnostic records but excluded from the duplicate-diagnostic coalescing key.
- Add log-focused coverage for readiness diagnostic levels: `AcceptedHistoryRepair` at `info`, `PendingToolResults` at `debug` or lower-noise behavior, and corrupted or unexplained categories at `error`.
- Add log-focused coverage that `AcceptedHistoryRepair` emits one summarized `info` diagnostic per request with record count and non-sensitive identifiers, rather than using blocked-request coalescing or logging each record separately.
- Add log-focused coverage that accepted-repair summaries include repair-source counts and omit placeholder payload content and other sensitive content.
- Add log-focused coverage that `NeedsCancellationCommit` starts at `debug`, remains low-noise after successful commit and ready rerun, and escalates to `error` only for persistence failure, repeated no-progress state, or loop non-convergence.
- Add log-focused coverage that `StaleRepairRecordIgnored` logs at `debug` when non-blocking, while the resulting unauthorized visible gap logs through its blocking `error` category.
- Add serializer-level coverage that projected normal-flow gaps are rejected before sanitizer repair, while accepted history-repair gaps can still be repaired.
- Add coverage or code structure assertions where practical that the renamed sanitizer is only called for accepted history repair and not for normal-flow missing results.
- Add serializer-level coverage that readiness is evaluated against the final outbound message projection after compaction and filtering, so hidden historical gaps do not block while visible assistant tool calls still require a result or repair record.
- Add coverage that controller preflight activity checks do not replace serializer validation over the final post-compaction/filtering projection.
- Add serializer-level coverage that compaction or filtering hides tool-call messages and their corresponding tool results together, and blocks if an orphan tool result remains visible.
- Add serializer-level coverage that compaction or filtering does not create new repair dependence by hiding a tool result while keeping its assistant tool-call message visible.
- Add serializer-level coverage that repair records for hidden assistant tool-call messages remain unused, are not consumed, and do not count as stale for the current projection.
- Add serializer-level coverage that current `RequestParams.input` action results are treated as real terminal results in the final outbound projection and are not mistaken for missing persisted history.
- Add controller/request-construction coverage that current input action results remain supported for valid existing paths but are not used as a fallback after controller preflight fails to persist a required drained or cancellation result.
- Add serializer-level coverage that current input action results without persisted message IDs receive diagnostic-only synthetic projection IDs and do not participate in durable repair-record matching.
- Add serializer-level coverage that once a current input result is persisted, subsequent projections use the real `ToolCallResult` message ID and `PersistedHistory` source instead of the old synthetic ID.
- Add serializer-level coverage that the same tool call result appearing in both history and current input is blocked as a duplicate outbound projection rather than silently deduplicated.
- Add serializer-level coverage for multi-tool-call assistant messages where some calls have real results and some are repaired, plus a blocking case where one visible call lacks both a real result and a repair record.
- Add serializer-level coverage that sendable multi-tool-call groups emit outbound tool responses in assistant `tool_calls` order, mixing real results and structured repair placeholders only where individually authorized.
- Add readiness coverage that duplicate persisted tool results for one visible tool call block serialization as corrupted or unexplained history.
- Add controller coverage that known late-cancellation duplicates are deduplicated before serializer validation and do not produce duplicate outbound tool responses.
- Add serializer-level coverage that out-of-order real tool results are blocked rather than reordered.
- Add controller-level coverage for any known safe normal-flow ordering repair before serializer validation, if such repair is implemented.
- Add controller-level coverage that the initial auto-repair scope is limited to finished-result draining and known late-cancellation duplicate deduplication, while arbitrary reordering, unknown duplicate deletion, and late-result movement across a user turn remain blocked.
- Add coverage that missing results without an explicit repair source are rejected as normal-flow defects.
- Add coverage that corrupted persisted history blocks serialization by default and does not produce placeholder tool results.
- Add coverage that each accepted repair source is categorized in diagnostics.
- Add coverage that only forked-history and restored-legacy-history repair sources authorize placeholder repair in the initial implementation.
- Add coverage that corrupted, unknown, or manual-style repair categories are not accepted unless a future explicit repair-and-continue source is introduced.
- Add coverage that repair records are created at fork and legacy restore/conversion boundaries, not lazily during readiness, serializer validation, or sanitizer repair.
- Add fork-creation coverage showing that only gaps introduced by the retained forked task/message set receive `ForkedHistory` records.
- Add fork-creation coverage that retained orphan tool results are not converted into repair records and are blocked as corrupted or unexplained history.
- Add legacy restore/conversion coverage showing that only known legacy gaps receive `RestoredLegacyHistory` records.
- Add legacy restore/conversion coverage that a missing result without conversion-time evidence is not treated as `RestoredLegacyHistory` and blocks by default.
- Add coverage that compacted tool results satisfy readiness without entering placeholder repair.
- Add coverage that forked-history repair metadata survives conversation serialization and remains available after fork routing tokens are cleared.
- Add `AgentConversationData` roundtrip and legacy-payload tests for the new BYOP repair metadata field.
- Add repair-state versioning tests, including legacy missing sidecar deserializing to empty state and invalid sidecar JSON blocking repair authorization instead of silently permitting placeholders.
- Add coverage that invalid repair sidecar JSON blocks only when the current outbound projection needs repair authorization, while healthy real-result projections continue with a diagnostic.
- Add `AIConversation` load tests showing invalid repair sidecar data is preserved as an explicit invalid status rather than collapsed into an empty repair state.
- Add repair sidecar load-category tests for missing sidecar, invalid JSON, and unsupported version, including no raw JSON in diagnostics.
- Add repair state version tests showing version `1` is accepted, missing version is unsupported, higher versions are unsupported, and unknown versions do not authorize repair.
- Add coverage that forked-history repair records authorize only the recorded missing tool calls and do not allow unrelated normal-flow gaps.
- Add coverage that restored-legacy-history repair records authorize only the recorded missing tool calls and do not allow unrelated gaps in restored conversations.
- Add coverage that repair records do not authorize repair when only `tool_call_id` matches but `task_id` or assistant tool-call message ID differs.
- Add coverage that restored conversations with unexplained missing tool results are treated as corrupted persisted history by default.
- Add coverage that placeholder tool results generated for repair do not mutate task messages or persisted conversation history.
- Add coverage that repair placeholder payloads are structured JSON with an accepted repair-source reason and do not use the legacy bare text `(tool execution results are not retained)`.- Add coverage that repair placeholder payload `status` is always `"unavailable"` regardless of repair source.
- Add coverage that repair placeholder payload reason strings map from internal repair-source enums to stable snake_case values.
- Add coverage that repair placeholder `note` is fixed machine-facing English text and does not vary by UI locale.
- Add coverage that repair placeholder payloads contain only `status`, `reason`, and `note`, and do not include raw tool names.
- Add coverage that structured repair payload assertions parse JSON and compare fields semantically rather than relying on field order.
- Add coverage that repair placeholder payloads do not include tool arguments, tool output, or user prompt text.
- Add coverage that repair records can be reused across multiple outbound requests while the historical gap remains, and become inactive when a real result appears or the referenced tool call disappears.
- Add coverage that stale repair records are ignored by read-only readiness or serializer validation, and that stale-record cleanup is best-effort controller behavior rather than required for request serialization.
- Add `AIConversation` restore/persist coverage showing that BYOP repair metadata is loaded from and saved back to the `byop_repair_state_json` sidecar.
- Extend sanitizer tests to confirm placeholders are inserted only for repair cases, not for ordinary in-flight tool execution.
- Add a regression test for the observed shape where one request contains `(tool execution result not retained)` and the next request contains the real cancellation result.- Add a regression test where a user prompt cancels a running asynchronous tool and the immediately serialized BYOP request contains the cancellation result with no placeholder.
- Add coverage that a late executor cancellation event after synchronous cancellation-result persistence is deduplicated by `tool_call_id` and does not create duplicate persisted tool results or duplicate outbound tool responses.
- Add controller or request-construction coverage for a user prompt submitted while a tool action is in progress.
- Add auto-resume coverage for locally intercepted tool results that do not enter the normal action queue.
- Add request-readiness coverage showing that committed local interception results satisfy tool-call readiness without requiring an `AIAgentActionResult` or protobuf result oneof.
- Add request-readiness coverage showing that committed `invalid_arguments` carrier results satisfy readiness as structured error results even though the target tool did not execute normally.
- Add request-readiness coverage showing that unreadable local interception payloads do not satisfy readiness, block as corrupted or unexplained history, and do not log the raw payload.
- Add request-readiness coverage showing that normal-flow unresolved tool calls block serialization and do not produce placeholder tool results.
- Add coverage that readiness and normal-flow placeholder blocking are always active in the BYOP Chat serialization path and are not gated by a feature flag.
- Keep existing tool-call ordering tests as prior art: normal adjacent tool messages, missing tool responses, orphan tool responses, out-of-order tool responses, late tool responses after a user query, placeholder replacement by late real results, and multiple independent tool-call groups.
- Validate generated BYOP request bodies with a strict Chat Completions ordering checker: no orphan tool responses, no duplicate tool result IDs, and no user/assistant message before pending tool responses are resolved.
- The strict Chat Completions ordering checker may be a test helper for externally visible request bodies, but production code must still run readiness and serializer validation before BYOP dispatch.
- Include log-focused assertions where practical: unexpected placeholder insertion should produce a categorized diagnostic.
- Include log-focused assertions where practical that readiness diagnostics include non-sensitive identifiers and omit tool arguments, tool output, and user prompt text.
- Include log-focused assertions that local interception results use stable redacted diagnostic tool kinds and do not log raw model-provided tool names, tool arguments, or carrier payloads.
- Do not add telemetry assertions for the initial implementation; readiness observability should be log-based unless a separate telemetry design is approved.

## Out of Scope

- Fixing network, SSE, proxy, or upstream model stream interruptions.
- Reducing noisy partial tool-argument parse warnings.
- Changing provider selection, endpoint configuration, model IDs, or BYOP authentication.
- Redesigning conversation compaction or request size management.
- Removing placeholder repair behavior for explicitly recorded forked-history or restored-legacy-history gaps.
- Publishing this PRD to GitHub issues; the local environment does not currently expose `gh` or a GitHub token.

## Further Notes

The observed request set did not show an upstream request rejection problem. All inspected BYOP requests were accepted and had valid tool-call pairing. The issue is conversation fidelity: a protocol-safe placeholder can still be semantically wrong when produced during normal execution.

The current evidence came from local request records and `waz.log`: one request contained `(tool execution result is not retained)` for a tool call, while the next request contained a real cancellation result for that same call. That suggests Waz can send a follow-up request before the real asynchronous result is persisted.
Regression fixtures derived from this observation should be synthetic, minimal, and redacted. Do not copy real logs, prompts, tool arguments, or user content into tests.
