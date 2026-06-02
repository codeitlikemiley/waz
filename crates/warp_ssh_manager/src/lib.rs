//! SSH Manager Data Layer — persistent server/folder tree + OS keychain credential store +
//! command construction. The UI and PTY injection logic are placed in the `app/src/ssh_manager/` and `secret_injector`
//! modules. This crate remains pure Rust, has no warpui dependency, and can be run with `cargo test` independently.

pub mod db;
pub mod repository;
pub mod secrets;
pub mod ssh_command;
pub mod ssh_config_parser;
pub mod sync_provider;
pub mod types;

pub use db::{set_database_path, with_conn};
pub use repository::{SshRepository, SshRepositoryError, SyncMetaRepository};
pub use secrets::{KeychainSecretStore, SecretKind, SshSecretStore, SshSecretStoreError};
pub use ssh_command::{build_ssh_args, build_ssh_command_line, test_connection, ConnectionTestResult};
pub use ssh_config_parser::{
    LoadOutcome, LoadResult, SshConfigCandidate, default_ssh_config_path, load_candidates,
    load_candidates_from, parse_ssh_config,
};
pub use sync_provider::{DbVersionStore, SshSyncData, SshSyncProvider, SyncNode, SyncServer};
pub use types::{AuthType, NodeKind, SshNode, SshServerInfo};
pub use types::ConnectionStatus;
