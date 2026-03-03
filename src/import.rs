use crate::db::HistoryDb;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

/// Import results summary.
#[derive(Debug, Default)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub errors: usize,
}

impl std::fmt::Display for ImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Imported: {}, Skipped: {}, Errors: {}",
            self.imported, self.skipped, self.errors
        )
    }
}

/// Import shell history into the database.
pub fn import_history(db: &HistoryDb, shell: Option<&str>) -> io::Result<ImportResult> {
    let home = dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME not found"))?;

    match shell {
        Some("zsh") => import_zsh_history(db, &home),
        Some("bash") => import_bash_history(db, &home),
        Some("fish") => import_fish_history(db, &home),
        Some(s) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Unsupported shell: {}", s),
        )),
        None => {
            // Auto-detect: try all shells
            let mut total = ImportResult::default();

            if let Ok(r) = import_zsh_history(db, &home) {
                eprintln!("  zsh: {}", r);
                total.imported += r.imported;
                total.skipped += r.skipped;
                total.errors += r.errors;
            }

            if let Ok(r) = import_bash_history(db, &home) {
                eprintln!("  bash: {}", r);
                total.imported += r.imported;
                total.skipped += r.skipped;
                total.errors += r.errors;
            }

            if let Ok(r) = import_fish_history(db, &home) {
                eprintln!("  fish: {}", r);
                total.imported += r.imported;
                total.skipped += r.skipped;
                total.errors += r.errors;
            }

            Ok(total)
        }
    }
}

/// Import zsh history file.
/// Zsh extended history format: `: <timestamp>:<duration>;<command>`
/// Plain format: just the command per line.
fn import_zsh_history(db: &HistoryDb, home: &PathBuf) -> io::Result<ImportResult> {
    // Try multiple locations for zsh history:
    // 1. $HISTFILE env var
    // 2. $ZDOTDIR/.zsh_history
    // 3. ~/.config/zsh/.zsh_history
    // 4. ~/.zsh_history (default)
    let candidates: Vec<PathBuf> = vec![
        std::env::var("HISTFILE").ok().map(PathBuf::from),
        std::env::var("ZDOTDIR").ok().map(|d| PathBuf::from(d).join(".zsh_history")),
        Some(home.join(".config").join("zsh").join(".zsh_history")),
        Some(home.join(".zsh_history")),
    ]
    .into_iter()
    .flatten()
    .collect();

    let hist_path = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "No zsh history file found. Searched: {:?}",
                    candidates
                ),
            )
        })?
        .clone();

    eprintln!("  Reading zsh history from: {}", hist_path.display());

    let content = fs::read(&hist_path)?;
    // zsh history can contain non-UTF8 bytes, handle gracefully
    let content = String::from_utf8_lossy(&content);

    let mut result = ImportResult::default();
    let session_id = "import_zsh";
    let home_str = home.to_string_lossy().to_string();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Try to parse extended history format: `: 1234567890:0;command`
        if line.starts_with(": ") {
            if let Some((meta, cmd)) = line.split_once(';') {
                let cmd = cmd.trim();
                if cmd.is_empty() {
                    result.skipped += 1;
                    continue;
                }

                // Parse timestamp from `: 1234567890:0`
                let timestamp = meta
                    .strip_prefix(": ")
                    .and_then(|s| s.split(':').next())
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(0);

                match db.insert_command_with_timestamp(cmd, &home_str, session_id, 0, timestamp) {
                    Ok(_) => result.imported += 1,
                    Err(_) => result.errors += 1,
                }
            } else {
                result.skipped += 1;
            }
        } else {
            // Plain history format
            match db.insert_command_with_timestamp(line, &home_str, session_id, 0, 0) {
                Ok(_) => result.imported += 1,
                Err(_) => result.errors += 1,
            }
        }
    }

    Ok(result)
}

/// Import bash history file (plain format, one command per line).
fn import_bash_history(db: &HistoryDb, home: &PathBuf) -> io::Result<ImportResult> {
    let hist_path = home.join(".bash_history");
    if !hist_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No bash history file found at {:?}", hist_path),
        ));
    }

    let file = fs::File::open(&hist_path)?;
    let reader = io::BufReader::new(file);
    let mut result = ImportResult::default();
    let session_id = "import_bash";
    let home_str = home.to_string_lossy().to_string();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => {
                result.errors += 1;
                continue;
            }
        };
        let line = line.trim().to_string();
        if line.is_empty() || line.starts_with('#') {
            result.skipped += 1;
            continue;
        }

        match db.insert_command_with_timestamp(&line, &home_str, session_id, 0, 0) {
            Ok(_) => result.imported += 1,
            Err(_) => result.errors += 1,
        }
    }

    Ok(result)
}

/// Import fish history file.
/// Fish history format:
/// ```
/// - cmd: some_command
///   when: 1234567890
///   paths:
///     - /some/path
/// ```
fn import_fish_history(db: &HistoryDb, home: &PathBuf) -> io::Result<ImportResult> {
    let hist_path = home
        .join(".local")
        .join("share")
        .join("fish")
        .join("fish_history");
    if !hist_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No fish history file found at {:?}", hist_path),
        ));
    }

    let content = fs::read_to_string(&hist_path)?;
    let mut result = ImportResult::default();
    let session_id = "import_fish";
    let home_str = home.to_string_lossy().to_string();

    let mut current_cmd: Option<String> = None;
    let mut current_ts: i64 = 0;

    for line in content.lines() {
        if let Some(cmd_str) = line.strip_prefix("- cmd: ") {
            // Flush previous command
            if let Some(cmd) = current_cmd.take() {
                match db.insert_command_with_timestamp(&cmd, &home_str, session_id, 0, current_ts) {
                    Ok(_) => result.imported += 1,
                    Err(_) => result.errors += 1,
                }
            }
            current_cmd = Some(cmd_str.trim().to_string());
            current_ts = 0;
        } else if let Some(ts_str) = line.strip_prefix("  when: ") {
            current_ts = ts_str.trim().parse().unwrap_or(0);
        }
    }

    // Flush last command
    if let Some(cmd) = current_cmd {
        match db.insert_command_with_timestamp(&cmd, &home_str, session_id, 0, current_ts) {
            Ok(_) => result.imported += 1,
            Err(_) => result.errors += 1,
        }
    }

    Ok(result)
}
