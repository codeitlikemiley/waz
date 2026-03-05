//! AI-powered TMP schema generator.
//!
//! Runs `<tool> --help` recursively, sends output to an LLM, and
//! saves the resulting schema as JSON to `~/.config/waz/schemas/`.

use crate::config::{Config, ProviderDefaults};
use crate::llm;
use crate::tui::app::{CommandEntry, DataSource, SchemaFile, SchemaMeta, TokenType};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Directory where user schemas are stored.
pub fn schemas_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
        .join("waz")
        .join("schemas");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Directory where curated schemas are shipped with the binary.
fn curated_schemas_dir() -> PathBuf {
    // Check if we're running from the repo (development) or installed
    let exe_dir = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    // Try repo-relative path first (for development)
    let repo_schemas = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas").join("curated");
    if repo_schemas.exists() {
        return repo_schemas;
    }

    // Fallback: next to the binary
    if let Some(dir) = exe_dir {
        let bin_schemas = dir.join("schemas").join("curated");
        if bin_schemas.exists() {
            return bin_schemas;
        }
    }

    repo_schemas // Return repo path even if it doesn't exist
}

/// Check if a schema already exists for the given tool.
pub fn schema_exists(tool: &str) -> bool {
    schemas_dir().join(format!("{}.json", tool)).exists()
}

