//! BYOP system prompt template rendering.
//!
//! `AIAgentContext` that has been collected by the warp client (env / git / skills / project_rules / current_time)
//! A `system` message string rendered as an OpenAI compatible endpoint.
//!
//! ## Workflow
//!
//! 1. Extract the latest `UserQuery.context: Arc<[AIAgentContext]>` from `params.input`
//!    (warp `convert_to.rs::convert_input` also takes the same copy)
//! 2. `collect_prompt_context` Pat each enum variant into a flat `PromptContext` struct
//! 3. `pick_template` matches the model id substring and selects `system/{anthropic,gpt,beast,codex,
//!    gemini,kimi,trinity,default}.j2`(align opencode
//!    `packages/opencode/src/session/system.ts::provider`)
//! 4. minijinja rendering
//!
//! ## Template loading
//!
//! All templates `include_str!` are compiled into binary (zero runtime IO), and templates need to be recompiled.

use std::sync::OnceLock;

use ai::LLMId;
use chrono::Local;
use minijinja::{Environment, Value};
use serde::Serialize;

use crate::ai::agent::AIAgentContext;

// ---------------------------------------------------------------------------
// Template environment
// ---------------------------------------------------------------------------

static ENV: OnceLock<Environment<'static>> = OnceLock::new();

fn build_env() -> Environment<'static> {
    let mut env = Environment::new();

    // Partials
    env.add_template("partials/env.j2", include_str!("prompts/partials/env.j2"))
        .expect("env partial parses");
    env.add_template(
        "partials/skills.j2",
        include_str!("prompts/partials/skills.j2"),
    )
    .expect("skills partial parses");
    env.add_template(
        "partials/project_rules.j2",
        include_str!("prompts/partials/project_rules.j2"),
    )
    .expect("project_rules partial parses");
    env.add_template(
        "partials/user_rules.j2",
        include_str!("prompts/partials/user_rules.j2"),
    )
    .expect("user_rules partial parses");
    env.add_template(
        "partials/tool_aliases.j2",
        include_str!("prompts/partials/tool_aliases.j2"),
    )
    .expect("tool_aliases partial parses");
    env.add_template(
        "partials/footer.j2",
        include_str!("prompts/partials/footer.j2"),
    )
    .expect("footer partial parses");
    env.add_template(
        "partials/plan_mode.j2",
        include_str!("prompts/partials/plan_mode.j2"),
    )
    .expect("plan_mode partial parses");
    env.add_template(
        "commands/init_project.j2",
        include_str!("prompts/commands/init_project.j2"),
    )
    .expect("init_project command template parses");
    env.add_template(
        "commands/generate_schema.j2",
        include_str!("prompts/commands/generate_schema.j2"),
    )
    .expect("generate_schema command template parses");

    // Distribute system prompt (aligned opencode) by model id substring matching
    // `packages/opencode/src/session/system.ts::provider`). The OpenRouter path looks like
    // `anthropic/claude-3.5-sonnet` / `google/gemini-2.5-flash` / `openai/gpt-4o`
    // Can also hit correctly. If the family cannot be identified, default.j2 will be used, so it is safe to customize the model id.
    for (name, src) in [
        (
            "system/default.j2",
            include_str!("prompts/system/default.j2") as &str,
        ),
        (
            "system/anthropic.j2",
            include_str!("prompts/system/anthropic.j2"),
        ),
        ("system/gpt.j2", include_str!("prompts/system/gpt.j2")),
        ("system/beast.j2", include_str!("prompts/system/beast.j2")),
        ("system/codex.j2", include_str!("prompts/system/codex.j2")),
        ("system/gemini.j2", include_str!("prompts/system/gemini.j2")),
        ("system/kimi.j2", include_str!("prompts/system/kimi.j2")),
        (
            "system/trinity.j2",
            include_str!("prompts/system/trinity.j2"),
        ),
    ] {
        env.add_template(name, src)
            .unwrap_or_else(|e| panic!("template {name} parses: {e}"));
    }

    env
}

fn env() -> &'static Environment<'static> {
    ENV.get_or_init(build_env)
}

