//! `ask_user_question`: Let the model proactively ask the user when key information is missing (single choice/multiple choice/free completion).
//!
//! Warp itself is `AskUserQuestion`, and internally all uses `MultipleChoice` which is a Question type
//! (Whether multiselect is allowed / whether "Other" free completion is allowed is determined by the internal bool).
//!
//! ## Usage suggestions (write in description so that the model can see it)
//!
//! Don't use this tool to ask trivial "do you want to continue"/"are you sure?" questions - just follow the answer strategy.
//! Use it only when the instructions given by the user contain multiple reasonable understandings and the cost of making the wrong choice is high.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use warp_multi_agent_api as api;

use super::OpenAiTool;

#[derive(Debug, Deserialize)]
struct Args {
    questions: Vec<QuestionArg>,
}

#[derive(Debug, Deserialize)]
struct QuestionArg {
    question: String,
    options: Vec<String>,
    /// 0-based, the subscript of the recommended option. Default = 0.
    #[serde(default)]
    recommended_index: i32,
    /// Whether to allow multiple selections.
    #[serde(default)]
    multi_select: bool,
    /// Whether to allow users to enter "Other" free text.
    #[serde(default)]
    supports_other: bool,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "description": "A list of questions to ask the user (usually 1 is enough, send multiple only if there are multiple dimensions to clarify).",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "Question text (concise and specific)."
                        },
                        "options": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 2,
                            "maxItems": 4,
                            "description": "List of option labels, 2-4 items, describing the consequence of each option specifically."
                        },
                        "recommended_index": {
                            "type": "integer",
                            "description": "0-based index of the recommended option.",
                            "default": 0
                        },
                        "multi_select": {
                            "type": "boolean",
                            "description": "Whether to allow the user to select multiple options.",
                            "default": false
                        },
                        "supports_other": {
                            "type": "boolean",
                            "description": "Whether to allow the user to input custom \"Other\" free text.",
                            "default": false
                        }
                    },
                    "required": ["question", "options"]
                }
            }
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: Args = serde_json::from_str(args)?;
    use api::ask_user_question::question::QuestionType;
    use api::ask_user_question::{MultipleChoice, Option as PbOption, Question};

    let questions: Vec<Question> = parsed
        .questions
        .into_iter()
        .map(|q| {
            let options: Vec<PbOption> = q
                .options
                .into_iter()
                .map(|label| PbOption { label })
                .collect();
            Question {
                question_id: Uuid::new_v4().to_string(),
                question: q.question,
                question_type: Some(QuestionType::MultipleChoice(MultipleChoice {
                    options,
                    recommended_option_index: q.recommended_index,
                    is_multiselect: q.multi_select,
                    supports_other: q.supports_other,
                })),
            }
        })
        .collect();

    Ok(api::message::tool_call::Tool::AskUserQuestion(
        api::AskUserQuestion { questions },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::ask_user_question_result::answer_item::Answer as A;
    use api::ask_user_question_result::Result as AR;
    use api::message::tool_call_result::Result as R;
    let r = match result {
        R::AskUserQuestion(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(AR::Success(s)) => {
            let answers: Vec<Value> = s
                .answers
                .iter()
                .map(|item| match &item.answer {
                    Some(A::MultipleChoice(mc)) => json!({
                        "question_id": item.question_id,
                        "selected": mc.selected_options,
                        "other_text": if mc.other_text.is_empty() {
                            Value::Null
                        } else {
                            Value::String(mc.other_text.clone())
                        },
                    }),
                    Some(A::Skipped(_)) => json!({
                        "question_id": item.question_id,
                        "skipped": true,
                    }),
                    None => json!({ "question_id": item.question_id, "no_answer": true }),
                })
                .collect();
            json!({ "status": "ok", "answers": answers })
        }
        Some(AR::Error(e)) => json!({ "status": "error", "message": e.message }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static ASK_USER_QUESTION: OpenAiTool = OpenAiTool {
    name: "ask_user_question",
    description: include_str!("../prompts/tool_descriptions/ask_user_question.md"),
    parameters,
    from_args,
    result_to_json,
};
