//! `read_skill`: Read Waz’s Skill markdown template.
//!
//! Skills are user/project predefined reusable workflows (`SKILL.md` file + optional metadata).
//! After the model reads the skill, it can advance the task according to the steps expected by the user. warp maintains its own `SkillManager`
//! Index all available skills, either by name (frontmatter `name` field) or by absolute path or
//! bundled id reference.
//!
//! ## Enter into the contract
//!
//! The BYOP path exposes the `name` field, the value is taken from the system prompt `<available_skills><skill><name>`.
//! `from_args` loads name into the `SkillReference::SkillPath` slot of proto (without changing proto),
//! When a cache miss occurs, the `read_skill` executor first searches for the real SKILL.md absolute path by name.
//! Read the disk again. This fallback is also compatible with models that directly pass absolute paths or bundled forms.
//! Old way of writing `@warp-skill:<id>`.
//!
//! ## Usage suggestions (write in description)
//!
//! The model can be actively adjusted in the following scenarios:
//! - User mentioned skill name / file name / path
//! - The task matches a certain skill description (such as "doing PR review" triggers the `review` skill)

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

#[derive(Debug, Deserialize)]
struct Args {
    name: String,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Skill name (must exactly match the <name> field inside <available_skills><skill> in the system prompt)."
            }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::read_skill::SkillReference;
    let parsed: Args = serde_json::from_str(args)?;
    // Reuse proto's `SkillPath` slot to carry name (avoid proto schema changes);
    // When a cache miss occurs, the executor side checks the real SKILL.md path by name.
    Ok(api::message::tool_call::Tool::ReadSkill(
        api::message::tool_call::ReadSkill {
            skill_reference: Some(SkillReference::SkillPath(parsed.name)),
            name: String::new(),
        },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::read_skill_result::Result as SR;
    let r = match result {
        R::ReadSkill(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Success(s)) => {
            // FileContent { file_path, content, line_range } is directly a single message
            // Not oneof, no need to unpack inner content.
            let (path, content) = s
                .content
                .as_ref()
                .map(|c| (c.file_path.clone(), c.content.clone()))
                .unwrap_or_default();
            json!({ "status": "ok", "path": path, "content": content })
        }
        Some(SR::Error(e)) => json!({ "status": "error", "message": e.message }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static READ_SKILL: OpenAiTool = OpenAiTool {
    name: "read_skill",
    description: include_str!("../prompts/tool_descriptions/read_skill.md"),
    parameters,
    from_args,
    result_to_json,
};