// ---------------------------------------------------------------------------
// Template selection
// ---------------------------------------------------------------------------

/// Select template by matching model id substring (align opencode
/// `packages/opencode/src/session/system.ts::provider`)。
///
/// Matching rules (order sensitive, first come first served):
/// - `gpt-4` / `o1` / `o3` / `o4` → beast (strong autonomy + sequential thinking)
/// - Other `gpt` contains `codex` → codex(apply_file_diffs + strict final answer formatting)
/// - Other `gpt` → gpt(pragmatic engineer + commentary/final dual channel)
/// - `gemini-` → gemini(Core Mandates + Workflows + lots of examples)
/// - `claude` / `sonnet` / `opus` / `haiku` → anthropic(Claude Code style)
/// - `trinity` → trinity(a tool a message style)
/// - `kimi` → kimi(SAME language + AGENTS.md)
/// - Others → default.j2 (all in all)
///
/// Matching after lowercase throughout, compatible with `GPT-4o` / `OPENAI/gpt-4o` / `Anthropic/Claude-3.5`
/// This is user-case writing. The OpenRouter form `provider/model` also hits correctly.
pub fn pick_template(model_id: &str) -> &'static str {
    let id = model_id.to_ascii_lowercase();

    if id.contains("gpt-4") || id.contains("o1") || id.contains("o3") || id.contains("o4") {
        return "system/beast.j2";
    }
    if id.contains("gpt") {
        if id.contains("codex") {
            return "system/codex.j2";
        }
        return "system/gpt.j2";
    }
    if id.contains("gemini-") {
        return "system/gemini.j2";
    }
    if id.contains("claude") || id.contains("sonnet") || id.contains("opus") || id.contains("haiku")
    {
        return "system/anthropic.j2";
    }
    if id.contains("trinity") {
        return "system/trinity.j2";
    }
    if id.contains("kimi") {
        return "system/kimi.j2";
    }
    "system/default.j2"
}

/// Extract the model id string from `LLMId`. BYOP encoding will take the model part,
/// Otherwise, it will be returned as is (theoretically, the BYOP path will only transmit the BYOP id, but be careful).
fn model_id_from_llm_id(id: &LLMId) -> String {
    if let Some((_pid, mid)) = super::llm_id::decode(id) {
        mid
    } else {
        id.as_str().to_owned()
    }
}

