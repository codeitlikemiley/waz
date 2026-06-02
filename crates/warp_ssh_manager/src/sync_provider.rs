//! SSH data sync provider, implementing the SyncDataProvider trait
//!
// author: logic
// date: 2026-05-26

use crate::db::with_conn;
use crate::repository::{SshRepository, SyncMetaRepository};
use crate::secrets::{KeychainSecretStore, SecretKind, SshSecretStore};
use crate::types::NodeKind;
use diesel::connection::{Connection, SimpleConnection};
use diesel::{QueryDsl, RunQueryDsl};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use waz_sync::crypto;
use waz_sync::{SyncDataProvider, SyncEngineError, SyncVersionStore};
use zeroize::Zeroizing;

/// Three credential kinds in the keychain, used for uniform iteration during collect/apply/orphan-cleanup
const ALL_SECRET_KINDS: [SecretKind; 3] = [
    SecretKind::Password,
    SecretKind::Passphrase,
    SecretKind::RootPassword,
];

/// Node data used for SSH sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub sort_order: i32,
    pub is_collapsed: bool,
}

/// Server data used for SSH sync (containing encrypted passwords)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncServer {
    pub node_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: String,
    pub key_path: Option<String>,
    pub startup_command: Option<String>,
    pub notes: Option<String>,
    pub password_encrypted: Option<String>,
    pub passphrase_encrypted: Option<String>,
    pub root_password_encrypted: Option<String>,
}

/// SSH sync data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SshSyncData {
    pub nodes: Vec<SyncNode>,
    pub servers: Vec<SyncServer>,
}

/// SSH data sync provider
pub struct SshSyncProvider {
    secret_store: KeychainSecretStore,
}

impl SshSyncProvider {
    /// Creates a new SshSyncProvider instance
    pub fn new() -> Self {
        Self {
            secret_store: KeychainSecretStore::default(),
        }
    }
}

impl SyncDataProvider for SshSyncProvider {
    fn section_key(&self) -> &str {
        "ssh"
    }

    fn collect_data(&self, token: &str) -> Result<serde_json::Value, SyncEngineError> {
        let nodes =
            with_conn(|conn| Ok(SshRepository::list_nodes(conn)?))
                .map_err(|e| SyncEngineError::Provider(e.to_string()))?;

        let mut sync_nodes = Vec::new();
        let mut sync_servers = Vec::new();

        for node in &nodes {
            sync_nodes.push(SyncNode {
                id: node.id.clone(),
                parent_id: node.parent_id.clone(),
                kind: node.kind.as_db_str().to_string(),
                name: node.name.clone(),
                sort_order: node.sort_order,
                is_collapsed: node.is_collapsed,
            });

            if node.kind == NodeKind::Server {
                let server_result =
                    with_conn(|conn| Ok(SshRepository::get_server(conn, &node.id)?))
                        .map_err(|e| SyncEngineError::Provider(e.to_string()))?;
                if let Some(server) = server_result {
                    // Distinguish between keychain errors and "no password set by the user":
                    // - Ok(Some) = password exists, encrypt and upload
                    // - Ok(None) = user did not set one, write None in the field
                    // - Err = abort the entire upload, avoiding serializing transient keychain failures as
                    //   "no password" and overwriting the real password on other devices (PR #161 review #5)
                    let password = read_secret(&self.secret_store, &node.id, SecretKind::Password)?;
                    let passphrase =
                        read_secret(&self.secret_store, &node.id, SecretKind::Passphrase)?;
                    let root_password =
                        read_secret(&self.secret_store, &node.id, SecretKind::RootPassword)?;

                    sync_servers.push(SyncServer {
                        node_id: server.node_id.clone(),
                        host: server.host.clone(),
                        port: server.port,
                        username: server.username.clone(),
                        auth_type: server.auth_type.as_db_str().to_string(),
                        key_path: server.key_path.clone(),
                        startup_command: server.startup_command.clone(),
                        notes: server.notes.clone(),
                        password_encrypted: encrypt_optional(token, password.as_deref())?,
                        passphrase_encrypted: encrypt_optional(token, passphrase.as_deref())?,
                        root_password_encrypted: encrypt_optional(token, root_password.as_deref())?,
                    });
                }
            }
        }

        let data = SshSyncData {
            nodes: sync_nodes,
            servers: sync_servers,
        };

        serde_json::to_value(&data)
            .map_err(|e: serde_json::Error| SyncEngineError::Serialization(e.to_string()))
    }

