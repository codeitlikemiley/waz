//! Diesel CRUD over `ssh_nodes` + `ssh_servers`. Returns plain data structures from `crate::types`
//! exclusively, hiding ORM implementation details within the crate boundaries.
//!
//! All write operations default sort_order to the current maximum +1 under the same parent.
//! When the UI does not care about ordering, it is appended directly. The caller of move_node is responsible for passing the new sort_order.

use chrono::Utc;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use diesel::sqlite::SqliteConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::types::{AuthType, NodeKind, SshNode, SshServerInfo};
use persistence::model::{NewSshNode, NewSshServer, NewSyncMeta, SshNodeRow, SshServerRow, SyncMetaRow};
use persistence::schema::{ssh_nodes, ssh_servers, sync_meta};

#[derive(Debug, Error)]
pub enum SshRepositoryError {
    #[error("database error: {0}")]
    Db(#[from] DieselError),
    #[error("node not found: {0}")]
    NotFound(String),
    #[error("invalid value in db column `{column}`: {value}")]
    InvalidEnum { column: &'static str, value: String },
}

/// Data access layer. Each method accepts `&mut SqliteConnection` and the caller holds the connection,
/// allowing transaction boundaries to be decided by the caller (typically the UI model layer opens a new transaction for each operation).
pub struct SshRepository;

impl SshRepository {
    /// List all nodes (folders + servers) without details. The caller handles tree construction.
    pub fn list_nodes(conn: &mut SqliteConnection) -> Result<Vec<SshNode>, SshRepositoryError> {
        let rows: Vec<SshNodeRow> = ssh_nodes::table
            .order((ssh_nodes::parent_id.asc(), ssh_nodes::sort_order.asc()))
            .load(conn)?;
        rows.into_iter().map(node_from_row).collect()
    }

    pub fn get_server(
        conn: &mut SqliteConnection,
        node_id: &str,
    ) -> Result<Option<SshServerInfo>, SshRepositoryError> {
        let row: Option<SshServerRow> = ssh_servers::table.find(node_id).first(conn).optional()?;
        row.map(server_from_row).transpose()
    }

    pub fn create_folder(
        conn: &mut SqliteConnection,
        parent_id: Option<&str>,
        name: &str,
    ) -> Result<SshNode, SshRepositoryError> {
        let id = new_uuid();
        let sort = next_sort_order(conn, parent_id)?;
        diesel::insert_into(ssh_nodes::table)
            .values(NewSshNode {
                id: &id,
                parent_id,
                kind: NodeKind::Folder.as_db_str(),
                name,
                sort_order: sort,
            })
            .execute(conn)?;
        let _ = Self::increment_sync_version(conn);
        Self::get_node(conn, &id)
    }

    pub fn create_server(
        conn: &mut SqliteConnection,
        parent_id: Option<&str>,
        name: &str,
        info: &SshServerInfo,
    ) -> Result<SshNode, SshRepositoryError> {
        let id = new_uuid();
        let sort = next_sort_order(conn, parent_id)?;
        conn.transaction::<_, DieselError, _>(|conn| {
            diesel::insert_into(ssh_nodes::table)
                .values(NewSshNode {
                    id: &id,
                    parent_id,
                    kind: NodeKind::Server.as_db_str(),
                    name,
                    sort_order: sort,
                })
                .execute(conn)?;
            diesel::insert_into(ssh_servers::table)
                .values(NewSshServer {
                    node_id: &id,
                    host: &info.host,
                    port: info.port as i32,
                    username: &info.username,
                    auth_type: info.auth_type.as_db_str(),
                    key_path: info.key_path.as_deref(),
                    startup_command: info.startup_command.as_deref(),
                    notes: info.notes.as_deref(),
                })
                .execute(conn)?;
            Ok(())
        })?;
        let _ = Self::increment_sync_version(conn);
        Self::get_node(conn, &id)
    }

    pub fn rename_node(
        conn: &mut SqliteConnection,
        node_id: &str,
        new_name: &str,
    ) -> Result<(), SshRepositoryError> {
        let n = diesel::update(ssh_nodes::table.find(node_id))
            .set((
                ssh_nodes::name.eq(new_name),
                ssh_nodes::updated_at.eq(Utc::now().naive_utc()),
            ))
            .execute(conn)?;
        if n == 0 {
            return Err(SshRepositoryError::NotFound(node_id.to_string()));
        }
        let _ = Self::increment_sync_version(conn);
        Ok(())
    }

    pub fn update_server(
        conn: &mut SqliteConnection,
        info: &SshServerInfo,
    ) -> Result<(), SshRepositoryError> {
        let n = diesel::update(ssh_servers::table.find(&info.node_id))
            .set((
                ssh_servers::host.eq(&info.host),
                ssh_servers::port.eq(info.port as i32),
                ssh_servers::username.eq(&info.username),
                ssh_servers::auth_type.eq(info.auth_type.as_db_str()),
                ssh_servers::key_path.eq(info.key_path.as_deref()),
                ssh_servers::startup_command.eq(info.startup_command.as_deref()),
                ssh_servers::notes.eq(info.notes.as_deref()),
            ))
            .execute(conn)?;
        if n == 0 {
            return Err(SshRepositoryError::NotFound(info.node_id.clone()));
        }
        diesel::update(ssh_nodes::table.find(&info.node_id))
            .set(ssh_nodes::updated_at.eq(Utc::now().naive_utc()))
            .execute(conn)?;
        let _ = Self::increment_sync_version(conn);
        Ok(())
    }

    /// Delete node. ON DELETE CASCADE will automatically delete children + ssh_servers rows.
    /// The caller is responsible for clearing the corresponding secret in the keychain.
    pub fn delete_node(
        conn: &mut SqliteConnection,
        node_id: &str,
    ) -> Result<(), SshRepositoryError> {
        let n = diesel::delete(ssh_nodes::table.find(node_id)).execute(conn)?;
        if n == 0 {
            return Err(SshRepositoryError::NotFound(node_id.to_string()));
        }
        let _ = Self::increment_sync_version(conn);
        Ok(())
    }

    /// Supports changing parent and reordering simultaneously. new_parent_id=None moves to root.
    pub fn move_node(
        conn: &mut SqliteConnection,
        node_id: &str,
        new_parent_id: Option<&str>,
        new_sort_order: i32,
    ) -> Result<(), SshRepositoryError> {
        let n = diesel::update(ssh_nodes::table.find(node_id))
            .set((
                ssh_nodes::parent_id.eq(new_parent_id),
                ssh_nodes::sort_order.eq(new_sort_order),
                ssh_nodes::updated_at.eq(Utc::now().naive_utc()),
            ))
            .execute(conn)?;
        if n == 0 {
            return Err(SshRepositoryError::NotFound(node_id.to_string()));
        }
        let _ = Self::increment_sync_version(conn);
        Ok(())
    }

    pub fn touch_last_connected(
        conn: &mut SqliteConnection,
        node_id: &str,
    ) -> Result<(), SshRepositoryError> {
        diesel::update(ssh_servers::table.find(node_id))
            .set(ssh_servers::last_connected_at.eq(Some(Utc::now().naive_utc())))
            .execute(conn)?;
        Ok(())
    }

    /// Update the collapsed state of a single folder. Server nodes are also allowed to be configured (though unused by UI),
    /// to simplify caller logic.
    pub fn set_collapsed(
        conn: &mut SqliteConnection,
        node_id: &str,
        collapsed: bool,
    ) -> Result<(), SshRepositoryError> {
        let n = diesel::update(ssh_nodes::table.find(node_id))
            .set((
                ssh_nodes::is_collapsed.eq(collapsed),
                ssh_nodes::updated_at.eq(Utc::now().naive_utc()),
            ))
            .execute(conn)?;
        if n == 0 {
            return Err(SshRepositoryError::NotFound(node_id.to_string()));
        }
        Ok(())
    }

    /// Increment the sync version number
    pub fn increment_sync_version(conn: &mut SqliteConnection) -> Result<i64, SshRepositoryError> {
        SyncMetaRepository::increment_sync_version(conn)
    }

    /// Set the `is_collapsed` status of all folder nodes to the given value at once.
    pub fn set_all_folders_collapsed(
        conn: &mut SqliteConnection,
        collapsed: bool,
    ) -> Result<(), SshRepositoryError> {
        diesel::update(ssh_nodes::table.filter(ssh_nodes::kind.eq(NodeKind::Folder.as_db_str())))
            .set((
                ssh_nodes::is_collapsed.eq(collapsed),
                ssh_nodes::updated_at.eq(Utc::now().naive_utc()),
            ))
            .execute(conn)?;
        Ok(())
    }

    fn get_node(conn: &mut SqliteConnection, node_id: &str) -> Result<SshNode, SshRepositoryError> {
        let row: SshNodeRow = ssh_nodes::table
            .find(node_id)
            .first(conn)
            .map_err(|e| match e {
                DieselError::NotFound => SshRepositoryError::NotFound(node_id.to_string()),
                other => SshRepositoryError::Db(other),
            })?;
        node_from_row(row)
    }
}

fn next_sort_order(
    conn: &mut SqliteConnection,
    parent_id: Option<&str>,
) -> Result<i32, SshRepositoryError> {
    let max: Option<i32> = match parent_id {
        Some(p) => ssh_nodes::table
            .filter(ssh_nodes::parent_id.eq(p))
            .select(diesel::dsl::max(ssh_nodes::sort_order))
            .first(conn)?,
        None => ssh_nodes::table
            .filter(ssh_nodes::parent_id.is_null())
            .select(diesel::dsl::max(ssh_nodes::sort_order))
            .first(conn)?,
    };
    Ok(max.unwrap_or(-1) + 1)
}

fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}

fn node_from_row(r: SshNodeRow) -> Result<SshNode, SshRepositoryError> {
    let kind = NodeKind::parse(&r.kind).ok_or_else(|| SshRepositoryError::InvalidEnum {
        column: "ssh_nodes.kind",
        value: r.kind.clone(),
    })?;
    Ok(SshNode {
        id: r.id,
        parent_id: r.parent_id,
        kind,
        name: r.name,
        sort_order: r.sort_order,
        created_at: r.created_at,
        updated_at: r.updated_at,
        is_collapsed: r.is_collapsed,
    })
}

fn server_from_row(r: SshServerRow) -> Result<SshServerInfo, SshRepositoryError> {
    let auth = AuthType::parse(&r.auth_type).ok_or_else(|| SshRepositoryError::InvalidEnum {
        column: "ssh_servers.auth_type",
        value: r.auth_type.clone(),
    })?;
    Ok(SshServerInfo {
        node_id: r.node_id,
        host: r.host,
        port: r.port as u16,
        username: r.username,
        auth_type: auth,
        key_path: r.key_path,
        startup_command: r.startup_command,
        notes: r.notes,
        last_connected_at: r.last_connected_at,
    })
}

/// Sync metadata repository, managing version numbers and sync records in the sync_meta table
pub struct SyncMetaRepository;

impl SyncMetaRepository {
    /// Get the sync version number
    pub fn get_sync_version(conn: &mut SqliteConnection) -> Result<i64, SshRepositoryError> {
        let row: Option<SyncMetaRow> = sync_meta::table
            .find("sync_version")
            .first(conn)
            .optional()?;
        Ok(row.and_then(|r| r.value.parse().ok()).unwrap_or(0))
    }

