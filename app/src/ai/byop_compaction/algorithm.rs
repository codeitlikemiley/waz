//! Compression core algorithm — 1:1 port of opencode `compaction.ts:141-341` (turns / select / splitTurn / prune).
//!
//! Decoupled from the specific message types of warp: externally abstracted through the [`MessageRef`] trait,
//! See `super::message_view` for the actual implementation.
use std::hash::Hash;

use super::consts::{PRUNE_MINIMUM, PRUNE_PROTECT, PRUNE_PROTECTED_TOOLS};
use super::overflow::{usable, ModelLimit};
use super::CompactionConfig;

/// The role of the message - used for turn detection and select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// Meta information output by a single tool (required for prune decision-making).
#[derive(Debug, Clone)]
pub struct ToolOutputRef<CallId> {
    pub call_id: CallId,
    pub tool_name: String,
    /// Estimate the number of tokens (aligned with opencode `Token.estimate(part.state.output)`).
    pub output_size: usize,
    pub completed: bool,
    /// It has been marked `compacted` by prune, and a break is encountered when continuing to traverse.
    pub already_compacted: bool,
}

/// Abstract message reference — the algorithm only interacts with this trait and is decoupled from the warp type.
pub trait MessageRef {
    type Id: Clone + Eq + Hash;
    type CallId: Clone + Eq + Hash;

    fn id(&self) -> Self::Id;
    fn role(&self) -> Role;

    /// Whether the user message carries a compaction trigger tag (opencode `parts.some(p => p.type === "compaction")`).
    fn is_compaction_marker(&self) -> bool;

    /// Whether the assistant message is the summary itself (opencode `info.summary === true`).
    fn is_summary(&self) -> bool;

    /// Token estimation for a single message — implemented using `serde_json` + `super::token::estimate`.
    fn estimate_size(&self) -> usize;

    /// All tool outputs (used by prune) within this message. Assistant message will only be available.
    fn tool_outputs(&self) -> Vec<ToolOutputRef<Self::CallId>>;
}

/// `compaction.ts:76-80` type correspondence.
#[derive(Debug, Clone)]
pub struct Turn<Id> {
    pub start: usize,
    pub end: usize,
    pub id: Id,
}

/// `compaction.ts:82-85`。
#[derive(Debug, Clone)]
pub struct Tail<Id> {
    pub start: usize,
    pub id: Id,
}

/// `select` return value: `head` is the range to be sent to the summary LLM, `tail_start_id` is the starting point of the reserved segment.
#[derive(Debug, Clone)]
pub struct SelectResult<Id> {
    pub head_end: usize,
    pub tail_start_id: Option<Id>,
}

/// `compaction.ts:141-157`。
pub fn turns<M: MessageRef>(messages: &[M]) -> Vec<Turn<M::Id>> {
    let mut result: Vec<Turn<M::Id>> = Vec::new();
    let n = messages.len();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role() != Role::User {
            continue;
        }
        if msg.is_compaction_marker() {
            continue;
        }
        result.push(Turn {
            start: i,
            end: n,
            id: msg.id(),
        });
    }
    let len = result.len();
    if len > 1 {
        for i in 0..len - 1 {
            result[i].end = result[i + 1].start;
        }
    }
    result
}

/// `compaction.ts:159-182` splitTurn — Find the first cut point inside the turn that can fit into the budget.
fn split_turn<M, EstFn>(
    messages: &[M],
    turn: &Turn<M::Id>,
    budget: usize,
    estimate: &EstFn,
) -> Option<Tail<M::Id>>
where
    M: MessageRef,
    EstFn: Fn(&[M]) -> usize,
{
    if budget == 0 {
        return None;
    }
    if turn.end.saturating_sub(turn.start) <= 1 {
        return None;
    }
    let mut start = turn.start + 1;
    while start < turn.end {
        let size = estimate(&messages[start..turn.end]);
        if size > budget {
            start += 1;
            continue;
        }
        return Some(Tail {
            start,
            id: messages[start].id(),
        });
    }
    None
}

/// `compaction.ts:244-293` select — Cut out head/tail.
///
/// `estimate_slice` corresponds to opencode `estimate({ messages: slice, model })`.
/// The caller passes in `Token.estimate` because it needs to decide how to serialize the message list (JSON).
pub fn select<M, EstFn>(
    messages: &[M],
    cfg: &CompactionConfig,
    model: ModelLimit,
    estimate_slice: EstFn,
) -> SelectResult<M::Id>
where
    M: MessageRef,
    EstFn: Fn(&[M]) -> usize,
{
    let limit = cfg.tail_turns;
    if limit == 0 {
        return SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        };
    }
    let usable_tokens = usable(cfg, model);
    let budget = cfg.preserve_recent_budget(usable_tokens);
    let all = turns(messages);
    if all.is_empty() {
        return SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        };
    }
    let recent_start = all.len().saturating_sub(limit);
    let recent: Vec<&Turn<M::Id>> = all[recent_start..].iter().collect();
    let sizes: Vec<usize> = recent
        .iter()
        .map(|t| estimate_slice(&messages[t.start..t.end]))
        .collect();

    let mut total: usize = 0;
    let mut keep: Option<Tail<M::Id>> = None;
    for i in (0..recent.len()).rev() {
        let turn = recent[i];
        let size = sizes[i];
        if total + size <= budget {
            total += size;
            keep = Some(Tail {
                start: turn.start,
                id: turn.id.clone(),
            });
            continue;
        }
        let remaining = budget.saturating_sub(total);
        let split = split_turn(messages, turn, remaining, &estimate_slice);
        if split.is_some() {
            keep = split;
        }
        // Note the implementation of opencode: the first time the size exceeds the budget, it will break, no matter whether splitTurn is found or not, it will not try to turn earlier.
        break;
    }

    match keep {
        None => SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        },
        Some(t) if t.start == 0 => SelectResult {
            head_end: messages.len(),
            tail_start_id: None,
        },
        Some(t) => SelectResult {
            head_end: t.start,
            tail_start_id: Some(t.id),
        },
    }
}

/// `compaction.ts:297-341` prune decision — Returns (message_id, tool_call_id) pairs that should be marked `compacted`.
///
/// The caller writes `CompactionState.markers` accordingly (the actual protobuf message does not change).
pub fn prune_decisions<M: MessageRef>(messages: &[M]) -> Vec<(M::Id, M::CallId)> {
    let mut total: usize = 0;
    let mut pruned: usize = 0;
    let mut to_prune: Vec<(M::Id, M::CallId)> = Vec::new();
    let mut user_turns_seen: usize = 0;

    'outer: for msg in messages.iter().rev() {
        if msg.role() == Role::User {
            user_turns_seen += 1;
        }
        // Keep at least the last 2 user turns intact (opencode `if (turns < 2) continue`).
        if user_turns_seen < 2 {
            continue;
        }
        // Summary boundary reached — look no further.
        if msg.role() == Role::Assistant && msg.is_summary() {
            break 'outer;
        }
        let outputs = msg.tool_outputs();
        for tp in outputs.into_iter().rev() {
            if !tp.completed {
                continue;
            }
            if PRUNE_PROTECTED_TOOLS.contains(&tp.tool_name.as_str()) {
                continue;
            }
            if tp.already_compacted {
                break 'outer;
            }
            let estimate = tp.output_size;
            total += estimate;
            if total <= PRUNE_PROTECT {
                continue;
            }
            pruned += estimate;
            to_prune.push((msg.id(), tp.call_id));
        }
    }

    if pruned > PRUNE_MINIMUM {
        to_prune
    } else {
        Vec::new()
    }
}