    fn apply_data(&self, token: &str, data: &serde_json::Value) -> Result<(), SyncEngineError> {
        let ssh_data: SshSyncData = serde_json::from_value(data.clone())
            .map_err(|e: serde_json::Error| SyncEngineError::Serialization(e.to_string()))?;

        // ---- Phase 0 ---- Decrypt all + collect explicit-clear list
        // pending_secrets: remote explicitly provided ciphertext -> need to write to keychain
        // explicit_clears: remote explicitly provided None -> need to delete keychain (user cleared password on other devices,
        //                  not cleaning it up will cause local to continue using the old password, violating user intent; PR #161 7 rounds of review)
        struct PendingSecret {
            node_id: String,
            kind: SecretKind,
            value: String,
        }
        let mut pending_secrets: Vec<PendingSecret> = Vec::new();
        let mut explicit_clears: Vec<(String, SecretKind)> = Vec::new();
        for server in &ssh_data.servers {
            for (kind, enc) in [
                (SecretKind::Password, &server.password_encrypted),
                (SecretKind::Passphrase, &server.passphrase_encrypted),
                (SecretKind::RootPassword, &server.root_password_encrypted),
            ] {
                match enc {
                    Some(enc) => {
                        let value = crypto::decrypt(token, enc)
                            .map_err(|e| SyncEngineError::Crypto(e.to_string()))?;
                        pending_secrets.push(PendingSecret {
                            node_id: server.node_id.clone(),
                            kind,
                            value,
                        });
                    }
                    None => {
                        explicit_clears.push((server.node_id.clone(), kind));
                    }
                }
            }
        }

        // ---- Phase 0.5 ---- Topologically sort nodes, parent nodes before child nodes; orphans (parent not in dataset)
        // are inserted as root nodes to avoid SQLite FK violations rolling back the entire transaction
        let sorted_nodes = topologically_sort_nodes(&ssh_data.nodes);

        // ---- Phase 0.6 ---- Collect existing local node_ids for subsequent orphan keychain cleanup
        let existing_node_ids: Vec<String> = with_conn(|conn| {
            Ok(persistence::schema::ssh_nodes::table
                .select(persistence::schema::ssh_nodes::id)
                .load::<String>(conn)?)
        })
        .map_err(|e| SyncEngineError::Provider(e.to_string()))?;

        // ---- Phase 1 ---- Write to keychain first. Any failure -> abort immediately without touching the DB.
        // Track (node_id, kind, prior_value) list. When DB phase fails:
        // - prior_value=Some(v) -> restore back to old value (avoid overwriting user's existing password)
        // - prior_value=None    -> delete (avoid contamination)
        // True "atomic rollback" is based on the idempotent overwrite semantics of secret_store.set (PR #161 3 rounds of review)
        let mut written_secrets: Vec<WrittenSecret> = Vec::new();
        for s in &pending_secrets {
            // Snapshot prior value before writing, so subsequent rollback can truly restore the old value.
            // Real keychain errors abort the whole process, but NoBackend (headless Linux, etc.) is treated as "no prior value".
            // This design is consistent with read_secret in collect_data — same environmental constraints.
            let prior_value = match self.secret_store.get(&s.node_id, s.kind) {
                // store.get already returns Option<Zeroizing<String>>, use it directly to preserve zeroization semantics
                Ok(opt) => opt,
                Err(e) => {
                    // Equally strict as read_secret: any keychain error aborts to avoid being unable to rollback
                    rollback_keychain_writes(&self.secret_store, &written_secrets);
                    return Err(SyncEngineError::Provider(format!(
                        "Failed to read prior keychain value ({}, {:?}): {e}. Rolled back {} items, please confirm the keystore is available and try downloading again",
                        s.node_id, s.kind, written_secrets.len()
                    )));
                }
            };
            if let Err(e) = self.secret_store.set(&s.node_id, s.kind, &s.value) {
                rollback_keychain_writes(&self.secret_store, &written_secrets);
                return Err(SyncEngineError::Provider(format!(
                    "Failed to write to keychain ({}, {:?}): {e}, please check keystore permissions and try downloading again",
                    s.node_id, s.kind
                )));
            }
            written_secrets.push(WrittenSecret {
                node_id: s.node_id.clone(),
                kind: s.kind,
                prior_value,
            });
        }

        // ---- Phase 2 ---- DB transaction: DELETE + INSERT in topological order
        let db_result = with_conn(|conn| {
            conn.transaction::<(), anyhow::Error, _>(|conn| {
                conn.batch_execute("DELETE FROM ssh_servers; DELETE FROM ssh_nodes;")?;

                for node in &sorted_nodes {
                    let kind = NodeKind::parse(&node.kind)
                        .ok_or_else(|| anyhow::anyhow!("Invalid kind: {}", node.kind))?;
                    diesel::insert_into(persistence::schema::ssh_nodes::table)
                        .values(persistence::model::NewSshNode {
                            id: &node.id,
                            parent_id: node.parent_id.as_deref(),
                            kind: kind.as_db_str(),
                            name: &node.name,
                            sort_order: node.sort_order,
                        })
                        .execute(conn)?;
                    if node.is_collapsed {
                        SshRepository::set_collapsed(conn, &node.id, true)?;
                    }
                }

                for server in &ssh_data.servers {
                    diesel::insert_into(persistence::schema::ssh_servers::table)
                        .values(persistence::model::NewSshServer {
                            node_id: &server.node_id,
                            host: &server.host,
                            port: server.port as i32,
                            username: &server.username,
                            auth_type: &server.auth_type,
                            key_path: server.key_path.as_deref(),
                            startup_command: server.startup_command.as_deref(),
                            notes: server.notes.as_deref(),
                        })
                        .execute(conn)?;
                }
                Ok(())
            })
        });
        if let Err(e) = db_result {
            // DB failure -> rollback the newly written keychain to avoid long-term remnants pointing to non-existent nodes
            let rolled = written_secrets.len();
            rollback_keychain_writes(&self.secret_store, &written_secrets);
            return Err(SyncEngineError::Provider(format!(
                "DB write failed ({e}); rolled back {rolled} keychain writes"
            )));
        }

        // ---- Phase 3a ---- Clean up explicit-clear: nodes still exist but remote set corresponding *_encrypted to None
        // The user cleared a password on another device -> must delete local keychain, otherwise connect will continue using the old password,
        // violating the user's intent to clear it (PR #161 7 rounds of review)
        for (node_id, kind) in &explicit_clears {
            if let Err(e) = self.secret_store.delete(node_id, *kind) {
                log::warn!("Failed to clean up explicit-clear keychain item {node_id}/{:?}: {e}", kind);
            }
        }

        // ---- Phase 3b ---- Clean up orphan keychain: passwords corresponding to node_ids that existed locally but were deleted remotely
        // must be explicitly deleted, otherwise when a node with the same UUID reappears, it will read stale passwords (PR #161 review #4)
        let new_node_ids: HashSet<&str> =
            ssh_data.nodes.iter().map(|n| n.id.as_str()).collect();
        for old_id in &existing_node_ids {
            if new_node_ids.contains(old_id.as_str()) {
                continue;
            }
            for kind in ALL_SECRET_KINDS {
                if let Err(e) = self.secret_store.delete(old_id, kind) {
                    log::warn!("Failed to clean up orphan keychain item {old_id}/{:?}: {e}", kind);
                }
            }
        }

        Ok(())
    }
}

