//! SSH Manager UI (Tool Panel on the left). It is currently a skeleton, and the content is to be implemented by Commit 2b:
//! Tree folder/server list + details form on the right.
//!
//! The data layer is in a separate crate `warp_ssh_manager` (`crates/warp_ssh_manager/`).

pub mod candidates;
pub mod notifier;
pub mod onekey;
pub mod panel;
pub mod password_prompt;
pub mod secret_injector;
pub mod server_view;
pub mod shell_prompt;
pub mod startup_command_injector;
pub mod su_password_injector;

// `CandidatesViewModel` is currently only referenced by `panel.rs`; `CandidateRow` is only referenced by panel
// An intermediate representation is used for internal layout and does not need to be exported. Add re-export when it needs to be consumed externally.
#[allow(unused_imports)]
pub use candidates::CandidatesViewModel;
pub use notifier::{SshTreeChangedEvent, SshTreeChangedNotifier};
pub use panel::SshManagerPanel;
// Re-exports for downstream UI consumers (Commit 2b).
#[allow(unused_imports)]
pub use panel::{SshManagerPanelAction, SshManagerPanelEvent};
