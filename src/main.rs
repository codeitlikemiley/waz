mod ask;
mod config;
mod context;
mod db;
pub mod generate;
pub mod hint;
mod import;
mod llm;
mod mcp;
mod normalize;
mod oauth;
mod plugin;
mod predict;
mod resolve;
mod run;
mod runnables;
mod session;
mod tmp;
pub mod tui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use db::HistoryDb;
use predict::PredictionEngine;

/// Waz — Warp-style command prediction for any terminal.
#[derive(Parser)]
#[command(name = "waz", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Record a command execution (called by shell hook after each command).
    Record {
        /// The command that was executed.
        #[arg(required = true)]
        command: Vec<String>,

        /// Current working directory.
        #[arg(long, env = "PWD")]
        cwd: String,

        /// Session ID (set by shell integration).
        #[arg(long, env = "WAZ_SESSION_ID")]
        session: Option<String>,

        /// Exit code of the command.
        #[arg(long, default_value = "0")]
        exit_code: i32,
    },

    /// Get a predicted next command.
    Predict {
        /// Current working directory.
        #[arg(long, env = "PWD")]
        cwd: String,

        /// What the user has typed so far.
        #[arg(long)]
        prefix: Option<String>,

        /// Session ID.
        #[arg(long, env = "WAZ_SESSION_ID")]
        session: Option<String>,

        /// Output format: "plain" (default) or "json".
        #[arg(long, default_value = "plain")]
        format: String,

        /// Skip LLM tier for fast interactive predictions.
        #[arg(long)]
        fast: bool,
    },

    /// Import existing shell history into the waz database.
    Import {
        /// Shell to import from (zsh, bash, fish, powershell). Auto-detects if omitted.
        #[arg(long)]
        shell: Option<String>,
    },

    /// Print shell integration script to stdout.
    Init {
        /// Shell to generate integration for (zsh, bash, fish, powershell).
        shell: String,
    },

    /// Generate a new session ID (used by shell integration).
    SessionId,

    /// Show database statistics.
    Stats,

    /// Ask a natural language question (used by command_not_found_handler).
    Ask {
        /// The natural language query.
        #[arg(required = true)]
        query: Vec<String>,

        /// Current working directory.
        #[arg(long, env = "PWD")]
        cwd: String,

        /// Session ID.
        #[arg(long, env = "WAZ_SESSION_ID")]
        session: Option<String>,

        /// Output structured JSON instead of text.
        #[arg(long)]
        json: bool,

        /// Pin an LLM provider (e.g. grok, anthropic, chatgpt). Default: fallback order.
        #[arg(long)]
        provider: Option<String>,
    },

    /// Check if input looks like natural language (returns exit code 0 if yes).
    CheckNl {
        /// The input text to check.
        #[arg(required = true)]
        input: Vec<String>,
    },

    /// Complete a partial natural language sentence (for ghost text autocompletion).
    Complete {
        /// The partial text to complete.
        #[arg(required = true)]
        text: Vec<String>,
    },

    /// Clear command history. Defaults to current directory only.
    Clear {
        /// Clear ALL history across all directories.
        #[arg(long)]
        all: bool,

        /// Directory to clear (defaults to current directory).
        #[arg(long, env = "PWD")]
        cwd: String,
    },

    /// Launch interactive TUI command palette.
    Tui {
        /// Pre-fill query (enters AI mode).
        #[arg(long)]
        query: Option<String>,

        /// Working directory.
        #[arg(long, env = "PWD")]
        cwd: String,

        /// Current file to seed TMP context with.
        #[arg(long)]
        file: Option<String>,

        /// Current line number within the file.
        #[arg(long)]
        line: Option<usize>,

        /// File to write the selected command to (used by ZLE widget).
        #[arg(long)]
        result_file: Option<String>,

        /// Self mode: show only waz commands for self-configuration.
        #[arg(long = "self")]
        self_mode: bool,
    },

    /// Run the best command for a file:line context directly.
    Run {
        /// File path, optionally with a line number suffix like src/main.rs:42.
        /// Defaults to the current workspace entry point when omitted.
        file: Option<String>,

        /// Print the resolved command instead of executing it.
        #[arg(long)]
        dry_run: bool,
    },

    /// List runnables for a file, module path, or current workspace.
    Runnables {
        /// File path, module path, or workspace target to inspect.
        target: Option<String>,
    },

    /// Parse command output for suggested follow-up commands.
    Hint {
        /// The command output to parse (last N lines of stdout/stderr).
        #[arg(long)]
        output: String,
    },

    /// Generate a TMP schema for a CLI tool using AI (background by default).
    Generate {
        /// Name of the CLI tool (e.g. brew, kubectl, docker).
        #[arg(required_unless_present = "jobs")]
        tool: Option<String>,

        /// Force regeneration even if schema exists.
        #[arg(long)]
        force: bool,

        /// Export built-in schema (cargo/git/npm) to JSON baseline.
        #[arg(long)]
        export: bool,

        /// Rollback to a previous version. Omit number for previous, or specify version (e.g. --rollback 2).
        #[arg(long)]
        rollback: Option<Option<u32>>,

        /// Show version history for this tool's schema.
        #[arg(long)]
        history: bool,

        /// Override the AI model for generation (e.g. gemini-2.5-pro-preview-05-06).
        #[arg(long)]
        model: Option<String>,

        /// Override the LLM provider (e.g. gemini, glm, qwen, minimax, openai, ollama).
        #[arg(long)]
        provider: Option<String>,

        /// Initialize curated schemas (copy built-in schemas to user config).
        #[arg(long)]
        init: bool,

        /// Launch verification TUI to review and approve schema commands.
        #[arg(long)]
        verify: bool,

        /// Run in the foreground (blocks the terminal until the schema is written).
        #[arg(long)]
        wait: bool,

        /// List background generate jobs.
        #[arg(long)]
        jobs: bool,
    },

    /// Agent Plugins: list bundled/user plugins (skills + MCP).
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },

    /// MCP stdio server for Codex, Claude Code, and other Agent Plugins clients.
    Mcp,

    /// Manage TMP schemas (list, share, import).
    Schema {
        #[command(subcommand)]
        action: SchemaAction,
    },

    /// Headless TMP: list, inspect, and build commands as JSON (for agents and CI).
    Tmp {
        #[command(subcommand)]
        action: TmpAction,
    },

    /// Report install health as JSON (for agents and CI).
    Doctor {
        /// Working directory used to count loaded schemas.
        #[arg(long, env = "PWD")]
        cwd: String,
    },

    /// Log in with a subscription (grok, anthropic/claude, chatgpt/codex).
    Login {
        /// Provider: grok, anthropic, or chatgpt/codex.
        #[arg(default_value = "grok")]
        provider: String,
        /// Device-code flow for SSH / VPS / no local browser callback.
        #[arg(long)]
        device: bool,
        /// Force the browser PKCE loopback (127.0.0.1:56121), even over SSH.
        #[arg(long)]
        browser: bool,
        /// Do not import ~/.grok/auth.json even if the Grok CLI is already signed in.
        #[arg(long)]
        no_import: bool,
        /// Start a new login even if stored tokens are still valid.
        #[arg(long)]
        force: bool,
        /// Print login status as JSON and exit.
        #[arg(long)]
        status: bool,
        /// After login, pin this provider (`llm.strategy=single`, `llm.default=<provider>`).
        #[arg(long)]
        default: bool,
    },

    /// Get or set waz config (llm.strategy, llm.default, …).
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
        /// JSON output (for agents).
        #[arg(long)]
        json: bool,
    },

    /// Remove stored OAuth credentials.
    Logout {
        /// Provider: grok, anthropic, or chatgpt/codex.
        #[arg(default_value = "grok")]
        provider: String,
    },

    /// AI + TMP: resolve natural language to a grounded command using schemas.
    Resolve {
        /// Natural language query (e.g. "run the backend package").
        #[arg(required = true)]
        query: Vec<String>,

        /// Working directory (for data source resolution).
        #[arg(long, env = "PWD")]
        cwd: String,

        /// Limit resolution to a specific tool's schema.
        #[arg(long)]
        tool: Option<String>,

        /// Output structured JSON (for AI agent consumption).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SchemaAction {
    /// List all installed schemas with version/status.
    List,

    /// Export a schema as a shareable file (written to CWD).
    Share {
        /// Tool name to export (e.g. cargo, git, brew).
        tool: String,
    },

    /// Import a schema from a local file or URL.
    Import {
        /// Path to .json file or URL (https://).
        source: String,
    },

    /// Set custom trigger keywords for AI query matching.
    Keywords {
        /// Tool name (e.g. psql, cargo, brew).
        tool: String,
        /// Keywords to set (e.g. postgres postgresql database db).
        /// If empty, shows current keywords.
        #[arg(trailing_var_arg = true)]
        words: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List discovered plugins.
    List,
    /// Install the bundled waz plugin into the user plugins directory.
    Install,
    /// Print Codex / Claude / Gemini / Cursor MCP install snippets.
    Doctor,
    /// Write MCP config into an agent client (gemini, claude, codex, cursor, all).
    Connect { client: String },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print one key (e.g. llm.default).
    Get { key: String },
    /// Set a key. Provider defaults must already be set up (login or API key).
    Set {
        key: String,
        #[arg(required = true)]
        value: Vec<String>,
    },
    /// Pin a provider: strategy=single and llm.default=<provider>.
    Use { provider: String },
}

#[derive(Subcommand)]
enum TmpAction {
    /// List TMP commands available in this directory.
    List {
        #[arg(long, env = "PWD")]
        cwd: String,
        /// Substring filter on command or group.
        #[arg(long)]
        query: Option<String>,
    },
    /// Show one command and its resolved token values.
    Show {
        /// Exact schema command, e.g. "cargo run".
        command: String,
        #[arg(long, env = "PWD")]
        cwd: String,
    },
    /// Fill tokens and print the argv string.
    Build {
        /// Exact schema command, e.g. "cargo run".
        command: String,
        /// Token assignments, e.g. --set bin=waz --set release=true
        #[arg(long = "set", value_name = "NAME=VALUE")]
        set: Vec<String>,
        #[arg(long, env = "PWD")]
        cwd: String,
    },
}

fn get_db_path() -> PathBuf {
    let data_dir =
        dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap().join(".local").join("share"));
    data_dir.join("waz").join("history.db")
}

