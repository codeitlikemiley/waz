use std::process::{Command, ExitStatus};

pub fn run_file(filepath_arg: &str, dry_run: bool) -> Result<ExitStatus, String> {
    let mut command = Command::new("cargo");
    command.args(["runner", "run", filepath_arg]);
    if dry_run {
        command.arg("--dry-run");
    }

    command
        .status()
        .map_err(|e| format!("failed to launch cargo runner: {}", e))
}
