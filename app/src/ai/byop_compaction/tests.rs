//! Phase 1 unit testing — covering pure functions (token/overflow/prompt/config/algorithm).
//!
//! Phase 3 (state + message_view) will add e2e integration testing after implementation.

use super::algorithm::{prune_decisions, select, turns, MessageRef, Role, ToolOutputRef};
use super::commit::commit_summarization;
use super::config::CompactionConfig;
use super::consts::*;
use super::overflow::{is_overflow, usable, ModelLimit, TokenCounts};
use super::prompt::{build_continue_message, build_prompt, SUMMARY_TEMPLATE};
use super::token::estimate;
use crate::ai::agent::conversation::{AIConversation, AIConversationId};
use warp_multi_agent_api as api;

// -- token ---------------------------------------------------------------

#[test]
fn token_estimate_empty() {
    assert_eq!(estimate(""), 0);
}

#[test]
fn token_estimate_short() {
    // "hello world" = 11 chars → round(11/4) = 3
    assert_eq!(estimate("hello world"), 3);
}

#[test]
fn token_estimate_aligned() {
    assert_eq!(estimate(&"a".repeat(40)), 10);
    assert_eq!(estimate(&"a".repeat(41)), 10); // 41/4 = 10.25 → 10 (banker's rounding does not affect)
    assert_eq!(estimate(&"a".repeat(42)), 11); // 42/4 = 10.5 → 11
}

// -- overflow ------------------------------------------------------------

fn cfg_default() -> CompactionConfig {
    CompactionConfig::default()
}

#[test]
fn usable_with_input_limit() {
    let cfg = cfg_default();
    let model = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    // reserved = min(20_000, 8_000) = 8_000
    // usable = max(0, 180_000 - 8_000) = 172_000
    assert_eq!(usable(&cfg, model), 172_000);
}

#[test]
fn usable_without_input_limit() {
    let cfg = cfg_default();
    let model = ModelLimit {
        context: 200_000,
        input: 0,
        max_output: 8_000,
    };
    // Take the second branch: context - max_output = 192_000
    assert_eq!(usable(&cfg, model), 192_000);
}

#[test]
fn usable_zero_context() {
    let cfg = cfg_default();
    let model = ModelLimit {
        context: 0,
        input: 0,
        max_output: 0,
    };
    assert_eq!(usable(&cfg, model), 0);
}

#[test]
fn usable_respects_cfg_reserved_override() {
    let mut cfg = cfg_default();
    cfg.reserved = Some(50_000);
    let model = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    // reserved coverage is 50_000 → 180_000 - 50_000 = 130_000
    assert_eq!(usable(&cfg, model), 130_000);
}

#[test]
fn is_overflow_auto_off() {
    let mut cfg = cfg_default();
    cfg.auto = false;
    let model = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    let tokens = TokenCounts {
        total: 999_999,
        ..Default::default()
    };
    assert!(!is_overflow(&cfg, tokens, model));
}

#[test]
fn is_overflow_at_threshold() {
    let cfg = cfg_default();
    let model = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    let usable_n = usable(&cfg, model);
    let tokens = TokenCounts {
        total: usable_n,
        ..Default::default()
    };
    assert!(is_overflow(&cfg, tokens, model));
    let tokens_below = TokenCounts {
        total: usable_n - 1,
        ..Default::default()
    };
    assert!(!is_overflow(&cfg, tokens_below, model));
}

#[test]
fn token_counts_count_uses_total_when_present() {
    let t = TokenCounts {
        total: 100,
        input: 50,
        output: 60,
        cache_read: 10,
        cache_write: 5,
    };
    assert_eq!(t.count(), 100); // total takes precedence
}

#[test]
fn token_counts_count_sums_when_total_zero() {
    let t = TokenCounts {
        total: 0,
        input: 50,
        output: 60,
        cache_read: 10,
        cache_write: 5,
    };
    assert_eq!(t.count(), 125);
}

// -- preserve_recent_budget ----------------------------------------------

