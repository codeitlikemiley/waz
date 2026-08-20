use rusqlite::{params, Connection, OptionalExtension, Result};
use std::path::PathBuf;

use crate::normalize::like_prefix_pattern;

/// Represents a recorded command entry.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CommandEntry {
    pub id: i64,
    pub command: String,
    pub cwd: String,
    pub timestamp: i64,
    pub session_id: String,
    pub exit_code: i32,
}

/// Database handle for the command history.
pub struct HistoryDb {
    conn: Connection,
}

impl HistoryDb {
    /// Open (or create) the history database at the given path.
    pub fn open(path: &PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                cwd TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                session_id TEXT NOT NULL,
                exit_code INTEGER DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_commands_cwd ON commands(cwd);
            CREATE INDEX IF NOT EXISTS idx_commands_session ON commands(session_id);
            CREATE INDEX IF NOT EXISTS idx_commands_timestamp ON commands(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_commands_session_ts ON commands(session_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_commands_cwd_session_ts
                ON commands(cwd, session_id, timestamp, id);",
        )?;
        Ok(())
    }

    /// Insert a new command record.
    pub fn insert_command(
        &self,
        command: &str,
        cwd: &str,
        session_id: &str,
        exit_code: i32,
    ) -> Result<()> {
        let ts = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO commands (command, cwd, timestamp, session_id, exit_code)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![command, cwd, ts, session_id, exit_code],
        )?;
        Ok(())
    }

