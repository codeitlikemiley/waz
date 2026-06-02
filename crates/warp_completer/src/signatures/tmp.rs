use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFile {
    #[serde(default)]
    pub meta: SchemaMeta,
    pub commands: Vec<CommandEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaMeta {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_generated_by")]
    pub generated_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_with: Option<String>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default = "default_coverage")]
    pub coverage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_method: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery_log: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waz_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_file_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_binary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

fn default_generated_by() -> String { "ai".to_string() }
fn default_coverage() -> String { "partial".to_string() }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEntry {
    pub command: String,
    pub description: String,
    pub tokens: Vec<TokenDef>,
    pub group: String,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    pub token_type: TokenType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<DataSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
    #[serde(default = "default_parse_mode")]
    pub parse: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Box<DataSource>>,
}

fn default_parse_mode() -> String { "lines".to_string() }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenType {
    String,
    Boolean,
    Enum,
    File,
    Number,
}

pub fn schemas_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap()
        .join(".config")
        .join("zap")
        .join("schemas");
    std::fs::create_dir_all(&dir).ok();
    dir
}

pub fn load_all_schemas(cwd: &str) -> Vec<CommandEntry> {
    let dir = schemas_dir();
    let mut commands = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return commands,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(schema_file) = serde_json::from_str::<SchemaFile>(&content) {
                if should_load_schema(&schema_file.meta, cwd) {
                    commands.extend(schema_file.commands);
                }
            } else if let Ok(legacy_entries) = serde_json::from_str::<Vec<CommandEntry>>(&content) {
                commands.extend(legacy_entries);
            }
        }
    }

    commands
}

fn should_load_schema(meta: &SchemaMeta, cwd: &str) -> bool {
    if let Some(ref file) = meta.requires_file {
        if !Path::new(cwd).join(file).exists() {
            return false;
        }
    }
    // Check requires_binary on PATH (only supported on native target_family != wasm)
    #[cfg(not(target_family = "wasm"))]
    if let Some(ref binary) = meta.requires_binary {
        if !binary_exists(binary) {
            return false;
        }
    }
    true
}

