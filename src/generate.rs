//! AI-powered TMP schema generator.
//!
//! Runs `<tool> --help` recursively, sends output to an LLM, and
//! saves the resulting schema as JSON to `~/.config/waz/schemas/`.

use crate::config::Config;
use crate::context::RuntimeContext;
use crate::llm;
use crate::tui::app::{CommandEntry, SchemaFile, SchemaMeta};
#[cfg(test)]
use crate::tui::app::{DataSource, TokenType};
use crate::tui::cargo_schema::CargoContext;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const CURATED_SCHEMAS: &[(&str, &str)] = &[
    ("bun.json", include_str!("../schemas/curated/bun.json")),
    ("bunx.json", include_str!("../schemas/curated/bunx.json")),
    (
        "cargo-script.json",
        include_str!("../schemas/curated/cargo-script.json"),
    ),
    ("cargo.json", include_str!("../schemas/curated/cargo.json")),
    ("git.json", include_str!("../schemas/curated/git.json")),
    ("npm.json", include_str!("../schemas/curated/npm.json")),
    ("npx.json", include_str!("../schemas/curated/npx.json")),
    (
        "rust-script.json",
        include_str!("../schemas/curated/rust-script.json"),
    ),
    ("waz.json", include_str!("../schemas/curated/waz.json")),
];

/// Embedded curated schema JSON keyed by filename (`cargo.json`, …).
pub fn curated_schema(filename: &str) -> Option<&'static str> {
    CURATED_SCHEMAS
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, body)| *body)
}

pub fn curated_schema_count() -> usize {
    CURATED_SCHEMAS.len()
}

thread_local! {
    static CARGO_CTX_CACHE: RefCell<Option<(String, CargoContext)>> = const { RefCell::new(None) };
    static WHICH_CACHE: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// Directory where user schemas are stored.
/// Override with `WAZ_SCHEMAS_DIR` so agents can test against a clean copy of curated JSON.
pub fn schemas_dir() -> PathBuf {
    let dir = if let Ok(override_dir) = std::env::var("WAZ_SCHEMAS_DIR") {
        PathBuf::from(override_dir)
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("waz")
            .join("schemas")
    };
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Check if a schema already exists for the given tool.
pub fn schema_exists(tool: &str) -> bool {
    schemas_dir().join(format!("{}.json", tool)).exists()
}

/// Initialize curated schemas — copy embedded JSON to the user's config dir.
/// Only copies schemas that don't already exist (won't overwrite user modifications).
pub fn init_schemas() -> Result<Vec<String>, String> {
    let target_dir = schemas_dir();
    let mut installed = Vec::new();

    for (filename, body) in CURATED_SCHEMAS {
        let target = target_dir.join(filename);
        if target.exists() {
            continue;
        }
        match std::fs::write(&target, body) {
            Ok(_) => {
                let tool = filename.trim_end_matches(".json");
                installed.push(tool.to_string());
            }
            Err(e) => {
                eprintln!("  ⚠️  Failed to install {}: {}", filename, e);
            }
        }
    }

    Ok(installed)
}

/// Load all JSON schemas from the schemas directory.
/// Supports both `SchemaFile` (new) and `Vec<CommandEntry>` (legacy) formats.
/// Filters schemas based on CWD context (requires_file, requires_binary).
pub fn load_all_schemas(cwd: &str) -> Vec<CommandEntry> {
    load_all_schemas_with_context(cwd, None)
}

pub fn load_all_schemas_with_context(
    cwd: &str,
    context: Option<&RuntimeContext>,
) -> Vec<CommandEntry> {
    // Auto-init curated schemas on first load
    if let Ok(installed) = init_schemas() {
        if !installed.is_empty() && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            eprintln!("📦 Initialized curated schemas: {}", installed.join(", "));
        }
    }

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

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                // Try new SchemaFile format first
                if let Ok(schema_file) = serde_json::from_str::<SchemaFile>(&content) {
                    // Check requirements
                    if !should_load_schema(&schema_file.meta, cwd, context) {
                        continue;
                    }
                    let cmds = schema_file.commands;
                    commands.extend(cmds);
                }
                // Fallback: legacy Vec<CommandEntry> format
                else if let Ok(entries) = serde_json::from_str::<Vec<CommandEntry>>(&content) {
                    commands.extend(entries);
                } else {
                    eprintln!("Warning: failed to parse schema {}", path.display());
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to read schema {}: {}", path.display(), e);
            }
        }
    }

    commands
}

/// Check if a schema should be loaded based on its requirements.
fn should_load_schema(meta: &SchemaMeta, cwd: &str, context: Option<&RuntimeContext>) -> bool {
    // Check requires_file (e.g. "Cargo.toml", "package.json")
    if let Some(ref file) = meta.requires_file {
        if !std::path::Path::new(cwd).join(file).exists() {
            return false;
        }
    }

    if let Some(ref file_kind) = meta.requires_file_kind {
        let Some(context) = context else {
            return false;
        };
        if context.file_kind != *file_kind {
            return false;
        }
    }

    // Check requires_binary (e.g. "git", "bun")
    if let Some(ref binary) = meta.requires_binary {
        if !which_exists(binary) {
            return false;
        }
    }

    true
}

fn which_exists(cmd: &str) -> bool {
    WHICH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(hit) = cache.get(cmd) {
            return *hit;
        }
        let found = binary_on_path(cmd);
        cache.insert(cmd.to_string(), found);
        found
    })
}

