use crate::{db::HistoryDb, session};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedRunCommand {
    command: String,
    working_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
}

pub fn run_file(filepath_arg: Option<&str>, dry_run: bool) -> Result<ExitStatus, String> {
    let resolved = resolve_run_command(filepath_arg)?;

    if dry_run {
        print!("{}", resolved.raw_output);
        return Ok(resolved.preview_status);
    }

    let status = execute_resolved_command(&resolved)?;
    record_resolved_command(&resolved.command.command, status.code().unwrap_or(1));
    Ok(status)
}

#[derive(Debug)]
struct ResolvedRun {
    command: ResolvedRunCommand,
    raw_output: String,
    preview_status: ExitStatus,
}

fn resolve_run_command(filepath_arg: Option<&str>) -> Result<ResolvedRun, String> {
    let mut args = vec!["runner", "run"];
    if let Some(filepath_arg) = filepath_arg {
        args.push(filepath_arg);
    }
    args.push("--dry-run");

    let output = Command::new("cargo")
        .args(args)
        .output()
        .map_err(|e| format!("failed to launch cargo runner: {}", e))?;

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| format!("cargo runner produced non-UTF8 output: {}", e))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|e| format!("cargo runner produced non-UTF8 stderr: {}", e))?;

    if !output.status.success() {
        let err = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(if err.is_empty() {
            "cargo runner failed to resolve command".to_string()
        } else {
            err
        });
    }

    let command = parse_dry_run_output(&stdout)?;
    Ok(ResolvedRun {
        command,
        raw_output: stdout,
        preview_status: output.status,
    })
}

fn parse_dry_run_output(stdout: &str) -> Result<ResolvedRunCommand, String> {
    let mut command: Option<String> = None;
    let mut working_dir: Option<PathBuf> = None;
    let mut env = Vec::new();
    let mut in_env_section = false;

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if let Some(dir) = trimmed.strip_prefix("Working directory: ") {
            working_dir = Some(PathBuf::from(dir.trim()));
            in_env_section = false;
            continue;
        }

        if trimmed.starts_with("Environment variables:") {
            in_env_section = true;
            continue;
        }

        if in_env_section && line.starts_with("  ") {
            if let Some((key, value)) = trimmed.split_once('=') {
                env.push((key.trim().to_string(), value.trim().to_string()));
            }
            continue;
        }

        if command.is_none() {
            command = Some(trimmed.to_string());
        }
    }

    let command = command.ok_or_else(|| {
        "cargo runner dry-run output did not include a command".to_string()
    })?;

    Ok(ResolvedRunCommand {
        command,
        working_dir,
        env,
    })
}

fn execute_resolved_command(resolved: &ResolvedRun) -> Result<ExitStatus, String> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(&resolved.command.command);
    if let Some(ref dir) = resolved.command.working_dir {
        command.current_dir(dir);
    }
    for (key, value) in &resolved.command.env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    command
        .status()
        .map_err(|e| format!("failed to execute resolved command: {}", e))
}

fn record_resolved_command(command: &str, exit_code: i32) {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return,
    };
    let session_id = session::get_session_id();

    if let Ok(db) = HistoryDb::open(&crate::get_db_path()) {
        let cwd_str = cwd.to_string_lossy().to_string();
        let _ = db.insert_command(command, &cwd_str, &session_id, exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_command() {
        let output = "cargo run --bin example\nWorking directory: /tmp/work\n";
        let parsed = parse_dry_run_output(output).unwrap();
        assert_eq!(parsed.command, "cargo run --bin example");
        assert_eq!(parsed.working_dir, Some(PathBuf::from("/tmp/work")));
    }

    #[test]
    fn parses_env_section() {
        let output = "bazel test //app:app\nWorking directory: /tmp/work\nEnvironment variables:\n  A=B\n  C=D\n";
        let parsed = parse_dry_run_output(output).unwrap();
        assert_eq!(parsed.command, "bazel test //app:app");
        assert_eq!(parsed.env, vec![("A".into(), "B".into()), ("C".into(), "D".into())]);
    }

    #[test]
    fn rejects_missing_command() {
        let err = parse_dry_run_output("Working directory: /tmp/work\n").unwrap_err();
        assert!(err.contains("did not include a command"));
    }

    #[test]
    fn builds_run_args_without_filepath() {
        let mut args = vec!["runner", "run"];
        args.push("--dry-run");
        assert_eq!(args, vec!["runner", "run", "--dry-run"]);
    }

    #[test]
    fn builds_run_args_with_filepath() {
        let mut args = vec!["runner", "run"];
        args.push("src/main.rs:1");
        args.push("--dry-run");
        assert_eq!(args, vec!["runner", "run", "src/main.rs:1", "--dry-run"]);
    }
}
