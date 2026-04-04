use crate::tui::cargo_schema::CargoContext;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeContext {
    #[serde(default)]
    pub context_version: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default)]
    pub build_system: String,
    #[serde(default)]
    pub file_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runnable_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub benches: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_target: Option<String>,
}

impl RuntimeContext {
    pub fn detect(cwd: &str, file: Option<&str>, line: Option<usize>) -> Self {
        if let Some(ctx) = detect_via_cargo_runner(cwd, file, line) {
            return ctx;
        }

        detect_local(cwd, file, line)
    }
}

fn detect_via_cargo_runner(
    cwd: &str,
    file: Option<&str>,
    line: Option<usize>,
) -> Option<RuntimeContext> {
    let mut command = Command::new("cargo");
    command.args(["runner", "context", "--json"]);
    if let Some(file) = file {
        let mut arg = file.to_string();
        if let Some(line) = line {
            arg.push(':');
            arg.push_str(&line.to_string());
        }
        command.arg(arg);
    }
    command.current_dir(cwd);

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    serde_json::from_str::<RuntimeContext>(stdout.trim()).ok()
}

fn detect_local(cwd: &str, file: Option<&str>, line: Option<usize>) -> RuntimeContext {
    let cwd_path = Path::new(cwd);
    let file_path = file.and_then(|file| resolve_path(cwd_path, file));
    let project_root = file_path
        .as_deref()
        .and_then(find_cargo_root)
        .or_else(|| find_cargo_root(cwd_path));
    let cargo_ctx = project_root.as_ref().map(|root| CargoContext::detect(root));
    let script_engine = file_path
        .as_deref()
        .and_then(detect_script_engine);
    let package_name = project_root
        .as_ref()
        .and_then(|root| read_package_name(root))
        .or_else(|| cargo_ctx.as_ref().and_then(|ctx| ctx.package_name.clone()));
    let file_kind = detect_file_kind(
        file_path.as_deref(),
        script_engine.as_deref(),
        cargo_ctx.as_ref(),
    );
    let build_system = detect_build_system(
        file_path.as_deref(),
        script_engine.as_deref(),
        cargo_ctx.as_ref(),
        &file_kind,
    );
    let runnable_kind = detect_runnable_kind(
        file_path.as_deref(),
        script_engine.as_deref(),
        cargo_ctx.as_ref(),
    );
    let recommended_target = detect_recommended_target(
        file_path.as_deref(),
        script_engine.as_deref(),
        package_name.as_deref(),
    );

    RuntimeContext {
        context_version: 1,
        cwd: cwd.to_string(),
        project_root: project_root
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        file_path: file_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        line,
        build_system,
        file_kind,
        runnable_kind,
        package_name,
        bins: cargo_ctx.as_ref().map(|ctx| ctx.bins.clone()).unwrap_or_default(),
        examples: cargo_ctx
            .as_ref()
            .map(|ctx| ctx.examples.clone())
            .unwrap_or_default(),
        tests: cargo_ctx.as_ref().map(|ctx| ctx.tests.clone()).unwrap_or_default(),
        benches: cargo_ctx
            .as_ref()
            .map(|ctx| ctx.benches.clone())
            .unwrap_or_default(),
        features: cargo_ctx
            .as_ref()
            .map(|ctx| ctx.features.clone())
            .unwrap_or_default(),
        profiles: cargo_ctx
            .as_ref()
            .map(|ctx| ctx.profiles.clone())
            .unwrap_or_default(),
        script_engine,
        recommended_target,
    }
}

fn resolve_path(cwd: &Path, path: &str) -> Option<PathBuf> {
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

fn find_cargo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if current.join("Cargo.toml").exists() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }

    None
}

fn detect_script_engine(file_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
    let first_line = content.lines().next()?;
    if !(first_line.starts_with("#!") && content.contains("fn main(")) {
        return None;
    }

    if first_line.contains("rust-script") {
        Some("rust-script".to_string())
    } else if first_line.contains("cargo") && first_line.contains("-Zscript") {
        Some("cargo +nightly -Zscript".to_string())
    } else {
        None
    }
}

fn detect_file_kind(
    file_path: Option<&Path>,
    script_engine: Option<&str>,
    cargo_ctx: Option<&CargoContext>,
) -> String {
    if script_engine.is_some() {
        return "single_file_script".to_string();
    }

    let Some(file_path) = file_path else {
        return if cargo_ctx.is_some() {
            "cargo_project".to_string()
        } else {
            "standalone".to_string()
        };
    };

    let normalized = file_path.to_string_lossy().replace('\\', "/");

    if normalized.ends_with("build.rs") {
        "build_script".to_string()
    } else if normalized.contains("/tests/") || normalized.starts_with("tests/") {
        "cargo_project".to_string()
    } else if normalized.contains("/benches/") || normalized.starts_with("benches/") {
        "cargo_project".to_string()
    } else if normalized.contains("/examples/") || normalized.starts_with("examples/") {
        "cargo_project".to_string()
    } else if normalized.ends_with("/src/main.rs")
        || normalized.ends_with("src/main.rs")
        || normalized.contains("/src/bin/")
        || normalized.starts_with("src/bin/")
        || normalized.ends_with("/src/lib.rs")
        || normalized.ends_with("src/lib.rs")
    {
        "cargo_project".to_string()
    } else if cargo_ctx.is_some() {
        "cargo_project".to_string()
    } else {
        "standalone".to_string()
    }
}