/// Initialize curated schemas — copy from repo/binary to user's config dir.
/// Only copies schemas that don't already exist (won't overwrite user modifications).
pub fn init_schemas() -> Result<Vec<String>, String> {
    let curated_dir = curated_schemas_dir();
    let target_dir = schemas_dir();
    let mut installed = Vec::new();

    let entries = std::fs::read_dir(&curated_dir)
        .map_err(|e| format!("No curated schemas found at {}: {}", curated_dir.display(), e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let target = target_dir.join(&filename);

        if !target.exists() {
            match std::fs::copy(&path, &target) {
                Ok(_) => {
                    let tool = filename.trim_end_matches(".json");
                    installed.push(tool.to_string());
                }
                Err(e) => {
                    eprintln!("  ⚠️  Failed to copy {}: {}", filename, e);
                }
            }
        }
    }

    Ok(installed)
}

/// Load all JSON schemas from the schemas directory.
/// Supports both `SchemaFile` (new) and `Vec<CommandEntry>` (legacy) formats.
/// Filters schemas based on CWD context (requires_file, requires_binary).
pub fn load_all_schemas(cwd: &str) -> Vec<CommandEntry> {
    // Auto-init curated schemas on first load
    if let Ok(installed) = init_schemas() {
        if !installed.is_empty() {
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
                    if !should_load_schema(&schema_file.meta, cwd) {
                        continue;
                    }
                    let mut cmds = schema_file.commands;
                    for entry in &mut cmds {
                        resolve_data_sources(entry, cwd);
                    }
                    commands.extend(cmds);
                }
                // Fallback: legacy Vec<CommandEntry> format
                else if let Ok(mut entries) = serde_json::from_str::<Vec<CommandEntry>>(&content) {
                    for entry in &mut entries {
                        resolve_data_sources(entry, cwd);
                    }
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
fn should_load_schema(meta: &SchemaMeta, cwd: &str) -> bool {
    // Check requires_file (e.g. "Cargo.toml", "package.json")
    if let Some(ref file) = meta.requires_file {
        if !std::path::Path::new(cwd).join(file).exists() {
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
    Command::new("which").arg(cmd).output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve any `data_source` fields in tokens (shell commands or built-in resolvers).
fn resolve_data_sources(entry: &mut CommandEntry, cwd: &str) {
    for token in &mut entry.tokens {
        if let Some(ref ds) = token.data_source {
            let values = if let Some(ref resolver) = ds.resolver {
                // Built-in resolver
                resolve_builtin(resolver, cwd)
            } else if let Some(ref cmd) = ds.command {
                // Shell command
                run_data_source_command(cmd, &ds.parse)
            } else {
                None
            };

            if let Some(vals) = values {
                if !vals.is_empty() {
                    token.values = Some(vals);
                    token.token_type = TokenType::Enum;
                }
            }
        }
    }
}

/// Public wrapper for verification TUI to test data sources.
pub fn resolve_data_sources_pub(entry: &mut CommandEntry, cwd: &str) {
    resolve_data_sources(entry, cwd);
}

/// Run a shell command and parse its output into values.
fn run_data_source_command(cmd: &str, parse: &str) -> Option<Vec<String>> {
    let output = Command::new("sh").args(["-c", cmd]).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let values: Vec<String> = match parse {
        "words" => stdout.split_whitespace().map(|s| s.to_string()).collect(),
        _ => stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    };
    if values.is_empty() { None } else { Some(values) }
}

// ──────────────────────────── Built-in Resolvers ────────────────────────────

/// Resolve a built-in named resolver (e.g. "cargo:bins", "git:branches", "waz:models:gemini").
fn resolve_builtin(resolver: &str, cwd: &str) -> Option<Vec<String>> {
    // Handle parameterized resolvers (e.g. "waz:models:gemini")
    let parts: Vec<&str> = resolver.splitn(3, ':').collect();
    
    match (parts.get(0).copied(), parts.get(1).copied(), parts.get(2).copied()) {
        (Some("cargo"), Some("bins"), _) => cargo_resolve_bins(cwd),
        (Some("cargo"), Some("examples"), _) => cargo_resolve_examples(cwd),
        (Some("cargo"), Some("packages"), _) => cargo_resolve_packages(cwd),
        (Some("cargo"), Some("features"), _) => cargo_resolve_features(cwd),
        (Some("cargo"), Some("profiles"), _) => cargo_resolve_profiles(cwd),
        (Some("cargo"), Some("tests"), _) => cargo_resolve_tests(cwd),
        (Some("cargo"), Some("benches"), _) => cargo_resolve_benches(cwd),
        (Some("git"), Some("branches"), _) => git_resolve_branches(cwd),
        (Some("git"), Some("remotes"), _) => git_resolve_remotes(cwd),
        (Some("npm"), Some("scripts"), _) => npm_resolve_scripts(cwd),
        (Some("waz"), Some("models"), Some(provider)) => waz_resolve_models(provider),
        (Some("waz"), Some("models"), None) => waz_resolve_models("gemini"),
        _ => {
            eprintln!("Warning: unknown resolver '{}'", resolver);
            None
        }
    }
}

/// Fetch available models from an LLM provider's API.
fn waz_resolve_models(provider: &str) -> Option<Vec<String>> {
    let config = crate::config::Config::load();
    
    // Find the provider's API key
    let api_key = config.llm.providers.iter()
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
                    "gpt-4o-mini".into(), "gpt-4o".into(), "gpt-4.1-mini".into(),
                    "gpt-4.1".into(), "o4-mini".into(),
                ])
            }
        }
        "ollama" => fetch_ollama_models(),
        "glm" => Some(vec!["glm-4.7".into(), "glm-4-plus".into(), "glm-4-flash".into()]),
        "qwen" => Some(vec!["qwen3.5-plus".into(), "qwen3.5-turbo".into(), "qwen-plus".into()]),
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
        .output().ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = json.get("models")?.as_array()?;
    let mut names: Vec<String> = models.iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            // "models/gemini-2.5-pro" → "gemini-2.5-pro"
            let short = name.strip_prefix("models/").unwrap_or(name);
            // Only include generateContent-capable models
            let methods = m.get("supportedGenerationMethods")?
                .as_array()?;
            if methods.iter().any(|m| m.as_str() == Some("generateContent")) {
                Some(short.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() { None } else { Some(names) }
}

fn fetch_openai_models(api_key: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "5",
               "-H", &format!("Authorization: Bearer {}", api_key),
               "https://api.openai.com/v1/models"])
        .output().ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let data = json.get("data")?.as_array()?;
    let mut names: Vec<String> = data.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            // Filter to chat models only
            if id.starts_with("gpt-") || id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    if names.is_empty() { None } else { Some(names) }
}

fn fetch_ollama_models() -> Option<Vec<String>> {
    let output = std::process::Command::new("curl")
        .args(["-s", "--max-time", "3", "http://localhost:11434/api/tags"])
        .output().ok()?;
    let body = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let models = json.get("models")?.as_array()?;
    let names: Vec<String> = models.iter()
        .filter_map(|m| m.get("name")?.as_str().map(|s| s.to_string()))
        .collect();
    if names.is_empty() { None } else { Some(names) }
}

/// Cargo: resolve binary targets from Cargo.toml and src/bin/.
fn cargo_resolve_bins(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.bins.is_empty() { None } else { Some(ctx.bins) }
}

fn cargo_resolve_examples(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.examples.is_empty() { None } else { Some(ctx.examples) }
}

fn cargo_resolve_packages(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.packages.is_empty() { None } else { Some(ctx.packages) }
}

fn cargo_resolve_features(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.features.is_empty() { None } else { Some(ctx.features) }
}

fn cargo_resolve_profiles(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    let mut profiles = ctx.profiles;
    // Always include the standard profiles
    for p in &["dev", "release", "test", "bench"] {
        if !profiles.contains(&p.to_string()) {
            profiles.push(p.to_string());
        }
    }
    if profiles.is_empty() { None } else { Some(profiles) }
}

fn cargo_resolve_tests(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.tests.is_empty() { None } else { Some(ctx.tests) }
}

fn cargo_resolve_benches(cwd: &str) -> Option<Vec<String>> {
    let cwd = std::path::Path::new(cwd);
    let ctx = crate::tui::cargo_schema::CargoContext::detect(cwd);
    if ctx.benches.is_empty() { None } else { Some(ctx.benches) }
}

/// Git: resolve branch names.
fn git_resolve_branches(cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
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

/// Git: resolve remote names.
fn git_resolve_remotes(cwd: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["remote"])
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

/// npm/bun: resolve script names from package.json.
fn npm_resolve_scripts(cwd: &str) -> Option<Vec<String>> {
    let pkg_path = std::path::Path::new(cwd).join("package.json");
    let content = std::fs::read_to_string(&pkg_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let names: Vec<String> = scripts.keys().cloned().collect();
    if names.is_empty() { None } else { Some(names) }
}

// ──────────────────────────── Schema Sharing ────────────────────────────

/// Export a schema as a clean shareable file.
/// Strips runtime-resolved values (token.values populated by resolvers),
/// keeping data_source definitions so importers can resolve them locally.
pub fn share_schema(tool: &str) -> Result<std::path::PathBuf, String> {
    let src = schemas_dir().join(format!("{}.json", tool));
    if !src.exists() {
        return Err(format!("No schema found for '{}'. Generate one first.", tool));
    }

    let content = std::fs::read_to_string(&src)
        .map_err(|e| format!("Read: {}", e))?;

    // Try SchemaFile format
    let mut schema: SchemaFile = serde_json::from_str(&content)
        .map_err(|e| format!("Parse: {}", e))?;

    // Strip runtime-resolved values (keep data_source definitions)
    for cmd in &mut schema.commands {
        for tok in &mut cmd.tokens {
            if tok.data_source.is_some() {
                // Clear values that were populated at load time by resolvers
                tok.values = None;
            }
        }
    }

    // Write to CWD for easy sharing
    let filename = format!("{}-schema-v{}.json", tool, schema.meta.version);
    let dest = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(&filename);

    let json = serde_json::to_string_pretty(&schema)
        .map_err(|e| format!("Serialize: {}", e))?;
    std::fs::write(&dest, &json)
        .map_err(|e| format!("Write: {}", e))?;

    Ok(dest)
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
    let schema: SchemaFile = serde_json::from_str(&content)
        .map_err(|e| format!("Invalid schema format: {}", e))?;

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
    std::fs::write(&dest, &content)
        .map_err(|e| format!("Write: {}", e))?;

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
        eprintln!("No schemas installed. Run `waz generate <tool> --init` to install curated schemas.");
        return;
    }

    // Sort by tool name
    schemas.sort_by(|a, b| a.0.cmp(&b.0));

    // Print header
    eprintln!("{:<12} {:<6} {:<10} {:<8} {:<6} {:<10}",
        "Tool", "Ver", "Status", "Cmds", "Source", "Coverage");
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

        eprintln!("{:<12} v{:<4} {:<10} {:<8} {:<6} {}",
            name,
            sf.meta.version,
            status,
            format!("{}/{}", verified_count, total),
            source,
            sf.meta.coverage,
        );
    }

    if legacy_count > 0 {
        eprintln!("\n  + {} legacy schema(s) (pre-SchemaFile format)", legacy_count);
    }

    eprintln!("\n📁 {}", dir.display());
}

/// Generate a TMP schema for a CLI tool using AI.
///
/// 1. Runs `<tool> --help` and subcommand help recursively
/// 2. Sends to LLM with a structured prompt
/// 3. Parses response into Vec<CommandEntry>
/// 4. Saves to ~/.config/waz/schemas/<tool>.json as SchemaFile
pub fn generate_schema(config: &Config, tool: &str, model_override: Option<&str>, provider_override: Option<&str>) -> Result<Vec<CommandEntry>, String> {
    // Step 1: Check tool exists
    let which = Command::new("which").arg(tool).output();
    match which {
        Ok(out) if out.status.success() => {},
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
    let subcommands = extract_subcommands(&main_help);
    let max_subcommands = 20; // Cap to avoid excessive API calls
    for (i, sub) in subcommands.iter().take(max_subcommands).enumerate() {
        eprintln!("   Running: {} {} --help ({}/{})", tool, sub, i + 1, subcommands.len().min(max_subcommands));
        let sub_help = run_help(tool, &[sub.as_str()]);
        if !sub_help.is_empty() {
            help_texts.push(format!("=== {} {} --help ===\n{}", tool, sub, sub_help));
        }
    }

    // Determine model info for display
    let model_name = model_override.map(|s| s.to_string()).unwrap_or_else(|| {
        config.llm.providers.first()
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
        std::fs::read_to_string(&path).ok()
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
            requires_binary: Some(tool.to_string()),
        },
        commands: commands.clone(),
    };

    let schema_path = schemas_dir().join(format!("{}.json", tool));
    let json = serde_json::to_string_pretty(&schema_file)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&schema_path, &json)
        .map_err(|e| format!("Failed to write schema: {}", e))?;

    eprintln!("   Found {} commands with {} tokens",
        commands.len(),
        commands.iter().map(|c| c.tokens.len()).sum::<usize>()
    );
    eprintln!("\n✅ Saved to {} (v{})", schema_path.display(), schema_file.meta.version);
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
fn extract_subcommands(help: &str) -> Vec<String> {
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

            // Extract first word as subcommand name
            let first_word = trimmed.split_whitespace().next().unwrap_or("");
            // Skip help, version, and meta entries
            if !first_word.is_empty()
                && first_word != "help"
                && first_word != "version"
                && !first_word.starts_with('-')
                && !first_word.starts_with('[')
                && first_word.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                subs.push(first_word.to_string());
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
  "command": "{} <subcommand>",
  "description": "Short description",
  "group": "{}",
  "tokens": [
    {{
      "name": "param_name",
      "description": "Short description",
      "required": true/false,
      "token_type": "String" or "Boolean" or "Enum" or "File" or "Number",
      "default": null or "default_value",
      "values": null or ["option1", "option2"],
      "flag": "--flag-name" or null,
      "data_source": null
    }}
  ]
}}

Rules:
- Include 5-15 of the MOST COMMONLY USED subcommands (not all)
- For each subcommand, include 1-4 of the most important flags/options
- Use token_type "Boolean" for flags like --verbose, --force
- Use token_type "Enum" when there are known choices
- Use token_type "File" for file/path arguments
- Use token_type "Number" for numeric values
- Set "flag" to the CLI flag (e.g. "--verbose", "-n")
- Set "flag" to null for positional arguments
- For data_source: if the values can be dynamically resolved (e.g. installed packages),
  set it to {{"command": "shell command to list values", "parse": "lines"}}
- Output ONLY the JSON array, no markdown, no explanation

Help output:
{}

JSON:"#,
        tool, tool, tool, help_text
    )
}

/// Call the LLM to generate schema JSON.
fn call_llm_for_schema(config: &Config, prompt: &str, model_override: Option<&str>, provider_override: Option<&str>) -> Result<String, String> {
    let mut state = llm::load_rotation_state();
    let mut providers: Vec<crate::config::ProviderConfig> = llm::get_ordered_providers_pub(&config.llm)
        .into_iter().cloned().collect();

    if providers.is_empty() {
        return Err("No LLM provider configured. Set GEMINI_API_KEY or configure ~/.config/waz/config.toml".to_string());
    }

    // Filter to specific provider if requested
    if let Some(prov) = provider_override {
        providers.retain(|p| p.name.eq_ignore_ascii_case(prov));
        if providers.is_empty() {
            return Err(format!("Provider '{}' not configured. Add its API key or configure it in config.toml.", prov));
        }
    }

    // Apply model override to the first (or selected) provider
    if let Some(model) = model_override {
        if let Some(p) = providers.first_mut() {
            p.model = model.to_string();
        }
    }

    for provider in &providers {
        if provider.keys.is_empty() {
            continue;
        }

        let key_idx = state.next_key_for(&provider.name, provider.keys.len());
        let key = match provider.keys.get(key_idx) {
            Some(k) => k,
            None => continue,
        };

        let result = match provider.name.as_str() {
            "gemini" => call_gemini_long(provider, key, prompt),
            _ => call_openai_long(provider, key, prompt),
        };

        if let Some(response) = result {
            state.save();
            return Ok(response);
        }
    }

    state.save();
    Err("All LLM providers failed. Check your API keys.".to_string())
}

/// Call Gemini with higher token limit for schema generation.
fn call_gemini_long(
    provider: &crate::config::ProviderConfig,
    key: &str,
    prompt: &str,
) -> Option<String> {
    let base_url = if provider.base_url.is_empty() {
        ProviderDefaults::base_url("gemini").to_string()
    } else {
        provider.base_url.clone()
    };
    let model = if provider.model.is_empty() {
        ProviderDefaults::model("gemini").to_string()
    } else {
        provider.model.clone()
    };

    let url = format!(
        "{}/models/{}:generateContent?key={}",
        base_url, model, key
    );

    let body = json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {
            "temperature": 0.2,
            "maxOutputTokens": 4096,
        }
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
}

/// Call OpenAI-compatible API with higher token limit.
fn call_openai_long(
    provider: &crate::config::ProviderConfig,
    key: &str,
    prompt: &str,
) -> Option<String> {
    let base_url = if provider.base_url.is_empty() {
        ProviderDefaults::base_url(&provider.name).to_string()
    } else {
        provider.base_url.clone()
    };
    let model = if provider.model.is_empty() {
        ProviderDefaults::model(&provider.name).to_string()
    } else {
        provider.model.clone()
    };

    let url = format!("{}/chat/completions", base_url);
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": 4096,
    });

    let resp = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", key))
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .ok()?;

    let json: serde_json::Value = resp.into_json().ok()?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
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
        before_close.strip_suffix("```").unwrap_or(before_close).trim()
    } else {
        trimmed
    };

    let commands: Vec<CommandEntry> = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse AI response as JSON: {}\n\nRaw response:\n{}", e, &response[..response.len().min(500)]))?;

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

    std::fs::copy(&source, &dest)
        .map_err(|e| format!("Failed to save version: {}", e))?;

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
                return Err(format!("No version history for '{}'. Use --history to check.", tool));
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

    std::fs::copy(&source, &target)
        .map_err(|e| format!("Failed to rollback: {}", e))?;

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
            eprintln!("📋 '{}' has a current schema but no version history yet.", tool);
            eprintln!("   Version history starts when you use --force to regenerate.");
        } else {
            eprintln!("📋 No schema or history found for '{}'.", tool);
        }
        return;
    }

    versions.sort_by_key(|(n, _)| *n);

    eprintln!("📋 Version history for '{}' ({} versions):", tool, versions.len());
    eprintln!("─────────────────────────────────────────");

    for (v, path) in &versions {
        let meta = std::fs::metadata(path).ok();
        let modified = meta.as_ref()
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
        let cmd_count = std::fs::read_to_string(path).ok()
            .and_then(|c| serde_json::from_str::<Vec<CommandEntry>>(&c).ok())
            .map(|cmds| format!("{} commands", cmds.len()))
            .unwrap_or_else(|| format!("{} bytes", size));

        let is_latest = *v == versions.last().map(|(n, _)| *n).unwrap_or(0);
        let marker = if is_latest { " ← latest" } else { "" };

        eprintln!("  v{:<4} │ {:<15} │ {}{}", v, modified, cmd_count, marker);
    }

    eprintln!("─────────────────────────────────────────");
    eprintln!("  Rollback: waz generate {} --rollback        (latest)", tool);
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

    eprintln!("\n📊 Diff: v{} ({} cmds) → current ({} cmds):", version, old_cmds.len(), new_cmds.len());
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
            let old_set: std::collections::HashSet<&str> = old_token_names.iter().copied().collect();
            let new_set: std::collections::HashSet<&str> = new_token_names.iter().copied().collect();
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
            let old_json = serde_json::to_string(old_cmds.iter().find(|c| &c.command == *cmd_name).unwrap()).unwrap_or_default();
            let new_json = serde_json::to_string(new_cmds.iter().find(|c| &c.command == *cmd_name).unwrap()).unwrap_or_default();
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