#[test]
fn preserve_recent_budget_default_formula() {
    let cfg = cfg_default();
    // usable=80_000 → 80_000/4 = 20_000 → max(2_000, 20_000)=20_000 → min(8_000, 20_000) = 8_000
    assert_eq!(
        cfg.preserve_recent_budget(80_000),
        MAX_PRESERVE_RECENT_TOKENS
    );
    // usable=4_000 → 1_000 → max(2_000, 1_000)=2_000 → min(8_000, 2_000)=2_000
    assert_eq!(
        cfg.preserve_recent_budget(4_000),
        MIN_PRESERVE_RECENT_TOKENS
    );
    // usable=20_000 → 5_000 → max(2_000, 5_000)=5_000 → min(8_000, 5_000)=5_000
    assert_eq!(cfg.preserve_recent_budget(20_000), 5_000);
}

#[test]
fn preserve_recent_budget_override() {
    let mut cfg = cfg_default();
    cfg.preserve_recent_tokens = Some(12_345);
    assert_eq!(cfg.preserve_recent_budget(80_000), 12_345);
}

// -- prompt --------------------------------------------------------------

#[test]
fn summary_template_contains_all_sections() {
    let must = [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "### Done",
        "### In Progress",
        "### Blocked",
        "## Key Decisions",
        "## Next Steps",
        "## Critical Context",
        "## Relevant Files",
        "Rules:",
        "<template>",
        "</template>",
    ];
    for m in must {
        assert!(SUMMARY_TEMPLATE.contains(m), "missing section: {m}");
    }
}

#[test]
fn build_prompt_no_previous() {
    let s = build_prompt(None, &[]);
    assert!(s.starts_with("Create a new anchored summary from the conversation history above."));
    assert!(s.contains(SUMMARY_TEMPLATE));
}

#[test]
fn build_prompt_with_previous() {
    let s = build_prompt(Some("OLD-SUMMARY"), &[]);
    assert!(s.starts_with("Update the anchored summary below"));
    assert!(s.contains("<previous-summary>\nOLD-SUMMARY\n</previous-summary>"));
    assert!(s.contains(SUMMARY_TEMPLATE));
}

#[test]
fn build_prompt_with_plugin_context() {
    let ctx = vec!["EXTRA1".to_string(), "EXTRA2".to_string()];
    let s = build_prompt(None, &ctx);
    assert!(s.contains("EXTRA1"));
    assert!(s.contains("EXTRA2"));
}

#[test]
fn continue_message_overflow_branch() {
    let s = build_continue_message(true);
    assert!(s.contains("previous request exceeded"));
    assert!(s.contains("Continue if you have next steps"));
}

#[test]
fn continue_message_normal_branch() {
    let s = build_continue_message(false);
    assert!(!s.contains("previous request exceeded"));
    assert!(s.starts_with("Continue if you have next steps"));
}

// -- algorithm: turns / select / prune ----------------------------------

/// Testing is implemented using mock messages.
#[derive(Debug, Clone)]
struct M {
    id: u32,
    role: Role,
    /// Whether the user message is marked with compaction
    is_compaction: bool,
    /// assistant whether the message is a summary
    is_summary: bool,
    size: usize,
    tools: Vec<ToolOutputRef<u32>>,
}

impl M {
    fn user(id: u32, size: usize) -> Self {
        Self {
            id,
            role: Role::User,
            is_compaction: false,
            is_summary: false,
            size,
            tools: vec![],
        }
    }
    fn user_compaction(id: u32) -> Self {
        Self {
            id,
            role: Role::User,
            is_compaction: true,
            is_summary: false,
            size: 0,
            tools: vec![],
        }
    }
    fn assistant(id: u32, size: usize) -> Self {
        Self {
            id,
            role: Role::Assistant,
            is_compaction: false,
            is_summary: false,
            size,
            tools: vec![],
        }
    }
    fn summary(id: u32) -> Self {
        Self {
            id,
            role: Role::Assistant,
            is_compaction: false,
            is_summary: true,
            size: 100,
            tools: vec![],
        }
    }
    fn assistant_with_tools(id: u32, size: usize, tools: Vec<ToolOutputRef<u32>>) -> Self {
        Self {
            id,
            role: Role::Assistant,
            is_compaction: false,
            is_summary: false,
            size,
            tools,
        }
    }
}

