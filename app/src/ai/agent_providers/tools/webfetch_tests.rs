//! `web_runtime::run_webfetch` Single test (mockito, no external network).

use super::*;
use mockito::{Matcher, Server};

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("reqwest client build")
}

fn args(url: &str) -> FetchArgs {
    FetchArgs {
        url: url.to_owned(),
        format: None,
        timeout: None,
    }
}

// ---------------------------------------------------------------------------
// URL validation (pure logic, no HTTP)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_non_https_scheme() {
    let client = build_client();
    for bad in [
        "ftp://example.com",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "http://example.com",
        "",
    ] {
        let err = run_webfetch(&client, args(bad)).await.unwrap_err();
        assert!(err.to_string().contains("HTTPS"), "bad={bad} err={err}");
    }
}

#[tokio::test]
async fn rejects_http_urls() {
    let client = build_client();
    let err = run_webfetch(&client, args("http://example.com"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("HTTPS"),
        "HTTP should be rejected: {err}"
    );
}

// ---------------------------------------------------------------------------
// Content type branch — call send_fetch directly since mockito only provides HTTP
// ---------------------------------------------------------------------------

/// Helper function: perform a webfetch-like process against the mockito server, skipping the HTTPS check
/// (mockito only provides HTTP). Test content processing pipeline without URL scheme verification.
/// Reuse `response_to_fetch_output` to avoid drifting from production logic.
async fn run_webfetch_test(
    server_url: &str,
    path: &str,
    format: Option<FetchFormat>,
) -> Result<FetchOutput> {
    let client = build_client();
    let url = format!("{server_url}{path}");
    let fmt = format.unwrap_or_default();
    let accept = fmt.accept_header();
    let timeout = std::time::Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECS);

    let resp = send_fetch(&client, &url, accept, CHROME_UA, timeout).await?;
    response_to_fetch_output(resp, &url, &fmt).await
}

#[tokio::test]
async fn html_to_markdown() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/page")
        .with_status(200)
        .with_header("content-type", "text/html; charset=utf-8")
        .with_body("<html><body><h1>Hello</h1><p>World</p></body></html>")
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/page", None)
        .await
        .expect("ok");
    assert!(
        out.output.contains("Hello"),
        "missing Hello: {}",
        out.output
    );
    assert!(
        out.output.contains("World"),
        "missing World: {}",
        out.output
    );
    assert!(
        out.output.contains('#') || !out.output.contains("<h1>"),
        "should be markdown not HTML: {}",
        out.output
    );
    assert_eq!(out.format, "markdown");
    assert!(out.attachments.is_empty());
}

#[tokio::test]
async fn text_plain_passthrough() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/text")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("just some text")
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/text", None)
        .await
        .expect("ok");
    assert_eq!(out.output, "just some text");
}

#[tokio::test]
async fn json_pretty_print() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/api")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"a":1,"b":[2,3]}"#)
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/api", None)
        .await
        .expect("ok");
    assert!(
        out.output.starts_with("```json\n"),
        "missing fence: {}",
        out.output
    );
    assert!(
        out.output.contains("\"a\": 1"),
        "not pretty: {}",
        out.output
    );
    assert!(out.output.ends_with("\n```"));
}

#[tokio::test]
async fn image_attachment_base64() {
    let mut server = Server::new_async().await;
    // 1x1 transparent PNG
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let _m = server
        .mock("GET", "/img.png")
        .with_status(200)
        .with_header("content-type", "image/png")
        .with_body(png_bytes.clone())
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/img.png", None)
        .await
        .expect("ok");
    assert_eq!(out.attachments.len(), 1);
    let att = &out.attachments[0];
    assert_eq!(att.mime, "image/png");
    assert!(att.url.starts_with("data:image/png;base64,"));
    let b64 = att.url.trim_start_matches("data:image/png;base64,");
    let decoded = BASE64.decode(b64).expect("decode");
    assert_eq!(decoded, png_bytes);
}

// ---------------------------------------------------------------------------
// format parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn format_html_returns_raw() {
    let mut server = Server::new_async().await;
    let raw = "<html><body><h1>Raw</h1></body></html>";
    let _m = server
        .mock("GET", "/x")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body(raw)
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/x", Some(FetchFormat::Html))
        .await
        .expect("ok");
    assert_eq!(out.output, raw);
    assert_eq!(out.format, "html");
}