/// Records of keychain entries written in apply_data Phase 1, with prior value snapshots for actual rollback.
/// `prior_value` is held using `Zeroizing<String>` to ensure the plaintext password on the rollback chain is zeroized when dropped.
struct WrittenSecret {
    node_id: String,
    kind: SecretKind,
    prior_value: Option<Zeroizing<String>>,
}

/// True "rollback": for each overwritten entry:
/// - prior_value=Some -> write back prior value, avoiding swallowing user's existing password
/// - prior_value=None -> delete, avoiding orphans
/// Any step failure is only logged and does not block the caller (best effort).
fn rollback_keychain_writes<S: SshSecretStore + ?Sized>(
    store: &S,
    written: &[WrittenSecret],
) {
    for entry in written {
        let res = match &entry.prior_value {
            Some(v) => store.set(&entry.node_id, entry.kind, v.as_str()),
            None => store.delete(&entry.node_id, entry.kind),
        };
        if let Err(e) = res {
            log::warn!(
                "Failed to rollback keychain write {}/{:?}: {e} (secret may remain at new value or become an orphan)",
                entry.node_id, entry.kind
            );
        }
    }
}

/// Reads keychain credentials.
/// - `Ok(Some)` = password exists, encrypt and upload
/// - `Ok(None)` = user did not set a password (valid state), write None in field
/// - `Err` = keychain failure (NoBackend / Locked / permission denied)
///
/// Note: No fallback is made for NoBackend. The upper keyring crate maps both locked keychains and
/// completely backend-less environments to NoBackend, making them indistinguishable reliably (documented behavior of keyring 3.6).
/// Treating NoBackend as Ok(None) would cause transient failures like "locked" to silently discard passwords -> cloud cleared,
/// and cannot be recovered after reinstallation (KDF/formats are still items to be optimized).
/// Headless Linux / CI users with no passwords will not trigger this function during upload; once Err is encountered,
/// the error message explicitly guides the user to unlock/enable the keychain.
fn read_secret(
    store: &dyn SshSecretStore,
    node_id: &str,
    kind: SecretKind,
) -> Result<Option<String>, SyncEngineError> {
    match store.get(node_id, kind) {
        Ok(opt) => Ok(opt.map(|z| z.to_string())),
        Err(e) => Err(SyncEngineError::Provider(format!(
            "Failed to read keychain ({node_id}, {kind:?}): {e}. \
             The keychain may be locked or there is no backend in the current environment (headless Linux / WSL, etc.). \
             Please unlock the keychain or enable secret-service / Credential Manager and try uploading again. \
             If this server indeed does not require password synchronization, you can clear this field in the SSH Manager."
        ))),
    }
}