    /// Increment the sync version number and return the new value
    pub fn increment_sync_version(conn: &mut SqliteConnection) -> Result<i64, SshRepositoryError> {
        let current = Self::get_sync_version(conn)?;
        let new_version = current + 1;
        let val = new_version.to_string();
        diesel::replace_into(sync_meta::table)
            .values(NewSyncMeta {
                key: "sync_version",
                value: &val,
            })
            .execute(conn)?;
        Ok(new_version)
    }

    /// Set the sync version number
    pub fn set_sync_version(conn: &mut SqliteConnection, version: i64) -> Result<(), SshRepositoryError> {
        let val = version.to_string();
        diesel::replace_into(sync_meta::table)
            .values(NewSyncMeta {
                key: "sync_version",
                value: &val,
            })
            .execute(conn)?;
        Ok(())
    }

    /// Get the last sync time
    pub fn get_last_sync_time(conn: &mut SqliteConnection) -> Result<String, SshRepositoryError> {
        let row: Option<SyncMetaRow> = sync_meta::table
            .find("last_sync_time")
            .first(conn)
            .optional()?;
        Ok(row.map(|r| r.value).unwrap_or_default())
    }

    /// Get the last sync platform
    pub fn get_last_sync_platform(conn: &mut SqliteConnection) -> Result<String, SshRepositoryError> {
        let row: Option<SyncMetaRow> = sync_meta::table
            .find("last_sync_platform")
            .first(conn)
            .optional()?;
        Ok(row.map(|r| r.value).unwrap_or_default())
    }