#[tokio::test]
async fn format_text_strips_html() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/x")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html><body><p>One</p><p>Two</p><script>alert(1)</script></body></html>")
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/x", Some(FetchFormat::Text))
        .await
        .expect("ok");
    assert!(out.output.contains("One"));
    assert!(out.output.contains("Two"));
    assert!(
        !out.output.contains("alert(1)"),
        "script 内容应被剥离: {}",
        out.output
    );
    assert_eq!(out.format, "text");
}

#[tokio::test]
async fn default_format_is_markdown() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/x")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body("<html><body><h2>x</h2></body></html>")
        .create_async()
        .await;
    let out = run_webfetch_test(&server.url(), "/x", None).await.unwrap();
    assert_eq!(out.format, "markdown");
}

#[tokio::test]
async fn accept_header_negotiation_for_markdown() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/x")
        .match_header(
            "accept",
            Matcher::Regex(r"text/markdown\s*;\s*q=1\.0".into()),
        )
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("ok")
        .create_async()
        .await;

    let out = run_webfetch_test(&server.url(), "/x", None)
        .await
        .expect("ok");
    assert_eq!(out.output, "ok");
}

// ---------------------------------------------------------------------------
// Size/Status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_oversized_content_length() {
    let big = vec![b'x'; MAX_RESPONSE_SIZE + 1024];
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/big")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body(big)
        .create_async()
        .await;

    let err = run_webfetch_test(&server.url(), "/big", None)
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("too large"), "got: {msg}");
}

#[tokio::test]
async fn http_error_status_propagates() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/404")
        .with_status(404)
        .create_async()
        .await;
    let err = run_webfetch_test(&server.url(), "/404", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "got: {err}");
}

// ---------------------------------------------------------------------------
// SSRF: is_blocked_ip coverage test
// ---------------------------------------------------------------------------

#[test]
fn blocked_ip_ipv4_basics() {
    use std::net::IpAddr;
    for blocked in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "0.0.0.0",
        "0.0.0.1",       // 0.0.0.0/8 This host range
        "0.255.255.255", // 0.0.0.0/8 upper bound
        "255.255.255.255",
        "100.64.0.1",      // CGNAT
        "192.0.2.1",       // TEST-NET-1
        "198.51.100.1",    // TEST-NET-2
        "203.0.113.1",     // TEST-NET-3
        "198.18.0.1",      // Performance test address
        "224.0.0.1",       // multicast
        "239.255.255.255", // Multicast upper bound
        "240.0.0.1",       // reserved address
    ] {
        let ip: IpAddr = blocked.parse().unwrap();
        assert!(is_blocked_ip(ip), "should block {blocked}");
    }
    // Public IP cannot be blocked by mistake.
    for allowed in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
        let ip: IpAddr = allowed.parse().unwrap();
        assert!(!is_blocked_ip(ip), "should allow {allowed}");
    }
}

#[test]
fn blocked_ip_ipv4_mapped_ipv6() {
    use std::net::IpAddr;
    // ::ffff:127.0.0.1 is IPv4-mapped IPv6 and must be intercepted by IPv4 loopback.
    let mapped_loopback: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_loopback),
        "::ffff:127.0.0.1 must be blocked"
    );

    let mapped_private: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_private),
        "::ffff:10.0.0.1 must be blocked"
    );

    let mapped_link_local: IpAddr = "::ffff:169.254.1.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_link_local),
        "::ffff:169.254.1.1 must be blocked"
    );

    // ::ffff:8.8.8.8 corresponds to public IPv4 and cannot be blocked by mistake.
    let mapped_public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
    assert!(
        !is_blocked_ip(mapped_public),
        "::ffff:8.8.8.8 should be allowed"
    );
}

#[test]
fn blocked_ip_ipv6_ranges() {
    use std::net::IpAddr;
    for blocked in [
        "::1",         // loopback
        "::",          // unspecified
        "fc00::1",     // unique-local
        "fe80::1",     // link-local
        "ff00::1",     // multicast
        "2001:db8::1", // Document example address
    ] {
        let ip: IpAddr = blocked.parse().unwrap();
        assert!(is_blocked_ip(ip), "should block {blocked}");
    }
    // Public IPv6 cannot be blocked by mistake.
    let public: IpAddr = "2606:4700:4700::1111".parse().unwrap();
    assert!(!is_blocked_ip(public), "public IPv6 should be allowed");
}