impl MessageRef for M {
    type Id = u32;
    type CallId = u32;
    fn id(&self) -> u32 {
        self.id
    }
    fn role(&self) -> Role {
        self.role
    }
    fn is_compaction_marker(&self) -> bool {
        self.is_compaction
    }
    fn is_summary(&self) -> bool {
        self.is_summary
    }
    fn estimate_size(&self) -> usize {
        self.size
    }
    fn tool_outputs(&self) -> Vec<ToolOutputRef<u32>> {
        self.tools.clone()
    }
}

fn sum_size(slice: &[M]) -> usize {
    slice.iter().map(|m| m.size).sum()
}

#[test]
fn turns_basic() {
    let msgs = vec![
        M::user(1, 10),
        M::assistant(2, 20),
        M::user(3, 10),
        M::assistant(4, 30),
        M::user(5, 10),
    ];
    let t = turns(&msgs);
    assert_eq!(t.len(), 3);
    assert_eq!(t[0].start, 0);
    assert_eq!(t[0].end, 2);
    assert_eq!(t[1].start, 2);
    assert_eq!(t[1].end, 4);
    assert_eq!(t[2].start, 4);
    assert_eq!(t[2].end, 5);
}

#[test]
fn turns_skips_compaction_marker() {
    let msgs = vec![
        M::user(1, 10),
        M::assistant(2, 20),
        M::user_compaction(99), // Not counting turn
        M::assistant(3, 30),
        M::user(4, 10),
    ];
    let t = turns(&msgs);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].id, 1);
    assert_eq!(t[1].id, 4);
}

#[test]
fn turns_empty() {
    let msgs: Vec<M> = vec![];
    assert_eq!(turns(&msgs).len(), 0);
}

#[test]
fn select_keeps_recent_turns_within_budget() {
    let msgs = vec![
        M::user(1, 100),
        M::assistant(2, 100), // turn1 size 200
        M::user(3, 100),
        M::assistant(4, 100), // turn2 size 200
        M::user(5, 100),
        M::assistant(6, 100), // turn3 size 200
    ];
    let cfg = CompactionConfig {
        tail_turns: 2,
        preserve_recent_tokens: Some(500), // Enough to hold the last 2 turns (200 each)
        ..Default::default()
    };
    let model = ModelLimit::FALLBACK;
    let r = select(&msgs, &cfg, model, sum_size);
    // The starting point of tail is user(idx=2) of the 2nd turn, head_end=2
    assert_eq!(r.head_end, 2);
    assert_eq!(r.tail_start_id, Some(3));
}

#[test]
fn select_split_turn_when_over_budget() {
    let msgs = vec![
        M::user(1, 100),
        M::user(2, 100), // turn 2 contains 5 messages totaling 500
        M::assistant(3, 100),
        M::assistant(4, 100),
        M::assistant(5, 100),
        M::assistant(6, 100),
    ];
    let cfg = CompactionConfig {
        tail_turns: 1,
        preserve_recent_tokens: Some(250), // Can't fit turn2 whole (500), trigger splitTurn
        ..Default::default()
    };
    let model = ModelLimit::FALLBACK;
    let r = select(&msgs, &cfg, model, sum_size);
    // splitTurn starts from turn2.start+1=2, messages[2..6]=400 > 250, [3..6]=300>250, [4..6]=200<=250 → start=4
    assert_eq!(r.head_end, 4);
    assert_eq!(r.tail_start_id, Some(5));
}

