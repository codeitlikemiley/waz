//! MCP stdio server so Codex, Claude Code, and other agents can use TMP
//! instead of guessing CLI argv. Wire protocol: JSON-RPC 2.0 with
//! Content-Length framing (MCP) plus newline-delimited JSON.

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

pub fn run() -> Result<(), String> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    loop {
        let Some(msg) = read_message(&mut reader)? else {
            break;
        };
        if msg.get("method").and_then(|m| m.as_str()) == Some("notifications/initialized") {
            continue;
        }
        if msg.get("id").is_none() {
            continue;
        }
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        match handle_method(method, &params) {
            Ok(result) => write_rpc(&mut writer, id, result)?,
            Err((code, message)) => write_rpc_error(&mut writer, id, code, &message)?,
        }
    }
    Ok(())
}

pub(crate) fn handle_method(method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(ok_initialize()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(call_tool(params)),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

fn ok_initialize() -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "waz",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Use waz TMP tools to list, show, and build real CLI commands from schemas. Do not guess flags. If a tool has no schema, call waz_generate."
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "waz_tmp_list",
            "description": "List TMP commands available in a working directory (from installed schemas).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Working directory" },
                    "query": { "type": "string", "description": "Optional substring filter" }
                },
                "required": ["cwd"]
            }
        },
        {
            "name": "waz_tmp_show",
            "description": "Show one TMP command and its resolved token values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string" },
                    "command": { "type": "string", "description": "Exact schema command, e.g. cargo run" }
                },
                "required": ["cwd", "command"]
            }
        },
        {
            "name": "waz_tmp_build",
            "description": "Fill tokens and return the argv string to run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string" },
                    "command": { "type": "string" },
                    "set": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Token assignments NAME=VALUE"
                    }
                },
                "required": ["cwd", "command"]
            }
        },
        {
            "name": "waz_resolve",
            "description": "Resolve natural language to a grounded TMP command using schemas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string" },
                    "query": { "type": "string" },
                    "tool": { "type": "string", "description": "Optional tool to pin, e.g. cargo" }
                },
                "required": ["cwd", "query"]
            }
        },
        {
            "name": "waz_generate",
            "description": "Generate a TMP schema for a CLI on PATH. Default wait=true so the schema exists before you list/show. Pass wait=false only if you will poll waz_generate_status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tool": { "type": "string" },
                    "force": { "type": "boolean" },
                    "wait": { "type": "boolean", "description": "Block until the schema is written. Default true." }
                },
                "required": ["tool"]
            }
        },
        {
            "name": "waz_generate_jobs",
            "description": "List background schema-generation jobs (reaps dead PIDs).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "waz_generate_status",
            "description": "Show one generate job. Optionally wait until it finishes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string" },
                    "wait": { "type": "boolean", "description": "If true, poll until done/error/cancelled" }
                },
                "required": ["job_id"]
            }
        },
        {
            "name": "waz_generate_cancel",
            "description": "Cancel a background generate job.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string" }
                },
                "required": ["job_id"]
            }
        },
        {
            "name": "waz_plugin_list",
            "description": "List Agent Plugins loaded by waz (skills + MCP).",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let result = match name {
        "waz_tmp_list" => {
            let cwd = str_arg(&args, "cwd").unwrap_or_else(|| ".".into());
            let query = args.get("query").and_then(|v| v.as_str());
            serde_json::to_value(crate::tmp::list(&cwd, query)).unwrap_or(json!({}))
        }
        "waz_tmp_show" => {
            let cwd = str_arg(&args, "cwd").unwrap_or_else(|| ".".into());
            let command = str_arg(&args, "command").unwrap_or_default();
            match crate::tmp::show(&cwd, &command) {
                Ok(v) => serde_json::to_value(v).unwrap_or(json!({})),
                Err(e) => return tool_error(&e),
            }
        }
        "waz_tmp_build" => {
            let cwd = str_arg(&args, "cwd").unwrap_or_else(|| ".".into());
            let command = str_arg(&args, "command").unwrap_or_default();
            let set: Vec<String> = args
                .get("set")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            match crate::tmp::build(&cwd, &command, &set) {
                Ok(v) => serde_json::to_value(v).unwrap_or(json!({})),
                Err(e) => return tool_error(&e),
            }
        }
        "waz_resolve" => {
            let cwd = str_arg(&args, "cwd").unwrap_or_else(|| ".".into());
            let query = str_arg(&args, "query").unwrap_or_default();
            let tool = args.get("tool").and_then(|v| v.as_str());
            let config = crate::config::Config::load();
            match crate::resolve::resolve(&config, &query, &cwd, tool) {
                Ok(v) => serde_json::to_value(v).unwrap_or(json!({})),
                Err(e) => return tool_error(&e),
            }
        }
        "waz_generate" => {
            let tool = str_arg(&args, "tool").unwrap_or_default();
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let wait = generate_wait_arg(&args);
            match crate::generate::start_generate(&tool, force, wait, None, None) {
                Ok(v) => v,
                Err(e) => return tool_error(&e),
            }
        }
        "waz_generate_jobs" => serde_json::to_value(crate::jobs::list_jobs()).unwrap_or(json!([])),
        "waz_generate_status" => {
            let id = str_arg(&args, "job_id").unwrap_or_default();
            let wait = args.get("wait").and_then(|v| v.as_bool()).unwrap_or(false);
            let result = if wait {
                crate::jobs::wait_job(&id, None)
            } else {
                crate::jobs::get_job(&id)
            };
            match result {
                Ok(job) => serde_json::to_value(job).unwrap_or(json!({})),
                Err(e) => return tool_error(&e),
            }
        }
        "waz_generate_cancel" => {
            match crate::jobs::cancel_job(&str_arg(&args, "job_id").unwrap_or_default()) {
                Ok(job) => serde_json::to_value(job).unwrap_or(json!({})),
                Err(e) => return tool_error(&e),
            }
        }
        "waz_plugin_list" => {
            let plugins = crate::plugin::discover();
            json!(plugins
                .iter()
                .map(|p| json!({
                    "name": p.manifest.name,
                    "version": p.manifest.version,
                    "source": p.source,
                    "skills": p.skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
                    "mcp": p.has_mcp,
                    "root": p.root,
                }))
                .collect::<Vec<_>>())
        }
        other => return tool_error(&format!("unknown tool: {other}")),
    };
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()) }],
        "structuredContent": result,
        "isError": false
    })
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn generate_wait_arg(args: &Value) -> bool {
    args.get("wait").and_then(|v| v.as_bool()).unwrap_or(true)
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut first = String::new();
    let n = reader.read_line(&mut first).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = first.trim();
    if trimmed.is_empty() {
        return read_message(reader);
    }
    if trimmed.starts_with('{') {
        let v = serde_json::from_str(trimmed).map_err(|e| format!("rpc json: {e}"))?;
        return Ok(Some(v));
    }
    let mut content_length: Option<usize> = None;
    let mut line = first;
    loop {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().ok();
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
    }
    let len = content_length.ok_or_else(|| "MCP message missing Content-Length".to_string())?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let v = serde_json::from_slice(&buf).map_err(|e| format!("rpc json: {e}"))?;
    Ok(Some(v))
}