    /// Insert a command with a specific timestamp (used for history import).
    pub fn insert_command_with_timestamp(
        &self,
        command: &str,
        cwd: &str,
        session_id: &str,
        exit_code: i32,
        timestamp: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO commands (command, cwd, timestamp, session_id, exit_code)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![command, cwd, timestamp, session_id, exit_code],
        )?;
        Ok(())
    }

    /// Get recent commands filtered by CWD, optionally filtered by a prefix.
    /// Returns deduplicated commands ordered by most recent first.
    pub fn get_recent_by_cwd(
        &self,
        cwd: &str,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match prefix {
            Some(pfx) => {
                let pattern = format!("{}%", pfx);
                (
                    format!(
                        "SELECT command FROM commands
                         WHERE cwd = ?1 AND command LIKE ?2 AND exit_code = 0
                         GROUP BY command
                         ORDER BY MAX(timestamp) DESC
                         LIMIT {}",
                        limit
                    ),
                    vec![
                        Box::new(cwd.to_string()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(pattern),
                    ],
                )
            }
            None => (
                format!(
                    "SELECT command FROM commands
                     WHERE cwd = ?1 AND exit_code = 0
                     GROUP BY command
                     ORDER BY MAX(timestamp) DESC
                     LIMIT {}",
                    limit
                ),
                vec![Box::new(cwd.to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
        };

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Get all commands from the current session in chronological order.
    pub fn get_session_commands(&self, session_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT command FROM commands
             WHERE session_id = ?1
             ORDER BY timestamp ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Most recent command in this session, including failed ones.
    pub fn get_last_command(&self, session_id: &str) -> Result<Option<(String, i32)>> {
        self.conn
            .query_row(
                "SELECT command, exit_code FROM commands
                 WHERE session_id = ?1
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Ranked next-command candidates after `prev_command` (exact or sequence-key match).
    ///
    /// Returns `(next_command, count, total)` sorted by count descending. `total` is the
    /// sum of all next-command counts for this predecessor so callers can compute confidence.
    /// When `cwd` is set, both sides of the pair must be from that directory.
    pub fn get_next_commands_by_sequence(
        &self,
        prev_command: &str,
        prev_key: &str,
        cwd: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, u32, u32)>> {
        if prev_command.is_empty() || prev_key.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let key_like = like_prefix_pattern(prev_key);
        let limit = limit as i64;

        let sql = if cwd.is_some() {
            "WITH pairs AS (
                 SELECT
                     command AS prev_cmd,
                     LEAD(command) OVER (
                         PARTITION BY session_id
                         ORDER BY timestamp ASC, id ASC
                     ) AS next_cmd
                 FROM commands
                 WHERE exit_code = 0 AND cwd = ?1
             ),
             ranked AS (
                 SELECT next_cmd, COUNT(*) AS cnt
                 FROM pairs
                 WHERE next_cmd IS NOT NULL
                   AND (
                       prev_cmd = ?2
                       OR prev_cmd = ?3
                       OR prev_cmd LIKE ?4 ESCAPE '\\'
                   )
                 GROUP BY next_cmd
             )
             SELECT next_cmd, cnt, SUM(cnt) OVER () AS total
             FROM ranked
             ORDER BY cnt DESC, next_cmd ASC
             LIMIT ?5"
        } else {
            "WITH pairs AS (
                 SELECT
                     command AS prev_cmd,
                     LEAD(command) OVER (
                         PARTITION BY session_id
                         ORDER BY timestamp ASC, id ASC
                     ) AS next_cmd
                 FROM commands
                 WHERE exit_code = 0
             ),
             ranked AS (
                 SELECT next_cmd, COUNT(*) AS cnt
                 FROM pairs
                 WHERE next_cmd IS NOT NULL
                   AND (
                       prev_cmd = ?1
                       OR prev_cmd = ?2
                       OR prev_cmd LIKE ?3 ESCAPE '\\'
                   )
                 GROUP BY next_cmd
             )
             SELECT next_cmd, cnt, SUM(cnt) OVER () AS total
             FROM ranked
             ORDER BY cnt DESC, next_cmd ASC
             LIMIT ?4"
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = if let Some(dir) = cwd {
            stmt.query_map(
                params![dir, prev_command, prev_key, key_like, limit],
                map_sequence_row,
            )?
            .collect::<Result<Vec<_>>>()?
        } else {
            stmt.query_map(
                params![prev_command, prev_key, key_like, limit],
                map_sequence_row,
            )?
            .collect::<Result<Vec<_>>>()?
        };

        Ok(rows)
    }

    /// Get total number of commands in the database.
    pub fn command_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
    }

    /// Delete all history for a specific working directory.
    pub fn clear_by_cwd(&self, cwd: &str) -> Result<usize> {
        self.conn
            .execute("DELETE FROM commands WHERE cwd = ?1", params![cwd])
    }

    /// Delete all history.
    pub fn clear_all(&self) -> Result<usize> {
        self.conn.execute("DELETE FROM commands", [])
    }

    /// Count commands for a specific working directory.
    pub fn count_by_cwd(&self, cwd: &str) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM commands WHERE cwd = ?1",
            params![cwd],
            |row| row.get(0),
        )
    }
}

fn map_sequence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u32, u32)> {
    Ok((
        row.get(0)?,
        row.get::<_, i64>(1)? as u32,
        row.get::<_, i64>(2)? as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_query() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("git status", "/home/user/project", "sess1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("git add .", "/home/user/project", "sess1", 0, 2000)
            .unwrap();
        db.insert_command_with_timestamp("cargo build", "/home/user/other", "sess1", 0, 3000)
            .unwrap();

        let results = db
            .get_recent_by_cwd("/home/user/project", None, 10)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], "git add .");
        assert_eq!(results[1], "git status");
    }

    #[test]
    fn test_prefix_filter() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("git status", "/home/user/project", "sess1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("git add .", "/home/user/project", "sess1", 0, 2000)
            .unwrap();
        db.insert_command_with_timestamp("cargo build", "/home/user/project", "sess1", 0, 3000)
            .unwrap();

        let results = db
            .get_recent_by_cwd("/home/user/project", Some("git"), 10)
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_sequence_stemming_groups_commit_messages() {
        let db = HistoryDb::open_in_memory().unwrap();

        db.insert_command_with_timestamp("git add .", "/proj", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("git commit -m 'msg'", "/proj", "s1", 0, 1001)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s1", 0, 1002)
            .unwrap();

        db.insert_command_with_timestamp("git add .", "/proj", "s2", 0, 2000)
            .unwrap();
        db.insert_command_with_timestamp("git commit -m 'fix'", "/proj", "s2", 0, 2001)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s2", 0, 2002)
            .unwrap();

        let key = crate::normalize::sequence_key("git commit -m 'later'");
        let candidates = db
            .get_next_commands_by_sequence("git commit -m 'later'", &key, Some("/proj"), 5)
            .unwrap();
        assert_eq!(candidates[0].0, "git push");
        assert_eq!(candidates[0].1, 2);
    }

    #[test]
    fn test_sequence_prediction() {
        let db = HistoryDb::open_in_memory().unwrap();

        // Build a pattern: git commit -> git push (3 times)
        for i in 0..3 {
            let sess = format!("s{}", i);
            let base = (i * 100) as i64;
            db.insert_command_with_timestamp("git commit -m 'msg'", "/proj", &sess, 0, base)
                .unwrap();
            db.insert_command_with_timestamp("git push", "/proj", &sess, 0, base + 1)
                .unwrap();
        }

        let key = crate::normalize::sequence_key("git commit -m 'msg'");
        let result = db
            .get_next_commands_by_sequence("git commit -m 'msg'", &key, Some("/proj"), 1)
            .unwrap();
        assert!(!result.is_empty());
        let (cmd, count, total) = &result[0];
        assert_eq!(cmd, "git push");
        assert_eq!(*count, 3);
        assert_eq!(*total, 3);
    }

    #[test]
    fn test_excludes_failed_commands() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("failing-cmd", "/proj", "s1", 1).unwrap();
        db.insert_command("good-cmd", "/proj", "s1", 0).unwrap();

        let results = db.get_recent_by_cwd("/proj", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "good-cmd");
    }

    #[test]
    fn test_last_command_includes_failures() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("good", "/proj", "s1", 0, 1)
            .unwrap();
        db.insert_command_with_timestamp("bad", "/proj", "s1", 1, 2)
            .unwrap();

        let last = db.get_last_command("s1").unwrap().unwrap();
        assert_eq!(last.0, "bad");
        assert_eq!(last.1, 1);
    }

    #[test]
    fn test_sequence_prefix_candidates_are_ranked() {
        let db = HistoryDb::open_in_memory().unwrap();
        for i in 0..3 {
            let sess = format!("a{i}");
            db.insert_command_with_timestamp("cargo test", "/proj", &sess, 0, i * 10)
                .unwrap();
            db.insert_command_with_timestamp("cargo run", "/proj", &sess, 0, i * 10 + 1)
                .unwrap();
        }
        db.insert_command_with_timestamp("cargo test", "/proj", "b", 0, 100)
            .unwrap();
        db.insert_command_with_timestamp("cargo clippy", "/proj", "b", 0, 101)
            .unwrap();

        let key = crate::normalize::sequence_key("cargo test");
        let candidates = db
            .get_next_commands_by_sequence("cargo test", &key, Some("/proj"), 5)
            .unwrap();
        assert_eq!(candidates[0].0, "cargo run");
        assert_eq!(candidates[0].1, 3);
        assert_eq!(candidates[1].0, "cargo clippy");
        assert_eq!(candidates[1].1, 1);
        assert_eq!(candidates[0].2, 4);
    }
}
