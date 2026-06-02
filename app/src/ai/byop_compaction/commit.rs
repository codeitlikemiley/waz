//! Write the output of the just completed SummarizeConversation stream back to conversation.compaction_state —
//! Align state changes at the end of opencode `compaction.ts processCompaction` + bus.publish(Compacted).
//!
//! This module is independent of the controller and serves as a unit-testable helper (although the actual call site is in controller.rs).

use warp_multi_agent_api as api;

use crate::ai::agent::conversation::AIConversation;

use super::algorithm::{prune_decisions, select, MessageRef};
use super::config::CompactionConfig;
use super::message_view::{build_tool_name_lookup, project};
use super::overflow::ModelLimit;
use super::state::CompletedCompaction;

/// Find the last `Message::AgentOutput` from the root task of conversation in reverse order —
/// It is the summary text of the model just emitted.
///
/// `user_msg_id` selects the id of the most recent real UserQuery before the last AgentOutput;
/// If not, synthesize an independent uuid (only used as marker key, build_chat_request's hidden
/// The projection will not hit the real message).
pub fn commit_summarization(
    conversation: &mut AIConversation,
    overflow: bool,
    cfg: &CompactionConfig,
) -> bool {
    // Use conversation’s existing linearized messages accessor — merged in time order across all tasks
    let mut all_msgs: Vec<&api::Message> = conversation.all_linearized_messages();
    all_msgs.sort_by_key(|m| {
        m.timestamp
            .as_ref()
            .map(|ts| (ts.seconds, ts.nanos))
            .unwrap_or((0, 0))
    });

    let last_agent_output: Option<(String, String)> = all_msgs.iter().rev().find_map(|m| {
        let inner = m.message.as_ref()?;
        match inner {
            api::message::Message::AgentOutput(a) => Some((m.id.clone(), a.text.clone())),
            _ => None,
        }
    });

    let Some((assistant_id, summary_text)) = last_agent_output else {
        log::warn!("[byop-compaction] commit: no AgentOutput found — nothing to commit");
        return false;
    };

    let assistant_id_str: &str = &assistant_id;
    let assistant_pos = all_msgs
        .iter()
        .position(|m| m.id.as_str() == assistant_id_str);
    let user_msg_id: String = assistant_pos
        .and_then(|pos| {
            all_msgs[..pos]
                .iter()
                .rev()
                .find_map(|m| match m.message.as_ref() {
                    Some(api::message::Message::UserQuery(_)) => Some(m.id.clone()),
                    _ => None,
                })
        })
        .unwrap_or_else(|| format!("compaction-trigger-{}", uuid::Uuid::new_v4()));

    let tool_names = build_tool_name_lookup(all_msgs.iter().copied());
    let state_snapshot = conversation.compaction_state.clone();
    let views = project(&all_msgs, &state_snapshot, &tool_names);
    let select_result = select(&views, cfg, ModelLimit::FALLBACK, |slice| {
        slice.iter().map(MessageRef::estimate_size).sum()
    });
    let head_message_ids = all_msgs[..select_result.head_end]
        .iter()
        .map(|m| m.id.clone())
        .collect::<Vec<_>>();
    let auto = overflow;
    let summary_len = summary_text.len();
    let completed = CompletedCompaction {
        user_msg_id: user_msg_id.clone(),
        assistant_msg_id: assistant_id.clone(),
        head_message_ids,
        tail_start_id: select_result.tail_start_id,
        summary_text: Some(summary_text),
        auto,
        overflow,
    };
    log::info!(
        "[byop-compaction] commit: assistant_msg={} user_msg={} summary_len={} auto={} overflow={} head_count={} tail_start={:?}",
        assistant_id,
        user_msg_id,
        summary_len,
        auto,
        overflow,
        completed.head_message_ids.len(),
        completed.tail_start_id,
    );
    conversation.compaction_state.push_completed(completed);
    true
}

/// Automatically run prune before every LLM request — 1:1 aligned opencode `compaction.ts:297-341`.
///
/// Compute the decision (which ToolCallResult 's output should be replaced with placeholders) and write
/// `conversation.compaction_state.markers.tool_output_compacted_at`。
/// The actual replacement occurs when `chat_stream::build_chat_request` is projected (read marker).
///
/// no-op when `cfg.prune == false`.
pub fn prune_now(conversation: &mut AIConversation, cfg: &CompactionConfig) -> usize {
    if !cfg.prune {
        return 0;
    }
    let all_msgs: Vec<&api::Message> = conversation.all_linearized_messages();
    if all_msgs.is_empty() {
        return 0;
    }
    let tool_names = build_tool_name_lookup(all_msgs.iter().copied());
    let state_snapshot = conversation.compaction_state.clone();
    let views = project(&all_msgs, &state_snapshot, &tool_names);
    // Avoid generic derivation ambiguities with trait references
    let views_ref: &[_] = &views;
    let decisions = prune_decisions::<super::message_view::WarpMessageView<'_>>(views_ref);
    if decisions.is_empty() {
        return 0;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let count = decisions.len();
    for (msg_id, _call_id) in decisions {
        // msg_id is the message id of ToolCallResult; mark_tool_compacted will write the timestamp on the marker
        conversation
            .compaction_state
            .mark_tool_compacted(msg_id, now_ms);
    }
    log::info!("[byop-compaction] pruned {count} tool output(s)");
    count
}