fn encrypt_optional(
    token: &str,
    value: Option<&str>,
) -> Result<Option<String>, SyncEngineError> {
    match value {
        None => Ok(None),
        // An empty string is treated as "no password" and not uploaded (compatible with past behavior, avoiding contaminating ciphertext with empty strings)
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Ok(Some(
            crypto::encrypt(token, s)
                .map_err(|e| SyncEngineError::Crypto(e.to_string()))?,
        )),
    }
}

/// BFS topological sort: parent nodes before child nodes. Orphan nodes whose parent_id references nodes outside the dataset
/// are treated as root nodes appended to the end, with parent_id cleared to prevent SQLite FK constraint failures from rolling back the entire download.
fn topologically_sort_nodes(nodes: &[SyncNode]) -> Vec<SyncNode> {
    use std::collections::HashMap;
    let mut by_parent: HashMap<Option<&str>, Vec<&SyncNode>> = HashMap::new();
    for n in nodes {
        by_parent.entry(n.parent_id.as_deref()).or_default().push(n);
    }

    let mut result: Vec<SyncNode> = Vec::with_capacity(nodes.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<&SyncNode> = VecDeque::new();
    if let Some(roots) = by_parent.get(&None) {
        for r in roots {
            queue.push_back(*r);
        }
    }
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.id.clone()) {
            continue;
        }
        result.push(node.clone());
        if let Some(children) = by_parent.get(&Some(node.id.as_str())) {
            for c in children {
                queue.push_back(*c);
            }
        }
    }

    // Remaining nodes are either orphans (parent_id points outside the dataset) or belong to a cycle.
    // In both cases, parent_id is cleared and downgraded to root insertion (recoverable with no data loss), and an explicit log warning is emitted
    // so users can see the structured reset in the logs.
    for n in nodes {
        if !seen.contains(&n.id) {
            if has_cycle_membership(n, nodes) {
                log::warn!(
                    "apply_data: Node {} is in a cyclic reference (parent_id {:?}), downgraded to root node",
                    n.id, n.parent_id
                );
            } else {
                log::warn!(
                    "apply_data: Node {}'s parent_id {:?} does not exist in the dataset, inserted as root node",
                    n.id, n.parent_id
                );
            }
            let mut orphan = n.clone();
            orphan.parent_id = None;
            result.push(orphan);
        }
    }

    result
}

/// Determines whether node `start` is in a cycle (starting from it and following the parent_id chain eventually returns to itself or a loop).
/// Used to distinguish "orphan" vs "cycle" in logs; limits maximum traversal steps to prevent exponential complexity.
fn has_cycle_membership(start: &SyncNode, all: &[SyncNode]) -> bool {
    let by_id: std::collections::HashMap<&str, &SyncNode> =
        all.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut current = start;
    let mut visited: HashSet<&str> = HashSet::new();
    let max_steps = all.len() + 1;
    for _ in 0..max_steps {
        let Some(pid) = current.parent_id.as_deref() else {
            return false;
        };
        if !visited.insert(current.id.as_str()) {
            // Walked through the same node -> cycle
            return true;
        }
        match by_id.get(pid) {
            Some(parent) => current = parent,
            None => return false, // parent outside the dataset -> orphan, not cycle
        }
    }
    // More than max_steps without terminating -> there must be a cycle
    true
}

/// Database sync version store adapter
pub struct DbVersionStore;

impl SyncVersionStore for DbVersionStore {
    fn get_sync_version(&self) -> Result<i64, SyncEngineError> {
        with_conn(|c| Ok(SyncMetaRepository::get_sync_version(c)?))
            .map_err(|e| SyncEngineError::VersionStore(e.to_string()))
    }

