use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::db::HistoryDb;
use crate::hint;
use crate::llm;
use crate::normalize::{
    is_hub_command, matches_prefix, prefix_is_exact, sequence_key, shell_quote, tokenize,
};

/// A prediction result with confidence.
#[derive(Debug, Clone)]
pub struct Prediction {
    pub command: String,
    pub confidence: f64,
    pub tier: PredictionTier,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PredictionTier {
    /// Tier 0: Extracted from previous command's output (highest priority)
    OutputHint,
    /// Tier 1: Based on command sequence patterns
    Sequence,
    /// Tier 2: Deterministic follow-up from the last command (mkdir → cd, …)
    Workflow,
    /// Tier 3: Based on CWD-filtered history
    CwdHistory,
    /// Tier 4: LLM-based prediction (lowest confidence)
    Llm,
}

impl std::fmt::Display for PredictionTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredictionTier::OutputHint => write!(f, "output_hint"),
            PredictionTier::Sequence => write!(f, "sequence"),
            PredictionTier::Workflow => write!(f, "workflow"),
            PredictionTier::CwdHistory => write!(f, "cwd_history"),
            PredictionTier::Llm => write!(f, "llm"),
        }
    }
}

/// Minimum confidence threshold for Tier 1 sequence predictions.
/// Set low to enable proactive prediction from limited history.
const SEQUENCE_MIN_CONFIDENCE: f64 = 0.15;
/// Minimum number of occurrences needed for sequence prediction.
/// Set to 1 so a single occurrence of a sequence is enough to suggest.
const SEQUENCE_MIN_COUNT: u32 = 1;
/// Hub commands (`ls`, `cat`, …) need a stronger repeating pattern.
const HUB_SEQUENCE_MIN_COUNT: u32 = 3;

/// Multi-tier prediction engine.
pub struct PredictionEngine<'a> {
    db: &'a HistoryDb,
    config: Config,
    hint_path: std::path::PathBuf,
}

impl<'a> PredictionEngine<'a> {
    pub fn new(db: &'a HistoryDb) -> Self {
        Self::with_config(db, Config::load())
    }

    /// Fast interactive path: skip disk config (and therefore the LLM provider list).
    pub fn new_fast(db: &'a HistoryDb) -> Self {
        Self::with_config(db, Config::default())
    }

    fn with_config(db: &'a HistoryDb, config: Config) -> Self {
        Self {
            db,
            config,
            hint_path: hint::hint_file_path(),
        }
    }

    /// Run multi-tier prediction. Returns the best prediction or None.
    ///
    /// - `session_id`: current shell session for sequence analysis
    /// - `cwd`: current working directory
    /// - `prefix`: what the user has typed so far (can be empty)
    /// - `fast`: if true, skip the LLM tier (for interactive typing)
    pub fn predict(
        &self,
        session_id: &str,
        cwd: &str,
        prefix: Option<&str>,
        fast: bool,
    ) -> Option<Prediction> {
        // Tier 0: Output hint from previous command's output
        if let Some(pred) = self.predict_by_output_hint(prefix) {
            return Some(pred);
        }

        // Tier 1: Sequence-based prediction (CWD first, then global)
        if let Some(pred) = self.predict_by_sequence(session_id, cwd, prefix) {
            return Some(pred);
        }

        // Tier 2: Deterministic workflow follow-up from the last command
        if let Some(pred) = self.predict_by_workflow(session_id, cwd, prefix) {
            return Some(pred);
        }

        // Tier 3: CWD-filtered history
        if let Some(pred) = self.predict_by_cwd(session_id, cwd, prefix) {
            return Some(pred);
        }

        // Tier 4: LLM fallback (skip in fast mode to avoid keystroke lag)
        if !fast {
            return self.predict_by_llm(session_id, cwd, prefix);
        }

        None
    }

    /// Tier 0: Check if the previous command's output suggested a follow-up command.
    /// Consumed only when the hint matches the current prefix, so typing something
    /// else does not throw the hint away.
    fn predict_by_output_hint(&self, prefix: Option<&str>) -> Option<Prediction> {
        let cmd = hint::peek_hint_at(&self.hint_path)?;
        if !matches_prefix(&cmd, prefix) || prefix_is_exact(&cmd, prefix) {
            return None;
        }
        let _ = hint::consume_hint_at(&self.hint_path);

        Some(Prediction {
            command: cmd,
            confidence: 1.0,
            tier: PredictionTier::OutputHint,
        })
    }