// ---------------------------------------------------------------------------
// AIAgentContext → flat template context
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize)]
struct ShellCtx {
    name: String,
    version: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct OsCtx {
    platform: String,
    distribution: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct GitCtx {
    head: String,
    branch: Option<String>,
}

#[derive(Debug, Serialize)]
struct SkillCtx {
    name: String,
    description: String,
    /// Absolute path to SKILL.md for filesystem skills; `None` for bundled skills.
    /// Bundled skills are loaded via `AIAgentInput::InvokeSkill`, not `read_skill`,
    /// so exposing `@warp-skill:<id>` here would mislead the model into calling a
    /// path that always fails the BYOP `skill_by_reference` lookup.
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProjectRuleCtx {
    path: String,
    content: String,
}

/// Waz BYOP fix Issue #116: Global Rules (user created in Settings → Agents → Rules)
/// A flat view, fed into `partials/user_rules.j2` that renders into the system prompt.
#[derive(Debug, Serialize)]
struct UserRuleCtx {
    name: Option<String>,
    content: String,
}

#[derive(Debug, Default, Serialize)]
struct InitProjectCommandContext {
    arguments: String,
}

#[derive(Debug, Default, Serialize)]
struct GenerateSchemaCommandContext {
    tool: String,
}

#[derive(Debug, Default, Serialize)]
struct PromptContext {
    cwd: Option<String>,
    shell: Option<ShellCtx>,
    os: Option<OsCtx>,
    git: Option<GitCtx>,
    skills: Vec<SkillCtx>,
    project_rules: Vec<ProjectRuleCtx>,
    /// Waz BYOP fix Issue #116: by caller(`render_system`) from
    /// `RequestParams.user_rules` is injected and rendered by `partials/user_rules.j2`.
    user_rules: Vec<UserRuleCtx>,
    current_time: String,
    model_id: String,
    /// The list of tool names actually fed to the upstream model in this round (provided by `chat_stream::available_tool_names`
    /// Calculation, including built-in tools and current MCP tools after gating).
    /// The template dynamically renders the whitelist according to this, no longer hard-coded.
    available_tools: Vec<String>,
    /// Whether this round is in Plan Mode (read-only research mode) triggered by `/plan`.
    /// Calculated by `chat_stream::is_plan_mode_turn`, the template is included here
    /// `partials/plan_mode.j2` injects read-only constraints + plan output guidance.
    plan_mode: bool,
    tmp_context: Option<String>,
}

fn collect_prompt_context(model_id: &str, ctx: &[AIAgentContext]) -> PromptContext {
    let mut out = PromptContext {
        // P0-1 prompt cache optimization: `current_time` is only retained to the natural day granularity,
        // No longer precise to the second. reason:
        // - Any content in the system prompt that changes with each request will cause Anthropic's first
        //   The hash written by system breakpoint is unique → it will be discarded after writing and will never hit.
        //   OpenAI's first 256 token routing hashes will be distributed to different machines in the same way.
        // - The model actually only needs to know "what day is today", skipping the natural day once
        //   The cost of a miss is acceptable (one day × all active conversations × system tokens).
        // - The cost will be the same across years as it will be across days, and no additional processing is required.
        // In the future, you can consider moving the "current time" to the end of the user message (P0-1 Plan C).
        // Make the system segment 100% stable; in this step, take the low-risk option B first.
        current_time: Local::now().format("%Y-%m-%d").to_string(),
        model_id: model_id.to_owned(),
        ..Default::default()
    };

    for c in ctx {
        match c {
            AIAgentContext::Directory { pwd, .. } => {
                if out.cwd.is_none() {
                    out.cwd = pwd.clone();
                }
            }
            AIAgentContext::ExecutionEnvironment(exec) => {
                out.shell = Some(ShellCtx {
                    name: exec.shell_name.clone(),
                    version: exec.shell_version.clone(),
                });
                let has_os = exec.os.category.is_some() || exec.os.distribution.is_some();
                if has_os {
                    out.os = Some(OsCtx {
                        platform: exec.os.category.clone().unwrap_or_default(),
                        distribution: exec.os.distribution.clone(),
                    });
                }
            }
            AIAgentContext::CurrentTime { current_time } => {
                // P0-1: Consistent with the default value, only retaining the natural day granularity.
                // The upstream Waz may pass in a timestamp accurate to seconds, and here it is uniformly pressed to the "current date".
                out.current_time = current_time.format("%Y-%m-%d").to_string();
            }
            // The code indexing function is not implemented, and the Codebase context does not enter the system prompt.
            AIAgentContext::Codebase { .. } => {}
            // P1-7 prompt cache Description: `Git { head, branch }` depends on the current warehouse status,
            // User cutting branches will cause the rendered system segment to change, causing all upstream suppliers to
            // (Anthropic / OpenAI / DeepSeek) system+messages cache are all invalid.
            // This is **expected behavior**:
            //   - Instruction model cannot be considered as old git context on new branch;
            //   - As a cost, the user first requests 100% miss on the new branch and writes to the new cache, and then the
            //     Branches will be reused. Developers who jump frequently across branches will see the most misses.
            // Alternatives considered: Move the git status to the end of the user message (same as P0-1 Plan C),
            // But in that case, the system section will lose the contextual meaning of "the model can know the current branch at a glance".
            // Models that rely on it for inference will suffer. This patch maintains the status quo.
            AIAgentContext::Git { head, branch } => {
                out.git = Some(GitCtx {
                    head: head.clone(),
                    branch: branch.clone(),
                });
            }
            AIAgentContext::Skills { skills } => {
                for s in skills {
                    let path = match &s.reference {
                        ai::skills::SkillReference::Path(p) => {
                            Some(p.to_string_lossy().into_owned())
                        }
                        // Bundled skills load via InvokeSkill, not read_skill.
                        // Omit skill_path to avoid guiding the model toward a
                        // value that will always fail BYOP's skill_by_reference.
                        ai::skills::SkillReference::BundledSkillId(_) => None,
                    };
                    out.skills.push(SkillCtx {
                        name: s.name.clone(),
                        description: s.description.clone(),
                        path,
                    });
                }
            }
            AIAgentContext::ProjectRules {
                root_path,
                active_rules,
                ..
            } => {
                use ai::agent::action_result::AnyFileContent;
                for rule in active_rules {
                    let content = match &rule.content {
                        AnyFileContent::StringContent(s) => s.clone(),
                        AnyFileContent::BinaryContent(_) => continue,
                    };
                    let path = if rule.file_name.starts_with('/') {
                        rule.file_name.clone()
                    } else {
                        format!("{root_path}/{}", rule.file_name)
                    };
                    out.project_rules.push(ProjectRuleCtx { path, content });
                }
            }
            // User attachment class context(File / Image / SelectedText / Block) does not enter the system prompt,
            // By `user_context::render_user_attachments` in the UserQuery branch of chat_stream
            // Inject into the current round user message. This is aligned with the semantics of warp's own paths being divided into two categories:
            // - Environment type → InputContext.{directory,shell,git,...} → Backend injection system area
            // - Attachment type → InputContext.{executed_shell_commands,selected_text,files,images}
            //            → Inject the user area into the backend
            AIAgentContext::File(_)
            | AIAgentContext::Image(_)
            | AIAgentContext::SelectedText(_)
            | AIAgentContext::Block(_) => {}
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn render_init_project_command(arguments: Option<&str>) -> String {
    let arguments = arguments
        .map(str::trim)
        .filter(|arguments| !arguments.is_empty())
        .unwrap_or("(none)")
        .to_owned();
    let ctx = InitProjectCommandContext { arguments };
    let env = env();
    let template_name = "commands/init_project.j2";
    let tmpl = match env.get_template(template_name) {
        Ok(t) => t,
        Err(e) => {
            log::error!("[byop prompt] failed to get template {template_name}: {e}");
            return fallback_init_project_command(&ctx.arguments);
        }
    };
    match tmpl.render(Value::from_serialize(&ctx)) {
        Ok(s) => s,
        Err(e) => {
            log::error!("[byop prompt] render {template_name} failed: {e}");
            fallback_init_project_command(&ctx.arguments)
        }
    }
}

pub fn render_generate_schema_command(tool: &str) -> String {
    let ctx = GenerateSchemaCommandContext {
        tool: tool.trim().to_owned(),
    };
    let env = env();
    let template_name = "commands/generate_schema.j2";
    let tmpl = match env.get_template(template_name) {
        Ok(t) => t,
        Err(e) => {
            log::error!("[byop prompt] failed to get template {template_name}: {e}");
            return fallback_generate_schema_command(&ctx.tool);
        }
    };
    match tmpl.render(Value::from_serialize(&ctx)) {
        Ok(s) => s,
        Err(e) => {
            log::error!("[byop prompt] render {template_name} failed: {e}");
            fallback_generate_schema_command(&ctx.tool)
        }
    }
}

/// Render the system message string ultimately sent to the upstream model.
///
/// `ctx` usually comes from the latest `AIAgentInput::UserQuery.context` in `params.input`.
/// It's OK even if you can't get the context (empty array) - the template will be rendered using the default placeholder.
///
/// `available_tools` is calculated by `chat_stream::available_tool_names`, and is actually exposed to
/// List of tool names for upstream LLM (built-in + MCP, gating applied). The template dynamically renders the whitelist according to this,
/// Stop hardcoding the "unavailable tools" blacklist - tools that are not visible to the model will naturally not be adjusted.
/// On the other hand, using a text blacklist will prevent the model from even adjusting real usable tools.
pub fn render_system(
    model: &LLMId,
    ctx: &[AIAgentContext],
    available_tools: &[String],
    plan_mode: bool,
    user_rules: &[(Option<String>, String)],
    query: Option<&str>,
) -> String {
    let model_id = model_id_from_llm_id(model);
    let template_name = pick_template(&model_id);
    let mut prompt_ctx = collect_prompt_context(&model_id, ctx);
    prompt_ctx.available_tools = available_tools.to_vec();
    prompt_ctx.plan_mode = plan_mode;
    prompt_ctx.user_rules = user_rules
        .iter()
        .map(|(name, content)| UserRuleCtx {
            name: name.clone(),
            content: content.clone(),
        })
        .collect();

    let tmp_context = prompt_ctx.cwd.as_ref().and_then(|cwd| {
        warp_completer::signatures::tmp::get_active_tmp_prompt(cwd, query)
    });
    prompt_ctx.tmp_context = tmp_context;

    let env = env();
    let tmpl = match env.get_template(template_name) {
        Ok(t) => t,
        Err(e) => {
            log::error!("[byop prompt] failed to get template {template_name}: {e}");
            return fallback_system(&model_id);
        }
    };
    match tmpl.render(Value::from_serialize(&prompt_ctx)) {
        Ok(s) => s,
        Err(e) => {
            log::error!("[byop prompt] render {template_name} failed: {e}");
            fallback_system(&model_id)
        }
    }
}

fn fallback_init_project_command(arguments: &str) -> String {
    format!(
        "Create or update `AGENTS.md` for this repository.\n\nUser-provided focus or constraints (honor these):\n{arguments}"
    )
}

fn fallback_generate_schema_command(tool: &str) -> String {
    format!(
        "Generate a Token Model Protocol (TMP) JSON schema for the CLI tool `{tool}`. \
         Please identify its most common subcommands, options, and parameters, and design the schema."
    )
}

/// Rendering system (only used when template loading/rendering fails, should not be triggered in normal paths).
fn fallback_system(model_id: &str) -> String {
    format!(
        "You are the AI coding agent inside Waz, an AI Development Environment (ADE). \
         Model: {model_id}. \
         Use the registered tools (run_shell_command / read_files / apply_file_diffs / grep / file_glob / ...) \
         to take actions on the user's behalf. Be concise."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::agent::AIAgentContext;
    use crate::ai_assistant::execution_context::{WarpAiExecutionContext, WarpAiOsContext};

    #[test]
    fn render_init_project_command_uses_command_template_arguments() {
        let out = render_init_project_command(Some("focus on test commands"));
        assert!(out.contains("Create or update `AGENTS.md`"), "{out}");
        assert!(out.contains("focus on test commands"), "{out}");
        assert!(out.contains("## Writing rules"), "{out}");
    }

    #[test]
    fn render_generate_schema_command_uses_template() {
        let out = render_generate_schema_command("docker");
        assert!(out.contains("Generate a Token Model Protocol (TMP) JSON schema for the CLI tool \"docker\""), "{out}");
    }

    #[test]
    fn pick_template_dispatches_by_model_family() {
        // Direct connection mode
        for (id, want) in [
            ("claude-sonnet-4-5", "system/anthropic.j2"),
            ("claude-opus-4-1", "system/anthropic.j2"),
            ("haiku-3-5", "system/anthropic.j2"),
            ("gpt-4o", "system/beast.j2"),
            ("gpt-4-turbo", "system/beast.j2"),
            ("o1-preview", "system/beast.j2"),
            ("o3-mini", "system/beast.j2"),
            ("o4-mini", "system/beast.j2"),
            ("gpt-5-codex", "system/codex.j2"),
            ("gpt-3.5-turbo", "system/gpt.j2"),
            ("gemini-2.0-flash", "system/gemini.j2"),
            ("gemini-2.5-pro", "system/gemini.j2"),
            ("kimi-k2", "system/kimi.j2"),
            ("trinity-v1", "system/trinity.j2"),
            // reveal all the details
            ("deepseek-chat", "system/default.j2"),
            ("qwen2.5-coder", "system/default.j2"),
            ("glm-4", "system/default.j2"),
            ("my-custom-model", "system/default.j2"),
            ("", "system/default.j2"),
        ] {
            assert_eq!(pick_template(id), want, "id={id}");
        }
    }

    #[test]
    fn pick_template_handles_openrouter_path_form() {
        // OpenRouter form `provider/model`, substring matching still hits the correct family
        for (id, want) in [
            ("anthropic/claude-3.5-sonnet", "system/anthropic.j2"),
            ("anthropic/claude-opus-4", "system/anthropic.j2"),
            ("openai/gpt-4o", "system/beast.j2"),
            ("openai/gpt-5-codex", "system/codex.j2"),
            ("openai/o1-preview", "system/beast.j2"),
            ("google/gemini-2.5-flash", "system/gemini.j2"),
            ("moonshot/kimi-k2", "system/kimi.j2"),
        ] {
            assert_eq!(pick_template(id), want, "id={id}");
        }
    }

    #[test]
    fn pick_template_is_case_insensitive() {
        for (id, want) in [
            ("Claude-Sonnet-4", "system/anthropic.j2"),
            ("GPT-4o", "system/beast.j2"),
            ("Gemini-2.5-Pro", "system/gemini.j2"),
            ("KIMI-K2", "system/kimi.j2"),
            ("Anthropic/Claude-3.5", "system/anthropic.j2"),
        ] {
            assert_eq!(pick_template(id), want, "id={id}");
        }
    }

    #[test]
    fn render_includes_env_block_with_cwd_and_shell() {
        let ctx = vec![
            AIAgentContext::Directory {
                pwd: Some("/home/user/project".into()),
                home_dir: Some("/home/user".into()),
                are_file_symbols_indexed: false,
            },
            AIAgentContext::ExecutionEnvironment(WarpAiExecutionContext {
                os: WarpAiOsContext {
                    category: Some("linux".into()),
                    distribution: Some("Ubuntu 22.04".into()),
                },
                shell_name: "bash".into(),
                shell_version: Some("5.1".into()),
            }),
        ];
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &ctx, &[], false, &[], None);
        assert!(
            out.contains("Working directory: /home/user/project"),
            "{out}"
        );
        assert!(out.contains("Shell: bash 5.1"), "{out}");
        assert!(out.contains("linux (Ubuntu 22.04)"), "{out}");
        // The home field has been aligned and cut off by opencode and will no longer be rendered.
        assert!(!out.contains("Home directory:"), "{out}");
    }

    #[test]
    fn render_produces_non_empty_for_all_families() {
        // Any model id can render a non-empty string (including Waz self-identification).
        for id in [
            "claude-sonnet-4-5",
            "gpt-4o",
            "gpt-5-codex",
            "gemini-2.5-pro",
            "kimi-k2",
            "trinity-v1",
            "deepseek-chat",
            "weird-model",
        ] {
            let out = render_system(
                &LLMId::from(format!("byop:p:{id}").as_str()),
                &[],
                &[],
                false,
                &[],
                None,
            );
            assert!(
                out.contains("Waz"),
                "id={id} should mention Waz, got: {out}"
            );
        }
    }

    #[test]
    fn render_omits_skills_block_when_empty() {
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &[], &[], false, &[], None);
        // The skills block should not appear when there are no skills
        assert!(
            !out.contains("Skills provide specialized instructions"),
            "{out}"
        );
    }

    /// Issue #169 Regression: The skill block in the system prompt must contain skill_path (absolute path),
    /// Rather than just name/description, otherwise the model cannot call the read_skill tool correctly.
    #[test]
    fn render_includes_skill_path_for_read_skill_tool() {
        use crate::ai::skills::SkillDescriptor;
        use ai::skills::{SkillProvider, SkillReference, SkillScope};

        let skill_path = "/home/user/.agents/skills/open-browser-use/SKILL.md";
        let skill = SkillDescriptor {
            reference: SkillReference::Path(skill_path.into()),
            name: "open-browser-use".into(),
            description: "Automates Chrome browser operations.".into(),
            scope: SkillScope::Project,
            provider: SkillProvider::Agents,
            icon_override: None,
        };
        let ctx = vec![AIAgentContext::Skills {
            skills: vec![skill],
        }];
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &ctx, &[], false, &[], None);
        assert!(
            out.contains(skill_path),
            "system prompt must expose the skill_path so the model can pass it to read_skill; got: {out}"
        );
    }

    /// Issue #169 Follow-up: BundledSkillId variant of bundled skill is not passable under BYOP path
    /// read_skill is loaded (invokeSkill), so <skill_path> should not be output in the system prompt
    /// To avoid models using @warp-skill:{id} values ​​that are bound to fail.
    #[test]
    fn render_omits_skill_path_for_bundled_skill() {
        use crate::ai::skills::SkillDescriptor;
        use ai::skills::{SkillProvider, SkillReference, SkillScope};
        use warp_core::ui::icons::Icon;

        let skill = SkillDescriptor {
            reference: SkillReference::BundledSkillId("find-skills".into()),
            name: "find-skills".into(),
            description: "Help discover and install new agent skills.".into(),
            scope: SkillScope::Bundled,
            provider: SkillProvider::Waz,
            icon_override: Some(Icon::WarpLogoLight),
        };
        let ctx = vec![AIAgentContext::Skills {
            skills: vec![skill],
        }];
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &ctx, &[], false, &[], None);
        assert!(
            out.contains("find-skills"),
            "bundled skill name should still appear in prompt: {out}"
        );
        assert!(
            !out.contains("@warp-skill:"),
            "bundled skill must NOT emit <skill_path> to avoid misleading the model: {out}"
        );
        assert!(
            !out.contains("<skill_path>"),
            "no <skill_path> tag should be rendered for bundled skills: {out}"
        );
    }

    #[test]
    fn fallback_does_not_panic() {
        // render_system will never panic, and will fallback_system if it fails.
        let out = render_system(&LLMId::from("byop:p:any"), &[], &[], false, &[], None);
        assert!(!out.is_empty());
    }

    #[test]
    fn render_lists_available_tools_dynamically() {
        // The incoming tool name must appear in the system prompt (dynamic whitelist)
        let tools: Vec<String> = vec![
            "run_shell_command".into(),
            "webfetch".into(),
            "websearch".into(),
            "mcp__github__create_issue".into(),
        ];
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &[], &tools, false, &[], None);
        for name in &tools {
            assert!(
                out.contains(name),
                "expected `{name}` in prompt, got: {out}"
            );
        }
        // The old blacklist wording should no longer appear
        assert!(
            !out.contains("Do not call unavailable tools"),
            "Blacklist section should be deleted: {out}"
        );
    }

    #[test]
    fn render_omits_tool_list_when_empty() {
        // tool_names is empty (theoretically it will not happen, the bottom line is: the whitelist segment will not be rendered)
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &[], &[], false, &[], None);
        assert!(!out.contains("Available Tools"), "{out}");
    }

    #[test]
    fn plan_mode_off_omits_plan_block() {
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &[], &[], false, &[], None);
        assert!(
            !out.contains("Plan Mode (Read-Only)"),
            "plan_mode=false should not contain Plan Mode section: {out}"
        );
    }

