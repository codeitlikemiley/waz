//! View model for the "Candidates" area - put `warp_ssh_manager::load_candidates()`
//! The results (and imported alias collections and collapsed states) are flattened into a UI-friendly [`CandidateRow`]
//! list.
//!
//! Design points (corresponding to `specs/gh-110-ssh-config-import/{PRODUCT,TECH}.md`):
//!
//! - `rows()` is a **pure function**: it only relies on the current fields of view-model and does not touch IO / runtime,
//!   Unit tests can directly construct a `CandidatesViewModel` and assert the output. This is what TDD is
//!   Points requested in the discussion - PR 2's rendering layer warpui test is too expensive, "which lines should be displayed"
//!   Extracting the logic and testing it alone is enough to cover the key judgments.
//! - `refresh()` calls `warp_ssh_manager::load_candidates()` synchronously (<10KB file,
//!   See TECH.md §3.1 for trade-offs), and store the result in `state`.
//! - `on_tree_changed()` is called by the panel after subscribing to `SshTreeChangedNotifier` - put
//!   Save the `host` fields of all servers in the tree and collect them into `HashSet` as the "Added" badge
//!   Judgment basis (PRODUCT.md decision E).
//! - The determination of "imported" is based on `host == alias`. The import logic is in `server.host` on the side of the panel
//!   Set to candidate alias (PRODUCT.md decision I), so the comparison semantics here are consistent with the import semantics.
//!
//! All fields are `pub(crate)`, only visible to `panel.rs`; `CandidatesViewModel` itself
//! Exposed to re-export of `mod.rs` via `pub`.

use std::collections::HashSet;

use warpui::{Entity, ModelContext};

use warp_ssh_manager::{LoadOutcome, LoadResult, SshConfigCandidate, load_candidates};

/// `~/.ssh/config` One line source + status view of the candidate server in the UI.
pub struct CandidatesViewModel {
    /// The latest loaded results. `None` means that the model has just been created and no refresh has been triggered yet.
    state: Option<LoadResult>,
    /// Save the `host` field set of all servers in the tree. `rows()` uses it to determine `added`.
    added_aliases: HashSet<String>,
    /// Section collapse state (PRODUCT.md UX table "Many candidates"). Expanded by default.
    expanded: bool,
}

impl Default for CandidatesViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidatesViewModel {
    /// Empty constructor - used when the model is first added to the App via `add_model`. `refresh()` must be
    /// The caller triggers at the appropriate time (just call it once in panel `new`).
    pub fn new() -> Self {
        Self {
            state: None,
            added_aliases: HashSet::new(),
            expanded: true,
        }
    }

    /// Constructor for testing: explicitly insert internal state, avoid runtime/IO, drive directly
    /// `rows()` various branches.
    #[cfg(test)]
    pub fn with_state(
        state: Option<LoadResult>,
        added_aliases: HashSet<String>,
        expanded: bool,
    ) -> Self {
        Self {
            state,
            added_aliases,
            expanded,
        }
    }