#[cfg(not(target_family = "wasm"))]
fn binary_exists(binary: &str) -> bool {
    command::blocking::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn resolve_data_sources(entry: &mut CommandEntry, cwd: &str) {
    for token in &mut entry.tokens {
        if let Some(ref ds) = token.data_source {
            let values = resolve_single_data_source(ds, cwd);

            if let Some(values) = values {
                if !values.is_empty() {
                    token.values = Some(values);
                    token.token_type = TokenType::Enum;
                }
            }
        }
    }
}

fn resolve_single_data_source(ds: &DataSource, cwd: &str) -> Option<Vec<String>> {
    let values = if let Some(ref resolver) = ds.resolver {
        resolve_builtin(resolver, cwd)
    } else if let Some(ref cmd) = ds.command {
        run_data_source_command(cmd, &ds.parse, cwd)
    } else {
        None
    };

    // If primary resolution returned results, use them; otherwise try fallback
    if values.as_ref().is_some_and(|v| !v.is_empty()) {
        values
    } else if let Some(ref fallback) = ds.fallback {
        resolve_single_data_source(fallback, cwd)
    } else {
        values
    }
}

#[cfg(not(target_family = "wasm"))]
fn run_data_source_command(cmd: &str, parse: &str, cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let values: Vec<String> = match parse {
        "words" => stdout.split_whitespace().map(|s| s.to_string()).collect(),
        _ => stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    };
    if values.is_empty() { None } else { Some(values) }
}

#[cfg(target_family = "wasm")]
fn run_data_source_command(_cmd: &str, _parse: &str, _cwd: &str) -> Option<Vec<String>> {
    None
}

fn resolve_builtin(resolver: &str, cwd: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = resolver.splitn(3, ':').collect();
    match (parts.first().copied(), parts.get(1).copied()) {
        (Some("cargo"), Some("bins")) => cargo_resolve_bins(cwd),
        (Some("cargo"), Some("examples")) => cargo_resolve_examples(cwd),
        (Some("cargo"), Some("packages")) => cargo_resolve_packages(cwd),
        (Some("cargo"), Some("features")) => cargo_resolve_features(cwd),
        (Some("cargo"), Some("profiles")) => cargo_resolve_profiles(cwd),
        (Some("cargo"), Some("tests")) => cargo_resolve_tests(cwd),
        (Some("cargo"), Some("benches")) => cargo_resolve_benches(cwd),
        (Some("git"), Some("branches")) => git_resolve_branches(cwd),
        (Some("git"), Some("remotes")) => git_resolve_remotes(cwd),
        (Some("git"), Some("status_files")) => git_resolve_status_files(cwd),
        (Some("git"), Some("tags")) => git_resolve_tags(cwd),
        (Some("npm"), Some("scripts")) => npm_resolve_scripts(cwd),
        _ => None,
    }
}

// ──────────────────────────── Rust-based Safe Resolvers ────────────────────────────

#[derive(Debug, Clone, Default)]
struct CargoContext {
    bins: Vec<String>,
    examples: Vec<String>,
    packages: Vec<String>,
    features: Vec<String>,
    profiles: Vec<String>,
    tests: Vec<String>,
    benches: Vec<String>,
}

fn detect_cargo_context(cwd: &Path) -> Option<CargoContext> {
    let cargo_path = cwd.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_path).ok()?;
    let doc = content.parse::<toml_edit::DocumentMut>().ok()?;

    let mut ctx = CargoContext {
        bins: Vec::new(),
        examples: Vec::new(),
        packages: Vec::new(),
        features: Vec::new(),
        profiles: vec!["dev".to_string(), "release".to_string(), "test".to_string(), "bench".to_string()],
        tests: Vec::new(),
        benches: Vec::new(),
    };

    // Packages / package name
    if let Some(name) = doc["package"]["name"].as_str() {
        ctx.packages.push(name.to_string());
    }

    // Bins from TOML
    if let Some(bin_array) = doc.get("bin").and_then(|b| b.as_array_of_tables()) {
        for entry in bin_array.iter() {
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                ctx.bins.push(name.to_string());
            }
        }
    }

    // Bins from filesystem
    let bin_dir = cwd.join("src").join("bin");
    if bin_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ctx.bins.push(stem.to_string());
                    }
                }
            }
        }
    }
    // Default main.rs bin
    if cwd.join("src").join("main.rs").exists() {
        if let Some(name) = ctx.packages.first() {
            ctx.bins.push(name.clone());
        }
    }

    // Examples from TOML
    if let Some(ex_array) = doc.get("example").and_then(|e| e.as_array_of_tables()) {
        for entry in ex_array.iter() {
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                ctx.examples.push(name.to_string());
            }
        }
    }
    // Examples from filesystem
    let examples_dir = cwd.join("examples");
    if examples_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&examples_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ctx.examples.push(stem.to_string());
                    }
                }
            }
        }
    }

    // Features
    if let Some(features_table) = doc.get("features").and_then(|f| f.as_table()) {
        for (key, _) in features_table.iter() {
            if key != "default" {
                ctx.features.push(key.to_string());
            }
        }
    }

    // Profiles
    if let Some(profile_table) = doc.get("profile").and_then(|p| p.as_table()) {
        for (key, _) in profile_table.iter() {
            ctx.profiles.push(key.to_string());
        }
    }

    // Tests from TOML/filesystem
    if let Some(test_array) = doc.get("test").and_then(|t| t.as_array_of_tables()) {
        for entry in test_array.iter() {
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                ctx.tests.push(name.to_string());
            }
        }
    }
    let tests_dir = cwd.join("tests");
    if tests_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&tests_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ctx.tests.push(stem.to_string());
                    }
                }
            }
        }
    }

    // Benches
    if let Some(bench_array) = doc.get("bench").and_then(|b| b.as_array_of_tables()) {
        for entry in bench_array.iter() {
            if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                ctx.benches.push(name.to_string());
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
                        ctx.benches.push(stem.to_string());
                    }
                }
            }
        }
    }

    // Workspace packages
    if let Some(ws_table) = doc.get("workspace").and_then(|w| w.as_table()) {
        if let Some(members) = ws_table.get("members").and_then(|m| m.as_array()) {
            for member in members.iter() {
                if let Some(pattern) = member.as_str() {
                    let member_path = cwd.join(pattern);
                    if pattern.contains('*') {
                        if let Ok(paths) = glob_member_dirs(cwd, pattern) {
                            for p in paths {
                                if let Some(name) = read_package_name(&p) {
                                    ctx.packages.push(name);
                                }
                            }
                        }
                    } else if member_path.join("Cargo.toml").exists() {
                        if let Some(name) = read_package_name(&member_path) {
                            ctx.packages.push(name);
                        }
                    }
                }
            }
        }
    }

    // Deduplicate all
    let dedup = |v: &mut Vec<String>| {
        let set: BTreeSet<String> = v.drain(..).collect();
        v.extend(set);
    };
    dedup(&mut ctx.bins);
    dedup(&mut ctx.examples);
    dedup(&mut ctx.packages);
    dedup(&mut ctx.features);
    dedup(&mut ctx.profiles);
    dedup(&mut ctx.tests);
    dedup(&mut ctx.benches);

    Some(ctx)
}

