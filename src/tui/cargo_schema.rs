//! Cargo command schema with dynamic token resolution from project context.
//!
//! Resolves bin targets, examples, packages, features, profiles, tests, and benches
//! from Cargo.toml and the filesystem.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// All dynamic values resolved from a Cargo project.
#[derive(Debug, Clone, Default)]
pub struct CargoContext {
    pub bins: Vec<String>,
    pub examples: Vec<String>,
    pub packages: Vec<String>,
    pub features: Vec<String>,
    pub profiles: Vec<String>,
    pub tests: Vec<String>,
    pub benches: Vec<String>,
    pub package_name: Option<String>,
}

impl CargoContext {
    /// Detect project context from a directory containing Cargo.toml.
    pub fn detect(cwd: &Path) -> Self {
        let cargo_path = cwd.join("Cargo.toml");
        let toml_value = std::fs::read_to_string(&cargo_path)
            .ok()
            .and_then(|s| s.parse::<toml::Value>().ok());

        let mut ctx = CargoContext::default();

        if let Some(ref toml) = toml_value {
            ctx.package_name = toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());

            ctx.bins = Self::resolve_bins(cwd, toml);
            ctx.examples = Self::resolve_examples(cwd, toml);
            ctx.packages = Self::resolve_packages(cwd, toml);
            ctx.features = Self::resolve_features(toml);
            ctx.profiles = Self::resolve_profiles(toml);
            ctx.tests = Self::resolve_tests(cwd, toml);
            ctx.benches = Self::resolve_benches(cwd, toml);
        }

        // Static profiles always available
        let mut profile_set: BTreeSet<String> = ctx.profiles.drain(..).collect();
        for p in ["dev", "release", "test", "bench"] {
            profile_set.insert(p.to_string());
        }
        ctx.profiles = profile_set.into_iter().collect();

