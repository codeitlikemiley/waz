use std::process::{Command, ExitStatus};

pub fn run_runnables(target: Option<&str>) -> Result<ExitStatus, String> {
    let mut command = Command::new("cargo");
    command.args(["runner", "runnables"]);
    if let Some(target) = target {
        command.arg(target);
    }

    command
        .status()
        .map_err(|e| format!("failed to launch cargo runner: {}", e))
}

#[cfg(test)]
mod tests {
    fn build_args(target: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "runner".to_string(),
            "runnables".to_string(),
        ];
        if let Some(target) = target {
            args.push(target.to_string());
        }
        args
    }

    #[test]
    fn builds_without_target() {
        assert_eq!(build_args(None), vec!["runner", "runnables"]);
    }

    #[test]
    fn builds_with_target() {
        assert_eq!(
            build_args(Some("runners::unified_runner::tests")),
            vec!["runner", "runnables", "runners::unified_runner::tests"]
        );
    }
}