fn binary_on_path(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    let path = Path::new(cmd);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    for dir in env::split_paths(&paths) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat", "com"] {
                if dir.join(format!("{cmd}.{ext}")).is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// Resolve any `data_source` fields in tokens (shell commands or built-in resolvers).
fn resolve_data_sources(entry: &mut CommandEntry, cwd: &str, context: Option<&RuntimeContext>) {
    for token in &mut entry.tokens {
        if let Some(ref ds) = token.data_source {
            let values = if let Some(ref resolver) = ds.resolver {
                // Built-in resolver
                resolve_builtin(resolver, cwd, context)
            } else if let Some(ref cmd) = ds.command {
                // Shell command
                run_data_source_command(cmd, &ds.parse, cwd)
            } else {
                None
            };

            if let Some(vals) = values {
                if !vals.is_empty() {
                    // Overlay completions; keep the declared token_type.
                    token.values = Some(vals);
                }
            }
        }
    }
}

/// Public wrapper for verification TUI to test data sources.
pub fn resolve_data_sources_pub(entry: &mut CommandEntry, cwd: &str) {
    resolve_data_sources(entry, cwd, None);
}

pub fn resolve_data_sources_pub_ctx(
    entry: &mut CommandEntry,
    cwd: &str,
    context: Option<&RuntimeContext>,
) {
    resolve_data_sources(entry, cwd, context);
}

/// Run a shell command and parse its output into values.
fn run_data_source_command(cmd: &str, parse: &str, cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let values: Vec<String> = match parse {
        "words" => stdout.split_whitespace().map(|s| s.to_string()).collect(),
        _ => stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

// ──────────────────────────── Built-in Resolvers ────────────────────────────

/// Resolve a built-in named resolver (e.g. "cargo:bins", "git:branches", "waz:models:gemini").
fn resolve_builtin(
    resolver: &str,
    cwd: &str,
    context: Option<&RuntimeContext>,
) -> Option<Vec<String>> {
    // Handle parameterized resolvers (e.g. "waz:models:gemini")
    let parts: Vec<&str> = resolver.splitn(3, ':').collect();

    match (
        parts.get(0).copied(),
        parts.get(1).copied(),
        parts.get(2).copied(),
    ) {
        (Some("cargo"), Some("bins"), _) => cargo_resolve_bins(cwd),
        (Some("cargo"), Some("examples"), _) => cargo_resolve_examples(cwd),
        (Some("cargo"), Some("packages"), _) => cargo_resolve_packages(cwd),
        (Some("cargo"), Some("features"), _) => cargo_resolve_features(cwd),
        (Some("cargo"), Some("profiles"), _) => cargo_resolve_profiles(cwd),
        (Some("cargo"), Some("tests"), _) => cargo_resolve_tests(cwd),
        (Some("cargo"), Some("benches"), _) => cargo_resolve_benches(cwd),
        (Some("git"), Some("branches"), _) => git_resolve_branches(cwd),
        (Some("git"), Some("remotes"), _) => git_resolve_remotes(cwd),
        (Some("git"), Some("status_files"), filter) => git_resolve_status_files(cwd, filter),
        (Some("npm"), Some("scripts"), _) => npm_resolve_scripts(cwd),
        (Some("waz"), Some("models"), Some(provider)) => waz_resolve_models(provider),
        (Some("waz"), Some("models"), None) => waz_resolve_models("gemini"),
        (Some("waz"), Some("context"), Some(field)) => resolve_waz_context(field, context),
        (Some("waz"), Some("context"), None) => resolve_waz_context("file_path", context),
        _ => {
            eprintln!("Warning: unknown resolver '{}'", resolver);
            None
        }
    }
}

fn resolve_waz_context(field: &str, context: Option<&RuntimeContext>) -> Option<Vec<String>> {
    let context = context?;
    let value = match field {
        "cwd" => Some(context.cwd.clone()),
        "project_root" => context.project_root.clone(),
        "file_path" => context.file_path.clone(),
        "line" => context.line.map(|line| line.to_string()),
        "build_system" => Some(context.build_system.clone()),
        "file_kind" => Some(context.file_kind.clone()),
        "runnable_kind" => context.runnable_kind.clone(),
        "package_name" => context.package_name.clone(),
        "script_engine" => context.script_engine.clone(),
        "recommended_target" => context.recommended_target.clone(),
        _ => None,
    }?;

    Some(vec![value])
}

/// Fetch available models from an LLM provider's API.
fn waz_resolve_models(provider: &str) -> Option<Vec<String>> {
    let config = crate::config::Config::load();

    // Find the provider's API key
    let api_key = config
        .llm
        .providers
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(provider))
        .and_then(|p| p.keys.first().cloned())
        .or_else(|| {
            // Try env vars
            crate::config::ProviderDefaults::env_vars(provider)
                .into_iter()
                .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
        });

    match provider {
        "gemini" => {
            if let Some(key) = api_key {
                fetch_gemini_models(&key)
            } else {
                Some(vec![
                    "gemini-3.1-flash-lite-preview".into(),
                    "gemini-2.5-pro-preview-05-06".into(),
                    "gemini-2.5-flash-preview-05-20".into(),
                    "gemini-2.0-flash".into(),
                ])
            }
        }
        "openai" => {
            if let Some(key) = api_key {
                fetch_openai_models(&key)
            } else {
                Some(vec![
                    "gpt-4o-mini".into(),
                    "gpt-4o".into(),
                    "gpt-4.1-mini".into(),
                    "gpt-4.1".into(),
                    "o4-mini".into(),
                ])
            }
        }
        "ollama" => fetch_ollama_models(),
        "glm" => Some(vec![
            "glm-4.7".into(),
            "glm-4-plus".into(),
            "glm-4-flash".into(),
        ]),
        "qwen" => Some(vec![
            "qwen3.5-plus".into(),
            "qwen3.5-turbo".into(),
            "qwen-plus".into(),
        ]),
        "minimax" => Some(vec!["MiniMax-M2.5".into(), "MiniMax-T1".into()]),
        _ => None,
    }
}

fn fetch_gemini_models(api_key: &str) -> Option<Vec<String>> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}",
        api_key
    );
    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "5", &url])
        .output()
        .ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = json.get("models")?.as_array()?;
    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            // "models/gemini-2.5-pro" → "gemini-2.5-pro"
            let short = name.strip_prefix("models/").unwrap_or(name);
            // Only include generateContent-capable models
            let methods = m.get("supportedGenerationMethods")?.as_array()?;
            if methods
                .iter()
                .any(|m| m.as_str() == Some("generateContent"))
            {
                Some(short.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn fetch_openai_models(api_key: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "5",
            "-H",
            &format!("Authorization: Bearer {}", api_key),
            "https://api.openai.com/v1/models",
        ])
        .output()
        .ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let data = json.get("data")?.as_array()?;
    let mut names: Vec<String> = data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            // Filter to chat models only
            if id.starts_with("gpt-")
                || id.starts_with("o1")
                || id.starts_with("o3")
                || id.starts_with("o4")
            {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn fetch_ollama_models() -> Option<Vec<String>> {
    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "3", "http://localhost:11434/api/tags"])
        .output()
        .ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = json.get("models")?.as_array()?;
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("name")?.as_str().map(|s| s.to_string()))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn cargo_context(cwd: &str) -> CargoContext {
    CARGO_CTX_CACHE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some((cached_cwd, ctx)) = slot.as_ref() {
            if cached_cwd == cwd {
                return ctx.clone();
            }
        }
        let ctx = CargoContext::detect(Path::new(cwd));
        *slot = Some((cwd.to_string(), ctx.clone()));
        ctx
    })
}