    #[test]
    fn plan_mode_on_injects_plan_block_for_all_families() {
        for id in [
            "claude-sonnet-4-5",
            "gpt-4o",
            "gpt-5-codex",
            "gemini-2.5-pro",
            "kimi-k2",
            "trinity-v1",
            "deepseek-chat",
            "weird-model",
        ] {
            let out = render_system(
                &LLMId::from(format!("byop:p:{id}").as_str()),
                &[],
                &[],
                true,
                &[],
                None,
            );
            assert!(
                out.contains("Plan Mode (Read-Only)"),
                "id={id} plan_mode=true should contain Plan Mode section: {out}"
            );
            assert!(
                out.contains("Stop and wait"),
                "id={id} plan_mode=true should contain Stop and wait guide: {out}"
            );
        }
    }

    // Issue #116: Global Rules (created by users in Settings → Agents → Rules) must be injected into the system prompt.
    // The following three use cases cover key branches of `partials/user_rules.j2`.

    #[test]
    fn render_omits_user_rules_block_when_empty() {
        let out = render_system(&LLMId::from("byop:p:deepseek-chat"), &[], &[], false, &[], None);
        assert!(
            !out.contains("# User rules"),
            "user rules block should not be rendered when user_rules is empty: {out}"
        );
    }

    #[test]
    fn render_includes_user_rules_when_present() {
        let rules = vec![(
            Some("My rule".to_string()),
            "Always use snake_case in Rust.".to_string(),
        )];
        let out = render_system(
            &LLMId::from("byop:p:deepseek-chat"),
            &[],
            &[],
            false,
            &rules,
            None,
        );
        assert!(out.contains("# User rules"), "should render user rules block: {out}");
        assert!(out.contains("## My rule"), "should contain rule name: {out}");
        assert!(
            out.contains("Always use snake_case in Rust."),
            "should contain rule content: {out}"
        );
    }