fn glob_member_dirs(cwd: &Path, pattern: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
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

fn read_package_name(dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let doc = content.parse::<toml_edit::DocumentMut>().ok()?;
    doc["package"]["name"].as_str().map(|s| s.to_string())
}

fn cargo_resolve_bins(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.bins)
}
fn cargo_resolve_examples(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.examples)
}
fn cargo_resolve_packages(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.packages)
}
fn cargo_resolve_features(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.features)
}
fn cargo_resolve_profiles(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.profiles)
}
fn cargo_resolve_tests(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.tests)
}
fn cargo_resolve_benches(cwd: &str) -> Option<Vec<String>> {
    detect_cargo_context(Path::new(cwd)).map(|c| c.benches)
}

#[cfg(not(target_family = "wasm"))]
fn git_resolve_branches(cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<String> = stdout.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if branches.is_empty() { None } else { Some(branches) }
}

#[cfg(target_family = "wasm")]
fn git_resolve_branches(_cwd: &str) -> Option<Vec<String>> {
    None
}

#[cfg(not(target_family = "wasm"))]
fn git_resolve_remotes(cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("git")
        .arg("remote")
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let remotes: Vec<String> = stdout.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if remotes.is_empty() { None } else { Some(remotes) }
}

#[cfg(target_family = "wasm")]
fn git_resolve_remotes(_cwd: &str) -> Option<Vec<String>> {
    None
}

#[cfg(not(target_family = "wasm"))]
fn git_resolve_status_files(cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = BTreeSet::new();
    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[0..2];
        let rest = line[3..].trim();
        
        let has_r = status.contains('R');
        let has_m = status.contains('M');
        let is_untracked = status == "??";
        
        if has_r {
            if let Some(pos) = rest.find(" -> ") {
                let new_path = &rest[pos + 4..];
                let path = strip_quotes(new_path.trim());
                if !path.is_empty() {
                    files.insert(path);
                }
            }
        } else if has_m || is_untracked {
            let path = strip_quotes(rest);
            if !path.is_empty() {
                files.insert(path);
            }
        }
    }
    if files.is_empty() {
        None
    } else {
        Some(files.into_iter().collect())
    }
}

#[cfg(target_family = "wasm")]
fn git_resolve_status_files(_cwd: &str) -> Option<Vec<String>> {
    None
}

#[cfg(not(target_family = "wasm"))]
fn git_resolve_tags(cwd: &str) -> Option<Vec<String>> {
    let output = command::blocking::Command::new("git")
        .arg("tag")
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tags: Vec<String> = stdout.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if tags.is_empty() { None } else { Some(tags) }
}

#[cfg(target_family = "wasm")]
fn git_resolve_tags(_cwd: &str) -> Option<Vec<String>> {
    None
}

fn strip_quotes(s: &str) -> String {
    let mut s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = &s[1..s.len() - 1];
    }
    s.to_string()
}