fn write_rpc(writer: &mut impl Write, id: Value, result: Value) -> Result<(), String> {
    let payload = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    write_frame(writer, &payload)
}

fn write_rpc_error(
    writer: &mut impl Write,
    id: Value,
    code: i64,
    message: &str,
) -> Result<(), String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    write_frame(writer, &payload)
}

fn write_frame(writer: &mut impl Write, payload: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).map_err(|e| e.to_string())?;
    writer.write_all(&body).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_tools() {
        let v = handle_method("initialize", &json!({})).unwrap();
        assert_eq!(v["serverInfo"]["name"], "waz");
        assert!(v["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_includes_tmp_and_generate() {
        let v = handle_method("tools/list", &json!({})).unwrap();
        let names: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"waz_tmp_list"));
        assert!(names.contains(&"waz_tmp_build"));
        assert!(names.contains(&"waz_generate"));
        assert!(names.contains(&"waz_generate_status"));
        assert!(names.contains(&"waz_generate_cancel"));
        assert!(names.contains(&"waz_resolve"));
    }

    #[test]
    fn generate_wait_defaults_true() {
        assert!(generate_wait_arg(&json!({})));
        assert!(generate_wait_arg(&json!({ "wait": true })));
        assert!(!generate_wait_arg(&json!({ "wait": false })));
    }

    #[test]
    fn generate_cancel_unknown_id_is_error() {
        let v = handle_method(
            "tools/call",
            &json!({ "name": "waz_generate_cancel", "arguments": { "job_id": "not-a-job" } }),
        )
        .unwrap();
        assert_eq!(v["isError"], true);
    }

    #[test]
    fn unknown_method_is_rpc_error() {
        let err = handle_method("nope", &json!({})).unwrap_err();
        assert_eq!(err.0, -32601);
    }

    #[test]
    fn unknown_tool_sets_is_error() {
        let v = handle_method(
            "tools/call",
            &json!({ "name": "not_a_tool", "arguments": {} }),
        )
        .unwrap();
        assert_eq!(v["isError"], true);
    }

    #[test]
    fn tmp_list_returns_structured_content() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        let v = handle_method(
            "tools/call",
            &json!({ "name": "waz_tmp_list", "arguments": { "cwd": cwd, "query": "cargo" } }),
        )
        .unwrap();
        assert_eq!(v["isError"], false);
        let n = v["structuredContent"]["count"].as_u64().unwrap();
        assert!(n >= 1, "expected cargo TMP commands, got {n}");
    }
}
