//! Command palette data source: SSH servers (exclusive to openWarp).
//!
//! Users perform fuzzy matching by server name / host in Ctrl+Shift+P, and selecting it emits
//! `WorkspaceAction::OpenSshTerminal` to open a new tab connection (automatically injecting passwords
//! via SecretInjector, which is completely equivalent to right-clicking "Connect" in the SSH manager).

pub mod data_source;
pub mod search_item;

pub use data_source::SshServersDataSource;
pub use search_item::SshServerSearchItem;
