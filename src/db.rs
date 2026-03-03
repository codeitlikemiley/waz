use rusqlite::{Connection, Result, params};
use std::collections::HashMap;
use std::path::PathBuf;

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
            CREATE INDEX IF NOT EXISTS idx_commands_session_ts ON commands(session_id, timestamp);",
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

    /// Build bigram frequency table: (prev_command, next_command) -> count.
    /// Only considers successful commands (exit_code = 0) within the same session.
    pub fn get_bigram_frequencies(&self) -> Result<HashMap<(String, String), u32>> {
        // Get all sessions with their commands in order
        let mut stmt = self.conn.prepare(
            "SELECT session_id, command FROM commands
             WHERE exit_code = 0
             ORDER BY session_id, timestamp ASC",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?;

        let mut bigrams: HashMap<(String, String), u32> = HashMap::new();
        let mut prev: Option<(String, String)> = None; // (session_id, command)

        for (session_id, command) in rows {
            if let Some((prev_session, prev_cmd)) = &prev {
                if prev_session == &session_id {
                    let key = (prev_cmd.clone(), command.clone());
                    *bigrams.entry(key).or_insert(0) += 1;
                }
            }
            prev = Some((session_id, command));
        }

        Ok(bigrams)
    }

    /// Get the most likely next command after `prev_command` based on bigram frequency.
    /// Returns (next_command, count, total) where total is all occurrences after prev_command.
    pub fn get_next_command_by_sequence(
        &self,
        prev_command: &str,
    ) -> Result<Option<(String, u32, u32)>> {
        let bigrams = self.get_bigram_frequencies()?;

        let mut candidates: Vec<(String, u32)> = Vec::new();
        let mut total = 0u32;

        for ((prev, next), count) in &bigrams {
            if prev == prev_command {
                candidates.push((next.clone(), *count));
                total += count;
            }
        }

        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(candidates
            .into_iter()
            .next()
            .map(|(cmd, count)| (cmd, count, total)))
    }

    /// Get total number of commands in the database.
    pub fn command_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
    }
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
    fn test_bigram_frequencies() {
        let db = HistoryDb::open_in_memory().unwrap();

        // Session 1: git add -> git commit -> git push
        db.insert_command_with_timestamp("git add .", "/proj", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("git commit -m 'msg'", "/proj", "s1", 0, 1001)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s1", 0, 1002)
            .unwrap();

        // Session 2: git add -> git commit -> git push (same pattern)
        db.insert_command_with_timestamp("git add .", "/proj", "s2", 0, 2000)
            .unwrap();
        db.insert_command_with_timestamp("git commit -m 'fix'", "/proj", "s2", 0, 2001)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s2", 0, 2002)
            .unwrap();

        let bigrams = db.get_bigram_frequencies().unwrap();
        // "git add ." -> "git commit *" should appear twice (different exact commands)
        let count_add_to_commit: u32 = bigrams
            .iter()
            .filter(|((prev, _next), _)| prev == "git add .")
            .map(|(_, c)| c)
            .sum();
        assert_eq!(count_add_to_commit, 2);
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

        let result = db
            .get_next_command_by_sequence("git commit -m 'msg'")
            .unwrap();
        assert!(result.is_some());
        let (cmd, count, total) = result.unwrap();
        assert_eq!(cmd, "git push");
        assert_eq!(count, 3);
        assert_eq!(total, 3);
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
}
