//! `websearch` BYOP tool descriptor.
//!
//! The actual HTTP execution is in `web_runtime::run_websearch` (via the Exa MCP endpoint). This descriptor
//! Provided to genai SDK for sending tool description to upstream LLM (name + description + JSON Schema).
//!
//! ## Do not leave protobuf executor
//!
//! `from_args` always returns `Err`, `result_to_json` always returns `None`. `chat_stream::
//! parse_incoming_tool_call` previously called `web_runtime` directly after hitting the name.
//!
//! Parameter schema is aligned with opencode `websearch.ts:7-22`.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

pub const TOOL_NAME: &str = "websearch";

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Web search query."
            },
            "numResults": {
                "type": "integer",
                "description": "Number of search results to return (default 8).",
                "minimum": 1,
                "maximum": 50
            },
            "livecrawl": {
                "type": "string",
                "enum": ["fallback", "preferred"],
                "description": "Live-crawl mode. 'fallback' (default): use cached content, live-crawl as backup. 'preferred': always live-crawl."
            },
            "type": {
                "type": "string",
                "enum": ["auto", "fast", "deep"],
                "description": "Search type. 'auto' (default, balanced), 'fast' (quick), 'deep' (comprehensive)."
            },
            "contextMaxCharacters": {
                "type": "integer",
                "description": "Cap for the LLM-optimized context string."
            }
        },
        "required": ["query"],
        "additionalProperties": false
    })
}

fn from_args(_args: &str) -> Result<api::message::tool_call::Tool> {
    Err(anyhow!(
        "websearch is intercepted by chat_stream BYOP web tool dispatcher; \
         from_args should never be called"
    ))
}

fn result_to_json(_result: &api::message::tool_call_result::Result) -> Option<Value> {
    None
}

pub static WEBSEARCH: OpenAiTool = OpenAiTool {
    name: TOOL_NAME,
    description: include_str!("../prompts/tool_descriptions/websearch.md"),
    parameters,
    from_args,
    result_to_json,
};