#[test]
fn select_returns_full_when_no_turns() {
    let msgs: Vec<M> = vec![];
    let cfg = CompactionConfig::default();
    let r = select(&msgs, &cfg, ModelLimit::FALLBACK, sum_size);
    assert_eq!(r.head_end, 0);
    assert_eq!(r.tail_start_id, None);
}

#[test]
fn select_tail_turns_zero_keeps_full() {
    let msgs = vec![M::user(1, 100), M::assistant(2, 100)];
    let cfg = CompactionConfig {
        tail_turns: 0,
        ..Default::default()
    };
    let r = select(&msgs, &cfg, ModelLimit::FALLBACK, sum_size);
    assert_eq!(r.head_end, msgs.len());
    assert_eq!(r.tail_start_id, None);
}

// -- prune ---------------------------------------------------------------

fn tool_output(call_id: u32, name: &str, size: usize) -> ToolOutputRef<u32> {
    ToolOutputRef {
        call_id,
        tool_name: name.to_string(),
        output_size: size,
        completed: true,
        already_compacted: false,
    }
}

#[test]
fn prune_below_minimum_returns_empty() {
    // There is only a small amount of tool output, less than PRUNE_MINIMUM(20_000)
    let msgs = vec![
        M::user(1, 10),
        M::assistant_with_tools(2, 0, vec![tool_output(101, "bash", 5_000)]),
        M::user(3, 10),
        M::assistant_with_tools(4, 0, vec![tool_output(102, "bash", 5_000)]),
        M::user(5, 10),
    ];
    let r = prune_decisions(&msgs);
    assert_eq!(r.len(), 0);
}

#[test]
fn prune_skips_protected_skill_tool() {
    // Big skill tool + big bash tool; skill is protected and never enters prune, bash does not cut in PRUNE_PROTECT
    let msgs = vec![
        M::user(1, 10),
        M::assistant_with_tools(
            2,
            0,
            vec![
                tool_output(101, "skill", 50_000), // skip
                tool_output(102, "bash", 30_000),
            ],
        ),
        M::user(3, 10),
        M::assistant_with_tools(4, 0, vec![tool_output(103, "bash", 30_000)]),
        M::user(5, 10),
    ];
    let r = prune_decisions(&msgs);
    // Only cut the part beyond PRUNE_PROTECT(40_000), and the total cutting amount > PRUNE_MINIMUM(20_000)
    // The last 2 user turns are not moving: turn5..end / turn3..turn5 are all retained.
    // The first turn here contains bash 30_000 (super PROTECT), so it should be cut
    // total: 30_000 (bash 102) + 30_000 (bash 103, but protected skip in turn3)...
    // Note: user_turns_seen changes to 1 when user(5) is encountered and to 2 when user(3) is encountered. Continue to read earlier messages.
    // Does the tools of assistant(4) continue in turn3 when user_turns_seen=2?
    //   Loop: idx=4 user(5), seen=1 → continue
    //         idx=3 assistant(4), seen=1 → continue
    //         idx=2 user(3), seen=2 → Enter processing
    //         idx=1 assistant(2), seen=2 → processing tools(skill skip; bash 30_000 → total=30_000 > PROTECT(40_000)? no, 30<40 → continue)
    //         idx=0 user(1), seen=3 → processing(no tools)
    // total=30_000,pruned=0,less than PRUNE_MINIMUM → return empty
    assert_eq!(r.len(), 0);
}

#[test]
fn prune_reaches_minimum_returns_decisions() {
    // Construct a tool output large enough to trigger prune
    let big_tool = |id: u32| tool_output(id, "bash", 25_000);
    let msgs = vec![
        M::user(1, 10),
        M::assistant_with_tools(2, 0, vec![big_tool(101), big_tool(102), big_tool(103)]),
        M::user(3, 10),
        M::assistant(4, 0),
        M::user(5, 10),
    ];
    let r = prune_decisions(&msgs);
    // Traverse in reverse order:
    //  idx=4 user(5) seen=1 continue
    //  idx=3 assistant(4) seen=1 continue
    //  idx=2 user(3) seen=2 continue (no tools)
    //  idx=1 assistant(2) seen=2,tools reverse order:103 → total=25_000 < 40_000 continue;
    //                                        102 → total=50_000 > 40_000 → pruned=25_000, push (2,102)
    //                                        101 → total=75_000 > 40_000 → pruned=50_000, push (2,101)
    //  idx=0 user(1) seen=3 continue
    // pruned=50_000 > PRUNE_MINIMUM(20_000) → return [(2,102),(2,101)]
    assert_eq!(r.len(), 2);
    assert!(r.contains(&(2, 102)));
    assert!(r.contains(&(2, 101)));
}

