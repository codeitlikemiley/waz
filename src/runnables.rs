use crate::context::RuntimeContext;
use crate::run::local_runnables;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub fn run_runnables(target: Option<&str>) -> Result<ExitStatus, String> {
    match run_via_cargo_runner(target) {
        Ok(status) => Ok(status),
        Err(cargo_err) => match run_locally(target) {
            Ok(status) => Ok(status),
            Err(local_err) => Err(format!(
                "{cargo_err}\n\nLocal fallback also failed: {local_err}"
            )),
        },
    }
}

fn run_via_cargo_runner(target: Option<&str>) -> Result<ExitStatus, String> {
    let mut command = Command::new("cargo");
    command.args(["runner", "runnables"]);
    if let Some(target) = target {
        command.arg(target);
    }

    let output = command
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
            "cargo runner failed to list runnables".to_string()
        } else {
            err
        });
    }

    print!("{}", stdout);
    if !stderr.trim().is_empty() {
        eprint!("{}", stderr);
    }
    Ok(output.status)
}

fn run_locally(target: Option<&str>) -> Result<ExitStatus, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("failed to determine current directory: {}", e))?;
    let (file, line) = split_target(target);
    let resolved_file = file
        .as_deref()
        .and_then(|path| resolve_existing_path(&cwd, path));
    let context = RuntimeContext::detect(
        &cwd.to_string_lossy(),
        resolved_file.as_deref().and_then(|path| path.to_str()),
        line,
    );

    let commands = if let Some(ref path) = resolved_file {
        local_runnables(&context, Some(path.as_path()))?
    } else {
        local_runnables(&context, None)?
    };

    if commands.is_empty() {
        return Err("local fallback did not produce any runnable commands".to_string());
    }

    println!("🔍 Local runnables: {}", cwd.display());
    println!("===============================================================================");
    println!();
    for (idx, command) in commands.iter().enumerate() {
        println!("{}. {}", idx + 1, command);
    }

    Ok(success_status())
}

fn split_target(target: Option<&str>) -> (Option<String>, Option<usize>) {
    let Some(target) = target else {
        return (None, None);
    };

    if target.contains("::") {
        return (None, None);
    }

    if let Some((path, line)) = target.rsplit_once(':') {
        if let Ok(line) = line.parse::<usize>() {
            return (Some(path.to_string()), Some(line));
        }
    }

    (Some(target.to_string()), None)
}

fn resolve_existing_path(cwd: &Path, path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
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

fn success_status() -> ExitStatus {
    Command::new("sh")
        .arg("-c")
        .arg(":")
        .status()
        .expect("shell should be available")
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_filepath_and_line() {
        let (file, line) = super::split_target(Some("src/main.rs:42"));
        assert_eq!(file.as_deref(), Some("src/main.rs"));
        assert_eq!(line, Some(42));
    }

    #[test]
    fn treats_module_paths_as_workspace_targets() {
        let (file, line) = super::split_target(Some("runners::unified_runner::tests"));
        assert_eq!(file, None);
        assert_eq!(line, None);
    }
}
