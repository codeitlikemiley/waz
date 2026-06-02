//! Exa MCP wire protocol (pure logic, no HTTP I/O).
//!
//! Mirror opencode `packages/opencode/src/tool/mcp-exa.ts`:
//! - Endpoint: `https://mcp.exa.ai/mcp` (default anonymous) or with `?exaApiKey=...`
//! - Protocol: JSON-RPC 2.0 POST,`Accept: application/json, text/event-stream`
//! - Response: SSE, progressive scan `data: ` prefix, parse `result.content[0].text`
//!
//! All HTTP calls are in `web_runtime.rs`; this module is only responsible for constructing the request body and parsing the response string.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXA_BASE_URL: &str = "https://mcp.exa.ai/mcp";
pub const SEARCH_TOOL_NAME: &str = "web_search_exa";

/// Spell out the final Exa endpoint URL. When `api_key=Some`, spell key to querystring(percent-encode).
pub fn endpoint_url(api_key: Option<&str>) -> String {
    match api_key {
        Some(k) if !k.trim().is_empty() => {
            let encoded: String = url::form_urlencoded::byte_serialize(k.as_bytes()).collect();
            format!("{EXA_BASE_URL}?exaApiKey={encoded}")
        }
        _ => EXA_BASE_URL.to_owned(),
    }
}

/// `web_search_exa` input parameter (sent directly to Exa).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchArgs {
    pub query: String,
    /// "auto" / "fast" / "deep"
    #[serde(rename = "type")]
    pub search_type: String,
    #[serde(rename = "numResults")]
    pub num_results: u32,
    /// "fallback" / "preferred"
    pub livecrawl: String,
    #[serde(
        rename = "contextMaxCharacters",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_max_characters: Option<u32>,
}

impl SearchArgs {
    /// opencode default value (websearch.ts:54-58).
    pub fn with_defaults(query: String) -> Self {
        Self {
            query,
            search_type: "auto".to_owned(),
            num_results: 8,
            livecrawl: "fallback".to_owned(),
            context_max_characters: None,
        }
    }
}

/// JSON-RPC 2.0 `tools/call` request body. `id` is fixed to 1 (single call, no id distinction required).
pub fn build_request_body(tool_name: &str, args: &SearchArgs) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": args,
        }
    })
}

/// Parse Exa SSE response: scan each line, first `data: ` line JSON parse and then get `result.content[0].text`.
///
/// Return `Ok(Some(text))` = found content; `Ok(None)` = no content (empty result);
/// `Err` = data row exists but JSON parsing failed/structural mismatch.
pub fn parse_sse_body(body: &str) -> Result<Option<String>> {
    let mut last_err: Option<anyhow::Error> = None;
    for line in body.split('\n') {
        let Some(payload) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(payload) {
            Ok(v) => {
                if let Some(text) = extract_first_text(&v) {
                    return Ok(Some(text));
                }
                // data: row parsed but no content, continue to the next item
            }
            Err(e) => {
                last_err = Some(anyhow!("invalid Exa SSE JSON payload: {e}"));
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e).context("no Exa SSE data line yielded usable content");
    }
    Ok(None)
}

fn extract_first_text(v: &Value) -> Option<String> {
    let content = v.get("result")?.get("content")?.as_array()?;
    let first = content.first()?;
    let text = first.get("text")?.as_str()?;
    Some(text.to_owned())
}

#[cfg(test)]
#[path = "exa_tests.rs"]
mod exa_tests;
