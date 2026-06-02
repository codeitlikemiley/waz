//! `mcp.rs` unit tests.
//!
//! Override P0-3 prompt cache optimization: `build_mcp_tool_defs` must be **lexicographically stable**,
//! Calling the same `MCPContext` multiple times across requests will produce a byte-equal tools list, otherwise
//! Anthropic will determine if the tools field has changed → all cache layers will be invalid.
//!
//! Note: `rmcp::model::Tool` and `rmcp::model::Resource`(= `Annotated<RawResource>`)
//! From the upstream vendor crate, only its public construction path (`Tool::new` / `RawResource::new`) is used here.

use rmcp::model::{AnnotateAble, RawResource, Tool};
use serde_json::json;
use std::sync::Arc;

use crate::ai::agent::{MCPContext, MCPServer};

use super::{build_mcp_tool_defs, function_name};

/// Construct a `rmcp::model::Tool` with minimal input schema.
fn mk_tool(name: &'static str, desc: &'static str) -> Tool {
    let schema: serde_json::Map<String, serde_json::Value> = json!({
        "type": "object",
        "properties": {
            "x": { "type": "string" }
        }
    })
    .as_object()
    .unwrap()
    .clone();
    // `Tool::new` accepts Arc<JsonObject>, where Map is passed directly (Into<Arc<JsonObject>> is implemented).
    Tool::new(name, desc, Arc::new(schema))
}

/// Construct MCPServer. The order of tools and resources are retained as input parameters (simulating upstream
/// Possible out-of-order input in HashMap iterate order).
fn mk_server(
    id: &str,
    name: &str,
    tools: Vec<Tool>,
    resources: Vec<rmcp::model::Resource>,
) -> MCPServer {
    MCPServer {
        id: id.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        resources,
        tools,
    }
}

fn mk_resource(uri: &str, name: &str) -> rmcp::model::Resource {
    // RawResource → Annotated<RawResource> (without annotation).
    // The safe conversion entry provided by upstream is `AnnotateAble::no_annotation`.
    RawResource::new(uri, name).no_annotation()
}

/// The same ctx, built twice, produces (name, description, schema) triples that must be byte-equal.
/// This is the lowest threshold for prompt cache hits - as long as it is unstable, all Anthropic caches will be invalid.
#[test]
fn build_mcp_tool_defs_is_stable_across_calls() {
    let ctx = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![
            mk_server(
                "id-b",
                "server-b",
                vec![mk_tool("zeta", "z"), mk_tool("alpha", "a")],
                vec![],
            ),
            mk_server(
                "id-a",
                "server-a",
                vec![mk_tool("beta", "b"), mk_tool("gamma", "g")],
                vec![],
            ),
        ],
    };
    let r1 = build_mcp_tool_defs(&ctx);
    let r2 = build_mcp_tool_defs(&ctx);
    assert_eq!(r1, r2, "build_mcp_tool_defs 必须确定性产出");
}

/// When the input server/tool ​​is out of order, the output is sorted lexicographically by function_name.
/// This is the core assertion of P0-3: if the order of upstream ctx.servers is different across requests (HashMap iterate
/// etc.), the output is still byte-equal.
#[test]
fn build_mcp_tool_defs_outputs_lexicographic_order() {
    let ctx = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![
            mk_server(
                "id-b",
                "server-b",
                // Out of order: zeta before alpha
                vec![mk_tool("zeta", "z"), mk_tool("alpha", "a")],
                vec![],
            ),
            mk_server(
                "id-a",
                "server-a",
                vec![mk_tool("beta", "b"), mk_tool("gamma", "g")],
                vec![],
            ),
        ],
    };
    let out = build_mcp_tool_defs(&ctx);
    let names: Vec<&str> = out.iter().map(|(n, _, _)| n.as_str()).collect();
    // After sorting by function_name: server-a/beta < server-a/gamma < server-b/alpha < server-b/zeta
    let expected = [
        function_name(&mk_server("id-a", "server-a", vec![], vec![]), "beta"),
        function_name(&mk_server("id-a", "server-a", vec![], vec![]), "gamma"),
        function_name(&mk_server("id-b", "server-b", vec![], vec![]), "alpha"),
        function_name(&mk_server("id-b", "server-b", vec![], vec![]), "zeta"),
    ];
    assert_eq!(
        names,
        expected.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    );
}

/// The order of input servers across requests is different (simulating HashMap rearrangement) and the output is still byte-equal.
#[test]
fn build_mcp_tool_defs_invariant_under_servers_permutation() {
    let server_a = mk_server(
        "id-a",
        "server-a",
        vec![mk_tool("beta", "b"), mk_tool("gamma", "g")],
        vec![],
    );
    let server_b = mk_server(
        "id-b",
        "server-b",
        vec![mk_tool("zeta", "z"), mk_tool("alpha", "a")],
        vec![],
    );
    let ctx1 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_a.clone(), server_b.clone()],
    };
    let ctx2 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_b, server_a],
    };
    assert_eq!(build_mcp_tool_defs(&ctx1), build_mcp_tool_defs(&ctx2));
}

/// When any server exposes resources, read_resource description available_uris
/// It must also be lexicographically stable, and read_resource is always at the end of the array.
#[test]
fn read_resource_description_is_stable_and_sorted() {
    let ctx1 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![mk_server(
            "id-a",
            "srv",
            vec![mk_tool("t", "")],
            vec![
                mk_resource("file:///z.txt", "Z"),
                mk_resource("file:///a.txt", "A"),
            ],
        )],
    };
    // Same as ctx but the order of resources is changed.
    let ctx2 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![mk_server(
            "id-a",
            "srv",
            vec![mk_tool("t", "")],
            vec![
                mk_resource("file:///a.txt", "A"),
                mk_resource("file:///z.txt", "Z"),
            ],
        )],
    };
    let r1 = build_mcp_tool_defs(&ctx1);
    let r2 = build_mcp_tool_defs(&ctx2);
    assert_eq!(r1, r2, "read_resource 描述必须 byte-equal");

    let last = r1.last().expect("应至少含 read_resource");
    assert_eq!(last.0, "mcp_read_resource");
    // After sorting, a.txt is before z.txt
    let pos_a = last.1.find("a.txt").expect("应含 a.txt");
    let pos_z = last.1.find("z.txt").expect("应含 z.txt");
    assert!(pos_a < pos_z, "available_uris 必须按字典序排");
}
