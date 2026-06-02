# BYOP Tool Result Readiness Technical Plan

## Status

Drafted from the accepted decisions in `PRODUCT.md`, `CONTEXT.md`, and `ADR.md`.

During the grilling phase, implementation decisions may remain duplicated between `PRODUCT.md` and this file to avoid losing accepted context. After decisions stabilize, do a cleanup pass that keeps `PRODUCT.md` focused on behavior and moves implementation matrices into this technical plan.

Before converting this plan into implementation tasks or issues, run a consistency pass across `CONTEXT.md`, `PRODUCT.md`, this file, and `ADR.md`.

## Goals

- Prevent normal BYOP Chat requests from sending placeholder tool results.
- Keep explicit forked-history and restored-legacy-history repair available.
- Validate OpenAI-compatible tool-call ordering before BYOP dispatch.
- Keep readiness logic testable without constructing full UI state.

## Main Code Areas

- `app/src/ai/byop_readiness/`: new module for pure readiness classification, repair state, diagnostics, and tests.
- `app/src/ai/blocklist/controller.rs`: controller preflight after `RequestParams` construction and before BYOP dispatch.
- `app/src/ai/blocklist/action_model.rs`: live action state, finished-result draining, cancellation-result commit and dedupe.
- `app/src/ai/agent/api.rs`: `RequestParams` should carry a read-only BYOP repair-state snapshot.
- `app/src/ai/agent/conversation.rs`: `AIConversation` should own deserialized BYOP repair sidecar state and persist it with conversation data.
- `crates/persistence/src/model.rs`: `AgentConversationData` gets an optional serialized repair sidecar field.
- `app/src/ai/agent_providers/chat_stream.rs`: serializer validation and renamed accepted-history repair sanitizer.
- Fork/restore creation paths, including `app/src/ai/blocklist/history_model.rs`, `app/src/terminal/view/load_ai_conversation.rs`, and legacy conversion code, should create repair records only at explicit history transformation points.

## Data Model

Add a BYOP repair sidecar that mirrors the existing compaction sidecar pattern:

```rust
pub struct RepairState {
    pub version: u32,
    pub records: Vec<RepairRecord>,
}

pub struct RepairRecord {
    pub source: RepairSource,
    pub task_id: String,
    pub assistant_tool_call_message_id: String,
    pub tool_call_id: String,
    pub fork_point_message_id: Option<String>,
    pub exchange_id: Option<String>,
}

pub enum RepairSource {
    ForkedHistory,
    RestoredLegacyHistory,
}

pub enum RepairStateStatus {
    Valid(RepairState),
    Invalid { error_category: RepairStateLoadError },
}

pub enum RepairStateLoadError {
    InvalidJson,
    UnsupportedVersion,
}
```

