use crate::{context::RuntimeContext, db::HistoryDb, session};
use std::path::{Path, PathBuf};
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
    match resolve_via_cargo_runner(filepath_arg) {
        Ok(resolved) => Ok(resolved),
        Err(cargo_err) => match resolve_locally(filepath_arg) {
            Ok(resolved) => Ok(resolved),
            Err(local_err) => Err(format!(
                "{cargo_err}\n\nLocal fallback also failed: {local_err}"
            )),
        },
    }
}

fn resolve_via_cargo_runner(filepath_arg: Option<&str>) -> Result<ResolvedRun, String> {
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

fn resolve_locally(filepath_arg: Option<&str>) -> Result<ResolvedRun, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("failed to determine current directory: {}", e))?;
    let (filepath, line) = split_filepath_and_line(filepath_arg);

    if filepath.as_deref().map(|path| path.contains("::")).unwrap_or(false) {
        return Err("module-path run requires cargo runner to be installed".to_string());
    }

    let resolved_file = filepath
        .as_deref()
        .and_then(|path| resolve_existing_path(&cwd, path));

    if filepath.is_some() && resolved_file.is_none() {
        return Err(format!(
            "file not found: {}",
            filepath.unwrap_or_default()
        ));
    }

    let cwd_str = cwd.to_string_lossy().to_string();
    let context = RuntimeContext::detect(&cwd_str, resolved_file.as_deref().and_then(|p| p.to_str()), line);
    let mut commands = local_runnables(&context, resolved_file.as_deref())?;
    let command = commands
        .drain(..)
        .next()
        .ok_or_else(|| "local fallback did not produce any runnable commands".to_string())?;
    let working_dir = context
        .project_root
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| Some(cwd.clone()));
    let preview = render_preview(&command, working_dir.as_ref(), &[]);
    Ok(ResolvedRun {
        command: ResolvedRunCommand {
            command,
            working_dir,
            env: Vec::new(),
        },
        raw_output: preview,
        preview_status: success_status(),
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

fn resolve_existing_path(cwd: &std::path::Path, path: &str) -> Option<PathBuf> {
    let candidate = std::path::Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };

    if resolved.exists() {
        Some(resolved.canonicalize().unwrap_or(resolved))
    } else {
        None
    }
}

fn split_filepath_and_line(filepath_arg: Option<&str>) -> (Option<String>, Option<usize>) {
    let Some(filepath_arg) = filepath_arg else {
        return (None, None);
    };

    if let Some((path, line)) = filepath_arg.rsplit_once(':') {
        if let Ok(line) = line.parse::<usize>() {
            return (Some(path.to_string()), Some(line));
        }
    }

    (Some(filepath_arg.to_string()), None)
}

pub(crate) fn local_runnables(
    context: &RuntimeContext,
    resolved_file: Option<&Path>,
) -> Result<Vec<String>, String> {
    if context.file_kind == "single_file_script" {
        let file = resolved_file
            .ok_or_else(|| "single-file scripts require a file path".to_string())?;
        let engine = context.script_engine.as_deref().unwrap_or("rust-script");
        let file = shell_quote(file.to_string_lossy().as_ref());
        return Ok(vec![if engine == "rust-script" {
            format!("rust-script {file}")
        } else {
            format!("cargo +nightly -Zscript {file}")
        }]);
    }

    if context.file_kind == "standalone" {
        let file = resolved_file
            .ok_or_else(|| "standalone Rust files require a file path".to_string())?;
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("waz-run");
        let output = std::env::temp_dir().join(format!("waz-{}-{}", stem, std::process::id()));
        let file = shell_quote(file.to_string_lossy().as_ref());
        let output = shell_quote(output.to_string_lossy().as_ref());
        return Ok(vec![format!("rustc {file} -o {output} && {output}")]);
    }

    let Some(file) = resolved_file else {
        return Ok(workspace_default_runnables(context));
    };

    let normalized = file.to_string_lossy().replace('\\', "/");
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");
    let package = context.package_name.as_deref();
    let mut commands = Vec::new();

    if normalized.ends_with("/src/main.rs") || normalized.ends_with("src/main.rs") {
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo run --package {}", shell_quote(package))
            }
            _ => "cargo run".to_string(),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo test --package {} --lib", shell_quote(package))
            }
            _ => "cargo test --lib".to_string(),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.contains("/src/bin/") || normalized.starts_with("src/bin/") {
        commands.push(match package {
            Some(package) if !package.is_empty() => format!(
                "cargo run --package {} --bin {}",
                shell_quote(package),
                shell_quote(stem)
            ),
            _ => format!("cargo run --bin {}", shell_quote(stem)),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo test --package {} --lib", shell_quote(package))
            }
            _ => "cargo test --lib".to_string(),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.contains("/examples/") || normalized.starts_with("examples/") {
        commands.push(match package {
            Some(package) if !package.is_empty() => format!(
                "cargo run --package {} --example {}",
                shell_quote(package),
                shell_quote(stem)
            ),
            _ => format!("cargo run --example {}", shell_quote(stem)),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo test --package {} --lib", shell_quote(package))
            }
            _ => "cargo test --lib".to_string(),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.contains("/tests/") || normalized.starts_with("tests/") {
        commands.push(match package {
            Some(package) if !package.is_empty() => format!(
                "cargo test --package {} --test {}",
                shell_quote(package),
                shell_quote(stem)
            ),
            _ => format!("cargo test --test {}", shell_quote(stem)),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.contains("/benches/") || normalized.starts_with("benches/") {
        commands.push(match package {
            Some(package) if !package.is_empty() => format!(
                "cargo bench --package {} --bench {}",
                shell_quote(package),
                shell_quote(stem)
            ),
            _ => format!("cargo bench --bench {}", shell_quote(stem)),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.ends_with("/src/lib.rs") || normalized.ends_with("src/lib.rs") {
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo test --package {} --lib", shell_quote(package))
            }
            _ => "cargo test --lib".to_string(),
        });
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo check --package {}", shell_quote(package))
            }
            _ => "cargo check".to_string(),
        });
        return Ok(commands);
    }

    if normalized.ends_with("build.rs") {
        return Ok(vec!["cargo check".to_string()]);
    }

    commands.push(match package {
        Some(package) if !package.is_empty() => {
            format!("cargo run --package {}", shell_quote(package))
        }
        _ => "cargo run".to_string(),
    });
    commands.push(match package {
        Some(package) if !package.is_empty() => {
            format!("cargo test --package {}", shell_quote(package))
        }
        _ => "cargo test".to_string(),
    });
    commands.push(match package {
        Some(package) if !package.is_empty() => {
            format!("cargo check --package {}", shell_quote(package))
        }
        _ => "cargo check".to_string(),
    });
    if !context.benches.is_empty() {
        commands.push(match package {
            Some(package) if !package.is_empty() => {
                format!("cargo bench --package {}", shell_quote(package))
            }
            _ => "cargo bench".to_string(),
        });
    }
    Ok(commands)
}

fn workspace_default_runnables(context: &RuntimeContext) -> Vec<String> {
    let mut commands = vec!["cargo run".to_string(), "cargo test".to_string(), "cargo check".to_string()];
    if !context.benches.is_empty() {
        commands.push("cargo bench".to_string());
    }
    commands
}

fn render_preview(command: &str, working_dir: Option<&PathBuf>, env: &[(String, String)]) -> String {
    let mut output = String::new();
    output.push_str(command);
    output.push('\n');
    if let Some(working_dir) = working_dir {
        output.push_str("Working directory: ");
        output.push_str(&working_dir.to_string_lossy());
        output.push('\n');
    }
    if !env.is_empty() {
        output.push_str("Environment variables:\n");
        for (key, value) in env {
            output.push_str("  ");
            output.push_str(key);
            output.push('=');
            output.push_str(value);
            output.push('\n');
        }
    }
    output
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn success_status() -> ExitStatus {
    Command::new("sh")
        .arg("-c")
        .arg(":")
        .status()
        .expect("shell should be available")
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

    #[test]
    fn quotes_shell_values_only_when_needed() {
        assert_eq!(shell_quote("src/main.rs"), "src/main.rs");
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }
}
