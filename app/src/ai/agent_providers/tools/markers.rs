//! UI signal marker tool: execution means "notifying the front end to do something", and the result is a fixed ack.
//!
//! - `open_code_review`: Open the Code Review panel
//! - `transfer_shell_command_control_to_user`: Transfer PTY control of long-running commands to the user
//!
//! The protobuf fields of these tools are very few (empty message or one field), and the executors are mostly
//! Directly transfer to the fixed result marker path, the actual side effects on the client side are determined by UI/Terminal
//! Triggered after listening to the corresponding ToolCall message.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

// ---------------------------------------------------------------------------
// open_code_review
// ---------------------------------------------------------------------------

fn empty_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn open_code_review_from_args(_args: &str) -> Result<api::message::tool_call::Tool> {
    Ok(api::message::tool_call::Tool::OpenCodeReview(
        api::message::tool_call::OpenCodeReview {},
    ))
}

fn open_code_review_result_to_json(
    result: &api::message::tool_call_result::Result,
) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    match result {
        R::OpenCodeReview(_) => Some(json!({ "status": "ok" })),
        _ => None,
    }
}

pub static OPEN_CODE_REVIEW: OpenAiTool = OpenAiTool {
    name: "open_code_review",
    description: "Open the Code Review panel for the current project (client UI signal, no parameters).\
                  Use when the user explicitly requests to open code review, or the context indicates starting the review phase.",
    parameters: empty_parameters,
    from_args: open_code_review_from_args,
    result_to_json: open_code_review_result_to_json,
};

// ---------------------------------------------------------------------------
// transfer_shell_command_control_to_user
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TransferArgs {
    /// Explanation for users: why control should be returned.
    #[serde(default)]
    reason: String,
}

fn transfer_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reason": {
                "type": "string",
                "description": "Explain to the user why PTY control needs to be returned (e.g. 'requires your manual login and interaction now')."
            }
        },
        "additionalProperties": false
    })
}

fn transfer_from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: TransferArgs = if args.trim().is_empty() {
        TransferArgs {
            reason: String::new(),
        }
    } else {
        serde_json::from_str(args)?
    };
    Ok(
        api::message::tool_call::Tool::TransferShellCommandControlToUser(
            api::message::tool_call::TransferShellCommandControlToUser {
                reason: parsed.reason,
            },
        ),
    )
}

fn transfer_result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::transfer_shell_command_control_to_user_result::Result as TR;
    let r = match result {
        R::TransferShellCommandControlToUser(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(TR::LongRunningCommandSnapshot(s)) => json!({
            "status": "transferred",
            "command_id": s.command_id,
            "output": s.output,
            "is_alt_screen_active": s.is_alt_screen_active,
        }),
        Some(TR::CommandFinished(f)) => json!({
            "status": "completed",
            "command_id": f.command_id,
            "exit_code": f.exit_code,
            "output": f.output,
        }),
        Some(TR::Error(_)) => json!({ "status": "error", "message": "block_not_found" }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static TRANSFER_SHELL_CONTROL: OpenAiTool = OpenAiTool {
    name: "transfer_shell_command_control_to_user",
    description: "Transfer the PTY control of the current long-running shell command back to the user.\
                  Use case: the command requires manual user interaction, or the scenario is not suitable for write_to_long_running_shell_command\
                  (e.g., interactive login, requiring real-time terminal display to decide next steps, etc.).\
                  The 'reason' field will be displayed to the user explaining why control is returned.",
    parameters: transfer_parameters,
    from_args: transfer_from_args,
    result_to_json: transfer_result_to_json,
};