/// Cargo: resolve binary targets from Cargo.toml and src/bin/.
fn cargo_resolve_bins(cwd: &str) -> Option<Vec<String>> {
    let bins = cargo_context(cwd).bins;
    if bins.is_empty() {
        None
    } else {
        Some(bins)
    }
}

fn cargo_resolve_examples(cwd: &str) -> Option<Vec<String>> {
    let examples = cargo_context(cwd).examples;
    if examples.is_empty() {
        None
    } else {
        Some(examples)
    }
}

fn cargo_resolve_packages(cwd: &str) -> Option<Vec<String>> {
    let packages = cargo_context(cwd).packages;
    if packages.is_empty() {
        None
    } else {
        Some(packages)
    }
}

fn cargo_resolve_features(cwd: &str) -> Option<Vec<String>> {
    let features = cargo_context(cwd).features;
    if features.is_empty() {
        None
    } else {
        Some(features)
    }
}

fn cargo_resolve_profiles(cwd: &str) -> Option<Vec<String>> {
    let mut profiles = cargo_context(cwd).profiles;
    for p in ["dev", "release", "test", "bench"] {
        if !profiles.iter().any(|existing| existing == p) {
            profiles.push(p.to_string());
        }
    }
    if profiles.is_empty() {
        None
    } else {
        Some(profiles)
    }
}

fn cargo_resolve_tests(cwd: &str) -> Option<Vec<String>> {
    let tests = cargo_context(cwd).tests;
    if tests.is_empty() {
        None
    } else {
        Some(tests)
    }
}

fn cargo_resolve_benches(cwd: &str) -> Option<Vec<String>> {
    let benches = cargo_context(cwd).benches;
    if benches.is_empty() {
        None
    } else {
        Some(benches)
    }
}

/// Git: resolve branch names.
fn git_resolve_branches(cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branches: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if branches.is_empty() {
        None
    } else {
        Some(branches)
    }
}

/// Git: resolve paths from `git status --porcelain` for `git add`.
fn git_resolve_status_files(cwd: &str, filter: Option<&str>) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let paths = parse_git_status_porcelain(&stdout, filter);
    if paths.is_empty() {
        None
    } else {
        Some(paths)
    }
}