        ctx
    }

    /// Resolve binary targets from multiple sources, deduplicated.
    ///
    /// Sources (priority order):
    /// 1. `[[bin]]` entries in Cargo.toml → `name` field
    /// 2. Files in `src/bin/*.rs` → filename stem
    /// 3. Default: if `src/main.rs` exists → package name
    fn resolve_bins(cwd: &Path, toml: &toml::Value) -> Vec<String> {
        let mut bins = BTreeSet::new();

        // Source 1: [[bin]] in Cargo.toml
        if let Some(bin_array) = toml.get("bin").and_then(|b| b.as_array()) {
            for entry in bin_array {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    bins.insert(name.to_string());
                }
            }
        }

        // Source 2: src/bin/*.rs files
        let bin_dir = cwd.join("src").join("bin");
        if bin_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&bin_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            bins.insert(stem.to_string());
                        }
                    } else if path.is_dir() {
                        // src/bin/<name>/main.rs pattern
                        if path.join("main.rs").exists() {
                            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                                bins.insert(dir_name.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Source 3: default main.rs → package name
        if cwd.join("src").join("main.rs").exists() {
            if let Some(pkg) = toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                bins.insert(pkg.to_string());
            }
        }

        bins.into_iter().collect()
    }

    /// Resolve example targets from multiple sources, deduplicated.
    ///
    /// Sources:
    /// 1. `[[example]]` entries in Cargo.toml
    /// 2. Files in `examples/*.rs` → filename stem
    /// 3. Directories `examples/*/main.rs` → directory name
    fn resolve_examples(cwd: &Path, toml: &toml::Value) -> Vec<String> {
        let mut examples = BTreeSet::new();

        // Source 1: [[example]] in Cargo.toml
        if let Some(ex_array) = toml.get("example").and_then(|e| e.as_array()) {
            for entry in ex_array {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    examples.insert(name.to_string());
                }
            }
        }

        // Source 2 & 3: examples/ directory
        let examples_dir = cwd.join("examples");
        if examples_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&examples_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            examples.insert(stem.to_string());
                        }
                    } else if path.is_dir() {
                        // examples/<name>/main.rs pattern
                        if path.join("main.rs").exists() {
                            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                                examples.insert(dir_name.to_string());
                            }
                        }
                    }
                }
            }
        }

        examples.into_iter().collect()
    }

    /// Resolve workspace packages from `[workspace].members`.
    /// Falls back to the single package name.
    fn resolve_packages(cwd: &Path, toml: &toml::Value) -> Vec<String> {
        let mut packages = BTreeSet::new();

        // Workspace members
        if let Some(members) = toml
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(|m| m.as_array())
        {
            for member in members {
                if let Some(pattern) = member.as_str() {
                    // Resolve glob patterns
                    let member_path = cwd.join(pattern);
                    if pattern.contains('*') {
                        if let Ok(paths) = glob_member_dirs(cwd, pattern) {
                            for p in paths {
                                if let Some(name) = read_package_name(&p) {
                                    packages.insert(name);
                                }
                            }
                        }
                    } else if member_path.join("Cargo.toml").exists() {
                        if let Some(name) = read_package_name(&member_path) {
                            packages.insert(name);
                        }
                    }
                }
            }
        }

        // Fallback: single package
        if packages.is_empty() {
            if let Some(name) = toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                packages.insert(name.to_string());
            }
        }

        packages.into_iter().collect()
    }

    /// Resolve feature names from `[features]`, excluding `default`.
    fn resolve_features(toml: &toml::Value) -> Vec<String> {
        let mut features = Vec::new();
        if let Some(feat_table) = toml.get("features").and_then(|f| f.as_table()) {
            for key in feat_table.keys() {
                if key != "default" {
                    features.push(key.clone());
                }
            }
        }
        features.sort();
        features
    }

    /// Resolve custom profiles from `[profile.*]`.
    fn resolve_profiles(toml: &toml::Value) -> Vec<String> {
        let mut profiles = Vec::new();
        if let Some(profile_table) = toml.get("profile").and_then(|p| p.as_table()) {
            for key in profile_table.keys() {
                profiles.push(key.clone());
            }
        }
        profiles
    }

    /// Resolve test targets from `[[test]]` and `tests/` directory.
    fn resolve_tests(cwd: &Path, toml: &toml::Value) -> Vec<String> {
        let mut tests = BTreeSet::new();

        // Source 1: [[test]] in Cargo.toml
        if let Some(test_array) = toml.get("test").and_then(|t| t.as_array()) {
            for entry in test_array {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    tests.insert(name.to_string());
                }
            }
        }

        // Source 2: tests/ directory
        let tests_dir = cwd.join("tests");
        if tests_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&tests_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            tests.insert(stem.to_string());
                        }
                    }
                }
            }
        }

        tests.into_iter().collect()
    }

    /// Resolve bench targets from `[[bench]]` and `benches/` directory.
    fn resolve_benches(cwd: &Path, toml: &toml::Value) -> Vec<String> {
        let mut benches = BTreeSet::new();

        if let Some(bench_array) = toml.get("bench").and_then(|b| b.as_array()) {
            for entry in bench_array {
                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                    benches.insert(name.to_string());
                }
            }
        }

        let benches_dir = cwd.join("benches");
        if benches_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&benches_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            benches.insert(stem.to_string());
                        }
                    }
                }
            }
        }

        benches.into_iter().collect()
    }
}

// ──────────────────────────── Helpers ────────────────────────────

/// Read `[package].name` from a Cargo.toml in the given directory.
fn read_package_name(dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let toml: toml::Value = content.parse().ok()?;
    toml.get("package")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

/// Simple glob expander for workspace member patterns like `crates/*`.
fn glob_member_dirs(cwd: &Path, pattern: &str) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut results = Vec::new();
    // Only handle simple `prefix/*` patterns
    if let Some(prefix) = pattern.strip_suffix("/*") {
        let base = cwd.join(prefix);
        if base.is_dir() {
            for entry in std::fs::read_dir(&base)? {
                let entry = entry?;
                if entry.path().is_dir() && entry.path().join("Cargo.toml").exists() {
                    results.push(entry.path());
                }
            }
        }
    }
    Ok(results)
}