Persistence shape:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub byop_repair_state_json: Option<String>,
```

The sidecar should live as app-layer BYOP state. `crates/persistence` should only store the optional JSON field and should not own BYOP-specific repair semantics.

`RepairState` starts at version `1`. Missing `byop_repair_state_json` from legacy conversation data should load as a valid empty state, not an error. A present sidecar with no version field or with a version greater than `1` should load as `UnsupportedVersion` until an explicit migration or reader is implemented. Invalid JSON and unsupported versions must be preserved as explicit invalid statuses rather than collapsed into empty state. Invalid status must be logged without raw JSON and must not authorize repair. It blocks only when the current outbound projection needs repair-record authorization for a visible missing result. If all visible tool calls have real terminal results, the request can continue with a high-severity diagnostic.

## Normalized Projection

Readiness should classify an internal normalized projection rather than provider `ChatMessage` values or raw `api::Message` shapes.

The initial projection should cover only the BYOP Chat readiness message kinds needed for tool-call validation:

- user boundary messages
- assistant tool-call messages
- tool result messages
- assistant/system or other boundary messages needed for ordering

Unknown or readiness-irrelevant message kinds should preserve ordering boundaries where needed but should not require a full abstraction of every raw `api::Message` variant in the initial implementation. Boundary behavior is based on final outbound visibility: if the message becomes a visible non-tool-response Chat message, it blocks any pending tool-call group before it; if it is fully filtered out before projection, it does not affect readiness.

The projection should preserve the metadata needed for readiness, diagnostics, and repair matching:

- task ID
- message ID for the current projected item's source message
- role or projected message kind
- assistant tool-call message ID, which is the same as `message_id` for assistant tool-call items and a back-reference for tool-result items when known
- tool call ID
- redacted tool kind
- result source, such as persisted history or current `RequestParams.input`
- whether the result is real, compacted, structured error, local interception, or accepted repair

Persisted history results and current input results both satisfy readiness, but the projection must keep the source distinction so duplicate projections can be diagnosed instead of silently deduplicated.

Current `RequestParams.input` action results may not yet have persisted message IDs. Projection construction should assign a diagnostic-only ID such as `current_input:{index}:{tool_call_id}` and mark the source as `CurrentInput`. The ID only needs to be stable within one request construction and serializer-validation pass. It is not stable across requests or app restarts. These IDs are not persisted and must not be used for durable repair-record matching, which still depends on task ID, assistant tool-call message ID, and tool call ID.

Once that action result is persisted as a real `ToolCallResult` message, later projections should use the persisted message ID and `PersistedHistory` source. Request construction should not project both the old current input result and the newly persisted history result for the same tool call.

Current input results remain valid for existing request-construction paths where the result is legitimately part of the current request. They must not be used as a fallback after controller preflight attempted and failed to persist a required drained or cancellation result.

When a tool-result item lacks an assistant back-reference, projection construction may infer `assistant_tool_call_message_id` only from a unique same-task pending assistant tool call with the same `tool_call_id` and valid ordering. Valid ordering means the result appears after the assistant tool-call item and before any later user or assistant message. All visible tool calls in an assistant tool-call group must be satisfied before the next user or assistant message appears. Ambiguous, cross-boundary, or invalid-order matches should remain unknown and be classified later as orphan, out-of-order, or corrupted. Repair authorization must not use ambiguous inferred links.

The projection should not carry full message content, tool output, tool arguments, user prompt text, or raw local interception payloads. If content-related diagnostics are necessary, use safe derived fields such as byte length, token estimate, or a non-reversible hash.

Provider `ChatMessage` construction should happen after readiness and accepted-history repair decisions. This keeps provider serialization as the final formatting step instead of the place where missing-result policy is decided.

## Readiness Categories

The initial classifier should preserve these categories for logs and tests:

- `Ready`
- `AcceptedHistoryRepair`
- `PendingToolResults`
- `NeedsCancellationCommit`
- `DuplicateToolResults`
- `OrphanToolResult`
- `OutOfOrderToolResult`
- `MissingResultWithoutRepairSource`
- `StaleRepairRecordIgnored`

`AcceptedHistoryRepair` is sendable but must remain distinct from ordinary `Ready`. Corrupted or unexplained categories map to `RenderableAIError::Other` with `will_attempt_resume=false` and `waiting_for_network=false`.

## Classification Rules

- A visible assistant tool call must have exactly one real terminal result or an exactly matching repair record.
- Repair records match only when task ID, assistant tool-call message ID, and tool call ID all match.
- Missing results in serializer validation are never considered pending, because serializer validation has no live action-model state.
- Controller preflight may classify missing results as pending or drainable only when the live action model has a matching running action or finished action result in the current conversation and task.
- Duplicate, orphan, and out-of-order real tool results block serialization.
- Serializer validation must not reorder persisted history or deduplicate ambiguous results.
- Current `RequestParams.input` action results count as real terminal results in the final outbound projection.
- If the same tool result appears in both history and current input, the outbound projection is duplicate and must block.

## Controller Preflight Flow

1. Build `RequestParams`.
2. Drain and commit finished action results where available, persisting them before dispatch when the current request depends on them.
3. Build a controller readiness view from current task history, current request input, repair state, and live action summaries.
4. If readiness returns `NeedsCancellationCommit`, synchronously commit cancellation results idempotently by tool call ID, persist the updated conversation/request state, discard the old `RequestParams`, rebuild from the updated conversation state, and rerun readiness.
5. If readiness returns `PendingToolResults`, do not open a BYOP request; leave the conversation in the existing waiting or auto-resume path.
6. If readiness returns corrupted or unexplained history, surface a blocked-request `RenderableAIError::Other` and do not schedule retry or auto-resume.
7. Continue to serializer only for `Ready` or `AcceptedHistoryRepair`.

Controller auto-repair is initially limited to finished-result draining and known late-cancellation duplicate dedupe. It should not reorder arbitrary persisted history, delete unknown duplicates, or move late results across a later user turn.

Readiness reruns should use a bounded loop with initial `max_iterations = 3`. Each iteration must record progress, such as a persisted drained result, committed cancellation result, or known duplicate dedupe. If the same actionable readiness state repeats without progress, block dispatch and log an internal readiness-loop diagnostic such as `ReadinessLoopDidNotConverge`. Loop diagnostics should include iteration count and final readiness category, not raw content. The user-visible error should remain the generalized blocked-request copy used for corrupted or unexplained history.

Blocked-request user copy should make clear that Waz did not send the BYOP request because a previous tool call is missing its recorded result. It should not read like a provider/model rejection.

The initial blocked-request UI should not offer automatic retry or an ordinary retry action. Manual continuation remains possible, but it must pass through readiness again and should not bypass the blocked history state. If the same state remains blocked, user feedback may be shown again for the new attempt. Diagnostics should be rate-limited or coalesced by conversation ID, task ID, assistant tool-call message ID, tool call ID, readiness category, and trigger layer to avoid high-severity log spam. The initial coalescing window is one request attempt rather than a time-based window; a later manual continuation starts a new window and may log a new first full non-sensitive diagnostic. Coalescing should log the first occurrence with full non-sensitive metadata, increment a suppressed count for repeated matching diagnostics, and emit a non-sensitive summary containing `suppressed_count` when the coalescing window ends or the readiness category changes. Future repair-and-continue should be a separate explicit action with its own repair source and diagnostics.

Each request attempt should have a non-persistent diagnostic identifier, such as `readiness_attempt_id` or an existing response/request stream ID. Use it to correlate controller preflight and serializer validation diagnostics. Do not persist it or use it for repair matching. Because the coalescing window is already one request attempt, the attempt identifier should be logged but should not be part of the duplicate-diagnostic coalescing key.

If cancellation-result persistence fails, BYOP dispatch should be blocked. Finished-result drain persistence failures should also block when those results are required for readiness. The failed result should not be reintroduced through current `RequestParams.input` to bypass persistence. After successful persistence, request construction should rebuild `RequestParams` from updated conversation state rather than patch old input in place, so the serializer does not see both current-input and persisted-history copies of the same result. The initial implementation should not proceed with only in-memory tool-result state, because retry or restart could recreate the missing-result gap.

If tool-result persistence succeeds but later BYOP serialization or dispatch fails, do not roll back the persisted result. The result is a real conversation fact and should remain available for the next request attempt.

## Serializer Validation Flow

1. Build the final outbound message projection after compaction/filtering and after merging persisted task messages with current `RequestParams.input`, or build it from inputs that already encode final post-compaction/filtering visibility.
2. Run serializer validation over the projected messages before accepted-history repair.
3. For `Ready`, serialize normally.
4. For `AcceptedHistoryRepair`, pass the exact accepted records to the renamed repair sanitizer.
5. For any normal-flow gap, duplicate, orphan, or ordering defect, block before sanitizer repair.

Compaction and filtering must preserve tool-call group integrity. If an assistant tool-call item is hidden, its corresponding tool-result items must be hidden too. A visible tool result whose assistant tool-call item was hidden is an orphan and should block serializer validation. If an assistant tool-call item remains visible, each visible tool call should have a real or compacted tool result unless a pre-existing forked-history or restored-legacy repair record already authorizes the gap. Compaction/filtering must not create new repair dependence by hiding a result while keeping its tool call visible. A repair record for a hidden assistant tool-call item is unused for the current projection and should not be consumed or considered stale.

Controller preflight may use an un-compacted task/activity view to decide pending, drainable, and cancellation actions. That does not replace serializer validation over the final post-compaction/filtering projection.

Rename the broad sanitizer entry point to narrow its meaning, for example:

```rust
repair_tool_call_pairs_for_accepted_history_gaps(...)
```

This repair function should only generate outbound structured JSON placeholders for accepted repair records. It must not persist placeholder messages.

## Placeholder Payload

Accepted repair placeholders should use structured JSON:

```json
{
  "status": "unavailable",
  "reason": "forked_history_repair",
  "note": "tool result was unavailable in repaired conversation history"
}
```

The `status` field should always be `"unavailable"`. Use a source-derived reason such as `forked_history_repair` or `restored_legacy_history_repair`. Do not include tool arguments, tool output, user prompt text, or raw local interception payloads.

Map internal enum values to outbound reason strings explicitly:

- `RepairSource::ForkedHistory` -> `forked_history_repair`
- `RepairSource::RestoredLegacyHistory` -> `restored_legacy_history_repair`

The `note` should be fixed English machine-facing text and should not be localized or vary by UI locale.

The payload should contain only `status`, `reason`, and `note`. Do not include raw tool names; diagnostics can carry redacted tool kinds separately. Tests should parse the JSON payload and compare field sets and values semantically rather than relying on object field order.

For multi-tool-call assistant messages, emit outbound tool responses in the assistant `tool_calls` order. Use real results where present and structured repair placeholders only at individually authorized positions.

## Repair Record Creation

Repair records are created only at explicit history transformation points:

- Fork creation: after building the retained forked task/message set, record only assistant tool calls whose matching results were intentionally omitted by the fork operation.
- Legacy restore/conversion: record only known legacy gaps proven by conversion-time input or documented old-format behavior.

Do not create repair records lazily in readiness, serializer validation, or sanitizer repair. Do not create records for orphan tool results.

## Local Interception Results

Committed local interception results satisfy readiness when their structured `server_message_data` can be interpreted.

Use stable diagnostic tool kinds:

- `local_interception:todowrite`
- `local_interception:webfetch`
- `local_interception:websearch`
- `local_interception:invalid_arguments`

`invalid_arguments` is a structured error result and satisfies readiness. Unreadable local interception payloads block as corrupted or unexplained history and must not log raw payload content.

## Diagnostics

Readiness and serializer diagnostics should include:

- readiness category
- conversation ID
- task ID
- assistant tool-call message ID
- tool call ID
- redacted tool kind
- trigger layer

Do not log tool arguments, tool output, raw user prompt text, raw carrier payloads, or raw local interception payloads.

Initial observability should be log-only. Do not add a new telemetry event as part of this implementation. If aggregate readiness-blocked metrics are needed later, design telemetry separately with its own schema and privacy review.

Initial log levels:

- `info`: `AcceptedHistoryRepair`
- `debug` or lower-noise behavior: `PendingToolResults`
- `debug`: initial `NeedsCancellationCommit` when controller preflight can commit the cancellation result and rerun readiness
- `debug`: non-blocking `StaleRepairRecordIgnored`
- `error`: corrupted or unexplained categories, including `DuplicateToolResults`, `OrphanToolResult`, `OutOfOrderToolResult`, `MissingResultWithoutRepairSource`, invalid repair sidecar, and `ReadinessLoopDidNotConverge`
- `error`: cancellation-result persistence failure, repeated `NeedsCancellationCommit` without progress, or readiness-loop non-convergence

`AcceptedHistoryRepair` is sendable and should not use blocked-request coalescing. If multiple repair records are used in one request, emit one summarized `info` diagnostic with record count, repair-source counts, and non-sensitive identifiers. Do not log placeholder payload content, tool arguments, tool output, user prompt text, or raw payloads.

## Test Plan

Unit tests near `app/src/ai/byop_readiness/`:

- every readiness category
- normalized projection construction and metadata retention
- minimal BYOP Chat readiness message-kind coverage without exhaustive raw `api::Message` abstraction
- visible unknown or readiness-irrelevant messages act as ordering boundaries, while filtered messages do not affect readiness
- projection construction omits raw content while preserving classification metadata
- result-source distinctions between persisted history and current `RequestParams.input`
- projection ID semantics for assistant tool-call items and tool-result items
- synthetic diagnostic IDs for current input results without persisted message IDs
- strict assistant back-reference inference for tool-result items
- ordering boundaries around assistant tool-call groups
- complete, pending, cancelled, duplicate, orphan, out-of-order, corrupted, compacted, and accepted-repair groups
- repair record exact matching by task ID, assistant tool-call message ID, and tool call ID
- structured error and unreadable local interception payload behavior

Controller tests:

- user continuation with in-flight tool commits cancellation result before serialization
- cancellation-result persistence failure blocks BYOP dispatch
- finished-result drain persistence failure blocks BYOP dispatch when required for readiness
- successful tool-result persistence rebuilds `RequestParams` before readiness is rerun
- old `RequestParams.input` is not patched in place after persistence
- bounded readiness reruns require progress and block on repeated actionable states without progress
- readiness rerun loop stops at initial `max_iterations = 3`
- readiness-loop non-convergence uses generalized user copy and `ReadinessLoopDidNotConverge` diagnostics
- persisted tool results remain committed when later BYOP serialization or dispatch fails
- late cancellation duplicate dedupes by tool call ID
- pending/drainable classification requires live action state in the current conversation and task
- corrupted/unexplained history does not retry, auto-resume, or open BYOP request
- blocked-request copy distinguishes local no-send behavior from provider rejection
- blocked-request UI does not expose automatic retry or ordinary retry in the initial implementation
- manual continuation still goes through readiness and cannot bypass the same blocked history state
- repeated manual continuation can show user feedback while coalescing duplicate diagnostics
- duplicate blocked-request diagnostics use a non-sensitive coalescing key
- coalesced diagnostics report suppressed counts in non-sensitive summaries
- diagnostic coalescing is scoped to a single request attempt, with manual continuation starting a new window
- readiness diagnostics include a non-persistent request-attempt identifier
- request-attempt identifier is excluded from the coalescing key
- readiness diagnostic log levels match category severity
- accepted history repair emits one summarized `info` diagnostic per request
- accepted history repair summaries include repair-source counts and omit sensitive content
- `NeedsCancellationCommit` remains low-noise on successful commit and escalates only on persistence failure or no-progress loop behavior
- non-blocking `StaleRepairRecordIgnored` logs at `debug`, while unauthorized visible gaps log through blocking `error` categories

Serializer/request-body tests:

- current `RequestParams.input` action results satisfy readiness
- current input action results are not used as a fallback after required persistence fails
- current input action results without persisted message IDs receive diagnostic-only synthetic projection IDs
- persisted current input results switch to real message IDs and `PersistedHistory` source in later projections
- duplicate history plus current input blocks
- final projection after compaction/filtering is the validation target
- controller preflight activity checks do not replace serializer validation over final projection
- compaction/filtering hides assistant tool-call items and corresponding tool-result items together
- compaction/filtering does not create new missing-result repair dependence
- hidden assistant tool-call repair records remain unused and are not consumed or cleaned up
- accepted repair placeholders use structured JSON and are not persisted
- accepted repair placeholder status is always `unavailable`
- accepted repair placeholder reasons use stable snake_case mappings from internal repair sources
- accepted repair placeholder note is fixed English machine-facing text
- accepted repair placeholder payload contains only `status`, `reason`, and `note`
- accepted repair placeholder tests compare parsed JSON semantics rather than field order
- multi-tool-call groups preserve assistant `tool_calls` order
- strict Chat Completions ordering helper verifies no orphan tools, duplicates, or user/assistant messages before required tool responses

Persistence and transformation tests:

- `AgentConversationData` roundtrip and legacy payload without `byop_repair_state_json`
- repair-state versioning and invalid-sidecar handling
- invalid repair sidecar blocks only repair-dependent projections
- invalid repair sidecar loads as explicit invalid status, not empty state
- load categories for missing sidecar, invalid JSON, and unsupported version
- strict version handling: v1 accepted, missing version and higher versions unsupported
- `AIConversation` loads and saves repair state sidecar
- fork creation records only retained tool calls with intentionally missing results
- retained orphan tool results block and do not produce repair records
- legacy restore records only conversion-proven legacy gaps

## Implementation Phases

1. Build `app/src/ai/byop_readiness/` with minimal BYOP Chat normalized projection construction, pure readiness classification, diagnostics types, and focused unit tests. Do not wire controller preflight or mutate action/conversation state in this phase.
2. Add BYOP repair sidecar persistence after Phase 1 defines `RepairState` and `RepairStateStatus`. Then wire `AgentConversationData` roundtrip tests and `AIConversation` load/save integration. Avoid adding a bare JSON field before the load semantics, version handling, and invalid-state behavior exist.
3. Add serializer validation over the final outbound projection, then rename or extract the accepted-history repair sanitizer so placeholders are produced only for accepted repair records. This may be implemented as substeps, first blocking normal-flow gaps and then generating accepted repair placeholders, but the phase should not be considered complete or merged as finished until both behaviors are present.
4. Add controller preflight with bounded readiness reruns, finished-result draining, synchronous cancellation-result persistence, duplicate dedupe for known late cancellation, and request rebuild after successful persistence. Wire this after Phase 3 serializer validation is in place, so the serializer already acts as the final safety net if a controller path is missed.
5. Add fork and legacy restore/conversion Repair Record creation at explicit history transformation boundaries. Do this after serializer accepted-repair consumption exists, so newly persisted records have a validated consumption path and tests can prove placeholders remain outbound-only.
6. Add regression, request-body, and log-focused coverage for the observed placeholder-then-cancellation shape, strict Chat Completions ordering, diagnostics, and blocked-request behavior. The observed-shape fixture should be synthetic, minimal, and redacted: reproduce the structure where one request would have sent a placeholder before the next request sees the real cancellation result, without embedding real logs, prompts, tool arguments, or user content.

## Implementation Checklist

Execute write changes in phase order. Parallelize only read-only investigation or test-fixture design unless write ownership is clearly disjoint. Avoid simultaneous edits to the serializer, controller preflight, and request-construction paths because their ordering dependencies are central to this design.

- [ ] Create `app/src/ai/byop_readiness/` with normalized projection types, safe metadata fields, and pure readiness result categories.
- [ ] Add focused unit tests for projection construction, content omission, result-source distinctions, assistant back-reference inference, and every readiness category.
- [ ] Define `RepairState`, `RepairRecord`, `RepairSource`, `RepairStateStatus`, and strict version/load handling.
- [ ] Add `byop_repair_state_json` persistence, `AgentConversationData` roundtrip tests, and `AIConversation` load/save integration.
- [ ] Pass a read-only repair state snapshot through `RequestParams`.
- [ ] Validate final post-compaction/filtering outbound projection before provider `ChatMessage` construction.
- [ ] Rename or extract the sanitizer so accepted history repair is the only placeholder-producing path.
- [ ] Emit structured outbound repair placeholders with only `status`, `reason`, and `note`.
- [ ] Add controller preflight with bounded reruns, progress checks, finished-result draining, cancellation-result commits, persistence failure blocking, and `RequestParams` rebuilds.
- [ ] Add fork and legacy restore/conversion repair-record creation at explicit transformation boundaries only.
- [ ] Add request-body regression tests for normal-flow blocking, accepted repair, current input results, duplicate projections, compaction/filtering boundaries, and strict Chat Completions ordering.
- [ ] Add log-focused tests for diagnostic levels, coalescing, request-attempt IDs, source counts, redaction, and blocked-request copy.
- [ ] Run the project-required validation, at minimum `cargo check`, before considering implementation complete.

## Phase Stop Points

- Phase 1 stop point: `byop_readiness` projection/classifier compiles and focused unit tests pass.
- Phase 2 stop point: repair sidecar load/save behavior is covered by `AgentConversationData` roundtrip tests and `AIConversation` restore/persist tests.
- Phase 3 stop point: serializer/request-body tests prove normal-flow gaps block and accepted repair placeholders are outbound-only structured JSON.
- Phase 4 stop point: controller tests prove bounded preflight, drain/cancellation persistence, request rebuild, pending handling, and no retry/auto-resume for corrupted history.
- Phase 5 stop point: fork and legacy restore/conversion tests prove repair records are created only at explicit transformation boundaries and only for authorized gaps.
- Phase 6 stop point: regression, strict ordering, and log-focused tests pass, followed by project-required validation, at minimum `cargo check`.

## Non-Goals

- No feature flag for readiness or normal-flow placeholder blocking.
- No new telemetry event in the initial implementation.
- No SQL migration or new persistence table/column.
- No non-BYOP provider branching.
- No automatic repair for corrupted, unknown, or manual gaps in the initial implementation.
- No serializer mutation of conversation state.