    /// Tier 1: Look at the last successful command in this session and predict
    /// the next one from historical sequences (stemmed bigrams).
    fn predict_by_sequence(
        &self,
        session_id: &str,
        cwd: &str,
        prefix: Option<&str>,
    ) -> Option<Prediction> {
        let (last_cmd, exit_code) = self.db.get_last_command(session_id).ok()??;
        if exit_code != 0 {
            return None;
        }

        let key = sequence_key(&last_cmd);
        if let Some(pred) = self.sequence_from_scope(&last_cmd, &key, Some(cwd), prefix) {
            return Some(pred);
        }

        self.sequence_from_scope(&last_cmd, &key, None, prefix)
            .map(|mut pred| {
                pred.confidence *= 0.7;
                pred
            })
    }

    fn sequence_from_scope(
        &self,
        last_cmd: &str,
        key: &str,
        cwd: Option<&str>,
        prefix: Option<&str>,
    ) -> Option<Prediction> {
        let candidates = self
            .db
            .get_next_commands_by_sequence(last_cmd, key, cwd, 20)
            .ok()?;
        if candidates.is_empty() {
            return None;
        }

        let hub = is_hub_command(last_cmd);
        let min_count = if hub {
            HUB_SEQUENCE_MIN_COUNT
        } else {
            SEQUENCE_MIN_COUNT
        };

        for (next_cmd, count, total) in candidates {
            if count < min_count || total == 0 {
                continue;
            }
            let confidence = count as f64 / total as f64;
            if confidence < SEQUENCE_MIN_CONFIDENCE {
                continue;
            }
            if !matches_prefix(&next_cmd, prefix) || prefix_is_exact(&next_cmd, prefix) {
                continue;
            }
            return Some(Prediction {
                command: next_cmd,
                confidence,
                tier: PredictionTier::Sequence,
            });
        }

        None
    }

    /// Tier 2: Argument-carrying follow-ups that do not need learned history.
    fn predict_by_workflow(
        &self,
        session_id: &str,
        cwd: &str,
        prefix: Option<&str>,
    ) -> Option<Prediction> {
        let (last_cmd, exit_code) = self.db.get_last_command(session_id).ok()??;
        if exit_code != 0 {
            return None;
        }

        let cmd = workflow_followup(&last_cmd, cwd)?;
        if !matches_prefix(&cmd, prefix) || prefix_is_exact(&cmd, prefix) || cmd == last_cmd {
            return None;
        }

        Some(Prediction {
            command: cmd,
            confidence: 0.8,
            tier: PredictionTier::Workflow,
        })
    }

    /// Tier 3: Find a recently used command in this CWD,
    /// optionally filtered by what the user is typing.
    fn predict_by_cwd(
        &self,
        session_id: &str,
        cwd: &str,
        prefix: Option<&str>,
    ) -> Option<Prediction> {
        let results = self.db.get_recent_by_cwd(cwd, prefix, 8).ok()?;
        let empty_prefix = prefix.map(|p| p.is_empty()).unwrap_or(true);
        let last = self
            .db
            .get_last_command(session_id)
            .ok()
            .flatten()
            .map(|(cmd, _)| cmd);

        let cmd = results.into_iter().find(|candidate| {
            if prefix_is_exact(candidate, prefix) {
                return false;
            }
            if empty_prefix {
                if let Some(last) = last.as_ref() {
                    if candidate == last {
                        return false;
                    }
                }
            }
            true
        })?;

        Some(Prediction {
            command: cmd,
            confidence: 0.2,
            tier: PredictionTier::CwdHistory,
        })
    }

    /// Tier 4: Use an LLM to predict the next command based on shell context.
    fn predict_by_llm(
        &self,
        session_id: &str,
        cwd: &str,
        prefix: Option<&str>,
    ) -> Option<Prediction> {
        let mut recent: Vec<String> = Vec::new();

        if let Ok(session_cmds) = self.db.get_session_commands(session_id) {
            recent.extend(session_cmds);
        }

        if let Ok(cwd_cmds) = self.db.get_recent_by_cwd(cwd, None, 10) {
            for cmd in cwd_cmds {
                if !recent.contains(&cmd) {
                    recent.push(cmd);
                }
            }
        }

        let context: Vec<String> = recent.into_iter().rev().take(15).collect();

        let cmd = llm::predict_with_llm(&self.config, &context, cwd, prefix)?;
        if prefix_is_exact(&cmd, prefix) {
            return None;
        }

        Some(Prediction {
            command: cmd,
            confidence: 0.1,
            tier: PredictionTier::Llm,
        })
    }
}

