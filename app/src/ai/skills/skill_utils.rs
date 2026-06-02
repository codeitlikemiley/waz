//! Utility functions for working with skills.

use super::{SkillDescriptor, SkillManager};
use crate::ai::blocklist::view_util::render_provider_icon_button;
use ai::skills::{
    home_skills_path, provider_rank, ParsedSkill, SkillProvider, SKILL_PROVIDER_DEFINITIONS,
};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::Icon;
use warpui::prelude::MouseStateHandle;
use warpui::EventContext;
use warpui::{AppContext, Element, SingletonEntity};

use crate::warp_managed_paths_watcher::warp_managed_skill_dirs;

/// Deduplicates skills by **name and owning directory**, keeping a single best representative per
/// skill name within each directory.
///
/// Priority rules (when there are multiple copies of the skill with the same name):
///
/// 1. **provider rank The smaller one wins**: in the order of [`SKILL_PROVIDER_DEFINITIONS`] (index 0 = highest priority),
///    For example `Agents > Waz > Claude > …`.
/// 2. **With the same rank, the one with the shortest reference path wins**: Take stable tiebreak.
///
/// This implementation covers three scenarios:
/// - `npx skills` soft links the skill with the same name to `~/.agents/skills/` / `~/.warp/skills/` / `~/.claude/skills/`
///   (Same name, different providers) → Keep high-priority providers.
/// - Skills with the same name exist in multiple directories at the same time (for example, repo root + subdir) → Each skill is reserved for the caller to process according to the path context.
/// - Different content with the same name (different providers) → retain the high-priority provider.
///
/// Each element of `skill_paths` is a `(dir_path, skill_file_path)` tuple where
/// `dir_path` is the directory that owns the skill and participates in the dedup key.
///
/// **P0-3 prompt cache trap**: Return Vec sorted in lexicographic order by `(name, reference)`.
/// Reason: `HashMap::into_values()` the iteration order is unstable, the return value will enter the system prompt
/// skills section, sequence drift will make all upstream suppliers (Anthropic / OpenAI / DeepSeek)
/// The prompt cache is completely invalid. Same nature as P0-3 MCP tools sorting.
/// Currently, `(name, owning directory)` is used to remove duplicates, so skills with the same name can be retained in different directories at the same time.
/// The reference still serves as the secondary key for stable sorting, ensuring that the output order is reproducible.
#[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
pub(crate) fn unique_skills(
    skill_paths: &[(PathBuf, PathBuf)],
    skills_by_path: &HashMap<PathBuf, ParsedSkill>,
) -> Vec<SkillDescriptor> {
    let mut name_map: HashMap<(String, PathBuf), SkillDescriptor> = HashMap::new();

    for (dir_path, path) in skill_paths {
        let Some(skill) = skills_by_path.get(path) else {
            continue;
        };
        let descriptor = SkillDescriptor::from(skill.clone());
        match name_map.entry((descriptor.name.clone(), dir_path.clone())) {
            Entry::Vacant(e) => {
                e.insert(descriptor);
            }
            Entry::Occupied(mut e) => {
                let new_rank = provider_rank(descriptor.provider);
                let existing_rank = provider_rank(e.get().provider);
                if new_rank < existing_rank
                    || (new_rank == existing_rank
                        && skill_reference_key(&descriptor.reference).len()
                            < skill_reference_key(&e.get().reference).len())
                {
                    e.insert(descriptor);
                }
            }
        }
    }

    let mut out: Vec<SkillDescriptor> = name_map.into_values().collect();
    // P0-3 Tackling: Sort by (name, reference literal) lexicographic order to stabilize the system prompt.
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| skill_reference_key(&a.reference).cmp(&skill_reference_key(&b.reference)))
    });
    out
}

/// Generate literal keys for sorting for `SkillReference`.
/// `Path` uses `to_string_lossy` to avoid cross-platform boundary issues; `BundledSkillId`
/// Use the id string directly; the two have the same key and will not conflict (bundled id does not contain path separators).
fn skill_reference_key(reference: &ai::skills::SkillReference) -> String {
    match reference {
        ai::skills::SkillReference::Path(p) => p.to_string_lossy().into_owned(),
        ai::skills::SkillReference::BundledSkillId(id) => id.clone(),
    }
}

/// List all skills applicable to the current working directory.
///
/// **Design Note**: The old version of `list_skills_if_changed` is sent differentially under the cloud protocol (compared to what was sent in the previous round)
/// `conversation.latest_skills()`, returns `None` when unchanged) to save upstream tokens - warp backend
/// Maintain the session state and keep it after the first round is received. After the project goes to the cloud, BYOP becomes stateless such as OpenAI/Anthropic
/// `/chat/completions`, system prompt is completely re-rendered on the client in each round, and the data must be delivered in each round.
/// Otherwise, the skills section in the system prompt will disappear from the second round.
/// Therefore it is simplified to return the full amount in each round.
pub fn list_skills(working_directory: Option<&Path>, app: &AppContext) -> Vec<SkillDescriptor> {
    SkillManager::as_ref(app).get_skills_for_working_directory(working_directory, app)
}

/// Renders an 'open skill' button for blocklist AI actions and the code diff view.
pub fn render_skill_button<F>(
    button_label: &str,
    button_handle: MouseStateHandle,
    appearance: &Appearance,
    skill_provider: SkillProvider,
    icon_override: Option<Icon>,
    on_click: F,
) -> Box<dyn Element>
where
    F: FnMut(&mut EventContext) + 'static,
{
    let theme = appearance.theme();
    let logo_fill = internal_colors::fg_overlay_6(theme);

    let icon = icon_override.unwrap_or_else(|| skill_provider.icon());

    let color = if icon_override.is_some() {
        logo_fill
    } else {
        skill_provider.icon_fill(logo_fill)
    };

    render_provider_icon_button(
        button_label,
        button_handle,
        appearance,
        icon,
        color,
        on_click,
    )
}

/// Returns a branded icon override for well-known skill names.
pub fn icon_override_for_skill_name(name: &str) -> Option<Icon> {
    match name {
        "stripe-projects-cli" => Some(Icon::StripeLogo),
        _ => None,
    }
}

pub fn skill_path_from_file_path(file_path: &Path) -> Option<PathBuf> {
    for definition in SKILL_PROVIDER_DEFINITIONS.iter() {
        let home_skill_dirs = if definition.provider == SkillProvider::Waz {
            warp_managed_skill_dirs()
        } else {
            home_skills_path(definition.provider).into_iter().collect()
        };
        for home_skills_path in home_skill_dirs {
            if let Ok(relative_path) = file_path.strip_prefix(&home_skills_path) {
                let skill_name = relative_path.components().next()?;
                return Some(home_skills_path.join(skill_name).join("SKILL.md"));
            }
        }
    }
    let path_components: Vec<_> = file_path.components().collect();

    for def in SKILL_PROVIDER_DEFINITIONS.iter() {
        let skill_components: Vec<_> = def.skills_path.components().collect();

        for (idx, window) in path_components.windows(skill_components.len()).enumerate() {
            if window == skill_components.as_slice() {
                let skill_dir = PathBuf::from_iter(
                    file_path
                        .components()
                        .take(idx + skill_components.len() + 1),
                );
                return Some(skill_dir.join("SKILL.md"));
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "skill_utils_tests.rs"]
mod tests;