fn open_db() -> HistoryDb {
    match HistoryDb::open(&get_db_path()) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Failed to open database: {}", e);
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Record {
            command,
            cwd,
            session,
            exit_code,
        } => {
            let db = open_db();
            let session_id = session.unwrap_or_else(session::get_session_id);
            let cmd_str = command.join(" ");
            if cmd_str.is_empty() {
                return;
            }
            if let Err(e) = db.insert_command(&cmd_str, &cwd, &session_id, exit_code) {
                eprintln!("Failed to record command: {}", e);
                std::process::exit(1);
            }
        }

        Commands::Predict {
            cwd,
            prefix,
            session,
            format,
            fast,
        } => {
            let db = open_db();
            let session_id = session.unwrap_or_else(session::get_session_id);
            let engine = if fast {
                PredictionEngine::new_fast(&db)
            } else {
                PredictionEngine::new(&db)
            };

            match engine.predict(&session_id, &cwd, prefix.as_deref(), fast) {
                Some(pred) => {
                    if format == "json" {
                        println!(
                            "{}",
                            serde_json::json!({
                                "command": pred.command,
                                "confidence": pred.confidence,
                                "tier": pred.tier.to_string(),
                            })
                        );
                    } else {
                        print!("{}", pred.command);
                    }
                }
                None => {
                    if format == "json" {
                        println!("{}", serde_json::json!(null));
                    }
                    // In plain mode, output nothing on no prediction.
                }
            }
        }

        Commands::Import { shell } => {
            let db = open_db();
            eprintln!("Importing shell history...");
            match import::import_history(&db, shell.as_deref()) {
                Ok(result) => {
                    eprintln!("Done! {}", result);
                }
                Err(e) => {
                    eprintln!("Error importing history: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Init { shell } => {
            let script = match shell.as_str() {
                "zsh" => include_str!("../shell/waz.zsh"),
                "bash" => include_str!("../shell/waz.bash"),
                "fish" => include_str!("../shell/waz.fish"),
                "powershell" | "pwsh" | "ps1" => include_str!("../shell/waz.ps1"),
                _ => {
                    eprintln!(
                        "Unsupported shell: {}. Supported: zsh, bash, fish, powershell",
                        shell
                    );
                    std::process::exit(1);
                }
            };
            print!("{}", script);
        }

        Commands::SessionId => {
            println!("{}", session::new_session_id());
        }

        Commands::Stats => {
            let db = open_db();
            let count = db.command_count().unwrap_or(0);
            let db_path = get_db_path();
            let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

            eprintln!("Waz Database Statistics");
            eprintln!("─────────────────────────");
            eprintln!("  Database path: {}", db_path.display());
            eprintln!("  Database size: {:.1} KB", size as f64 / 1024.0);
            eprintln!("  Total commands: {}", count);
        }

        Commands::Ask {
            query,
            cwd,
            session,
            json,
            provider,
        } => {
            let query_str = query.join(" ");
            if query_str.is_empty() {
                eprintln!("No query provided.");
                std::process::exit(1);
            }

            let config = config::Config::load();
            let db = open_db();
            let session_id = session.unwrap_or_else(session::get_session_id);
            let recent = db.get_session_commands(&session_id).unwrap_or_default();
            let provider = provider.as_deref();

            if json {
                // Structured JSON mode for interactive resolver
                match ask::ask_structured_on(&config, &query_str, &cwd, &recent, provider) {
                    Some(resp) => {
                        println!("{}", serde_json::to_string(&resp).unwrap());
                    }
                    None => {
                        eprintln!("No LLM provider configured.");
                        std::process::exit(1);
                    }
                }
            } else {
                // Legacy text mode
                match ask::ask_on(&config, &query_str, &cwd, &recent, provider) {
                    Some(result) => {
                        eprintln!("using {} / {}", result.provider, result.model);
                        println!("{}", result.response);
                        if let Some(cmd) = &result.suggested_command {
                            println!("\n__WAZ_CMD__:{}", cmd);
                        }
                    }
                    None => {
                        eprintln!("No LLM provider configured. Set an API key, run `waz login grok`, or configure ~/.config/waz/config.toml");
                        std::process::exit(1);
                    }
                }
            }
        }

        Commands::CheckNl { input } => {
            let text = input.join(" ");
            if ask::is_natural_language(&text) {
                std::process::exit(0);
            } else {
                std::process::exit(1);
            }
        }

        Commands::Complete { text } => {
            let partial = text.join(" ");
            if partial.is_empty() {
                std::process::exit(1);
            }
            let config = config::Config::load();
            match ask::complete_sentence(&config, &partial) {
                Some(completion) => print!("{}", completion),
                None => std::process::exit(1),
            }
        }

        Commands::Clear { all, cwd } => {
            let db = open_db();

            if all {
                let total = db.command_count().unwrap_or(0);
                let deleted = db.clear_all().unwrap_or(0);
                eprintln!("🗑  Cleared all history ({} commands deleted)", deleted);
                if total != deleted as i64 {
                    eprintln!("  (had {} total)", total);
                }
            } else {
                let deleted = db.clear_by_cwd(&cwd).unwrap_or(0);
                eprintln!("🗑  Cleared history for {}", cwd);
                eprintln!("  {} commands deleted", deleted);
                let remaining = db.command_count().unwrap_or(0);
                eprintln!("  {} commands remaining (other directories)", remaining);
            }
        }

        Commands::Tui {
            query,
            cwd,
            file,
            line,
            result_file,
            self_mode,
        } => {
            match tui::launch(cwd, file, line, query, self_mode) {
                Ok(Some(cmd)) => {
                    if let Some(ref path) = result_file {
                        // ZLE widget mode — write to temp file
                        std::fs::write(path, &cmd).ok();
                    } else {
                        use std::io::IsTerminal;
                        if std::io::stdout().is_terminal() {
                            // Manual invocation — execute directly
                            eprintln!("\x1b[0;32m→ {}\x1b[0m", cmd);
                            let status = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(&cmd)
                                .status();
                            match status {
                                Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                                Err(e) => {
                                    eprintln!("Failed to execute: {}", e);
                                    std::process::exit(1);
                                }
                            }
                        } else {
                            // Captured by some other mechanism
                            println!("{}", cmd);
                        }
                    }
                }
                Ok(None) => {
                    // User cancelled
                }
                Err(e) => {
                    eprintln!("TUI error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Run { file, dry_run } => match run::run_file(file.as_deref(), dry_run) {
            Ok(status) => {
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        },

        Commands::Runnables { target } => match runnables::run_runnables(target.as_deref()) {
            Ok(status) => {
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        },

        Commands::Hint { output } => {
            if let Some(cmd) = hint::extract_hint(&output) {
                hint::save_hint(&cmd);
            }
        }

        Commands::Generate {
            tool,
            force,
            export,
            rollback,
            history,
            model,
            provider,
            init,
            verify,
            wait,
            jobs,
        } => {
            if jobs {
                let jobs = generate::list_jobs();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&jobs).unwrap_or_else(|_| "[]".into())
                );
                return;
            }

            let tool = match tool {
                Some(t) => t,
                None => {
                    eprintln!(
                        "Specify a tool, e.g. `waz generate docker`, or `waz generate --jobs`."
                    );
                    std::process::exit(1);
                }
            };

            // Handle --verify (launch verification TUI)
            if verify {
                if let Err(e) = tui::verify::launch(&tool) {
                    eprintln!("❌ Verification failed: {}", e);
                    std::process::exit(1);
                }
                return;
            }

            // Handle --init (copy curated schemas to user config)
            if init {
                match generate::init_schemas() {
                    Ok(installed) => {
                        if installed.is_empty() {
                            eprintln!("✅ All curated schemas already installed.");
                        } else {
                            eprintln!("✅ Installed curated schemas: {}", installed.join(", "));
                        }
                    }
                    Err(e) => {
                        eprintln!("❌ Init failed: {}", e);
                        std::process::exit(1);
                    }
                }
                return;
            }

            // Handle --history
            if history {
                generate::show_version_history(&tool);
                return;
            }

            // Handle --rollback (Some(None) = no version specified, Some(Some(n)) = specific version)
            if let Some(version) = rollback {
                match generate::rollback_schema(&tool, version) {
                    Ok(v) => eprintln!("✅ Rolled back '{}' to v{}.", tool, v),
                    Err(e) => {
                        eprintln!("❌ Rollback failed: {}", e);
                        std::process::exit(1);
                    }
                }
                return;
            }

            // Handle --export (dump built-in schemas to JSON)
            if export {
                let cwd = std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                match generate::export_builtin_schema(&tool, &cwd) {
                    Ok(path) => eprintln!("✅ Exported '{}' schema to {}", tool, path.display()),
                    Err(e) => {
                        eprintln!("❌ Export failed: {}", e);
                        std::process::exit(1);
                    }
                }
                return;
            }

            // Normal generate flow
            if !force && generate::schema_exists(&tool) {
                eprintln!(
                    "Schema for '{}' already exists at {:?}",
                    tool,
                    generate::schemas_dir().join(format!("{}.json", tool))
                );
                eprintln!("Use --force to regenerate, --history to see versions, or --rollback to restore.");
                std::process::exit(0);
            }

            // Version-save existing schema before overwrite
            let prev_version = if force && generate::schema_exists(&tool) {
                generate::version_save(&tool).ok()
            } else {
                None
            };

            let config = config::Config::load();

            // Merge CLI flags with [generate] config: CLI > config > defaults
            let effective_model = model.as_deref().or(config.generate.model.as_deref());
            let effective_provider = provider.as_deref().or(config.generate.provider.as_deref());

            if !wait {
                match generate::start_generate(
                    &tool,
                    force,
                    false,
                    effective_model,
                    effective_provider,
                ) {
                    Ok(info) => {
                        println!("{}", serde_json::to_string_pretty(&info).unwrap());
                        eprintln!(
                            "Started background generate for '{tool}'. Your shell is free — `waz generate --jobs` to check."
                        );
                    }
                    Err(e) => {
                        eprintln!("❌ {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            match generate::generate_schema(&config, &tool, effective_model, effective_provider) {
                Ok(commands) => {
                    eprintln!("\n🎉 Generated {} commands for '{}'", commands.len(), tool);

                    // Show diff against previous version
                    if let Some(v) = prev_version {
                        generate::show_schema_diff(&tool, v);
                    }
                }
                Err(e) => {
                    eprintln!("❌ Failed to generate schema: {}", e);
                    // Restore from versioned backup if generation failed
                    if let Some(v) = prev_version {
                        if generate::rollback_schema(&tool, Some(v)).is_ok() {
                            eprintln!("↩️  Restored previous schema (v{}).", v);
                        }
                    }
                    std::process::exit(1);
                }
            }
        }

        Commands::Schema { action } => {
            match action {
                SchemaAction::List => {
                    generate::list_schemas();
                }
                SchemaAction::Share { tool } => match generate::share_schema(&tool) {
                    Ok(path) => eprintln!("✅ Exported shareable schema to {}", path.display()),
                    Err(e) => {
                        eprintln!("❌ Share failed: {}", e);
                        std::process::exit(1);
                    }
                },
                SchemaAction::Import { source } => match generate::import_schema(&source) {
                    Ok(tool) => {
                        eprintln!("✅ Imported schema for '{}'", tool);
                        eprintln!("   Run `waz generate {} --verify` to review.", tool);
                    }
                    Err(e) => {
                        eprintln!("❌ Import failed: {}", e);
                        std::process::exit(1);
                    }
                },
                SchemaAction::Keywords { tool, words } => {
                    let path = generate::schemas_dir().join(format!("{}.json", tool));
                    if !path.exists() {
                        eprintln!(
                            "❌ No schema for '{}'. Run `waz generate {}` first.",
                            tool, tool
                        );
                        std::process::exit(1);
                    }
                    let content = std::fs::read_to_string(&path).expect("read schema");
                    let mut schema: tui::app::SchemaFile =
                        serde_json::from_str(&content).expect("parse schema");

                    if words.is_empty() {
                        // Show current keywords
                        if schema.meta.keywords.is_empty() {
                            eprintln!("📝 No keywords set for '{}'.", tool);
                            eprintln!(
                                "   Usage: waz schema keywords {} postgres postgresql database db",
                                tool
                            );
                        } else {
                            eprintln!(
                                "🔑 Keywords for '{}': {}",
                                tool,
                                schema.meta.keywords.join(", ")
                            );
                        }
                    } else {
                        schema.meta.keywords = words.clone();
                        let json = serde_json::to_string_pretty(&schema).expect("serialize");
                        std::fs::write(&path, json).expect("write schema");
                        eprintln!("✅ Set keywords for '{}': {}", tool, words.join(", "));
                        eprintln!("   AI mode will now match queries containing these words to the {} schema.", tool);
                    }
                }
            }
        }

        Commands::Resolve {
            query,
            cwd,
            tool,
            json,
        } => {
            let config = config::Config::load();
            let query_str = query.join(" ");
            let tool_ref = tool.as_deref();

            match resolve::resolve(&config, &query_str, &cwd, tool_ref) {
                Ok(result) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&result).unwrap());
                    } else {
                        // Human-readable output
                        eprintln!("🎯 {}", result.explanation);
                        println!("{}", result.command);
                        if !result.tokens_filled.is_empty() {
                            eprintln!();
                            for tf in &result.tokens_filled {
                                eprintln!("   {} = {} ({})", tf.name, tf.value, tf.source);
                            }
                        }
                        eprintln!("   confidence: {}", result.confidence);
                    }
                }
                Err(e) => {
                    eprintln!("❌ {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Tmp { action } => match action {
            TmpAction::List { cwd, query } => {
                let listed = tmp::list(&cwd, query.as_deref());
                println!("{}", serde_json::to_string_pretty(&listed).unwrap());
            }
            TmpAction::Show { command, cwd } => match tmp::show(&cwd, &command) {
                Ok(shown) => println!("{}", serde_json::to_string_pretty(&shown).unwrap()),
                Err(e) => {
                    eprintln!("{}", serde_json::json!({ "error": e }));
                    std::process::exit(1);
                }
            },
            TmpAction::Build { command, set, cwd } => match tmp::build(&cwd, &command, &set) {
                Ok(built) => println!("{}", serde_json::to_string_pretty(&built).unwrap()),
                Err(e) => {
                    eprintln!("{}", serde_json::json!({ "error": e }));
                    std::process::exit(1);
                }
            },
        },

        Commands::Doctor { cwd } => {
            let report = tmp::doctor(&cwd, &get_db_path().display().to_string());
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }

        Commands::Config { action, json } => match action {
            None => {
                let view = config::view();
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&view).unwrap_or_else(|_| "{}".into())
                    );
                } else {
                    println!("{}", config::format_view(&view));
                }
            }
            Some(ConfigAction::Get { key }) => match config::get_value(&key) {
                Ok(v) => println!("{v}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
            Some(ConfigAction::Set { key, value }) => {
                match config::set_value(&key, &value.join(" ")) {
                    Ok(v) => println!("{key} = {v}"),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            Some(ConfigAction::Use { provider }) => match config::use_provider(&provider) {
                Ok(name) => {
                    eprintln!("Pinned {name} (llm.strategy=single, llm.default={name}).");
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&config::view())
                                .unwrap_or_else(|_| "{}".into())
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
        },

        Commands::Login {
            provider,
            device,
            browser,
            no_import,
            force,
            status,
            default,
        } => {
            let name = oauth::canonical_provider(&provider);
            if status {
                if provider == "grok" && name == "grok" {
                    // `waz login --status` with the default provider prints every slot.
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&oauth::status_all())
                            .unwrap_or_else(|_| "{}".into())
                    );
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&oauth::status_for(&name))
                            .unwrap_or_else(|_| "{}".into())
                    );
                }
                return;
            }
            if !oauth::supported_providers().contains(&name.as_str()) {
                eprintln!(
                    "OAuth login supports {}. Got: {provider}",
                    oauth::supported_providers().join(", ")
                );
                std::process::exit(1);
            }
            match oauth::login(
                &name,
                oauth::LoginOptions {
                    device,
                    browser,
                    import_grok_cli: !no_import,
                    force,
                },
            ) {
                Ok(result) => {
                    eprintln!(
                        "Logged in to {name} as {} ({}).",
                        result.identity(),
                        result.source
                    );
                    if let Some(warning) = result.warning {
                        eprintln!("{warning}");
                    }
                    match name.as_str() {
                        "grok" => eprintln!(
                            "waz will use SuperGrok OAuth for grok (model grok-4.6 unless you set one)."
                        ),
                        "anthropic" => eprintln!(
                            "waz will use Claude Pro/Max OAuth for anthropic (Bearer + claude-code beta)."
                        ),
                        "codex" => eprintln!(
                            "waz will use ChatGPT/Codex OAuth via chatgpt.com/backend-api/codex."
                        ),
                        _ => {}
                    }
                    if default {
                        match config::use_provider(&name) {
                            Ok(_) => eprintln!(
                                "Pinned {name} (llm.strategy=single, llm.default={name})."
                            ),
                            Err(e) => eprintln!("Logged in, but could not pin default: {e}"),
                        }
                    } else {
                        eprintln!("To pin this provider: waz config use {name}");
                    }
                }
                Err(e) => {
                    eprintln!("Login failed: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Logout { provider } => match oauth::logout(&provider) {
            Ok(true) => eprintln!("Logged out of {provider}."),
            Ok(false) => eprintln!("No stored {provider} credentials."),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },

        Commands::Plugin { action } => match action {
            PluginAction::List => {
                let plugins = plugin::discover();
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &plugins
                            .iter()
                            .map(|p| serde_json::json!({
                                "name": p.manifest.name,
                                "version": p.manifest.version,
                                "description": p.manifest.description,
                                "source": p.source,
                                "root": p.root,
                                "skills": p.skills.iter().map(|s| serde_json::json!({
                                    "name": s.name,
                                    "description": s.description,
                                })).collect::<Vec<_>>(),
                                "mcp": p.has_mcp,
                            }))
                            .collect::<Vec<_>>()
                    )
                    .unwrap_or_else(|_| "[]".into())
                );
            }
            PluginAction::Install => match plugin::install_bundled() {
                Ok(root) => {
                    eprintln!("Installed Agent Plugin at {}", root.display());
                    eprintln!("Skills: tmp-use (agents), tmp-schema (generate). MCP: `waz mcp`.");
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
            PluginAction::Doctor => {
                let root =
                    plugin::install_bundled().unwrap_or_else(|_| plugin::plugins_dir().join("waz"));
                println!("Agent Plugins package: {}", root.display());
                println!();
                println!("Preferred: waz plugin connect <gemini|claude|codex|cursor|all>");
                println!(
                    "That writes MCP config so the client does not need Agent Plugins support."
                );
                println!();
                println!("Manual snippets:");
                println!("  Gemini CLI  ~/.gemini/settings.json  mcpServers.waz");
                println!("  Claude Code ~/.claude.json           mcpServers.waz");
                println!("  Codex       ~/.codex/config.toml     [mcp_servers.waz]");
                println!("  Cursor      ~/.cursor/mcp.json       mcpServers.waz");
            }
            PluginAction::Connect { client } => match plugin::connect_client(&client) {
                Ok(msg) => {
                    eprintln!("Connected waz MCP:\n{msg}");
                    eprintln!("Restart the agent CLI/app so it picks up the server.");
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            },
        },

        Commands::Mcp => {
            if let Err(e) = mcp::run() {
                eprintln!("MCP server error: {e}");
                std::process::exit(1);
            }
        }
    }
}