// ---------------------------------------------------------------------------
// SSRF redirect protection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ssrf_safe_client_builds_with_redirect_policy() {
    let client = build_ssrf_safe_client().expect("build client");
    // Verify that the client builds successfully with the custom SSRF redirect policy and DNS resolver.
    // TODO: After mockito supports redirection, add real internal IP redirection integration testing.
    assert!(client.get("https://example.invalid").build().is_ok());
}

// ---------------------------------------------------------------------------
// FetchOutput serialization test
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The real endpoint smoke test is skipped by default to prevent CI or offline development environments from relying on the external network.
// Set WARP_RUN_WEB_INTEGRATION=1 when manual verification of the real endpoint is required.
// ---------------------------------------------------------------------------

fn skip_real() -> bool {
    std::env::var("WARP_RUN_WEB_INTEGRATION").is_err()
}

#[tokio::test]
async fn real_example_com_markdown() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://example.com"))
        .await
        .expect("real example.com");
    assert!(
        out.output.to_lowercase().contains("example domain"),
        "got: {}",
        out.output
    );
}

#[tokio::test]
async fn real_httpbin_html_to_markdown() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/html"))
        .await
        .expect("real httpbin html");
    assert!(!out.output.trim().is_empty());
    assert_eq!(out.format, "markdown");
}

#[tokio::test]
async fn real_httpbin_json_pretty() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/json"))
        .await
        .expect("real httpbin json");
    assert!(out.output.contains("```json"), "got: {}", out.output);
}

#[tokio::test]
async fn real_httpbin_image_attachment() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/image/png"))
        .await
        .expect("real png");
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].mime, "image/png");
}

#[tokio::test]
async fn real_httpbin_404_errors() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let err = run_webfetch(&client, args("https://httpbin.org/status/404"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Documentation/opencode byte-level alignment regression
// ---------------------------------------------------------------------------

/// Lock webfetch.md and opencode `packages/opencode/src/tool/webfetch.txt`
/// Byte level consistency. When modifying, both sides need to be synchronized.
#[test]
fn webfetch_description_matches_opencode_verbatim() {
    use super::super::webfetch::WEBFETCH;
    let expected = "- Fetches content from a specified URL\n\
                    - Takes a URL and optional format as input\n\
                    - Fetches the URL content, converts to requested format (markdown by default)\n\
                    - Returns the content in the specified format\n\
                    - Use this tool when you need to retrieve and analyze web content\n\
                    \n\
                    Usage notes:\n\
                    \x20\x20- IMPORTANT: if another tool is present that offers better web fetching capabilities, is more targeted to the task, or has fewer restrictions, prefer using that tool instead of this one.\n\
                    \x20\x20- The URL must be a fully-formed valid URL\n\
                    \x20\x20- The URL must use HTTPS (http:// URLs are rejected)\n\
                    \x20\x20- Format options: \"markdown\" (default), \"text\", or \"html\"\n\
                    \x20\x20- This tool is read-only and does not modify any files\n\
                    \x20\x20- Results may be summarized if the content is very large\n";
    assert_eq!(WEBFETCH.description, expected);
}

#[test]
fn fetch_output_omits_empty_attachments_in_json() {
    let out = FetchOutput {
        url: "https://x".into(),
        status: 200,
        content_type: "text/plain".into(),
        format: "markdown".into(),
        output: "hi".into(),
        attachments: vec![],
    };
    let v = fetch_output_to_json(&out);
    assert!(
        v.get("attachments").is_none(),
        "空 attachments 应被 skip: {v}"
    );
    assert_eq!(v["output"], "hi");
}

/// `_byop_intercepted` sentinel must exist in all web tool results (including error),
/// Otherwise controller (`controller.rs::needs_byop_local_resume`) will not trigger auto-resume,
/// The model will get stuck waiting for results and the UI will show silent failure.
#[test]
fn fetch_output_carries_byop_sentinel() {
    let out = FetchOutput {
        url: "https://x".into(),
        status: 200,
        content_type: "text/plain".into(),
        format: "markdown".into(),
        output: "hi".into(),
        attachments: vec![],
    };
    let v = fetch_output_to_json(&out);
    assert_eq!(v["_byop_intercepted"], true);

    let err = error_to_json("webfetch", &anyhow::anyhow!("boom"));
    assert_eq!(err["_byop_intercepted"], true);
    assert_eq!(err["status"], "error");
}