#[test]
fn prune_stops_at_summary_boundary() {
    let big_tool = |id: u32| tool_output(id, "bash", 50_000);
    let msgs = vec![
        M::user(1, 10),
        M::assistant_with_tools(2, 0, vec![big_tool(101)]),
        M::summary(3), // boundary
        M::user(4, 10),
        M::assistant(5, 0),
        M::user(6, 10),
    ];
    let r = prune_decisions(&msgs);
    // When seen=2, break is encountered when summary(3) is encountered, and big_tool with idx=1 will not be processed.
    assert_eq!(r.len(), 0);
}

#[test]
fn prune_stops_at_already_compacted() {
    let mut already = tool_output(101, "bash", 50_000);
    already.already_compacted = true;
    let msgs = vec![
        M::user(1, 10),
        M::assistant_with_tools(2, 0, vec![already, tool_output(102, "bash", 50_000)]),
        M::user(3, 10),
        M::assistant(4, 0),
        M::user(5, 10),
    ];
    // Reverse order tools:102 size=50_000 → total=50_000 > 40_000 → pruned=50_000, push (2,102)
    //              101 already_compacted → break outer
    let r = prune_decisions(&msgs);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0], (2, 102));
}

// -- commit --------------------------------------------------------------

fn ts(seconds: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds, nanos: 0 }
}

fn user_query(id: &str, task_id: &str, request_id: &str, seconds: i64) -> api::Message {
    api::Message {
        id: id.to_string(),
        task_id: task_id.to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: format!("query-{id}"),
            context: None,
            mode: None,
            referenced_attachments: Default::default(),
            intended_agent: Default::default(),
        })),
        request_id: request_id.to_string(),
        timestamp: Some(ts(seconds)),
    }
}

fn agent_output(id: &str, task_id: &str, request_id: &str, seconds: i64) -> api::Message {
    api::Message {
        id: id.to_string(),
        task_id: task_id.to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: format!("output-{id}"),
            },
        )),
        request_id: request_id.to_string(),
        timestamp: Some(ts(seconds)),
    }
}

fn conversation_with_messages(messages: Vec<api::Message>) -> AIConversation {
    let task = api::Task {
        id: "root".to_string(),
        messages,
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    };
    AIConversation::new_restored(AIConversationId::new(), vec![task], None).unwrap()
}

#[test]
fn commit_summarization_records_head_message_ids() {
    let mut conversation = conversation_with_messages(vec![
        user_query("u1", "root", "r1", 1),
        agent_output("a1", "root", "r1", 2),
        user_query("u2", "root", "r2", 3),
        agent_output("a2", "root", "r2", 4),
        user_query("u3", "root", "r3", 5),
        agent_output("a3", "root", "r3", 6),
    ]);
    let cfg = CompactionConfig {
        tail_turns: 1,
        preserve_recent_tokens: Some(1_000),
        ..Default::default()
    };

    assert!(commit_summarization(&mut conversation, false, &cfg));
    let completed = conversation.compaction_state.completed().last().unwrap();
    assert_eq!(completed.user_msg_id, "u3");
    assert_eq!(completed.assistant_msg_id, "a3");
    assert_eq!(completed.tail_start_id.as_deref(), Some("u3"));
    assert_eq!(completed.head_message_ids, ["u1", "a1", "u2", "a2"]);
}