// ──────────────────────────── Tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temp project with given structure and return its path.
    fn setup_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("waz_test_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src").join("bin")).unwrap();
        fs::create_dir_all(dir.join("examples")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::create_dir_all(dir.join("benches")).unwrap();
        dir
    }

    #[test]
    fn test_detect_bins_from_manifest() {
        let dir = setup_project("bins_manifest");
        fs::write(
            dir.join("Cargo.toml"),
            r#"
[package]
name = "myapp"
version = "0.1.0"

[[bin]]
name = "server"
path = "src/server.rs"

[[bin]]
name = "cli"
path = "src/cli.rs"
"#,
        )
        .unwrap();

        let ctx = CargoContext::detect(&dir);
        assert!(ctx.bins.contains(&"server".to_string()));
        assert!(ctx.bins.contains(&"cli".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_bins_from_filesystem() {
        let dir = setup_project("bins_fs");
        fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "myapp"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(
            dir.join("src").join("bin").join("worker.rs"),
            "fn main() {}",
        )
        .unwrap();
        fs::write(
            dir.join("src").join("bin").join("daemon.rs"),
            "fn main() {}",
        )
        .unwrap();

        let ctx = CargoContext::detect(&dir);
        assert!(
            ctx.bins.contains(&"myapp".to_string()),
            "should include pkg name from main.rs"
        );
        assert!(ctx.bins.contains(&"worker".to_string()));
        assert!(ctx.bins.contains(&"daemon".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dedup_bins() {
        let dir = setup_project("bins_dedup");
        fs::write(
            dir.join("Cargo.toml"),
            r#"
[package]
name = "myapp"
version = "0.1.0"

[[bin]]
name = "myapp"
path = "src/main.rs"
"#,
        )
        .unwrap();
        fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();

        let ctx = CargoContext::detect(&dir);
        // "myapp" appears from both [[bin]] and main.rs → should only appear once
        let count = ctx.bins.iter().filter(|b| *b == "myapp").count();
        assert_eq!(count, 1, "bin name should be deduplicated");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_examples() {
        let dir = setup_project("examples");
        fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "myapp"
version = "0.1.0"

[[example]]
name = "demo"
"#,
        )
        .unwrap();
        fs::write(dir.join("examples").join("hello.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("examples").join("demo.rs"), "fn main() {}").unwrap();

        let ctx = CargoContext::detect(&dir);
        assert!(ctx.examples.contains(&"hello".to_string()));
        assert!(ctx.examples.contains(&"demo".to_string()));
        // demo is defined in both TOML and filesystem — should be deduplicated
        let count = ctx.examples.iter().filter(|e| *e == "demo").count();
        assert_eq!(count, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_features() {
        let dir = setup_project("features");
        fs::write(
            dir.join("Cargo.toml"),
            r#"
[package]
name = "myapp"
version = "0.1.0"

[features]
default = ["json"]
json = []
yaml = ["dep:serde_yaml"]
xml = []
"#,
        )
        .unwrap();

        let ctx = CargoContext::detect(&dir);
        assert!(
            !ctx.features.contains(&"default".to_string()),
            "default should be excluded"
        );
        assert!(ctx.features.contains(&"json".to_string()));
        assert!(ctx.features.contains(&"yaml".to_string()));
        assert!(ctx.features.contains(&"xml".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_profiles() {
        let dir = setup_project("profiles");
        fs::write(
            dir.join("Cargo.toml"),
            r#"
[package]
name = "myapp"
version = "0.1.0"

[profile.production]
opt-level = 3
"#,
        )
        .unwrap();

        let ctx = CargoContext::detect(&dir);
        // Should have the 4 static profiles + custom
        assert!(ctx.profiles.contains(&"dev".to_string()));
        assert!(ctx.profiles.contains(&"release".to_string()));
        assert!(ctx.profiles.contains(&"test".to_string()));
        assert!(ctx.profiles.contains(&"bench".to_string()));
        assert!(ctx.profiles.contains(&"production".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_tests_and_benches() {
        let dir = setup_project("tests_benches");
        fs::write(
            dir.join("Cargo.toml"),
            r#"
[package]
name = "myapp"
version = "0.1.0"

[[test]]
name = "integration"
path = "tests/integration.rs"
"#,
        )
        .unwrap();
        fs::write(dir.join("tests").join("integration.rs"), "").unwrap();
        fs::write(dir.join("tests").join("e2e.rs"), "").unwrap();
        fs::write(dir.join("benches").join("perf.rs"), "").unwrap();

        let ctx = CargoContext::detect(&dir);
        assert!(ctx.tests.contains(&"integration".to_string()));
        assert!(ctx.tests.contains(&"e2e".to_string()));
        // dedup: integration from TOML + filesystem
        assert_eq!(ctx.tests.iter().filter(|t| *t == "integration").count(), 1);
        assert!(ctx.benches.contains(&"perf".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}