fn parse_git_status_porcelain(stdout: &str, filter: Option<&str>) -> Vec<String> {
    let mut paths = Vec::new();
    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let staged = line.as_bytes()[0];
        let unstaged = line.as_bytes()[1];
        let include = match filter {
            Some("staged") => staged != b' ' && staged != b'?',
            Some("unstaged") => unstaged != b' ' || (staged == b'?' && unstaged == b'?'),
            _ => true,
        };
        if !include {
            continue;
        }
        if let Some(path) = porcelain_path(line) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn porcelain_path(line: &str) -> Option<String> {
    let rest = line.get(3..)?.trim();
    if rest.is_empty() {
        return None;
    }
    let path = rest
        .rsplit_once(" -> ")
        .map(|(_, dst)| dst)
        .unwrap_or(rest)
        .trim()
        .trim_matches('"');
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Git: resolve remote names.
fn git_resolve_remotes(cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["remote"])
        .current_dir(cwd)
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let remotes: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if remotes.is_empty() {
        None
    } else {
        Some(remotes)
    }
}

/// npm/bun: resolve script names from package.json.
fn npm_resolve_scripts(cwd: &str) -> Option<Vec<String>> {
    let pkg_path = std::path::Path::new(cwd).join("package.json");
    let content = std::fs::read_to_string(&pkg_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let names: Vec<String> = scripts.keys().cloned().collect();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

// ──────────────────────────── Schema Sharing ────────────────────────────

/// Export a schema as a clean shareable file.
/// Strips runtime-resolved values (token.values populated by resolvers),
/// keeping data_source definitions so importers can resolve them locally.
pub fn share_schema(tool: &str) -> Result<std::path::PathBuf, String> {
    let src = schemas_dir().join(format!("{}.json", tool));
    if !src.exists() {
        return Err(format!(
            "No schema found for '{}'. Generate one first.",
            tool
        ));
    }

    let content = std::fs::read_to_string(&src).map_err(|e| format!("Read: {}", e))?;

    // Try SchemaFile format
    let mut schema: SchemaFile =
        serde_json::from_str(&content).map_err(|e| format!("Parse: {}", e))?;

    strip_runtime_values(&mut schema);

    // Write to CWD for easy sharing
    let filename = format!("{}-schema-v{}.json", tool, schema.meta.version);
    let dest = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(&filename);

    let json = serde_json::to_string_pretty(&schema).map_err(|e| format!("Serialize: {}", e))?;
    std::fs::write(&dest, &json).map_err(|e| format!("Write: {}", e))?;

    Ok(dest)
}

fn strip_runtime_values(schema: &mut SchemaFile) {
    for cmd in &mut schema.commands {
        for tok in &mut cmd.tokens {
            if tok.data_source.is_some() {
                tok.values = None;
            }
        }
    }
}

/// Import a schema from a local path or URL.
pub fn import_schema(source: &str) -> Result<String, String> {
    let content = if source.starts_with("http://") || source.starts_with("https://") {
        // Download from URL
        download_schema(source)?
    } else {
        // Read from local file
        std::fs::read_to_string(source)
            .map_err(|e| format!("Failed to read '{}': {}", source, e))?
    };

    // Parse and validate
    let schema: SchemaFile =
        serde_json::from_str(&content).map_err(|e| format!("Invalid schema format: {}", e))?;

    let tool = schema.meta.tool.clone();
    if tool.is_empty() {
        return Err("Schema has no tool name in meta.tool".to_string());
    }

    // Version-save existing schema before overwrite
    if schema_exists(&tool) {
        if let Ok(v) = version_save(&tool) {
            eprintln!("  📦 Backed up existing schema as v{}", v);
        }
    }

    // Save to schemas dir
    let dest = schemas_dir().join(format!("{}.json", tool));
    std::fs::write(&dest, &content).map_err(|e| format!("Write: {}", e))?;

    Ok(tool)
}

/// Download schema content from a URL.
fn download_schema(url: &str) -> Result<String, String> {
    // Use curl since it's universally available
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "10", url])
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Download failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// List all installed schemas with their status.
pub fn list_schemas() {
    let dir = schemas_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            eprintln!("No schemas directory found at {}", dir.display());
            return;
        }
    };

    let mut schemas: Vec<(String, SchemaFile)> = Vec::new();
    let mut legacy_count = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Ok(sf) = serde_json::from_str::<SchemaFile>(&content) {
            schemas.push((path.file_stem().unwrap().to_string_lossy().to_string(), sf));
        } else if serde_json::from_str::<Vec<CommandEntry>>(&content).is_ok() {
            legacy_count += 1;
        }
    }

    if schemas.is_empty() && legacy_count == 0 {
        eprintln!(
            "No schemas installed. Run `waz generate <tool> --init` to install curated schemas."
        );
        return;
    }

    // Sort by tool name
    schemas.sort_by(|a, b| a.0.cmp(&b.0));

    // Print header
    eprintln!(
        "{:<12} {:<6} {:<10} {:<8} {:<6} {:<10}",
        "Tool", "Ver", "Status", "Cmds", "Source", "Coverage"
    );
    eprintln!("{}", "─".repeat(56));

    for (name, sf) in &schemas {
        let verified_count = sf.commands.iter().filter(|c| c.verified).count();
        let total = sf.commands.len();
        let status = if sf.meta.verified {
            "✅ verified"
        } else if verified_count > 0 {
            "🔍 partial"
        } else {
            "○  pending"
        };

        let source = match sf.meta.generated_by.as_str() {
            "human" => "curated",
            "ai" => "ai-gen",
            "hybrid" => "hybrid",
            _ => &sf.meta.generated_by,
        };

        eprintln!(
            "{:<12} v{:<4} {:<10} {:<8} {:<6} {}",
            name,
            sf.meta.version,
            status,
            format!("{}/{}", verified_count, total),
            source,
            sf.meta.coverage,
        );
    }

    if legacy_count > 0 {
        eprintln!(
            "\n  + {} legacy schema(s) (pre-SchemaFile format)",
            legacy_count
        );
    }

    eprintln!("\n📁 {}", dir.display());
}

/// Generate a TMP schema for a CLI tool using AI.
///
/// 1. Runs `<tool> --help` and subcommand help recursively
/// 2. Sends to LLM with a structured prompt
/// 3. Parses response into Vec<CommandEntry>
/// 4. Saves to ~/.config/waz/schemas/<tool>.json as SchemaFile
pub fn generate_schema(
    config: &Config,
    tool: &str,
    model_override: Option<&str>,
    provider_override: Option<&str>,
) -> Result<Vec<CommandEntry>, String> {
    // Step 1: Check tool exists
    let which = Command::new("which").arg(tool).output();
    match which {
        Ok(out) if out.status.success() => {}
        _ => return Err(format!("'{}' not found on PATH", tool)),
    }

    eprintln!("🔍 Detecting {} commands...", tool);

    // Step 2: Gather help text
    let mut help_texts = Vec::new();

    // Main help
    let main_help = run_help(tool, &[]);
    if main_help.is_empty() {
        return Err(format!("'{}' --help produced no output", tool));
    }
    eprintln!("   Running: {} --help", tool);
    help_texts.push(format!("=== {} --help ===\n{}", tool, main_help));

    // Extract subcommands from the main help and run --help on each
    let subcommands: Vec<String> = extract_subcommands(&main_help, tool)
        .into_iter()
        .filter(|s| s != tool) // Don't run `tool tool --help`
        .collect();
    let max_subcommands = 20; // Cap to avoid excessive API calls
    for (i, sub) in subcommands.iter().take(max_subcommands).enumerate() {
        eprintln!(
            "   Running: {} {} --help ({}/{})",
            tool,
            sub,
            i + 1,
            subcommands.len().min(max_subcommands)
        );
        let sub_help = run_help(tool, &[sub.as_str()]);
        if !sub_help.is_empty() {
            help_texts.push(format!("=== {} {} --help ===\n{}", tool, sub, sub_help));
        }
    }

    // Determine model info for display
    let model_name = model_override.map(|s| s.to_string()).unwrap_or_else(|| {
        config
            .llm
            .providers
            .first()
            .map(|p| p.model.clone())
            .unwrap_or_else(|| "default".to_string())
    });
    eprintln!("\n🤖 Generating schema with AI (model: {})...", model_name);

    // Step 3: Build prompt and call LLM
    let help_combined = help_texts.join("\n\n");
    // Truncate if too long (keep last portion which has subcommands)
    let help_truncated = if help_combined.len() > 12000 {
        &help_combined[help_combined.len() - 12000..]
    } else {
        &help_combined
    };

    let prompt = build_generate_prompt(tool, help_truncated);
    let response = call_llm_for_schema(config, &prompt, model_override, provider_override)?;

    // Step 4: Parse response
    let commands = parse_schema_response(tool, &response)?;

    // Step 5: Save as SchemaFile with meta
    let existing_version = if schema_exists(tool) {
        // Try to read existing version
        let path = schemas_dir().join(format!("{}.json", tool));
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<SchemaFile>(&c).ok())
            .map(|s| s.meta.version)
            .unwrap_or(0)
    } else {
        0
    };

    let schema_file = SchemaFile {
        meta: SchemaMeta {
            tool: tool.to_string(),
            version: existing_version + 1,
            generated_by: "ai".to_string(),
            generated_with: Some(model_name),
            verified: false,
            verified_at: None,
            coverage: "partial".to_string(),
            waz_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            requires_file: None,
            requires_file_kind: None,
            requires_binary: Some(tool.to_string()),
            keywords: vec![],
        },
        commands: commands.clone(),
    };

    let schema_path = schemas_dir().join(format!("{}.json", tool));
    let json = serde_json::to_string_pretty(&schema_file)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&schema_path, &json).map_err(|e| format!("Failed to write schema: {}", e))?;

    eprintln!(
        "   Found {} commands with {} tokens",
        commands.len(),
        commands.iter().map(|c| c.tokens.len()).sum::<usize>()
    );
    eprintln!(
        "\n✅ Saved to {} (v{})",
        schema_path.display(),
        schema_file.meta.version
    );
    eprintln!("   Next time you open the TUI, these commands will auto-load.");

    Ok(commands)
}

