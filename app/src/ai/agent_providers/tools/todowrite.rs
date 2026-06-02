//! `todowrite` BYOP tool descriptor.
//!
//! Not mapped to protobuf executor variant like webfetch / websearch - by
//! `chat_stream.rs` is intercepted by name before `parse_incoming_tool_call` and synthesized directly
//! `Message::UpdateTodos` is written to conversation, triggering chip + popup UI update.
//!
//! Protocol design alignment opencode `todowrite`:
//! - Input parameters: `{ todos: [{ content, status, priority? }] }`, **full coverage** (replace the entire list with each call)
//! - status:`pending` / `in_progress` / `completed` / `cancelled`
//! - The client calculates the stable id (SHA-256 prefix, 16 hex) based on `content` to avoid refreshing the chip number every time
//!
//! ## emit strategy
//!
//! Intercept a todowrite call each time → synthesize two `Message::UpdateTodos`:
//! 1. `CreateTodoList { initial_todos: [all todos] }` (all pending)
//! 2. `MarkTodosCompleted { todo_ids: [id of status=completed/cancelled] }`
//!
//! `update_todo_list_from_todo_op` will move the second hit item from pending to completed
//! (`mark_todos_complete` in pending lookup id), final `AIAgentTodoList` status:
//! `completed_items = [completed]`、`pending_items = [pending + in_progress]`。
//! Waz UI `in_progress_item()` takes `pending_items.first()`, so in_progress
//! todo should be the first `status != completed/cancelled` item in the `todos` array.
//!
//! Then synthesize a pair of `Message::ToolCall`(carrier,tool=None) + `Message::ToolCallResult`
//! Unblock the upstream model.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use warp_multi_agent_api as api;

use super::OpenAiTool;

pub const TOOL_NAME: &str = "todowrite";

#[derive(Debug, Deserialize)]
pub struct Args {
    pub todos: Vec<TodoArg>,
}

#[derive(Debug, Deserialize)]
pub struct TodoArg {
    pub content: String,
    /// `pending` | `in_progress` | `completed` | `cancelled`. The model will occasionally send off strings,
    /// When parsing, the unrecognized value is treated as `pending`.
    #[serde(default)]
    pub status: String,
    /// The opencode protocol has priority, and the Waz data model does not differentiate. It is accepted here but not used.
    /// The purpose of retaining is to allow the model to send parameters according to opencode habits without reporting errors.
    #[serde(default, rename = "priority")]
    pub _priority: Option<String>,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "The full updated todo list. Pass every item every call (overwrite semantics).",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "Brief description of the task (1 line)."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "Current status."
                        },
                        "priority": {
                            "type": "string",
                            "enum": ["high", "medium", "low"],
                            "description": "Optional priority. Currently advisory only."
                        }
                    },
                    "required": ["content", "status"]
                }
            }
        },
        "required": ["todos"],
        "additionalProperties": false
    })
}

fn from_args(_args: &str) -> Result<api::message::tool_call::Tool> {
    Err(anyhow!(
        "todowrite is intercepted by chat_stream BYOP todo dispatcher; \
         from_args should never be called"
    ))
}

fn result_to_json(_result: &api::message::tool_call_result::Result) -> Option<Value> {
    None
}

pub static TODOWRITE: OpenAiTool = OpenAiTool {
    name: TOOL_NAME,
    description: include_str!("../prompts/tool_descriptions/todowrite.md"),
    parameters,
    from_args,
    result_to_json,
};

/// Synthesize the todowrite tool_result for the upstream model.
///
/// `todowrite` is a local interception tool and does not generate `AIAgentAction`, so it must be brought
/// `_byop_intercepted` sentinel. The controller will use this tag to trigger auto-resume,
/// Let the model continue looping after receiving tool_result in the next round.
pub fn success_result_to_json(message: &'static str) -> Value {
    json!({
        "_byop_intercepted": true,
        "status": "ok",
        "message": message,
    })
}

pub fn invalid_arguments_result_to_json(detail: String, received_args: &str) -> Value {
    json!({
        "_byop_intercepted": true,
        "error": "invalid_arguments",
        "detail": detail,
        "tool": TOOL_NAME,
        "received_args": received_args,
        "hint": "Expected { todos: [{ content: string, status: string }] }.",
    })
}

/// Compute stable id based on content. When the model uses the same content to send todo for the second time, it will get the same id.
/// In this way, `mark_todos_complete(todo_ids)` can be hit in pending → move it to completed.
fn stable_id(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    let bytes = h.finalize();
    // Take the first 8 bytes = 16 hex, which is stable and short enough.
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn to_todo_item(arg: &TodoArg) -> api::TodoItem {
    api::TodoItem {
        id: stable_id(&arg.content),
        title: arg.content.clone(),
        description: String::new(),
    }
}

fn is_completed_status(s: &str) -> bool {
    matches!(s, "completed" | "cancelled")
}

/// Combine two `Message::UpdateTodos` (create a new list + mark completed).
/// Chat_stream calls this function when intercepting todowrite and yields the returned messages.
pub fn build_update_todos_messages(
    args_str: &str,
    task_id: &str,
    request_id: &str,
) -> Result<Vec<api::Message>> {
    let parsed: Args =
        serde_json::from_str(args_str).map_err(|e| anyhow!("todowrite args parse error: {e}"))?;

    // All todos are pending (the order is maintained in the order given by the model), which is the entry point of CreateTodoList.
    let initial_todos: Vec<api::TodoItem> = parsed.todos.iter().map(to_todo_item).collect();
    // Then mark those ids with status=completed/cancelled for completion.
    let completed_ids: Vec<String> = parsed
        .todos
        .iter()
        .filter(|t| is_completed_status(&t.status))
        .map(|t| stable_id(&t.content))
        .collect();

    let mut messages = Vec::with_capacity(2);

    messages.push(make_update_todos_message(
        task_id,
        request_id,
        api::message::update_todos::Operation::CreateTodoList(api::CreateTodoList {
            initial_todos,
        }),
    ));

    if !completed_ids.is_empty() {
        messages.push(make_update_todos_message(
            task_id,
            request_id,
            api::message::update_todos::Operation::MarkTodosCompleted(api::MarkTodosCompleted {
                todo_ids: completed_ids,
            }),
        ));
    }

    Ok(messages)
}

fn make_update_todos_message(
    task_id: &str,
    request_id: &str,
    operation: api::message::update_todos::Operation,
) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UpdateTodos(
            api::message::UpdateTodos {
                operation: Some(operation),
            },
        )),
        request_id: request_id.to_owned(),
        timestamp: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intercepted_result_payloads_include_auto_resume_sentinel() {
        let ok = success_result_to_json("todo list updated");
        assert_eq!(ok["_byop_intercepted"], true);
        assert_eq!(ok["status"], "ok");
        let ok_string = serde_json::to_string(&ok).unwrap();
        assert!(ok_string.contains(r#""_byop_intercepted":true"#));

        let err = invalid_arguments_result_to_json("bad args".to_owned(), "{}");
        assert_eq!(err["_byop_intercepted"], true);
        assert_eq!(err["error"], "invalid_arguments");
        assert_eq!(err["tool"], TOOL_NAME);
        let err_string = serde_json::to_string(&err).unwrap();
        assert!(err_string.contains(r#""_byop_intercepted":true"#));
    }
}