fn workflow_followup(last_cmd: &str, cwd: &str) -> Option<String> {
    let tokens = tokenize(last_cmd);
    let bin = tokens.first()?.as_str();
    match bin {
        "mkdir" => mkdir_cd(&tokens),
        "git" => git_followup(&tokens, cwd),
        "cargo" => cargo_followup(&tokens),
        "npm" | "pnpm" | "yarn" | "bun" => package_manager_followup(&tokens, cwd),
        _ => None,
    }
}

fn mkdir_cd(tokens: &[String]) -> Option<String> {
    let dir = tokens
        .iter()
        .skip(1)
        .filter(|t| *t != "--" && !t.starts_with('-'))
        .next_back()?;
    if dir == "mkdir" {
        return None;
    }
    Some(format!("cd {}", shell_quote(dir)))
}

fn git_followup(tokens: &[String], cwd: &str) -> Option<String> {
    match tokens.get(1).map(|s| s.as_str()) {
        Some("clone") => git_clone_cd(tokens),
        Some("commit") if git_has_remote(cwd) => Some("git push".to_string()),
        _ => None,
    }
}

fn git_has_remote(cwd: &str) -> bool {
    find_git_dir(Path::new(cwd))
        .and_then(|git_dir| fs::read_dir(git_dir.join("refs").join("remotes")).ok())
        .is_some_and(|entries| entries.flatten().next().is_some())
}

