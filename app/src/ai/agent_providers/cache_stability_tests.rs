//! Prompt cache serialization stability test suite (corresponding to documents P1-8 / P1-9 / P1-13).
//!
//! The Anthropic documentation explicitly warns:
//! > Verify that the keys in your `tool_use` content blocks have stable
//! > ordering as some languages (for example, Swift, Go) randomize key order
//! > during JSON conversion, breaking caches
//!
//! This means any `serde_json::Value` output on the Rust side **must**:
//!   1. Be byte-equal across calls for the same input (deterministic)
//!   2. Not depend on `HashMap` iteration order
//!   3. Not depend on external state (timestamps, randomness, PID, etc.)
//!
//! This test suite serves as Waz's "anti-regression guardrail" — if any subsequent changes to the prompt
//! construction path break byte-level stability, the assertions here will fail.

use crate::ai::agent::{MCPContext, MCPServer};
use api::message;
use warp_multi_agent_api as api;

use super::chat_stream;
use super::tools;

// ---------------------------------------------------------------------------
// P1-8: Tool schema field order stability
// ---------------------------------------------------------------------------

/// Calls `(parameters)()` twice for each tool in `REGISTRY` and asserts they are byte-equal.
///
/// Risk: If `HashMap<String, Schema>` is used internally in tool schemas containing nested enums / oneof
/// when converting to Value, the order becomes random. The `serde_json::Map` produced by the `json!({...})` macro
/// preserves the **insertion order** by default (the `preserve_order` feature is enabled in Cargo.toml), meaning
/// the hardcoded key order is stable across calls. This test protects this invariant.
#[test]
fn registry_tool_schemas_are_deterministic() {
    for tool in tools::REGISTRY {
        let s1 = (tool.parameters)();
        let s2 = (tool.parameters)();
        let j1 = serde_json::to_string(&s1).unwrap();
        let j2 = serde_json::to_string(&s2).unwrap();
        assert_eq!(
            j1, j2,
            "tool `{}` schema must be byte-equal across calls (prompt cache hit prerequisite)",
            tool.name
        );
    }
}

/// Repeatedly calls each tool in `REGISTRY` 50 times and asserts that all outputs are byte-equal.
/// Prevents occasional `HashMap` iteration order drift (running only twice might coincidentally match).
#[test]
fn registry_tool_schemas_stable_under_repetition() {
    for tool in tools::REGISTRY {
        let baseline = serde_json::to_string(&(tool.parameters)()).unwrap();
        for i in 0..50 {
            let candidate = serde_json::to_string(&(tool.parameters)()).unwrap();
            assert_eq!(
                baseline, candidate,
                "tool `{}` call {i} output differs from baseline (possible HashMap order drift)",
                tool.name
            );
        }
    }
}

/// `tools::REGISTRY` order is static, but we still verify it:
/// Iterating multiple times within the same process yields the same (name, description) sequence.
#[test]
fn registry_iteration_order_is_stable() {
    let names1: Vec<&str> = tools::REGISTRY.iter().map(|t| t.name).collect();
    let names2: Vec<&str> = tools::REGISTRY.iter().map(|t| t.name).collect();
    assert_eq!(names1, names2);
}

// ---------------------------------------------------------------------------
// P1-9: serialize_outgoing_tool_call history playback stability
// ---------------------------------------------------------------------------

/// Simulates a Grep tool call and verifies that the two serialized outputs are byte-equal.
/// `serialize_outgoing_tool_call` is re-run on every `build_chat_request` to convert
/// the historical ToolCalls into (name, args Value). Any instability related to HashMap or time
/// would invalidate the cache for the latter half of the messages segment.
///
/// Grep is chosen because its fields are simple (`queries: Vec<String>`, `path: String`),
/// and it does not rely on any implicit default fields in prost.
#[test]
fn serialize_grep_tool_call_is_deterministic() {
    let tc = message::ToolCall {
        tool_call_id: "call-grep-1".to_owned(),
        tool: Some(message::tool_call::Tool::Grep(message::tool_call::Grep {
            queries: vec!["fn main".to_owned(), "Result<".to_owned()],
            path: "src/".to_owned(),
        })),
    };

    let (n1, v1) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    let (n2, v2) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    assert_eq!(n1, n2, "tool name must be consistent");
    let j1 = serde_json::to_string(&v1).unwrap();
    let j2 = serde_json::to_string(&v2).unwrap();
    assert_eq!(j1, j2, "same ToolCall must be byte-equal across serialization");
}

/// Grep `queries` is a `Vec<String>`; the order must be stable (Vec is naturally stable, but this is a defensive assertion).
/// This reflects a larger rule: any Vec fields within a user ToolCall must preserve the input order.
#[test]
fn serialize_grep_preserves_queries_order() {
    let tc = message::ToolCall {
        tool_call_id: "call-grep-2".to_owned(),
        tool: Some(message::tool_call::Tool::Grep(message::tool_call::Grep {
            queries: vec!["zzz".to_owned(), "aaa".to_owned()],
            path: ".".to_owned(),
        })),
    };
    let (_, v) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    let s = serde_json::to_string(&v).unwrap();
    let pos_z = s.find("zzz").expect("queries should contain zzz");
    let pos_a = s.find("aaa").expect("queries should contain aaa");
    assert!(pos_z < pos_a, "Vec order must be preserved according to input (zzz first, aaa second)");
}

