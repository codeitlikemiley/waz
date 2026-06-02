//! Prompt user class tools: `suggest_new_conversation` / `suggest_prompt`.
//!
//! These two tools are **pure local channel signals** + UI pop-ups — the model proactively suggests an action,
//! The user accepts/rejects in the UI, and the executor writes back the result after the user decides. Does not depend on any server.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

// ---------------------------------------------------------------------------
// suggest_new_conversation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct NewConvArgs {
    /// The id of the current assistant message (if the model does not know that it can pass an empty string, the controller will hide it).
    #[serde(default)]
    message_id: String,
}

fn new_conv_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message_id": {
                "type": "string",
                "description": "Optional: branch a new conversation from a specific assistant message (if empty, uses the current message)."
            }
        },
        "additionalProperties": false
    })
}

fn new_conv_from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: NewConvArgs = if args.trim().is_empty() {
        NewConvArgs {
            message_id: String::new(),
        }
    } else {
        serde_json::from_str(args)?
    };
    Ok(api::message::tool_call::Tool::SuggestNewConversation(
        api::message::tool_call::SuggestNewConversation {
            message_id: parsed.message_id,
        },
    ))
}

fn new_conv_result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::suggest_new_conversation_result::Result as SR;
    let r = match result {
        R::SuggestNewConversation(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Accepted(a)) => json!({ "status": "accepted", "message_id": a.message_id }),
        Some(SR::Rejected(_)) => json!({ "status": "rejected" }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static SUGGEST_NEW_CONVERSATION: OpenAiTool = OpenAiTool {
    name: "suggest_new_conversation",
    description: "Suggest that the user branch a new conversation from the current message.\
                  Use case: the current conversation context has become very long and the topic is about to switch, or the current task is completed and the next task is unrelated. A confirmation dialog will pop up in the UI, and branching only occurs upon user acceptance.\
                  Do not abuse — call only when context switching benefits are obvious.",
    parameters: new_conv_parameters,
    from_args: new_conv_from_args,
    result_to_json: new_conv_result_to_json,
};

// ---------------------------------------------------------------------------
// suggest_prompt
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PromptArgs {
    /// The prompt text actually sent to the agent.
    prompt: String,
    /// Optional: short label displayed on the UI (used as chip display if the prompt is too long).
    #[serde(default)]
    label: String,
}

fn prompt_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "prompt": {
                "type": "string",
                "description": "The next prompt suggested to the user (actually sent to the agent when clicked)."
            },
            "label": {
                "type": "string",
                "description": "Optional: short label displayed on the chip (recommended when prompt is long)."
            }
        },
        "required": ["prompt"],
        "additionalProperties": false
    })
}

fn prompt_from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::suggest_prompt::{DisplayMode, PromptChip};
    let parsed: PromptArgs = serde_json::from_str(args)?;
    let chip = PromptChip {
        prompt: parsed.prompt,
        label: parsed.label,
    };
    Ok(api::message::tool_call::Tool::SuggestPrompt(
        api::message::tool_call::SuggestPrompt {
            display_mode: Some(DisplayMode::PromptChip(chip)),
            is_trigger_irrelevant: false,
        },
    ))
}

fn prompt_result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::suggest_prompt_result::Result as SR;
    let r = match result {
        R::SuggestPrompt(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Accepted(_)) => json!({ "status": "accepted" }),
        Some(SR::Rejected(_)) => json!({ "status": "rejected" }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static SUGGEST_PROMPT: OpenAiTool = OpenAiTool {
    name: "suggest_prompt",
    description: "Suggest the next prompt to the user at the end of the response (displayed as a chip).\
                  Use case: the task naturally extends to obvious follow-ups (e.g. suggesting to run lint after tests pass, suggesting to add unit tests after reading code).\
                  Avoid duplicate or obvious suggestions.",
    parameters: prompt_parameters,
    from_args: prompt_from_args,
    result_to_json: prompt_result_to_json,
};