/// Run `<tool> [args...] --help` and return stdout+stderr.
fn run_help(tool: &str, args: &[&str]) -> String {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    cmd.arg("--help");

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!("{}{}", stdout, stderr)
        }
        Err(_) => String::new(),
    }
}

/// Extract subcommand names from help text.
/// Handles formats like `gemini mcp` (tool-prefixed) and `mcp` (bare subcommand).
fn extract_subcommands(help: &str, tool: &str) -> Vec<String> {
    let mut subs = Vec::new();
    let mut in_commands = false;

    for line in help.lines() {
        let trimmed = line.trim();

        // Detect command section headers
        if trimmed.to_lowercase().contains("commands:")
            || trimmed.to_lowercase().contains("subcommands:")
            || trimmed.to_lowercase() == "commands"
        {
            in_commands = true;
            continue;
        }

        // End of command section (blank line or new section)
        if in_commands {
            if trimmed.is_empty() {
                // Could be end of section, but allow one blank line
                continue;
            }
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') && trimmed.ends_with(':') {
                in_commands = false;
                continue;
            }

            // Extract subcommand name, handling tool-prefixed formats
            // e.g. "gemini mcp  — Manage MCP" → words are ["gemini", "mcp", ...]
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            // If first word is the tool name, take the second word as subcommand
            let sub_word = if words.first() == Some(&tool) && words.len() > 1 {
                words[1]
            } else {
                words.first().copied().unwrap_or("")
            };
            // Skip help, version, meta entries, flags, and bracketed positional args
            if !sub_word.is_empty()
                && sub_word != tool
                && sub_word != "help"
                && sub_word != "version"
                && !sub_word.starts_with('-')
                && !sub_word.starts_with('[')
                && !sub_word.starts_with('<')
                && sub_word
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                subs.push(sub_word.to_string());
            }
        }
    }

    subs
}

/// Build the LLM prompt for schema generation.
fn build_generate_prompt(tool: &str, help_text: &str) -> String {
    format!(
        r#"You are a CLI tool analyzer. Given the help output of '{}', generate a JSON array of command entries.

Each entry must have this exact format:
{{
  "command": "{} subcommand",
  "description": "Short description",
  "group": "{}",
  "tokens": [
    {{
      "name": "param_name",
      "description": "Short description",
      "required": true,
      "token_type": "String",
      "default": null,
      "values": null,
      "flag": "--flag-name",
      "data_source": null
    }}
  ]
}}

CRITICAL RULES:
- The "command" field must be ONLY the binary name and subcommand, e.g. "{} install" or "{}"
  NEVER put positional args, brackets, or angle brackets in the command field.
  Positional arguments go in the tokens array with "flag": null.
- Include the most commonly used subcommands
- For the BASE command (no subcommand), create an entry with command: "{}"
  and include ALL its options as tokens
- For each subcommand, include ALL flags and options shown in the help output
- "default" must ALWAYS be either null or a string like "false", "0", never a bare boolean or number
- "flag" must be either a string like "--verbose" or null (for positional args), never false
- "values" must be either null or a string array like ["option1", "option2"]
- Use token_type "Boolean" for flags that are on/off switches
- Use token_type "Enum" when there are specific allowed values (put them in "values")
- Use token_type "File" for file/directory/path arguments
- Use token_type "Number" for numeric values
- Set "flag" to the CLI flag (e.g. "--verbose", "-n")
- Set "flag" to null for positional arguments
- Do NOT make flags into separate commands. "--resume" is a flag, not a subcommand.
- Output ONLY the JSON array, no markdown, no explanation, no code fences

Help output:
{}

JSON:"#,
        tool, tool, tool, tool, tool, tool, help_text
    )
}

/// Call the LLM to generate schema JSON.
fn call_llm_for_schema(
    config: &Config,
    prompt: &str,
    model_override: Option<&str>,
    provider_override: Option<&str>,
) -> Result<String, String> {
    if config.llm.providers.is_empty() {
        return Err(
            "No LLM provider configured. Run `waz login grok`, `waz login anthropic`, or `waz login chatgpt`, set GEMINI_API_KEY / XAI_API_KEY / ANTHROPIC_API_KEY / OPENAI_API_KEY, or configure ~/.config/waz/config.toml"
                .to_string(),
        );
    }
    llm::complete_filtered(
        config,
        prompt,
        &llm::CompleteOptions::generate(),
        provider_override,
        model_override,
    )
    .ok_or_else(|| "All LLM providers failed. Check your API keys.".to_string())
}

