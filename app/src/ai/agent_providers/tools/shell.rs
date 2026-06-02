//! `RunShellCommand` adaptation.
//!
//! Corresponding to `api::message::tool_call::Tool::RunShellCommand` in warp,
//! After execution, the result is `ToolCallResultType::RunShellCommand(RunShellCommandResult)`.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

#[derive(Debug, Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    is_read_only: bool,
    #[serde(default)]
    uses_pager: bool,
    #[serde(default)]
    is_risky: bool,
    /// `None` (default / true) = wait for the command to complete before returning; `Some(false)` = return immediately after starting
    /// A LongRunningCommandSnapshot, which can be used later read/write_to_long_running_*
    /// Tools continue to interact (suitable for dev server / tail -f classes to continue running commands).
    #[serde(default)]
    wait_until_complete: Option<bool>,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell command to execute (complete command line)."
            },
            "is_read_only": {
                "type": "boolean",
                "description": "Whether the command only reads information without modifying the file system or external state (when true, no user confirmation is required).",
                "default": false
            },
            "uses_pager": {
                "type": "boolean",
                "description": "Whether the command triggers a pager (e.g. less/more). It is recommended to be false; you can append '| cat' or similar to prevent blocking.",
                "default": false
            },
            "is_risky": {
                "type": "boolean",
                "description": "Whether the command is risky/destructive (e.g. 'rm -rf', changing global system settings, etc.). Set to true to display a prominent confirmation prompt to the user.",
                "default": false
            },
            "wait_until_complete": {
                "type": "boolean",
                "description": "Default is true (waits for the command to finish before returning, suitable for one-off tasks). Commands that do not exit naturally (e.g. dev servers, background processes, 'tail -f', interactive REPLs) MUST set this to false, otherwise the current turn will block indefinitely. If set to false, it returns a LongRunningCommandSnapshot immediately, and subsequent turns can continue interaction via read/write_to_long_running_shell_command.",
                "default": true
            }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::run_shell_command::WaitUntilCompleteValue;
    let parsed: Args = serde_json::from_str(args)?;
    // When None, it explicitly defaults to true (returns after the command is completed) to avoid implicit default behavior on the controller side.
    // Ambiguity occurs under different warp versions/paths. If the model wants long-running mode, it must explicitly pass false.
    let wait_until_complete_value = Some(WaitUntilCompleteValue::WaitUntilComplete(
        parsed.wait_until_complete.unwrap_or(true),
    ));
    Ok(api::message::tool_call::Tool::RunShellCommand(
        api::message::tool_call::RunShellCommand {
            command: parsed.command,
            is_read_only: parsed.is_read_only,
            uses_pager: parsed.uses_pager,
            is_risky: parsed.is_risky,
            citations: vec![],
            wait_until_complete_value,
            risk_category: 0,
        },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::run_shell_command_result::Result as ShellR;
    let r = match result {
        R::RunShellCommand(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(ShellR::CommandFinished(f)) => json!({
            "status": "completed",
            "command": r.command,
            "exit_code": f.exit_code,
            "output": f.output,
        }),
        // Long running command: Started but not finished yet. Expose the snapshot to the model so that the model can
        // Decide whether to continue reading (read_shell_command_output) or writing (write_to_long_running_*).
        Some(ShellR::LongRunningCommandSnapshot(s)) => json!({
            "status": "running",
            "command": r.command,
            "command_id": s.command_id,
            "output": s.output,
            "is_alt_screen_active": s.is_alt_screen_active,
        }),
        Some(ShellR::PermissionDenied(_)) => json!({
            "status": "permission_denied",
            "command": r.command,
        }),
        None => json!({ "status": "cancelled", "command": r.command }),
    };
    Some(value)
}

pub static RUN_SHELL_COMMAND: OpenAiTool = OpenAiTool {
    name: "run_shell_command",
    description: include_str!("../prompts/tool_descriptions/run_shell_command.md"),
    parameters,
    from_args,
    result_to_json,
};