/// Export a built-in schema (cargo/git/npm) to JSON file.
pub fn export_builtin_schema(tool: &str, cwd: &str) -> Result<PathBuf, String> {
    use crate::tui::app::{CommandEntry, TokenDef, TokenType};

    let commands: Vec<CommandEntry> = match tool {
        "cargo" => {
            let cargo_path = std::path::Path::new(cwd).join("Cargo.toml");
            if !cargo_path.exists() {
                return Err("No Cargo.toml found in current directory. Run from a Cargo project.".to_string());
            }
            let ctx = crate::tui::cargo_schema::CargoContext::detect(std::path::Path::new(cwd));
            crate::tui::cargo_schema::build_cargo_commands(&ctx)
        }
        "git" => {
            // Build git commands programmatically (mirror of load_git_commands)
            let branches: Vec<String> = Command::new("git")
                .args(["branch", "--format=%(refname:short)"])
                .current_dir(cwd)
                .output()
                .ok()
                .map(|out| {
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            vec![
                CommandEntry {
                    command: "git status".to_string(),
                    description: "Show working tree status".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![],
                },
                CommandEntry {
                    command: "git add".to_string(),
                    description: "Stage files for commit".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![TokenDef {
                        name: "path".to_string(),
                        description: "File or directory to stage".to_string(),
                        required: true,
                        token_type: TokenType::File,
                        default: Some(".".to_string()),
                        values: None,
                        flag: None,
                        data_source: None,
                    }],
                },
                CommandEntry {
                    command: "git commit".to_string(),
                    description: "Record changes to the repository".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![TokenDef {
                        name: "m".to_string(),
                        description: "Commit message".to_string(),
                        required: true,
                        token_type: TokenType::String,
                        default: None,
                        values: None,
                        flag: None,
                        data_source: None,
                    }],
                },
                CommandEntry {
                    command: "git checkout".to_string(),
                    description: "Switch branches".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![TokenDef {
                        name: "branch".to_string(),
                        description: "Branch to switch to".to_string(),
                        required: true,
                        token_type: if branches.is_empty() { TokenType::String } else { TokenType::Enum },
                        default: None,
                        values: if branches.is_empty() { None } else { Some(branches.clone()) },
                        flag: None,
                        data_source: None,
                    }],
                },
                CommandEntry {
                    command: "git push".to_string(),
                    description: "Push to remote".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![],
                },
                CommandEntry {
                    command: "git pull".to_string(),
                    description: "Pull from remote".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![],
                },
                CommandEntry {
                    command: "git log".to_string(),
                    description: "Show commit logs".to_string(),
                    group: "git".to_string(),
                    verified: false,
                    tokens: vec![
                        TokenDef {
                            name: "n".to_string(),
                            description: "Number of commits to show".to_string(),
                            required: false,
                            token_type: TokenType::Number,
                            default: Some("10".to_string()),
                            values: None,
                            flag: None,
                            data_source: None,
                        },
                        TokenDef {
                            name: "oneline".to_string(),
                            description: "Show in one-line format".to_string(),
                            required: false,
                            token_type: TokenType::Boolean,
                            default: Some("true".to_string()),
                            values: None,
                            flag: None,
                            data_source: None,
                        },
                    ],
                },
            ]
        }
        "npm" => {
            let pkg_path = std::path::Path::new(cwd).join("package.json");
            let scripts: Vec<String> = if let Ok(content) = std::fs::read_to_string(&pkg_path) {
                serde_json::from_str::<serde_json::Value>(&content).ok()
                    .and_then(|v| v.get("scripts")?.as_object().map(|obj| {
                        obj.keys().cloned().collect()
                    }))
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let mut commands = vec![
                CommandEntry {
                    command: "npm install".to_string(),
                    description: "Install dependencies".to_string(),
                    group: "npm".to_string(),
                    verified: false,
                    tokens: vec![],
                },
            ];

            if !scripts.is_empty() {
                commands.push(CommandEntry {
                    command: "npm run".to_string(),
                    description: "Run a script".to_string(),
                    group: "npm".to_string(),
                    verified: false,
                    tokens: vec![TokenDef {
                        name: "script".to_string(),
                        description: "Script to run".to_string(),
                        required: true,
                        token_type: TokenType::Enum,
                        default: None,
                        values: Some(scripts),
                        flag: None,
                        data_source: None,
                    }],
                });
            }

            commands
        }
        _ => return Err(format!("'{}' is not a built-in schema. Built-in schemas: cargo, git, npm", tool)),
    };

    let schema_path = schemas_dir().join(format!("{}.json", tool));
    let json = serde_json::to_string_pretty(&commands)
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&schema_path, &json)
        .map_err(|e| format!("Failed to write: {}", e))?;

    Ok(schema_path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let subs = extract_subcommands(help);
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
    fn test_schemas_dir() {
        let dir = schemas_dir();
        assert!(dir.to_str().unwrap().contains("waz"));
        assert!(dir.to_str().unwrap().contains("schemas"));
    }
}