    /// Update the sync metadata
    pub fn update_sync_meta(
        conn: &mut SqliteConnection,
        last_time: &str,
        last_platform: &str,
    ) -> Result<(), SshRepositoryError> {
        diesel::replace_into(sync_meta::table)
            .values(&[
                NewSyncMeta { key: "last_sync_time", value: last_time },
                NewSyncMeta { key: "last_sync_platform", value: last_platform },
            ])
            .execute(conn)?;
        Ok(())
    }
}

/// For testing: run all SSH-related migrations in an in-memory SQLite database.
/// When a new migration is added, its path needs to be appended using include_str! here.
#[cfg(test)]
pub(crate) fn setup_in_memory() -> SqliteConnection {
    use diesel::connection::SimpleConnection;
    let mut conn = SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
    for up in [
        include_str!(
            "../../persistence/migrations/2026-05-04-120000_add_ssh_manager_tables/up.sql"
        ),
        include_str!(
            "../../persistence/migrations/2026-05-04-130000_add_ssh_nodes_is_collapsed/up.sql"
        ),
        include_str!(
            "../../persistence/migrations/2026-05-23-140000_add_startup_command_and_notes/up.sql"
        ),
        include_str!(
            "../../persistence/migrations/2026-05-24-150000_add_sync_meta/up.sql"
        ),
    ] {
        conn.batch_execute(up).unwrap();
    }
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_server(name: &str) -> SshServerInfo {
        SshServerInfo {
            node_id: String::new(), // Allocated by create_server
            host: format!("{name}.example.com"),
            port: 22,
            username: "root".into(),
            auth_type: AuthType::Password,
            key_path: None,
            startup_command: None,
            notes: None,
            last_connected_at: None,
        }
    }

    #[test]
    fn create_and_list_root_folder() {
        let mut conn = setup_in_memory();
        let f = SshRepository::create_folder(&mut conn, None, "Prod").unwrap();
        assert_eq!(f.kind, NodeKind::Folder);
        assert_eq!(f.name, "Prod");
        assert!(f.parent_id.is_none());

        let all = SshRepository::list_nodes(&mut conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn nested_folders_and_server() {
        let mut conn = setup_in_memory();
        let prod = SshRepository::create_folder(&mut conn, None, "Prod").unwrap();
        let web = SshRepository::create_folder(&mut conn, Some(&prod.id), "Web").unwrap();
        let srv = SshRepository::create_server(
            &mut conn,
            Some(&web.id),
            "edge1",
            &sample_server("edge1"),
        )
        .unwrap();

        let all = SshRepository::list_nodes(&mut conn).unwrap();
        assert_eq!(all.len(), 3);
        let by_id: std::collections::HashMap<_, _> =
            all.into_iter().map(|n| (n.id.clone(), n)).collect();
        assert_eq!(by_id[&web.id].parent_id.as_deref(), Some(prod.id.as_str()));
        assert_eq!(by_id[&srv.id].parent_id.as_deref(), Some(web.id.as_str()));

        let server = SshRepository::get_server(&mut conn, &srv.id)
            .unwrap()
            .unwrap();
        assert_eq!(server.host, "edge1.example.com");
        assert_eq!(server.port, 22);
    }

    #[test]
    fn sort_order_appends_within_parent() {
        let mut conn = setup_in_memory();
        let a = SshRepository::create_folder(&mut conn, None, "A").unwrap();
        let b = SshRepository::create_folder(&mut conn, None, "B").unwrap();
        let c = SshRepository::create_folder(&mut conn, None, "C").unwrap();
        assert_eq!(a.sort_order, 0);
        assert_eq!(b.sort_order, 1);
        assert_eq!(c.sort_order, 2);

        // Different parents start from 0 respectively
        let child = SshRepository::create_folder(&mut conn, Some(&a.id), "child").unwrap();
        assert_eq!(child.sort_order, 0);
    }

    #[test]
    fn rename_and_update_server() {
        let mut conn = setup_in_memory();
        let s =
            SshRepository::create_server(&mut conn, None, "old", &sample_server("foo")).unwrap();
        SshRepository::rename_node(&mut conn, &s.id, "new").unwrap();
        let mut info = SshRepository::get_server(&mut conn, &s.id)
            .unwrap()
            .unwrap();
        info.host = "bar.example.com".into();
        info.port = 2222;
        info.auth_type = AuthType::Key;
        info.key_path = Some("/k".into());
        SshRepository::update_server(&mut conn, &info).unwrap();

        let got = SshRepository::get_server(&mut conn, &s.id)
            .unwrap()
            .unwrap();
        assert_eq!(got.host, "bar.example.com");
        assert_eq!(got.port, 2222);
        assert_eq!(got.auth_type, AuthType::Key);
        assert_eq!(got.key_path.as_deref(), Some("/k"));
    }

    #[test]
    fn delete_cascades_to_children_and_server_row() {
        let mut conn = setup_in_memory();
        let parent = SshRepository::create_folder(&mut conn, None, "P").unwrap();
        let _child =
            SshRepository::create_server(&mut conn, Some(&parent.id), "c", &sample_server("c"))
                .unwrap();
        SshRepository::delete_node(&mut conn, &parent.id).unwrap();

        assert!(SshRepository::list_nodes(&mut conn).unwrap().is_empty());
    }

    #[test]
    fn move_node_changes_parent_and_order() {
        let mut conn = setup_in_memory();
        let a = SshRepository::create_folder(&mut conn, None, "A").unwrap();
        let b = SshRepository::create_folder(&mut conn, None, "B").unwrap();
        let leaf =
            SshRepository::create_server(&mut conn, Some(&a.id), "x", &sample_server("x")).unwrap();

        SshRepository::move_node(&mut conn, &leaf.id, Some(&b.id), 5).unwrap();
        let nodes = SshRepository::list_nodes(&mut conn).unwrap();
        let leaf_now = nodes.iter().find(|n| n.id == leaf.id).unwrap();
        assert_eq!(leaf_now.parent_id.as_deref(), Some(b.id.as_str()));
        assert_eq!(leaf_now.sort_order, 5);
    }

    #[test]
    fn delete_missing_returns_not_found() {
        let mut conn = setup_in_memory();
        let err = SshRepository::delete_node(&mut conn, "nope").unwrap_err();
        assert!(matches!(err, SshRepositoryError::NotFound(_)));
    }

    // ---- SyncMetaRepository tests ----

    #[test]
    fn sync_meta_get_version_default() {
        let mut conn = setup_in_memory();
        let version = SyncMetaRepository::get_sync_version(&mut conn).unwrap();
        assert_eq!(version, 0, "sync_version should be 0 when there is no data");
    }

    #[test]
    fn sync_meta_set_and_get_version() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::set_sync_version(&mut conn, 42).unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 42);
    }

    #[test]
    fn sync_meta_increment_version() {
        let mut conn = setup_in_memory();
        let v1 = SyncMetaRepository::increment_sync_version(&mut conn).unwrap();
        assert_eq!(v1, 1);
        let v2 = SyncMetaRepository::increment_sync_version(&mut conn).unwrap();
        assert_eq!(v2, 2);
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 2);
    }

