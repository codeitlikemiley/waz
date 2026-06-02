//! `ProjectRulesPersister` — Project rules (WARP.md / AGENTS.md) persistence bridge.
//!
//! This thin singleton model has only two responsibilities:
//!
//! 1. Subscribe to the [`KnownRulesChanged`] event of [`ProjectContextModel`] and put
//!    `discovered_rules` / `deleted_rules` converted to [`ModelEvent::UpsertProjectRules`] /
//!    [`ModelEvent::DeleteProjectRules`] writes to SQLite `project_rules` table;
//! 2. Subscribe to the `DetectedGitRepo` event of [`DetectedRepositories`], when the user enters a new git
//!    Triggered when the warehouse [`ProjectContextModel::index_and_store_rules`] scans WARP.md /
//!    AGENTS.md。
//!
//! These two pieces of logic historically hang within `PersistedWorkspace::new`, related to LSP enabling persistence and "visited
//! The git warehouse history is "tightly coupled." This bridge must survive independently after the LSP + workspace history goes offline.
//! Otherwise, project rules will no longer be written to the disk/will no longer be automatically scanned along with the CD.

use std::sync::mpsc::SyncSender;

use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};
use repo_metadata::repositories::{DetectedRepositories, DetectedRepositoriesEvent};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::persistence::ModelEvent;

/// See the module-level documentation for details.
pub struct ProjectRulesPersister {
    /// Writing to the SQLite channel, `None` indicates that persistence is not enabled for the current build.
    persistence_tx: Option<SyncSender<ModelEvent>>,
}

impl Entity for ProjectRulesPersister {
    type Event = ();
}

impl SingletonEntity for ProjectRulesPersister {}

impl ProjectRulesPersister {
    /// Sign up for two subscriptions:
    /// - `ProjectContextModel` → Convert rule delta into SQLite ModelEvent;
    /// - `DetectedRepositories` → Trigger rule scanning when entering the git repository.
    pub fn new(
        persistence_tx: Option<SyncSender<ModelEvent>>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&ProjectContextModel::handle(ctx), |me, event, _ctx| {
            let ProjectContextModelEvent::KnownRulesChanged(delta) = event else {
                return;
            };

            let mut events = vec![];

            if !delta.discovered_rules.is_empty() {
                events.push(ModelEvent::UpsertProjectRules {
                    project_rule_paths: delta.discovered_rules.clone(),
                });
            }

            if !delta.deleted_rules.is_empty() {
                events.push(ModelEvent::DeleteProjectRules {
                    path: delta.deleted_rules.clone(),
                });
            }

            if events.is_empty() {
                return;
            }

            let Some(tx) = me.persistence_tx.as_ref() else {
                return;
            };

            for event in events {
                if let Err(err) = tx.send(event) {
                    log::warn!("ProjectRulesPersister: Failed to write to SQLite: {err}");
                }
            }
        });

        ctx.subscribe_to_model(&DetectedRepositories::handle(ctx), |_me, event, ctx| {
            let DetectedRepositoriesEvent::DetectedGitRepo { repository, .. } = event;
            let repo_path = repository.as_ref(ctx).root_dir().to_local_path_lossy();

            ProjectContextModel::handle(ctx).update(ctx, |model, ctx| {
                let _ = model.index_and_store_rules(repo_path, ctx);
            });
        });

        Self { persistence_tx }
    }

    /// For testing only: do not bind to a persistent channel, nor subscribe to any model.
    #[cfg(test)]
    pub fn new_for_test(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            persistence_tx: None,
        }
    }
}