/// MCP tool call contains `prost_types::Struct`, verifying serialization stability.
/// `prost_types::Struct.fields` uses `BTreeMap` internally, which is inherently stable; this is a safety coverage check.
#[test]
fn serialize_mcp_tool_call_is_deterministic() {
    use prost_types::{value::Kind, Struct, Value as ProstValue};
    use std::collections::BTreeMap;

    let mut fields = BTreeMap::new();
    fields.insert(
        "key_z".to_owned(),
        ProstValue {
            kind: Some(Kind::StringValue("v_z".to_owned())),
        },
    );
    fields.insert(
        "key_a".to_owned(),
        ProstValue {
            kind: Some(Kind::NumberValue(42.0)),
        },
    );

    let server_id = "srv-uuid-1".to_owned();
    let tc = message::ToolCall {
        tool_call_id: "call-mcp-1".to_owned(),
        tool: Some(message::tool_call::Tool::CallMcpTool(
            message::tool_call::CallMcpTool {
                name: "echo".to_owned(),
                args: Some(Struct { fields }),
                server_id: server_id.clone(),
            },
        )),
    };

    // Construct an mcp_context so sanitize_server_name can look up the server name
    let ctx = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![MCPServer {
            id: server_id.clone(),
            name: "my-server".to_owned(),
            description: String::new(),
            resources: vec![],
            tools: vec![],
        }],
    };

    let (n1, v1) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, Some(&ctx), "");
    let (n2, v2) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, Some(&ctx), "");
    assert_eq!(n1, n2);
    let j1 = serde_json::to_string(&v1).unwrap();
    let j2 = serde_json::to_string(&v2).unwrap();
    assert_eq!(j1, j2);
    // BTreeMap should output in lexicographical order of keys (key_a before key_z)
    let pos_a = j1.find("key_a").expect("should contain key_a");
    let pos_z = j1.find("key_z").expect("should contain key_z");
    assert!(
        pos_a < pos_z,
        "prost_types::Struct should be sorted lexicographically by BTreeMap keys"
    );
}

// ---------------------------------------------------------------------------
// P1-13: Overall stability of build_tools_array (cooperating with MCP sorting in P0-3)
// ---------------------------------------------------------------------------

/// End-to-end assertion: Running tools array concatenation twice with the same `(REGISTRY + same mcp_context)`
/// yields byte-equal strings. This covers the critical stability constraint of the tools array in prompts
/// (Anthropic docs: tool definitions change → all cache invalidated).
///
/// We don't call `build_tools_array(params: &RequestParams)` directly because `RequestParams`
/// has too many fields and is hard to construct; this replicates its core concatenation logic for REGISTRY and MCP.
#[test]
fn full_tools_array_serialization_is_stable() {
    let assemble = || -> String {
        let mut buf = String::new();
        // Built-in tools (REGISTRY iteration order is static)
        for t in tools::REGISTRY {
            buf.push_str(t.name);
            buf.push('|');
            buf.push_str(t.description);
            buf.push('|');
            let schema = (t.parameters)();
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        // MCP tools (already sorted in build_mcp_tool_defs, empty when no ctx is present)
        buf
    };
    let a = assemble();
    let b = assemble();
    assert_eq!(a.len(), b.len());
    assert_eq!(a, b, "tools array serialization result must be byte-equal across calls");
}

/// End-to-end concatenation stability with MCP server (integrating with P0-3 sorting guarantee).
#[test]
fn full_tools_array_with_mcp_is_stable() {
    use rmcp::model::{AnnotateAble, RawResource, Tool as McpTool};
    use serde_json::json;
    use std::sync::Arc;

    let schema_obj = json!({
        "type": "object",
        "properties": { "x": { "type": "string" } }
    })
    .as_object()
    .unwrap()
    .clone();

    let server_a = MCPServer {
        id: "id-a".to_owned(),
        name: "server-a".to_owned(),
        description: String::new(),
        resources: vec![RawResource::new("file:///x.txt", "X").no_annotation()],
        tools: vec![
            McpTool::new("zeta", "Z desc", Arc::new(schema_obj.clone())),
            McpTool::new("alpha", "A desc", Arc::new(schema_obj.clone())),
        ],
    };
    let ctx1 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_a.clone()],
    };
    // Reconstruct once with the same ctx (servers Vec order matches):
    let ctx2 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_a],
    };

    let assemble = |ctx: &MCPContext| -> String {
        let mut buf = String::new();
        for t in tools::REGISTRY {
            buf.push_str(t.name);
            buf.push('|');
            buf.push_str(t.description);
            buf.push('|');
            let schema = (t.parameters)();
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        for (name, desc, schema) in tools::mcp::build_mcp_tool_defs(ctx) {
            buf.push_str(&name);
            buf.push('|');
            buf.push_str(&desc);
            buf.push('|');
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        buf
    };

    let a = assemble(&ctx1);
    let b = assemble(&ctx2);
    assert_eq!(a, b, "tools array containing MCP must be byte-equal across calls");
    // Verify that MCP tools are in function_name lexicographical order (alpha before zeta)
    let pos_alpha = a.find("mcp__server-a__alpha").expect("should contain alpha");
    let pos_zeta = a.find("mcp__server-a__zeta").expect("should contain zeta");
    assert!(pos_alpha < pos_zeta, "P0-3 sorting guarantees alpha < zeta");
}