fn find_git_dir(start: &Path) -> Option<std::path::PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let git = current.join(".git");
        if git.is_dir() {
            return Some(git);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn git_clone_cd(tokens: &[String]) -> Option<String> {
    let mut args = Vec::new();
    let mut iter = tokens.iter().skip(2);
    while let Some(token) = iter.next() {
        if token == "--" {
            args.extend(iter.cloned());
            break;
        }
        if token.starts_with('-') {
            if matches!(
                token.as_str(),
                "-b" | "--branch"
                    | "-o"
                    | "--origin"
                    | "--depth"
                    | "-c"
                    | "--config"
                    | "--separate-git-dir"
            ) {
                let _ = iter.next();
            }
            continue;
        }
        args.push(token.clone());
    }

    let dir = match args.as_slice() {
        [repo] => repo_name_from_url(repo)?,
        [_, dir] => dir.clone(),
        _ => return None,
    };
    Some(format!("cd {}", shell_quote(&dir)))
}

fn repo_name_from_url(repo: &str) -> Option<String> {
    let trimmed = repo.trim_end_matches('/').trim_end_matches(".git");
    let name = trimmed.rsplit(['/', ':']).next()?;
    if name.is_empty() || name == "." {
        None
    } else {
        Some(name.to_string())
    }
}

fn cargo_followup(tokens: &[String]) -> Option<String> {
    if tokens.get(1).map(|s| s.as_str()) != Some("new") {
        return None;
    }
    let name = tokens.iter().skip(2).find(|t| !t.starts_with('-'))?;
    Some(format!("cd {}", shell_quote(name)))
}

fn package_manager_followup(tokens: &[String], cwd: &str) -> Option<String> {
    if !is_install_command(tokens) {
        return None;
    }
    let script = preferred_package_script(cwd)?;
    Some(format_pm_script(&tokens[0], &script))
}

fn is_install_command(tokens: &[String]) -> bool {
    match tokens.first().map(|s| s.as_str()) {
        Some("yarn") => {
            tokens.len() == 1
                || matches!(
                    tokens.get(1).map(|s| s.as_str()),
                    Some("install") | Some("add")
                )
        }
        Some("npm") | Some("pnpm") | Some("bun") => matches!(
            tokens.get(1).map(|s| s.as_str()),
            Some("install") | Some("i") | Some("ci") | Some("add")
        ),
        _ => false,
    }
}

fn preferred_package_script(cwd: &str) -> Option<String> {
    let path = Path::new(cwd).join("package.json");
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let scripts = value.get("scripts")?.as_object()?;
    for name in ["dev", "start", "serve"] {
        if scripts.contains_key(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn format_pm_script(pm: &str, script: &str) -> String {
    match (pm, script) {
        ("npm", "start") => "npm start".to_string(),
        ("npm", s) => format!("npm run {s}"),
        ("yarn", s) => format!("yarn {s}"),
        ("pnpm", "start") => "pnpm start".to_string(),
        ("pnpm", s) => format!("pnpm {s}"),
        ("bun", s) => format!("bun run {s}"),
        (other, s) => format!("{other} run {s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::HistoryDb;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn engine<'a>(db: &'a HistoryDb) -> PredictionEngine<'a> {
        PredictionEngine {
            db,
            config: Config::default(),
            hint_path: std::env::temp_dir().join(format!(
                "waz-predict-hint-unused-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
        }
    }

    fn unique_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("waz-{name}-{unique}"))
    }

    #[test]
    fn test_sequence_prediction() {
        let db = HistoryDb::open_in_memory().unwrap();

        for i in 0..5 {
            let sess = format!("old_s{}", i);
            let base = (i * 100) as i64;
            db.insert_command_with_timestamp("git commit -m 'msg'", "/proj", &sess, 0, base)
                .unwrap();
            db.insert_command_with_timestamp("git push", "/proj", &sess, 0, base + 1)
                .unwrap();
        }

        db.insert_command("git commit -m 'msg'", "/proj", "current", 0)
            .unwrap();

        let engine = engine(&db);
        let pred = engine.predict("current", "/proj", None, true).unwrap();
        assert_eq!(pred.command, "git push");
        assert_eq!(pred.tier, PredictionTier::Sequence);
        assert!(pred.confidence >= SEQUENCE_MIN_CONFIDENCE);
    }

    #[test]
    fn sequence_matches_different_commit_messages() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("git commit -m 'one'", "/proj", "s1", 0, 1)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s1", 0, 2)
            .unwrap();
        db.insert_command("git commit -m 'two'", "/proj", "current", 0)
            .unwrap();

        let pred = engine(&db).predict("current", "/proj", None, true).unwrap();
        assert_eq!(pred.command, "git push");
        assert_eq!(pred.tier, PredictionTier::Sequence);
    }

    #[test]
    fn sequence_uses_prefix_to_pick_among_candidates() {
        let db = HistoryDb::open_in_memory().unwrap();
        for i in 0..3 {
            let sess = format!("s{i}");
            db.insert_command_with_timestamp("cargo test", "/proj", &sess, 0, i * 10)
                .unwrap();
            db.insert_command_with_timestamp("cargo run", "/proj", &sess, 0, i * 10 + 1)
                .unwrap();
        }
        db.insert_command_with_timestamp("cargo test", "/proj", "other", 0, 100)
            .unwrap();
        db.insert_command_with_timestamp("cargo clippy", "/proj", "other", 0, 101)
            .unwrap();
        db.insert_command("cargo test", "/proj", "current", 0)
            .unwrap();

        let pred = engine(&db)
            .predict("current", "/proj", Some("cargo c"), true)
            .unwrap();
        assert_eq!(pred.command, "cargo clippy");
        assert_eq!(pred.tier, PredictionTier::Sequence);
    }

    #[test]
    fn test_cwd_fallback() {
        let db = HistoryDb::open_in_memory().unwrap();

        db.insert_command_with_timestamp("npm test", "/frontend", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("npm run build", "/frontend", "s1", 0, 2000)
            .unwrap();

        let pred = engine(&db)
            .predict("new_session", "/frontend", None, true)
            .unwrap();
        assert_eq!(pred.command, "npm run build");
        assert_eq!(pred.tier, PredictionTier::CwdHistory);
    }

    #[test]
    fn cwd_skips_just_run_command_on_empty_prompt() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("npm test", "/frontend", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("npm run build", "/frontend", "current", 0, 2000)
            .unwrap();

        let pred = engine(&db)
            .predict("current", "/frontend", None, true)
            .unwrap();
        assert_eq!(pred.command, "npm test");
        assert_eq!(pred.tier, PredictionTier::CwdHistory);
    }

    #[test]
    fn test_prefix_filtering() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("npm test", "/frontend", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("cargo build", "/frontend", "s1", 0, 2000)
            .unwrap();

        let pred = engine(&db)
            .predict("new_session", "/frontend", Some("npm"), true)
            .unwrap();
        assert_eq!(pred.command, "npm test");
    }

    #[test]
    fn test_no_local_prediction() {
        let db = HistoryDb::open_in_memory().unwrap();
        let pred = engine(&db).predict("empty", "/nowhere", None, true);
        assert!(pred.is_none());
    }

    #[test]
    fn failed_last_command_skips_sequence_and_workflow() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("git commit -m 'x'", "/proj", "s1", 0, 1)
            .unwrap();
        db.insert_command_with_timestamp("git push", "/proj", "s1", 0, 2)
            .unwrap();
        db.insert_command("git commit -m 'y'", "/proj", "current", 1)
            .unwrap();

        let pred = engine(&db).predict("current", "/proj", None, true);
        assert!(pred.is_none() || pred.unwrap().tier != PredictionTier::Sequence);
    }

    #[test]
    fn workflow_mkdir_suggests_cd() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("mkdir -p foo/bar", "/tmp", "s", 0)
            .unwrap();

        let pred = engine(&db).predict("s", "/tmp", None, true).unwrap();
        assert_eq!(pred.command, "cd foo/bar");
        assert_eq!(pred.tier, PredictionTier::Workflow);
    }

    #[test]
    fn workflow_git_clone_suggests_cd() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("git clone https://github.com/foo/waz.git", "/tmp", "s", 0)
            .unwrap();

        let pred = engine(&db).predict("s", "/tmp", None, true).unwrap();
        assert_eq!(pred.command, "cd waz");
        assert_eq!(pred.tier, PredictionTier::Workflow);
    }

    #[test]
    fn workflow_npm_install_reads_package_scripts() {
        let dir = unique_path("npm-proj");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"dev":"vite","start":"node index.js"}}"#,
        )
        .unwrap();

        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("npm install", dir.to_str().unwrap(), "s", 0)
            .unwrap();

        let pred = engine(&db)
            .predict("s", dir.to_str().unwrap(), None, true)
            .unwrap();
        assert_eq!(pred.command, "npm run dev");
        assert_eq!(pred.tier, PredictionTier::Workflow);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_hint_prefix_mismatch_does_not_consume() {
        let db = HistoryDb::open_in_memory().unwrap();
        let path = unique_path("hint.txt");
        hint::save_hint_at(&path, "npm start");

        let engine = PredictionEngine {
            db: &db,
            config: Config::default(),
            hint_path: path.clone(),
        };

        assert!(engine.predict("s", "/tmp", Some("git"), true).is_none());
        assert_eq!(hint::peek_hint_at(&path).as_deref(), Some("npm start"));

        let pred = engine.predict("s", "/tmp", Some("npm"), true).unwrap();
        assert_eq!(pred.command, "npm start");
        assert_eq!(pred.tier, PredictionTier::OutputHint);
        assert_eq!(hint::peek_hint_at(&path), None);
    }

    #[test]
    fn workflow_git_commit_suggests_push_when_remote_exists() {
        let dir = unique_path("git-repo");
        fs::create_dir_all(dir.join(".git/refs/remotes/origin")).unwrap();
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("git commit -m 'init'", dir.to_str().unwrap(), "s", 0)
            .unwrap();

        let pred = engine(&db)
            .predict("s", dir.to_str().unwrap(), None, true)
            .unwrap();
        assert_eq!(pred.command, "git push");
        assert_eq!(pred.tier, PredictionTier::Workflow);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_git_commit_skips_push_without_remote() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command("git commit -m 'init'", "/tmp/not-a-repo", "s", 0)
            .unwrap();

        let pred = engine(&db).predict("s", "/tmp/not-a-repo", None, true);
        assert!(pred.is_none() || pred.unwrap().command != "git push");
    }

    #[test]
    fn hub_command_needs_repeated_sequence() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("ls", "/proj", "s1", 0, 1)
            .unwrap();
        db.insert_command_with_timestamp("pwd", "/proj", "s1", 0, 2)
            .unwrap();
        db.insert_command("ls", "/proj", "current", 0).unwrap();

        let pred = engine(&db).predict("current", "/proj", None, true);
        assert!(pred
            .as_ref()
            .map(|p| p.tier != PredictionTier::Sequence)
            .unwrap_or(true));
    }

    #[test]
    fn sequence_falls_back_to_other_directories() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("cargo test", "/other", "s1", 0, 1)
            .unwrap();
        db.insert_command_with_timestamp("cargo clippy", "/other", "s1", 0, 2)
            .unwrap();
        db.insert_command("cargo test", "/proj", "current", 0)
            .unwrap();

        let pred = engine(&db).predict("current", "/proj", None, true).unwrap();
        assert_eq!(pred.command, "cargo clippy");
        assert_eq!(pred.tier, PredictionTier::Sequence);
        assert!(pred.confidence < 1.0);
    }
}