/// Normalize LLM JSON output to fix common type errors.
/// LLMs frequently output `"default": false` instead of `"default": null`,
/// `"flag": false` instead of `"flag": null`, and positional args in command names.
fn normalize_llm_json(json_str: &str) -> String {
    // Parse as serde_json::Value, fix types, serialize back
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return json_str.to_string();
    };

    if let Some(arr) = val.as_array_mut() {
        for cmd in arr.iter_mut() {
            // Clean command name: remove [brackets] and <angle brackets>
            if let Some(command) = cmd
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                // Remove bracketed/angle-bracketed positional args from command name
                let clean: String = command
                    .split_whitespace()
                    .filter(|part| {
                        !part.starts_with('[') && !part.starts_with('<') && !part.starts_with("--")
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !clean.is_empty() {
                    cmd["command"] = serde_json::Value::String(clean);
                }
            }

            // Fix token types
            if let Some(tokens) = cmd.get_mut("tokens").and_then(|t| t.as_array_mut()) {
                for token in tokens.iter_mut() {
                    // Fix "default": false/true/number → "default": "false"/"true"/"123"
                    if let Some(default) = token.get("default") {
                        match default {
                            serde_json::Value::Bool(b) => {
                                token["default"] = serde_json::Value::String(b.to_string());
                            }
                            serde_json::Value::Number(n) => {
                                token["default"] = serde_json::Value::String(n.to_string());
                            }
                            _ => {}
                        }
                    }

                    // Fix "flag": false/true → "flag": null
                    if let Some(flag) = token.get("flag") {
                        match flag {
                            serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                                token["flag"] = serde_json::Value::Null;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    serde_json::to_string(&val).unwrap_or_else(|_| json_str.to_string())
}

/// Parse the LLM response into Vec<CommandEntry>.
fn parse_schema_response(tool: &str, response: &str) -> Result<Vec<CommandEntry>, String> {
    // Strip markdown code fences if present
    let trimmed = response.trim();
    let json_str = if trimmed.starts_with("```") {
        // Remove opening fence (```json or ```)
        let after_open = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };
        // Remove closing fence
        let before_close = after_open.trim();
        before_close
            .strip_suffix("```")
            .unwrap_or(before_close)
            .trim()
    } else {
        trimmed
    };

    // Normalize LLM JSON output to fix common type errors
    let json_str = &normalize_llm_json(json_str);

    let commands: Vec<CommandEntry> = serde_json::from_str(json_str).map_err(|e| {
        format!(
            "Failed to parse AI response as JSON: {}\n\nRaw response:\n{}",
            e,
            &json_str[..json_str.len().min(500)]
        )
    })?;

    if commands.is_empty() {
        return Err(format!("AI generated 0 commands for '{}'", tool));
    }

    Ok(commands)
}

// ──────────────────────────── Versioned Backup / Rollback / Diff ────────────────────────────

/// Directory for versioned schemas: `~/.config/waz/schemas/versions/<tool>/`
fn versions_dir(tool: &str) -> PathBuf {
    let dir = schemas_dir().join("versions").join(tool);
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Get the latest version number for a tool (0 if no versions exist).
fn latest_version(tool: &str) -> u32 {
    let dir = versions_dir(tool);
    let mut max = 0u32;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix('v') {
                if let Some(num_str) = rest.strip_suffix(".json") {
                    if let Ok(n) = num_str.parse::<u32>() {
                        max = max.max(n);
                    }
                }
            }
        }
    }
    max
}

/// Save the current schema as a new version. Returns the version number.
pub fn version_save(tool: &str) -> Result<u32, String> {
    let source = schemas_dir().join(format!("{}.json", tool));
    if !source.exists() {
        return Err(format!("No schema found for '{}'", tool));
    }

    let next = latest_version(tool) + 1;
    let dest = versions_dir(tool).join(format!("v{}.json", next));

    std::fs::copy(&source, &dest).map_err(|e| format!("Failed to save version: {}", e))?;

    eprintln!("📦 Saved as v{} → {}", next, dest.display());
    Ok(next)
}

/// Rollback to a specific version, or the latest if None.
pub fn rollback_schema(tool: &str, version: Option<u32>) -> Result<u32, String> {
    let target = schemas_dir().join(format!("{}.json", tool));
    let v = match version {
        Some(v) => v,
        None => {
            let latest = latest_version(tool);
            if latest == 0 {
                return Err(format!(
                    "No version history for '{}'. Use --history to check.",
                    tool
                ));
            }
            latest
        }
    };

    let source = versions_dir(tool).join(format!("v{}.json", v));
    if !source.exists() {
        let latest = latest_version(tool);
        return Err(format!(
            "Version v{} not found for '{}'. Latest version: v{}. Use --history to see all.",
            v, tool, latest
        ));
    }

    std::fs::copy(&source, &target).map_err(|e| format!("Failed to rollback: {}", e))?;

    Ok(v)
}

/// Show version history for a tool.
pub fn show_version_history(tool: &str) {
    let dir = versions_dir(tool);

    // Collect and sort versions
    let mut versions: Vec<(u32, std::path::PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix('v') {
                if let Some(num_str) = rest.strip_suffix(".json") {
                    if let Ok(n) = num_str.parse::<u32>() {
                        versions.push((n, entry.path()));
                    }
                }
            }
        }
    }

    if versions.is_empty() {
        let current = schemas_dir().join(format!("{}.json", tool));
        if current.exists() {
            eprintln!(
                "📋 '{}' has a current schema but no version history yet.",
                tool
            );
            eprintln!("   Version history starts when you use --force to regenerate.");
        } else {
            eprintln!("📋 No schema or history found for '{}'.", tool);
        }
        return;
    }

    versions.sort_by_key(|(n, _)| *n);

    eprintln!(
        "📋 Version history for '{}' ({} versions):",
        tool,
        versions.len()
    );
    eprintln!("─────────────────────────────────────────");

    for (v, path) in &versions {
        let meta = std::fs::metadata(path).ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let elapsed = t.elapsed().unwrap_or_default();
                if elapsed.as_secs() < 60 {
                    "just now".to_string()
                } else if elapsed.as_secs() < 3600 {
                    format!("{}m ago", elapsed.as_secs() / 60)
                } else if elapsed.as_secs() < 86400 {
                    format!("{}h ago", elapsed.as_secs() / 3600)
                } else {
                    format!("{}d ago", elapsed.as_secs() / 86400)
                }
            })
            .unwrap_or_else(|| "unknown".to_string());

        let size = meta.map(|m| m.len()).unwrap_or(0);

        // Parse to get command count
        let cmd_count = std::fs::read_to_string(path)
            .ok()
            .and_then(|c| serde_json::from_str::<Vec<CommandEntry>>(&c).ok())
            .map(|cmds| format!("{} commands", cmds.len()))
            .unwrap_or_else(|| format!("{} bytes", size));

        let is_latest = *v == versions.last().map(|(n, _)| *n).unwrap_or(0);
        let marker = if is_latest { " ← latest" } else { "" };

        eprintln!("  v{:<4} │ {:<15} │ {}{}", v, modified, cmd_count, marker);
    }

    eprintln!("─────────────────────────────────────────");
    eprintln!(
        "  Rollback: waz generate {} --rollback        (latest)",
        tool
    );
    eprintln!("  Specific: waz generate {} --rollback <N>", tool);
}

/// Show diff between current schema and a specific versioned backup.
pub fn show_schema_diff(tool: &str, version: u32) {
    let current_path = schemas_dir().join(format!("{}.json", tool));
    let version_path = versions_dir(tool).join(format!("v{}.json", version));

    let current = match std::fs::read_to_string(&current_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let backup = match std::fs::read_to_string(&version_path) {
        Ok(b) => b,
        Err(_) => return,
    };

    if current == backup {
        eprintln!("\n✅ Schema is identical to v{}.", version);
        return;
    }

    // Parse both to compare at command level
    let old_cmds: Vec<CommandEntry> = serde_json::from_str(&backup).unwrap_or_default();
    let new_cmds: Vec<CommandEntry> = serde_json::from_str(&current).unwrap_or_default();

    let old_names: std::collections::HashSet<String> =
        old_cmds.iter().map(|c| c.command.clone()).collect();
    let new_names: std::collections::HashSet<String> =
        new_cmds.iter().map(|c| c.command.clone()).collect();

    eprintln!(
        "\n📊 Diff: v{} ({} cmds) → current ({} cmds):",
        version,
        old_cmds.len(),
        new_cmds.len()
    );
    eprintln!("─────────────────────────────────────────");

    // Added commands
    let added: Vec<&String> = new_names.difference(&old_names).collect();
    for cmd in &added {
        eprintln!("  \x1b[32m+ {}\x1b[0m", cmd);
    }

    // Removed commands
    let removed: Vec<&String> = old_names.difference(&new_names).collect();
    for cmd in &removed {
        eprintln!("  \x1b[31m- {}\x1b[0m", cmd);
    }

    // Changed commands (same name, different tokens)
    let common: Vec<&String> = new_names.intersection(&old_names).collect();
    for cmd_name in &common {
        let old_cmd = old_cmds.iter().find(|c| &c.command == *cmd_name).unwrap();
        let new_cmd = new_cmds.iter().find(|c| &c.command == *cmd_name).unwrap();

        let old_token_names: Vec<&str> = old_cmd.tokens.iter().map(|t| t.name.as_str()).collect();
        let new_token_names: Vec<&str> = new_cmd.tokens.iter().map(|t| t.name.as_str()).collect();

        if old_token_names != new_token_names || old_cmd.description != new_cmd.description {
            eprintln!("  \x1b[33m~ {}\x1b[0m", cmd_name);
            let old_set: std::collections::HashSet<&str> =
                old_token_names.iter().copied().collect();
            let new_set: std::collections::HashSet<&str> =
                new_token_names.iter().copied().collect();
            for tok in new_set.difference(&old_set) {
                eprintln!("    \x1b[32m+ token: {}\x1b[0m", tok);
            }
            for tok in old_set.difference(&new_set) {
                eprintln!("    \x1b[31m- token: {}\x1b[0m", tok);
            }
        }
    }

    if added.is_empty() && removed.is_empty() {
        let mut any_changed = false;
        for cmd_name in &common {
            let old_json =
                serde_json::to_string(old_cmds.iter().find(|c| &c.command == *cmd_name).unwrap())
                    .unwrap_or_default();
            let new_json =
                serde_json::to_string(new_cmds.iter().find(|c| &c.command == *cmd_name).unwrap())
                    .unwrap_or_default();
            if old_json != new_json {
                any_changed = true;
                break;
            }
        }
        if !any_changed {
            eprintln!("  (no structural changes)");
        }
    }

    eprintln!("─────────────────────────────────────────");
    eprintln!("  Use --rollback {} to restore v{}.", version, version);
}

// ──────────────────────────── Export Built-in Schemas ────────────────────────────

/// Install a curated schema into the user schemas dir (overwrites that tool's file).
pub fn export_builtin_schema(tool: &str, _cwd: &str) -> Result<PathBuf, String> {
    let filename = format!("{}.json", tool);
    let body = curated_schema(&filename).ok_or_else(|| {
        let names: Vec<_> = CURATED_SCHEMAS
            .iter()
            .map(|(name, _)| name.trim_end_matches(".json"))
            .collect();
        format!(
            "'{}' is not a built-in schema. Built-in schemas: {}",
            tool,
            names.join(", ")
        )
    })?;
    let schema_path = schemas_dir().join(&filename);
    std::fs::write(&schema_path, body).map_err(|e| format!("Failed to write: {}", e))?;
    Ok(schema_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RuntimeContext;
    use crate::tui::app::SchemaMeta;

    #[test]
    fn test_extract_subcommands() {
        let help = r#"
Usage: brew <command> [options]

Commands:
  install       Install a formula or cask
  uninstall     Uninstall a formula or cask
  search        Search for formulae and casks
  list          List installed formulae and casks
  update        Update Homebrew
  upgrade       Upgrade outdated formulae and casks
  info          Show information about a formula or cask
  help          Show help

Options:
  --version     Show version
"#;
        let subs = extract_subcommands(help, "brew");
        assert!(subs.contains(&"install".to_string()));
        assert!(subs.contains(&"search".to_string()));
        assert!(subs.contains(&"upgrade".to_string()));
        assert!(!subs.contains(&"help".to_string()));
    }

    #[test]
    fn test_parse_schema_response() {
        let response = r#"```json
[
  {
    "command": "brew install",
    "description": "Install a formula or cask",
    "group": "brew",
    "tokens": [
      {
        "name": "formula",
        "description": "Formula or cask to install",
        "required": true,
        "token_type": "String",
        "default": null,
        "values": null,
        "flag": null,
        "data_source": null
      }
    ]
  }
]
```"#;
        let commands = parse_schema_response("brew", response).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command, "brew install");
        assert_eq!(commands[0].tokens[0].name, "formula");
    }

    #[test]
    fn test_resolve_waz_context_file_path() {
        let context = RuntimeContext {
            context_version: 1,
            cwd: "/tmp/work".to_string(),
            project_root: Some("/tmp/work".to_string()),
            file_path: Some("/tmp/work/power.rs".to_string()),
            line: Some(7),
            build_system: "cargo".to_string(),
            file_kind: "single_file_script".to_string(),
            runnable_kind: Some("single_file_script".to_string()),
            package_name: None,
            bins: vec![],
            examples: vec![],
            tests: vec![],
            benches: vec![],
            features: vec![],
            profiles: vec![],
            script_engine: Some("rust-script".to_string()),
            recommended_target: Some("/tmp/work/power.rs".to_string()),
        };

        let values = resolve_builtin("waz:context:file_path", "/tmp/work", Some(&context)).unwrap();
        assert_eq!(values, vec!["/tmp/work/power.rs".to_string()]);
    }

    #[test]
    fn test_should_load_schema_requires_file_kind() {
        let meta = SchemaMeta {
            tool: "rust-script".to_string(),
            version: 1,
            generated_by: "human".to_string(),
            generated_with: None,
            verified: true,
            verified_at: None,
            coverage: "full".to_string(),
            waz_version: Some("0.1.6".to_string()),
            requires_file: None,
            requires_file_kind: Some("single_file_script".to_string()),
            requires_binary: None,
            keywords: vec![],
        };

        let script_context = RuntimeContext {
            file_kind: "single_file_script".to_string(),
            ..RuntimeContext::default()
        };
        let cargo_context = RuntimeContext {
            file_kind: "cargo_project".to_string(),
            ..RuntimeContext::default()
        };

        assert!(should_load_schema(&meta, "/tmp", Some(&script_context)));
        assert!(!should_load_schema(&meta, "/tmp", Some(&cargo_context)));
        assert!(!should_load_schema(&meta, "/tmp", None));
    }

    #[test]
    fn test_schemas_dir() {
        let dir = schemas_dir();
        assert!(dir.to_str().unwrap().contains("waz"));
        assert!(dir.to_str().unwrap().contains("schemas"));
    }

    #[test]
    fn curated_schemas_all_deserialize() {
        for (name, body) in CURATED_SCHEMAS {
            let parsed: SchemaFile = serde_json::from_str(body)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
            assert!(
                !parsed.commands.is_empty(),
                "{name} should contain commands"
            );
        }
        assert_eq!(CURATED_SCHEMAS.len(), 9);
    }

    #[test]
    fn unknown_resolver_keeps_the_command() {
        let mut entry = CommandEntry {
            command: "git add".to_string(),
            description: "Stage files".to_string(),
            group: "git".to_string(),
            verified: true,
            tokens: vec![crate::tui::app::TokenDef {
                name: "path".to_string(),
                description: "files".to_string(),
                required: true,
                token_type: TokenType::File,
                default: Some(".".to_string()),
                values: None,
                flag: None,
                data_source: Some(DataSource {
                    command: None,
                    resolver: Some("nope:missing".to_string()),
                    parse: "lines".to_string(),
                }),
            }],
        };
        resolve_data_sources(&mut entry, "/tmp", None);
        assert_eq!(entry.command, "git add");
        assert_eq!(entry.tokens[0].token_type, TokenType::File);
        assert!(entry.tokens[0].values.is_none());
    }

    #[test]
    fn share_strips_resolver_values_and_keeps_data_source() {
        let mut schema: SchemaFile =
            serde_json::from_str(curated_schema("git.json").unwrap()).unwrap();
        schema.commands[1].tokens[0].values = Some(vec!["src/main.rs".into()]);
        strip_runtime_values(&mut schema);
        let add = schema
            .commands
            .iter()
            .find(|c| c.command == "git add")
            .unwrap();
        assert!(add.tokens[0].values.is_none());
        assert!(add.tokens[0].data_source.is_some());
    }

    #[test]
    fn parse_git_status_porcelain_paths() {
        let porcelain = "\
M  staged.rs
 M unstaged.rs
?? new.rs
R  old.rs -> renamed.rs
";
        let all = parse_git_status_porcelain(porcelain, None);
        assert_eq!(
            all,
            vec!["staged.rs", "unstaged.rs", "new.rs", "renamed.rs"]
        );
        let staged = parse_git_status_porcelain(porcelain, Some("staged"));
        assert_eq!(staged, vec!["staged.rs", "renamed.rs"]);
        let unstaged = parse_git_status_porcelain(porcelain, Some("unstaged"));
        assert_eq!(unstaged, vec!["unstaged.rs", "new.rs"]);
    }

    #[test]
    fn binary_on_path_finds_sh_or_self() {
        assert!(binary_on_path("sh") || binary_on_path("bash") || !cfg!(unix));
    }
}