fn npm_resolve_scripts(cwd: &str) -> Option<Vec<String>> {
    let pkg_path = Path::new(cwd).join("package.json");
    let content = std::fs::read_to_string(&pkg_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let names: Vec<String> = scripts.keys().cloned().collect();
    if names.is_empty() { None } else { Some(names) }
}

pub fn get_active_tmp_prompt(cwd: &str, query: Option<&str>) -> Option<String> {
    let dir = schemas_dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut matched_schemas: Vec<SchemaFile> = Vec::new();

    let query_lower = query.map(|q| q.to_lowercase());
    let query_words: Vec<&str> = query_lower.as_ref()
        .map(|q| q.split_whitespace().collect())
        .unwrap_or_default();

    let aliases: &[(&str, &str)] = &[
        ("postgres", "psql"),
        ("postgresql", "psql"),
        ("node", "npm"),
        ("nodejs", "npm"),
        ("yarn", "npm"),
        ("pnpm", "npm"),
        ("rust", "cargo"),
        ("rustc", "cargo"),
        ("homebrew", "brew"),
        ("python", "pip"),
        ("python3", "pip"),
        ("pip3", "pip"),
        ("golang", "go"),
        ("kubectl", "kubernetes"),
        ("k8s", "kubernetes"),
    ];

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(schema_file) = serde_json::from_str::<SchemaFile>(&content) {
                let tool = schema_file.meta.tool.to_lowercase();
                
                let is_cwd_active = should_load_schema(&schema_file.meta, cwd);
                
                let mut is_query_matched = false;
                if !query_words.is_empty() {
                    if query_words.contains(&tool.as_str()) {
                        is_query_matched = true;
                    }
                    
                    if !is_query_matched {
                        for kw in &schema_file.meta.keywords {
                            if query_words.contains(&kw.to_lowercase().as_str()) {
                                is_query_matched = true;
                                break;
                            }
                        }
                    }
                    
                    if !is_query_matched {
                        for (alias, target) in aliases {
                            if query_words.contains(alias) && tool == *target {
                                is_query_matched = true;
                                break;
                            }
                        }
                    }
                }
                
                if is_cwd_active || is_query_matched {
                    matched_schemas.push(schema_file);
                }
            }
        }
    }

    if matched_schemas.is_empty() {
        return None;
    }

    let mut md = String::new();
    md.push_str("Here are the active tool schemas (TMP) for the current workspace. Use this information to suggest or execute exact, valid CLI commands:\n\n");

    for mut schema in matched_schemas {
        md.push_str(&format!("## Tool: {}\n", schema.meta.tool));
        for entry in &mut schema.commands {
            resolve_data_sources(entry, cwd);

            md.push_str(&format!("- `{}`: {}\n", entry.command, entry.description));
            if !entry.tokens.is_empty() {
                md.push_str("  Arguments:\n");
                for token in &entry.tokens {
                    let required_str = if token.required { " (required)" } else { " (optional)" };
                    let flag_str = match &token.flag {
                        Some(f) => format!(" flag: `{}`", f),
                        None => " (positional)".to_string(),
                    };
                    let type_str = match token.token_type {
                        TokenType::String => "String",
                        TokenType::Boolean => "Boolean",
                        TokenType::Enum => "Enum",
                        TokenType::File => "File",
                        TokenType::Number => "Number",
                    };
                    md.push_str(&format!(
                        "    * `{}`{}{}: {} (Type: {})\n",
                        token.name, flag_str, required_str, token.description, type_str
                    ));
                    if let Some(ref default) = token.default {
                        md.push_str(&format!("      Default: `{}`\n", default));
                    }
                    if let Some(ref values) = token.values {
                        if !values.is_empty() {
                            md.push_str(&format!("      Allowed values: {:?}\n", values));
                        }
                    }
                }
            }
        }
        md.push('\n');
    }

    Some(md)
}