fn detect_build_system(
    file_path: Option<&Path>,
    script_engine: Option<&str>,
    cargo_ctx: Option<&CargoContext>,
    file_kind: &str,
) -> String {
    if let Some(engine) = script_engine {
        return if engine == "rust-script" {
            "rust-script".to_string()
        } else {
            "cargo".to_string()
        };
    }

    if file_kind == "standalone" {
        return "rustc".to_string();
    }

    if file_path.is_none() && cargo_ctx.is_some() {
        return "cargo".to_string();
    }

    "cargo".to_string()
}

fn detect_runnable_kind(
    file_path: Option<&Path>,
    script_engine: Option<&str>,
    cargo_ctx: Option<&CargoContext>,
) -> Option<String> {
    if script_engine.is_some() {
        return Some("single_file_script".to_string());
    }

    let file_path = file_path?;
    let normalized = file_path.to_string_lossy().replace('\\', "/");

    if normalized.ends_with("build.rs") {
        Some("build_script".to_string())
    } else if normalized.contains("/benches/") || normalized.starts_with("benches/") {
        Some("benchmark".to_string())
    } else if normalized.contains("/tests/") || normalized.starts_with("tests/") {
        Some("test".to_string())
    } else if normalized.contains("/examples/") || normalized.starts_with("examples/") {
        Some("binary".to_string())
    } else if normalized.ends_with("/src/main.rs")
        || normalized.ends_with("src/main.rs")
        || normalized.contains("/src/bin/")
        || normalized.starts_with("src/bin/")
    {
        Some("binary".to_string())
    } else if normalized.ends_with("/src/lib.rs") || normalized.ends_with("src/lib.rs") {
        Some("module_tests".to_string())
    } else if cargo_ctx.is_some() {
        Some("cargo_project".to_string())
    } else {
        Some("standalone".to_string())
    }
}

fn detect_recommended_target(
    file_path: Option<&Path>,
    script_engine: Option<&str>,
    package_name: Option<&str>,
) -> Option<String> {
    if script_engine.is_some() {
        return file_path.map(|path| path.to_string_lossy().to_string());
    }

    let Some(file_path) = file_path else {
        return package_name.map(|name| name.to_string());
    };
    let stem = file_path.file_stem().and_then(|s| s.to_str())?.to_string();
    let normalized = file_path.to_string_lossy().replace('\\', "/");

    if normalized.ends_with("/src/main.rs") || normalized.ends_with("src/main.rs") {
        return package_name.map(|name| name.to_string()).or(Some(stem));
    }

    if normalized.contains("/src/bin/") || normalized.starts_with("src/bin/") {
        return Some(stem);
    }

    if normalized.contains("/examples/") || normalized.starts_with("examples/") {
        return Some(stem);
    }

    if normalized.contains("/tests/") || normalized.starts_with("tests/") {
        return Some(stem);
    }

    if normalized.contains("/benches/") || normalized.starts_with("benches/") {
        return Some(stem);
    }

    if normalized.ends_with("build.rs") {
        return Some("build".to_string());
    }

    Some(stem)
}

fn read_package_name(root: &Path) -> Option<String> {
    let cargo_toml = root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).ok()?;

    let mut in_package_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package_section = trimmed == "[package]";
            continue;
        }
        if in_package_section {
            if let Some(rest) = trimmed.strip_prefix("name") {
                if let Some(value) = rest.trim().strip_prefix('=') {
                    let name = value.trim().trim_matches('"').trim_matches('\'');
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scaffold_cargo_project(dir: &Path) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "sample"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("waz-{name}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_cargo_runner_json() {
        let json = r#"{
            "context_version": 1,
            "cwd": "/tmp/work",
            "project_root": "/tmp/work",
            "file_path": "/tmp/work/power.rs",
            "line": 1,
            "build_system": "cargo",
            "file_kind": "single_file_script",
            "runnable_kind": "single_file_script",
            "package_name": null,
            "bins": [],
            "examples": [],
            "tests": [],
            "benches": [],
            "features": [],
            "profiles": [],
            "script_engine": "rust-script",
            "recommended_target": "/tmp/work/power.rs"
        }"#;

        let ctx: RuntimeContext = serde_json::from_str(json).unwrap();
        assert_eq!(ctx.file_kind, "single_file_script");
        assert_eq!(ctx.script_engine.as_deref(), Some("rust-script"));
        assert_eq!(ctx.recommended_target.as_deref(), Some("/tmp/work/power.rs"));
    }

    #[test]
    fn detects_local_single_file_script_context() {
        let tmp = temp_dir("single-file-script");
        let file = tmp.join("power.rs");
        fs::write(
            &file,
            r#"#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! ```
//!
fn main() {}
        "#,
        )
        .unwrap();

        let ctx = detect_local(
            tmp.to_str().unwrap(),
            Some(file.to_str().unwrap()),
            Some(12),
        );

        assert_eq!(ctx.file_kind, "single_file_script");
        assert_eq!(ctx.build_system, "rust-script");
        assert_eq!(ctx.script_engine.as_deref(), Some("rust-script"));
        assert_eq!(ctx.line, Some(12));
        let expected = file.canonicalize().unwrap();
        assert_eq!(ctx.recommended_target.as_deref(), expected.to_str());
    }

    #[test]
    fn detects_local_cargo_project_context() {
        let tmp = temp_dir("cargo-project");
        scaffold_cargo_project(&tmp);

        assert_eq!(read_package_name(&tmp).as_deref(), Some("sample"));

        let ctx = detect_local(tmp.to_str().unwrap(), None, None);

        assert_eq!(ctx.file_kind, "cargo_project");
        assert_eq!(ctx.build_system, "cargo");
        assert_eq!(ctx.package_name.as_deref(), Some("sample"));
        assert_eq!(ctx.recommended_target.as_deref(), Some("sample"));
    }
}
