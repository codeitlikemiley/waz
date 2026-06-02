//! `exa.rs` Pure protocol logic unit test (no HTTP).

use super::*;

// ---------------------------------------------------------------------------
// URL builder
// ---------------------------------------------------------------------------

#[test]
fn endpoint_url_anonymous() {
    assert_eq!(endpoint_url(None), "https://mcp.exa.ai/mcp");
    assert_eq!(endpoint_url(Some("")), "https://mcp.exa.ai/mcp");
    assert_eq!(endpoint_url(Some("   ")), "https://mcp.exa.ai/mcp");
}

#[test]
fn endpoint_url_with_simple_key() {
    let url = endpoint_url(Some("abc123"));
    assert_eq!(url, "https://mcp.exa.ai/mcp?exaApiKey=abc123");
}

#[test]
fn endpoint_url_percent_encodes_special_chars() {
    // key contains dangerous characters such as + / = in querystring
    let url = endpoint_url(Some("a+b/c=d&e"));
    assert!(url.starts_with("https://mcp.exa.ai/mcp?exaApiKey="));
    assert!(url.contains("%2B"), "+ should be encoded: {url}");
    assert!(url.contains("%2F"), "/ should be encoded: {url}");
    assert!(url.contains("%3D"), "= should be encoded: {url}");
    assert!(url.contains("%26"), "& should be encoded: {url}");
}

// ---------------------------------------------------------------------------
// Request body shape
// ---------------------------------------------------------------------------

#[test]
fn request_body_has_jsonrpc_envelope() {
    let args = SearchArgs::with_defaults("rust async".to_owned());
    let body = build_request_body(SEARCH_TOOL_NAME, &args);

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert_eq!(body["method"], "tools/call");
    assert_eq!(body["params"]["name"], "web_search_exa");
}

#[test]
fn request_body_default_args_match_opencode() {
    let args = SearchArgs::with_defaults("hello".to_owned());
    let body = build_request_body(SEARCH_TOOL_NAME, &args);
    let a = &body["params"]["arguments"];

    assert_eq!(a["query"], "hello");
    assert_eq!(a["type"], "auto");
    assert_eq!(a["numResults"], 8);
    assert_eq!(a["livecrawl"], "fallback");
    // contextMaxCharacters should not be serialized by default
    assert!(
        a.get("contextMaxCharacters").is_none(),
        "contextMaxCharacters should be skipped when None"
    );
}

#[test]
fn request_body_full_args_passthrough() {
    let args = SearchArgs {
        query: "deep research".to_owned(),
        search_type: "deep".to_owned(),
        num_results: 20,
        livecrawl: "preferred".to_owned(),
        context_max_characters: Some(15000),
    };
    let body = build_request_body(SEARCH_TOOL_NAME, &args);
    let a = &body["params"]["arguments"];

    assert_eq!(a["query"], "deep research");
    assert_eq!(a["type"], "deep");
    assert_eq!(a["numResults"], 20);
    assert_eq!(a["livecrawl"], "preferred");
    assert_eq!(a["contextMaxCharacters"], 15000);
}

// ---------------------------------------------------------------------------
// SSE parser
// ---------------------------------------------------------------------------

#[test]
fn sse_parser_single_data_line() {
    let body = r#"data: {"result":{"content":[{"type":"text","text":"hello world"}]}}
"#;
    let out = parse_sse_body(body).expect("parse ok").expect("non-empty");
    assert_eq!(out, "hello world");
}

#[test]
fn sse_parser_skips_non_data_lines() {
    let body = "event: message\n\
                : keep-alive comment\n\
                retry: 5000\n\
                data: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"yo\"}]}}\n";
    let out = parse_sse_body(body).expect("parse ok").expect("non-empty");
    assert_eq!(out, "yo");
}

#[test]
fn sse_parser_returns_first_with_content() {
    // The first data has no content, only the second one does
    let body = "data: {\"result\":{\"content\":[]}}\n\
                data: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}\n";
    let out = parse_sse_body(body).expect("parse ok").expect("non-empty");
    assert_eq!(out, "second");
}

#[test]
fn sse_parser_empty_results_returns_none() {
    let body = "data: {\"result\":{\"content\":[]}}\n";
    let out = parse_sse_body(body).expect("parse ok");
    assert!(out.is_none(), "empty content should return None");
}

#[test]
fn sse_parser_no_data_lines() {
    let body = "event: open\n\nevent: close\n";
    let out = parse_sse_body(body).expect("parse ok");
    assert!(out.is_none());
}

#[test]
fn sse_parser_invalid_json_returns_err() {
    // The only data line is not valid JSON and does not have any content lines
    let body = "data: not_a_json\n";
    let err = parse_sse_body(body).expect_err("should error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Exa SSE") || msg.contains("invalid"),
        "error message should be readable: {msg}"
    );
}

#[test]
fn sse_parser_handles_data_with_no_space() {
    // The SSE specification allows `data:foo` (without spaces) and `data: foo` (with spaces)
    let body = "data:{\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"z\"}]}}\n";
    let out = parse_sse_body(body).expect("parse ok").expect("non-empty");
    assert_eq!(out, "z");
}
