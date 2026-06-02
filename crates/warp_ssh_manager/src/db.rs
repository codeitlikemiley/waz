//! The entire process shares a single `Mutex<SqliteConnection>` for the SSH manager.
//!
//! Current situation: The main write connection of openWarp is in a dedicated write thread (see `app/src/persistence/sqlite.rs`)
//! processed asynchronously via the `ModelEvent` channel. Integrating that event bus into the SSH manager would require adding 6+ enum
//! variants and cross-crate type exposure, which is too costly.
//!
//! Alternative solution: **SQLite WAL mode naturally supports multiple write connections** (writes are mutually exclusive but with busy_timeout retries),
//! so we open another independent write connection here, whose behavior is completely localized inside this crate. The write operations of the SSH manager
//! are user-driven (creating/deleting nodes) and extremely infrequent, so conflicts with the main write thread can be neglected.
//!
//! The path is passed in by the caller during initialization (`set_database_path`), preventing this crate from directly depending on the app
//! layer's `database_file_path()`. If the path is not provided, `with_conn` returns `Err(NotInitialized)`.

use anyhow::{Result, anyhow};
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static DB_PATH: OnceLock<PathBuf> = OnceLock::new();
static CONN: OnceLock<Mutex<SqliteConnection>> = OnceLock::new();

/// Called once by the app during startup, passing in the sqlite db file path. Subsequent calls are ignored
/// (OnceLock semantics).
pub fn set_database_path(path: PathBuf) {
    let _ = DB_PATH.set(path);
}

fn open() -> Result<SqliteConnection> {
    let path = DB_PATH
        .get()
        .ok_or_else(|| anyhow!("warp_ssh_manager::db: database path not initialized"))?;
    let url = path.to_string_lossy();
    let mut conn = SqliteConnection::establish(&url)?;
    conn.batch_execute(
        "PRAGMA foreign_keys = ON; \
         PRAGMA busy_timeout = 2000; \
         PRAGMA journal_mode = WAL;",
    )?;
    Ok(conn)
}

/// Executes a closure within the lock. Lazily opens the connection on first call; subsequent calls reuse it.
pub fn with_conn<R>(f: impl FnOnce(&mut SqliteConnection) -> Result<R>) -> Result<R> {
    let mtx = CONN.get_or_init(|| Mutex::new(open().expect("warp_ssh_manager db open")));
    let mut guard = mtx
        .lock()
        .map_err(|_| anyhow!("warp_ssh_manager db mutex poisoned"))?;
    f(&mut guard)
}