    /// Synchronously re-read `~/.ssh/config` and store the result in `state`.
    ///
    /// The design does not return errors - `LoadOutcome::Error` has brought back the error message string,
    /// The UI is displayed with red error lines (see PRODUCT.md UX table "Parse / IO error").
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        self.state = Some(load_candidates());
        ctx.notify();
    }

    /// Tree change callback - rebuild `added_aliases` with the passed server hosts.
    ///
    /// Receive `impl IntoIterator<Item = String>` instead of `&SshRepository` for testing
    /// There is no need to plug a real SQLite connection; the caller (panel) is responsible for `list_nodes` +
    /// The host field of `get_server` is collected into an iterator and passed in.
    pub fn on_tree_changed<I>(&mut self, hosts: I, ctx: &mut ModelContext<Self>)
    where
        I: IntoIterator<Item = String>,
    {
        self.added_aliases = hosts.into_iter().collect();
        ctx.notify();
    }

    /// Toggles the "section collapsed" state.
    pub fn toggle_expanded(&mut self, ctx: &mut ModelContext<Self>) {
        self.expanded = !self.expanded;
        ctx.notify();
    }

    /// Whether to expand (it determines whether to display the body row when the panel is rendered).
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Find candidates by alias - `ImportCandidate { alias }` used during action processing,
    /// After getting the complete fields, call `SshRepository::create_server`.
    pub fn find_candidate(&self, alias: &str) -> Option<&SshConfigCandidate> {
        let state = self.state.as_ref()?;
        match &state.outcome {
            LoadOutcome::Loaded(v) => v.iter().find(|c| c.alias == alias),
            LoadOutcome::NotFound | LoadOutcome::Error(_) => None,
        }
    }

    /// A human-readable string of the current `~/.ssh/config` path (given `notes = "Imported from {}"`
    /// use). `None` means it has not been loaded, or even home cannot be obtained.
    pub fn path_display(&self) -> Option<String> {
        self.state
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .map(|p| p.display().to_string())
    }

    /// Flatten the current state into a list of rows - see the "pure function" convention in the module documentation.
    ///
    /// Output semantics (corresponding to PRODUCT.md §5 UX table):
    /// - Not yet refresh: returns an empty Vec (panel does not render sections when it gets `state == None`).
    /// - `NotFound`:Header + a line of `NotFound`.
    /// - `Error`:Header + one line `Error` (can_refresh=true allows the user to change the config and try again).
    /// - `Loaded(empty)`:Header + a line of `Empty`.
    /// - `Loaded(non-empty)`:Header(count = N)+ N lines `Candidate`, each line
    ///   `added` is determined by `added_aliases.contains(alias)`.
    pub fn rows(&self) -> Vec<CandidateRow> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };

        let path_display = state
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        let mut out = Vec::new();
        let count = match &state.outcome {
            LoadOutcome::Loaded(v) => v.len(),
            LoadOutcome::NotFound | LoadOutcome::Error(_) => 0,
        };
        // Header is always the first line - even if the section is collapsed, the panel still has to draw the header (that is
        // toggle entrance). `can_refresh = true` is always true: the user is allowed to click in any state
        // Refresh reread.
        out.push(CandidateRow::Header {
            path_display: path_display.clone(),
            count,
            can_refresh: true,
        });

        // When the section is collapsed, only the header is retained, and the body is not rendered.
        if !self.expanded {
            return out;
        }

        match &state.outcome {
            LoadOutcome::NotFound => {
                out.push(CandidateRow::NotFound { path_display });
            }
            LoadOutcome::Error(msg) => {
                out.push(CandidateRow::Error {
                    path_display,
                    message: msg.clone(),
                });
            }
            LoadOutcome::Loaded(v) if v.is_empty() => {
                out.push(CandidateRow::Empty { path_display });
            }
            LoadOutcome::Loaded(v) => {
                for c in v {
                    out.push(CandidateRow::Candidate {
                        alias: c.alias.clone(),
                        hostname: c.hostname.clone(),
                        user: c.user.clone(),
                        port: c.port,
                        identity_file: c.identity_file.as_ref().map(|p| p.display().to_string()),
                        added: self.added_aliases.contains(&c.alias),
                    });
                }
            }
        }

        out
    }
}

/// One line of UI friendly. Header is always at the front, followed by either a single status line (NotFound /
/// Empty / Error), or a string of Candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateRow {
    Header {
        path_display: String,
        count: usize,
        can_refresh: bool,
    },
    NotFound {
        path_display: String,
    },
    Empty {
        path_display: String,
    },
    Error {
        path_display: String,
        message: String,
    },
    Candidate {
        alias: String,
        hostname: Option<String>,
        user: Option<String>,
        port: Option<u16>,
        identity_file: Option<String>,
        added: bool,
    },
}

impl Entity for CandidatesViewModel {
    type Event = ();
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;

// Let the test code not care about the specific disk path of PathBuf - the helper uses `LoadResult` to spell one
// Fixed display string. It will also be used in the test module, so it is placed in the outer layer to facilitate #[cfg(test)] reuse.
#[cfg(test)]
pub(crate) fn fake_load_result_loaded(path: &str, cands: Vec<SshConfigCandidate>) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Loaded(cands),
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_not_found(path: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::NotFound,
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_error(path: &str, msg: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Error(msg.to_string()),
    }
}