pub fn build_assembled_command(entry: &CommandEntry, token_values: &[String], is_preview: bool) -> String {
    if entry.command.contains('<') {
        let mut assembled = entry.command.clone();
        for (i, token) in entry.tokens.iter().enumerate() {
            let val = token_values.get(i).cloned().unwrap_or_default();
            let replacement = if val.is_empty() {
                if is_preview {
                    format!("<{}>", token.name)
                } else {
                    String::new()
                }
            } else {
                val
            };
            assembled = assembled.replace(&format!("<{}>", token.name), &replacement);
        }
        return assembled.split_whitespace().collect::<Vec<&str>>().join(" ");
    }

    // New logic: no placeholders in entry.command
    let mut parts = vec![entry.command.clone()];
    
    // Sort tokens so flags come first, and positional arguments come last.
    let mut indexed_tokens: Vec<(usize, &TokenDef)> = entry.tokens.iter().enumerate().collect();
    // Stable sort: keep flags first, positional last.
    indexed_tokens.sort_by_key(|(_, token)| token.flag.is_none());

    for (i, token) in indexed_tokens {
        let val = token_values.get(i).cloned().unwrap_or_default();
        let val_formatted = if val.contains(' ') && !val.starts_with('"') && !val.starts_with('\'') {
            format!("\"{}\"", val)
        } else {
            val.clone()
        };

        if token.token_type == TokenType::Boolean {
            if let Some(ref flag) = token.flag {
                if val == "true" {
                    parts.push(flag.clone());
                }
            }
        } else if val.is_empty() {
            if is_preview && token.required {
                if let Some(ref flag) = token.flag {
                    parts.push(format!("{} <{}>", flag, token.name));
                } else {
                    parts.push(format!("<{}>", token.name));
                }
            }
        } else if let Some(ref flag) = token.flag {
            parts.push(format!("{} {}", flag, val_formatted));
        } else {
            parts.push(val_formatted);
        }
    }

    parts.join(" ")
}

fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '\\' => {
                if let Some(next_c) = chars.peek() {
                    if *next_c == '"' || *next_c == '\'' || *next_c == '\\' || *next_c == ' ' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push(c);
                    }
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && !in_double_quote && !in_single_quote => {
                if !current.is_empty() {
                    args.push(current);
                    current = String::new();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub fn extract_token_values(command_tmpl: &str, tokens: &[TokenDef], buffer: &str) -> Vec<String> {
    if command_tmpl.contains('<') {
        let mut values = vec![String::new(); tokens.len()];
        
        // Set defaults first
        for (i, t) in tokens.iter().enumerate() {
            if let Some(default) = &t.default {
                values[i] = default.clone();
            } else if let Some(vals) = &t.values {
                if vals.len() == 1 {
                    values[i] = vals[0].clone();
                }
            }
        }

        // Split template into text spans and placeholder names
        let mut parts = Vec::new(); // elements are either Literal(String) or Placeholder(String)
        let mut remaining = command_tmpl;
        while let Some(start) = remaining.find('<') {
            if start > 0 {
                parts.push(TemplatePart::Literal(remaining[..start].to_string()));
            }
            if let Some(end) = remaining[start..].find('>') {
                let placeholder_name = remaining[start + 1..start + end].to_string();
                parts.push(TemplatePart::Placeholder(placeholder_name));
                remaining = &remaining[start + end + 1..];
            } else {
                break;
            }
        }
        if !remaining.is_empty() {
            parts.push(TemplatePart::Literal(remaining.to_string()));
        }

        #[derive(Debug)]
        enum TemplatePart {
            Literal(String),
            Placeholder(String),
        }

        // Now, let's try to match the buffer text against these parts
        let mut buffer_remaining = buffer.trim_start();
        let mut token_val_map = std::collections::HashMap::new();

        for i in 0..parts.len() {
            match &parts[i] {
                TemplatePart::Literal(lit) => {
                    let lit_trimmed = lit.trim_start();
                    if buffer_remaining.starts_with(lit_trimmed) {
                        buffer_remaining = &buffer_remaining[lit_trimmed.len()..];
                    } else {
                        // Mismatch, stop extraction
                        break;
                    }
                }
                TemplatePart::Placeholder(name) => {
                    // Read until the next literal starts
                    let next_lit = if i + 1 < parts.len() {
                        match &parts[i + 1] {
                            TemplatePart::Literal(l) => Some(l.trim()),
                            _ => None,
                        }
                    } else {
                        None
                    };

                    let val = if let Some(nl) = next_lit {
                        if nl.is_empty() {
                            let v = buffer_remaining.trim_end();
                            buffer_remaining = "";
                            v
                        } else if let Some(pos) = buffer_remaining.find(nl) {
                            let v = buffer_remaining[..pos].trim();
                            buffer_remaining = &buffer_remaining[pos..];
                            v
                        } else {
                            let v = buffer_remaining.trim_end();
                            buffer_remaining = "";
                            v
                        }
                    } else {
                        let v = buffer_remaining.trim_end();
                        buffer_remaining = "";
                        v
                    };
                    if !val.is_empty() {
                        token_val_map.insert(name.clone(), val.to_string());
                    }
                }
            }
        }

        for (i, t) in tokens.iter().enumerate() {
            if let Some(val) = token_val_map.get(&t.name) {
                values[i] = val.clone();
            }
        }

        return values;
    }

    // New logic: command_tmpl does not contain placeholders.
    // Parse buffer against command_tmpl and tokens.
    let mut values = vec![String::new(); tokens.len()];
    
    // Set defaults first
    for (i, t) in tokens.iter().enumerate() {
        if let Some(default) = &t.default {
            values[i] = default.clone();
        } else if let Some(vals) = &t.values {
            if vals.len() == 1 {
                values[i] = vals[0].clone();
            }
        }
    }

    let buffer = buffer.trim_start();
    if !buffer.starts_with(command_tmpl) {
        return values;
    }

    // Get the remaining string after the base command
    let remaining = buffer[command_tmpl.len()..].trim();
    if remaining.is_empty() {
        return values;
    }

    let words = split_args(remaining);
    let mut consumed = vec![false; words.len()];

    // Phase 1: Match flagged tokens.
    for (i, token) in tokens.iter().enumerate() {
        if let Some(ref flag) = token.flag {
            // Find the flag in words
            if let Some(flag_idx) = words.iter().position(|w| w == flag) {
                if !consumed[flag_idx] {
                    consumed[flag_idx] = true;
                    if token.token_type == TokenType::Boolean {
                        values[i] = "true".to_string();
                    } else {
                        // Value flag: consume the next word as the value
                        if flag_idx + 1 < words.len() && !consumed[flag_idx + 1] {
                            values[i] = words[flag_idx + 1].clone();
                            consumed[flag_idx + 1] = true;
                        }
                    }
                }
            } else if token.token_type == TokenType::Boolean {
                values[i] = "false".to_string();
            }
        }
    }

    // Phase 2: Match positional tokens (no flag).
    let mut word_idx = 0;
    for (i, token) in tokens.iter().enumerate() {
        if token.flag.is_none() {
            while word_idx < words.len() && consumed[word_idx] {
                word_idx += 1;
            }
            if word_idx < words.len() {
                values[i] = words[word_idx].clone();
                consumed[word_idx] = true;
                word_idx += 1;
            }
        }
    }

    values
}

pub fn find_matching_tmp_command(buffer: &str, cwd: &str) -> Option<(CommandEntry, String)> {
    let buffer = buffer.trim_start();
    if buffer.is_empty() {
        return None;
    }
    let commands = load_all_schemas(cwd);
    let mut best_match: Option<(CommandEntry, String)> = None;
    let mut max_len = 0;

    for mut entry in commands {
        // Derive prefix before any placeholder
        let prefix = match entry.command.split_once('<') {
            Some((pre, _)) => pre.trim_end().to_string(),
            None => entry.command.clone(),
        };
        
        if prefix.is_empty() {
            continue;
        }

        // Check if buffer starts with prefix.
        let matches_prefix = if buffer == prefix {
            true
        } else if buffer.starts_with(&prefix) {
            let next_char = buffer.chars().nth(prefix.chars().count());
            next_char.map(|c| c.is_whitespace()).unwrap_or(false)
        } else {
            false
        };

        if matches_prefix {
            let len = prefix.len();
            if len > max_len {
                max_len = len;
                // Resolve dynamic variables for this entry
                resolve_data_sources(&mut entry, cwd);
                best_match = Some((entry, prefix));
            }
        }
    }
    best_match
}

#[cfg(test)]
#[path = "tmp_tests.rs"]
mod tests;



