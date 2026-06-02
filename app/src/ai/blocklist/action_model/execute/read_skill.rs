use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
#[cfg(feature = "local_fs")]
use crate::ai::agent::AIAgentActionResultType;
use crate::ai::skills::{SkillManager, SkillTelemetryEvent};
#[cfg(feature = "local_fs")]
use crate::ai::skills::extract_skill_parent_directory;
use crate::send_telemetry_from_ctx;
use ai::agent::action_result::AnyFileContent;
use ai::skills::SkillReference;
#[cfg(feature = "local_fs")]
use ai::skills::parse_skill;
use std::path::Path;
use warpui::{ModelContext, SingletonEntity};

use crate::ai::agent::AIAgentActionType;
use crate::ai::agent::ReadSkillRequest;
use crate::ai::agent::ReadSkillResult;
use ai::agent::action_result::FileContext;
use futures::future::{BoxFuture, FutureExt};
use warpui::Entity;

pub struct ReadSkillExecutor;

impl ReadSkillExecutor {
    pub fn new() -> Self {
        Self
    }

    pub(super) fn should_autoexecute(
        &self,
        _input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> bool {
        // User-created skills are readable on demand.
        true
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::ReadSkill(ReadSkillRequest { skill: skill_ref }) = &action.action
        else {
            return ActionExecution::InvalidAction;
        };

        let manager = SkillManager::as_ref(ctx);

        // Cache hit: `SkillReference::Path(p)` in proto matches at this step only when p
        // happens to be the exact absolute path to a real SKILL.md in the index.
        if let Some(skill) = manager.skill_by_reference(skill_ref) {
            send_telemetry_from_ctx!(
                SkillTelemetryEvent::Read {
                    reference: skill_ref.clone(),
                    name: Some(skill.name.clone()),
                    scope: Some(skill.scope),
                    provider: Some(skill.provider),
                    error: false,
                },
                ctx
            );
            return success_execution(skill);
        }

        // The argument for the BYOP `read_skill` tool is a skill **name**, which is packed into
        // the `SkillReference::SkillPath(name)` slot by `from_args` (to avoid changes to the proto schema).
        // On cache miss, we lookup the real SKILL.md path by name, covering all skills visible
        // to the Skill Manager (file skills + bundled skills).
        if let SkillReference::Path(p) = skill_ref {
            if let Some(candidate_name) = name_candidate(p) {
                if let Some(skill) = manager.find_skill_by_name(candidate_name) {
                    send_telemetry_from_ctx!(
                        SkillTelemetryEvent::Read {
                            reference: skill_ref.clone(),
                            name: Some(skill.name.clone()),
                            scope: Some(skill.scope),
                            provider: Some(skill.provider),
                            error: false,
                        },
                        ctx
                    );
                    return success_execution(skill);
                }
            }
        }

        // Cache miss fallback: For references in the form of `SkillReference::Path`,
        // if the path shape is a valid skill file
        // (`.../<provider>/skills/<name>/SKILL.md` or under the warp managed skill directory),
        // we directly read and parse from disk, fixing the scenario where the skill exists
        // but the cache is not warm, as described in issue #99.
        //
        // Design trade-offs:
        // - Do not actively warm the SkillManager cache. The cache is maintained unidirectionally by
        //   SkillWatcher; writing to it here would break the data flow. Repeatedly calling read_skill on the
        //   same path will result in multiple reads from disk, but SKILL.md is typically small enough to ignore.
        // - `extract_skill_parent_directory` only validates the path shape, which shares the same security
        //   level as the path returned on cache hit - neither is restricted by a home directory prefix. This is intentional:
        //   skills inside projects (`/some/repo/.agents/skills/...`) also need to be readable.
        // - On Windows, path separators are backslashes, so Linux-style `/home/<u>/...` paths will be
        //   rejected; this means this fallback does not work for a "Windows host + WSL session" setup,
        //   which is a known limitation of issue #99 (see PR description).
        // The cache miss fallback is only available in builds with a local filesystem;
        // in builds without a filesystem (such as WASM), `extract_skill_parent_directory` and `parse_skill`
        // do not exist, so reading from disk is not possible.
        #[cfg(feature = "local_fs")]
        if let SkillReference::Path(path) = skill_ref {
            if extract_skill_parent_directory(path).is_ok() {
                let path = path.clone();
                let skill_ref_for_async = skill_ref.clone();
                return ActionExecution::new_async(
                    async move { parse_skill(&path) },
                    move |parsed, _app| match parsed {
                        Ok(skill) => AIAgentActionResultType::ReadSkill(
                            ReadSkillResult::Success {
                                content: FileContext::new(
                                    skill.path.to_string_lossy().into_owned(),
                                    AnyFileContent::StringContent(skill.content.clone()),
                                    skill.line_range.clone(),
                                    None,
                                ),
                            },
                        ),
                        Err(err) => AIAgentActionResultType::ReadSkill(
                            ReadSkillResult::Error(format!(
                                "Skill not found: {skill_ref_for_async:?} ({err})"
                            )),
                        ),
                    },
                );
            }
        }

        send_telemetry_from_ctx!(
            SkillTelemetryEvent::Read {
                reference: skill_ref.clone(),
                name: None,
                scope: None,
                provider: None,
                error: true,
            },
            ctx
        );
        ActionExecution::Sync(
            ReadSkillResult::Error(format!("Skill not found: {:?}", skill_ref)).into(),
        )
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

/// Build a sync success execution from a parsed skill.
///
/// Extracting this helper ensures that the generic type `T` of `ActionExecution<T>` resolves to
/// the same type in both the `success_execution` and `new_async` paths (otherwise Rust would require
/// the function to explicitly declare its return type).
fn success_execution(skill: &ai::skills::ParsedSkill) -> ActionExecution<anyhow::Result<ai::skills::ParsedSkill>> {
    let content = FileContext::new(
        skill.path.to_string_lossy().into_owned(),
        AnyFileContent::StringContent(skill.content.clone()),
        skill.line_range.clone(),
        None,
    );
    ActionExecution::Sync(ReadSkillResult::Success { content }.into())
}

/// Determine whether the value in `SkillReference::Path` should be treated as a skill **name** lookup.
///
/// A real SKILL.md path contains path separators (`/` or `\`) or is an absolute path, whereas a BYOP
/// tool-called name (like `"build-feature"`) is a plain string. Separating these two cases prevents
/// `/home/.../SKILL.md` from being misinterpreted as a name and skipping the filesystem fallback.
fn name_candidate(p: &Path) -> Option<&str> {
    if p.is_absolute() {
        return None;
    }
    let s = p.to_str()?;
    if s.is_empty() || s.contains('/') || s.contains('\\') {
        return None;
    }
    Some(s)
}

impl Entity for ReadSkillExecutor {
    type Event = ();
}

#[cfg(test)]
#[path = "read_skill_tests.rs"]
mod tests;