    #[test]
    fn render_includes_user_rules_across_all_template_families() {
        // user_rules.j2 is injected by footer.j2, and all system template families reference footer.
        // This regression use case ensures that anthropic/beast/codex/gemini/kimi/trinity/
        // By default, any template family will render user rules, and injection will not be missed because a certain family does not have a footer.
        let rules = vec![(Some("Family Coverage".to_string()), "snake_case only.".to_string())];
        for id in [
            "claude-sonnet-4-5",
            "gpt-4o",
            "gpt-5-codex",
            "gemini-2.5-pro",
            "kimi-k2",
            "trinity-v1",
            "deepseek-chat",
            "weird-model",
        ] {
            let out = render_system(
                &LLMId::from(format!("byop:p:{id}").as_str()),
                &[],
                &[],
                false,
                &rules,
                None,
            );
            assert!(
                out.contains("snake_case only."),
                "id={id} should contain user rule content: {out}"
            );
        }
    }

    #[test]
    fn render_user_rules_separates_multiple_rules_with_blank_line() {
        // Multiple rules should be separated by blank lines (`{% if not loop.last %}`), and no blank lines should be left after the last one.
        let rules = vec![
            (Some("R1".to_string()), "first content".to_string()),
            (Some("R2".to_string()), "second content".to_string()),
            (Some("R3".to_string()), "third content".to_string()),
        ];
        let out = render_system(
            &LLMId::from("byop:p:deepseek-chat"),
            &[],
            &[],
            false,
            &rules,
            None,
        );

        // There should be at least one "blank line" (two adjacent line breaks) between two rules.
        // Do not hard-code the specific number of line breaks because minijinja’s trim_blocks/lstrip_blocks default behavior
        // The specific number of line breaks determined is easy to change with the fine-tuning of the template (the reviewer actually measured 3 line breaks).
        // The contract we want is "visually blank lines + correct order".
        let pos_r1 = out.find("first content").expect("R1 content not found");
        let pos_r2 = out.find("## R2").expect("R2 title not found");
        let pos_r3 = out.find("## R3").expect("R3 title not found");
        assert!(pos_r1 < pos_r2 && pos_r2 < pos_r3, "order should be preserved: {out}");
        let between_r1_r2 = &out[pos_r1 + "first content".len()..pos_r2];
        let between_r2_r3 = &out[pos_r2..pos_r3];
        assert!(
            between_r1_r2.contains("\n\n"),
            "R1 and R2 should have a blank line in between, actual: {between_r1_r2:?}"
        );
        assert!(
            between_r2_r3.contains("\n\n"),
            "R2 and R3 should have a blank line in between, actual: {between_r2_r3:?}"
        );
    }

    #[test]
    fn render_user_rules_handles_no_name() {
        let rules = vec![(None, "Be terse.".to_string())];
        let out = render_system(
            &LLMId::from("byop:p:deepseek-chat"),
            &[],
            &[],
            false,
            &rules,
            None,
        );
        assert!(out.contains("# User rules"), "{out}");
        assert!(out.contains("Be terse."), "{out}");
        // An empty `##` header line should not be rendered without name
        assert!(
            !out.contains("## \n"),
            "empty '## ' header should not be rendered when name is missing: {out}"
        );
    }
}
