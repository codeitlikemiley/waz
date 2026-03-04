use crate::config::Config;
use crate::db::HistoryDb;
use crate::llm;

/// A prediction result with confidence.
#[derive(Debug, Clone)]
pub struct Prediction {
    pub command: String,
    pub confidence: f64,
    pub tier: PredictionTier,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PredictionTier {
    /// Tier 1: Based on command sequence patterns (highest confidence)
    Sequence,
    /// Tier 2: Based on CWD-filtered history
    CwdHistory,
    /// Tier 3: LLM-based prediction (lowest confidence)
    Llm,
}

impl std::fmt::Display for PredictionTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredictionTier::Sequence => write!(f, "sequence"),
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

/// Multi-tier prediction engine.
pub struct PredictionEngine<'a> {
    db: &'a HistoryDb,
    config: Config,
}

impl<'a> PredictionEngine<'a> {
    pub fn new(db: &'a HistoryDb) -> Self {
        Self { db, config: Config::load() }
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
        // Tier 1: Sequence-based prediction
        if let Some(pred) = self.predict_by_sequence(session_id, prefix) {
            return Some(pred);
        }

        // Tier 2: CWD-filtered history
        if let Some(pred) = self.predict_by_cwd(cwd, prefix) {
            return Some(pred);
        }

        // Tier 3: LLM fallback (skip in fast mode to avoid keystroke lag)
        if !fast {
            return self.predict_by_llm(session_id, cwd, prefix);
        }

        None
    }

    /// Tier 1: Look at the last command in this session and predict the next one
    /// based on historical command sequences (bigram frequency).
    fn predict_by_sequence(&self, session_id: &str, prefix: Option<&str>) -> Option<Prediction> {
        let session_cmds = self.db.get_session_commands(session_id).ok()?;
        let last_cmd = session_cmds.last()?;

        let (next_cmd, count, total) = self.db.get_next_command_by_sequence(last_cmd).ok()??;

        // Check minimum thresholds
        if count < SEQUENCE_MIN_COUNT {
            return None;
        }

        let confidence = count as f64 / total as f64;
        if confidence < SEQUENCE_MIN_CONFIDENCE {
            return None;
        }

        // If user has typed a prefix, check it matches
        if let Some(pfx) = prefix {
            if !pfx.is_empty() && !next_cmd.starts_with(pfx) {
                return None;
            }
        }

        Some(Prediction {
            command: next_cmd,
            confidence,
            tier: PredictionTier::Sequence,
        })
    }

    /// Tier 2: Find the most recently used command in this CWD,
    /// optionally filtered by what the user is typing.
    fn predict_by_cwd(&self, cwd: &str, prefix: Option<&str>) -> Option<Prediction> {
        let results = self.db.get_recent_by_cwd(cwd, prefix, 1).ok()?;
        let cmd = results.into_iter().next()?;

        // If a prefix is provided, don't suggest the exact same thing they already typed
        if let Some(pfx) = prefix {
            if !pfx.is_empty() && cmd == pfx {
                return None;
            }
        }

        Some(Prediction {
            command: cmd,
            confidence: 0.2, // lower confidence for CWD-only matches
            tier: PredictionTier::CwdHistory,
        })
    }

    /// Tier 3: Use an LLM to predict the next command based on shell context.
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

        Some(Prediction {
            command: cmd,
            confidence: 0.1,
            tier: PredictionTier::Llm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::HistoryDb;

    #[test]
    fn test_sequence_prediction() {
        let db = HistoryDb::open_in_memory().unwrap();

        // Build a strong pattern: git commit -> git push (5 times)
        for i in 0..5 {
            let sess = format!("old_s{}", i);
            let base = (i * 100) as i64;
            db.insert_command_with_timestamp("git commit -m 'msg'", "/proj", &sess, 0, base)
                .unwrap();
            db.insert_command_with_timestamp("git push", "/proj", &sess, 0, base + 1)
                .unwrap();
        }

        // Current session: user just ran "git commit -m 'msg'"
        db.insert_command("git commit -m 'msg'", "/proj", "current", 0)
            .unwrap();

        let engine = PredictionEngine::new(&db);
        let pred = engine.predict("current", "/proj", None, false);
        assert!(pred.is_some());
        let pred = pred.unwrap();
        assert_eq!(pred.command, "git push");
        assert_eq!(pred.tier, PredictionTier::Sequence);
        assert!(pred.confidence >= SEQUENCE_MIN_CONFIDENCE);
    }

    #[test]
    fn test_cwd_fallback() {
        let db = HistoryDb::open_in_memory().unwrap();

        // Only CWD history, no sequence data
        db.insert_command_with_timestamp("npm test", "/frontend", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("npm run build", "/frontend", "s1", 0, 2000)
            .unwrap();

        // New session with no prior commands
        let engine = PredictionEngine::new(&db);
        let pred = engine.predict("new_session", "/frontend", None, false);
        assert!(pred.is_some());
        let pred = pred.unwrap();
        assert_eq!(pred.command, "npm run build"); // most recent
        assert_eq!(pred.tier, PredictionTier::CwdHistory);
    }

    #[test]
    fn test_prefix_filtering() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert_command_with_timestamp("npm test", "/frontend", "s1", 0, 1000)
            .unwrap();
        db.insert_command_with_timestamp("cargo build", "/frontend", "s1", 0, 2000)
            .unwrap();

        let engine = PredictionEngine::new(&db);
        let pred = engine.predict("new_session", "/frontend", Some("npm"), false);
        assert!(pred.is_some());
        assert_eq!(pred.unwrap().command, "npm test");
    }

    #[test]
    fn test_no_local_prediction() {
        let db = HistoryDb::open_in_memory().unwrap();
        let engine = PredictionEngine::new(&db);
        let pred = engine.predict("empty", "/nowhere", None, false);
        // With empty DB, tiers 1 & 2 return nothing.
        // Tier 3 (LLM) may or may not return something depending on API availability.
        if let Some(ref p) = pred {
            assert_eq!(p.tier, PredictionTier::Llm, "Only LLM tier should produce results from empty DB");
        }
    }
}