    #[test]
    fn sync_meta_increment_after_set() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::set_sync_version(&mut conn, 99).unwrap();
        let v = SyncMetaRepository::increment_sync_version(&mut conn).unwrap();
        assert_eq!(v, 100);
    }

    #[test]
    fn sync_meta_last_sync_time_default_empty() {
        let mut conn = setup_in_memory();
        let time = SyncMetaRepository::get_last_sync_time(&mut conn).unwrap();
        assert_eq!(time, "");
    }

    #[test]
    fn sync_meta_last_sync_platform_default_empty() {
        let mut conn = setup_in_memory();
        let platform = SyncMetaRepository::get_last_sync_platform(&mut conn).unwrap();
        assert_eq!(platform, "");
    }

    #[test]
    fn sync_meta_update_and_read() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::update_sync_meta(&mut conn, "2026-05-26T10:00:00Z", "github").unwrap();
        assert_eq!(SyncMetaRepository::get_last_sync_time(&mut conn).unwrap(), "2026-05-26T10:00:00Z");
        assert_eq!(SyncMetaRepository::get_last_sync_platform(&mut conn).unwrap(), "github");
    }

    #[test]
    fn sync_meta_update_overwrites_previous() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::update_sync_meta(&mut conn, "t1", "gitee").unwrap();
        SyncMetaRepository::update_sync_meta(&mut conn, "t2", "github").unwrap();
        assert_eq!(SyncMetaRepository::get_last_sync_time(&mut conn).unwrap(), "t2");
        assert_eq!(SyncMetaRepository::get_last_sync_platform(&mut conn).unwrap(), "github");
    }

    #[test]
    fn sync_meta_version_independent_of_meta() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::set_sync_version(&mut conn, 10).unwrap();
        SyncMetaRepository::update_sync_meta(&mut conn, "t1", "gitee").unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 10);
    }

    // ---- Collapsing operations should not increment sync_version ----

    #[test]
    fn set_collapsed_does_not_increment_sync_version() {
        let mut conn = setup_in_memory();
        let folder = SshRepository::create_folder(&mut conn, None, "F").unwrap();
        // create_folder increments it once; reset to 0 before testing
        SyncMetaRepository::set_sync_version(&mut conn, 0).unwrap();

        SshRepository::set_collapsed(&mut conn, &folder.id, true).unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 0,
            "set_collapsed should not increment sync_version");

        let node = SshRepository::list_nodes(&mut conn).unwrap().into_iter().next().unwrap();
        assert!(node.is_collapsed);
    }

    #[test]
    fn set_collapsed_false_does_not_increment_sync_version() {
        let mut conn = setup_in_memory();
        let folder = SshRepository::create_folder(&mut conn, None, "F").unwrap();
        SshRepository::set_collapsed(&mut conn, &folder.id, true).unwrap();
        SyncMetaRepository::set_sync_version(&mut conn, 0).unwrap();

        SshRepository::set_collapsed(&mut conn, &folder.id, false).unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 0,
            "set_collapsed(false) should not increment sync_version");
    }

    #[test]
    fn set_all_folders_collapsed_does_not_increment_sync_version() {
        let mut conn = setup_in_memory();
        SshRepository::create_folder(&mut conn, None, "A").unwrap();
        SshRepository::create_folder(&mut conn, None, "B").unwrap();
        SyncMetaRepository::set_sync_version(&mut conn, 0).unwrap();

        SshRepository::set_all_folders_collapsed(&mut conn, true).unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 0,
            "set_all_folders_collapsed should not increment sync_version");

        let nodes = SshRepository::list_nodes(&mut conn).unwrap();
        assert!(nodes.iter().all(|n| n.is_collapsed));
    }

    #[test]
    fn set_collapsed_missing_node_returns_not_found() {
        let mut conn = setup_in_memory();
        let err = SshRepository::set_collapsed(&mut conn, "nonexistent", true).unwrap_err();
        assert!(matches!(err, SshRepositoryError::NotFound(_)));
    }

    #[test]
    fn write_operations_do_increment_sync_version() {
        let mut conn = setup_in_memory();
        SyncMetaRepository::set_sync_version(&mut conn, 0).unwrap();

        let folder = SshRepository::create_folder(&mut conn, None, "F").unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 1);

        SshRepository::rename_node(&mut conn, &folder.id, "G").unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 2);

        SshRepository::delete_node(&mut conn, &folder.id).unwrap();
        assert_eq!(SyncMetaRepository::get_sync_version(&mut conn).unwrap(), 3);
    }
}