    fn set_sync_version(&self, version: i64) -> Result<(), SyncEngineError> {
        with_conn(|c| Ok(SyncMetaRepository::set_sync_version(c, version)?))
            .map_err(|e| SyncEngineError::VersionStore(e.to_string()))
    }

    fn update_sync_meta(&self, time: &str, platform: &str) -> Result<(), SyncEngineError> {
        with_conn(|c| Ok(SyncMetaRepository::update_sync_meta(c, time, platform)?))
            .map_err(|e| SyncEngineError::VersionStore(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_section_key() {
        let provider = SshSyncProvider::new();
        assert_eq!(provider.section_key(), "ssh");
    }

    #[test]
    fn test_sync_node_serialization_roundtrip() {
        let node = SyncNode {
            id: "n1".to_string(),
            parent_id: Some("p1".to_string()),
            kind: "folder".to_string(),
            name: "Prod".to_string(),
            sort_order: 0,
            is_collapsed: true,
        };
        let json = serde_json::to_string(&node).unwrap();
        let parsed: SyncNode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "n1");
        assert_eq!(parsed.parent_id, Some("p1".to_string()));
        assert_eq!(parsed.kind, "folder");
        assert_eq!(parsed.name, "Prod");
        assert_eq!(parsed.sort_order, 0);
        assert!(parsed.is_collapsed);
    }

    #[test]
    fn test_sync_server_serialization_with_secrets() {
        let server = SyncServer {
            node_id: "s1".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: "password".to_string(),
            key_path: Some("/key".to_string()),
            startup_command: None,
            notes: Some("test".to_string()),
            password_encrypted: Some("enc123".to_string()),
            passphrase_encrypted: None,
            root_password_encrypted: Some("enc456".to_string()),
        };
        let json = serde_json::to_string(&server).unwrap();
        let parsed: SyncServer = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "s1");
        assert_eq!(parsed.port, 22);
        assert_eq!(parsed.password_encrypted, Some("enc123".to_string()));
        assert_eq!(parsed.passphrase_encrypted, None);
        assert_eq!(parsed.root_password_encrypted, Some("enc456".to_string()));
    }

    #[test]
    fn test_sync_server_no_secrets() {
        let server = SyncServer {
            node_id: "s2".to_string(),
            host: "host".to_string(),
            port: 2222,
            username: "admin".to_string(),
            auth_type: "key".to_string(),
            key_path: None,
            startup_command: None,
            notes: None,
            password_encrypted: None,
            passphrase_encrypted: None,
            root_password_encrypted: None,
        };
        let json = serde_json::to_string(&server).unwrap();
        let parsed: SyncServer = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.password_encrypted, None);
        assert_eq!(parsed.passphrase_encrypted, None);
        assert_eq!(parsed.root_password_encrypted, None);
    }

    #[test]
    fn test_ssh_sync_data_roundtrip() {
        let data = SshSyncData {
            nodes: vec![
                SyncNode {
                    id: "n1".to_string(),
                    parent_id: None,
                    kind: "folder".to_string(),
                    name: "Root".to_string(),
                    sort_order: 0,
                    is_collapsed: false,
                },
            ],
            servers: vec![
                SyncServer {
                    node_id: "s1".to_string(),
                    host: "h".to_string(),
                    port: 22,
                    username: "u".to_string(),
                    auth_type: "password".to_string(),
                    key_path: None,
                    startup_command: None,
                    notes: None,
                    password_encrypted: Some("enc".to_string()),
                    passphrase_encrypted: None,
                    root_password_encrypted: None,
                },
            ],
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: SshSyncData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.servers.len(), 1);
        assert_eq!(parsed.nodes[0].id, "n1");
        assert_eq!(parsed.servers[0].password_encrypted, Some("enc".to_string()));
    }

    #[test]
    fn test_ssh_sync_data_default_empty() {
        let data = SshSyncData::default();
        assert!(data.nodes.is_empty());
        assert!(data.servers.is_empty());
    }

    #[test]
    fn test_sync_node_null_parent() {
        let node = SyncNode {
            id: "root".to_string(),
            parent_id: None,
            kind: "folder".to_string(),
            name: "R".to_string(),
            sort_order: 0,
            is_collapsed: false,
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"parent_id\":null"), "parent_id=None should serialize to null");
        let parsed: SyncNode = serde_json::from_str(&json).unwrap();
        assert!(parsed.parent_id.is_none());
    }
}
