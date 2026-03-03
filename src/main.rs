mod ask;
mod config;
mod db;
mod import;
mod llm;
mod predict;
mod session;

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
    },

    /// Import existing shell history into the waz database.
    Import {
        /// Shell to import from (zsh, bash, fish). Auto-detects if omitted.
        #[arg(long)]
        shell: Option<String>,
    },

    /// Print shell integration script to stdout.
    Init {
        /// Shell to generate integration for (zsh, bash, fish).
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
    },

    /// Check if input looks like natural language (returns exit code 0 if yes).
    CheckNl {
        /// The input text to check.
        #[arg(required = true)]
        input: Vec<String>,
    },
}

fn get_db_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local").join("share"));
    data_dir.join("waz").join("history.db")
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
            let db = HistoryDb::open(&get_db_path()).expect("Failed to open database");
            let session_id = session.unwrap_or_else(session::get_session_id);
            let cmd_str = command.join(" ");
            if cmd_str.is_empty() {
                return;
            }
            db.insert_command(&cmd_str, &cwd, &session_id, exit_code)
                .expect("Failed to record command");
        }

        Commands::Predict {
            cwd,
            prefix,
            session,
            format,
        } => {
            let db = HistoryDb::open(&get_db_path()).expect("Failed to open database");
            let session_id = session.unwrap_or_else(session::get_session_id);
            let engine = PredictionEngine::new(&db);

            match engine.predict(&session_id, &cwd, prefix.as_deref()) {
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
            let db = HistoryDb::open(&get_db_path()).expect("Failed to open database");
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
                _ => {
                    eprintln!("Unsupported shell: {}. Supported: zsh, bash, fish", shell);
                    std::process::exit(1);
                }
            };
            print!("{}", script);
        }

        Commands::SessionId => {
            println!("{}", session::new_session_id());
        }

        Commands::Stats => {
            let db = HistoryDb::open(&get_db_path()).expect("Failed to open database");
            let count = db.command_count().unwrap_or(0);
            let db_path = get_db_path();
            let size = std::fs::metadata(&db_path)
                .map(|m| m.len())
                .unwrap_or(0);

            eprintln!("Waz Database Statistics");
            eprintln!("─────────────────────────");
            eprintln!("  Database path: {}", db_path.display());
            eprintln!("  Database size: {:.1} KB", size as f64 / 1024.0);
            eprintln!("  Total commands: {}", count);
        }

        Commands::Ask { query, cwd, session } => {
            let query_str = query.join(" ");
            if query_str.is_empty() {
                eprintln!("No query provided.");
                std::process::exit(1);
            }

            let config = config::Config::load();
            let db = HistoryDb::open(&get_db_path()).expect("Failed to open database");
            let session_id = session.unwrap_or_else(session::get_session_id);

            // Gather recent commands for context
            let recent = db.get_session_commands(&session_id).unwrap_or_default();

            match ask::ask(&config, &query_str, &cwd, &recent) {
                Some(result) => {
                    println!("{}", result.response);
                    if let Some(cmd) = &result.suggested_command {
                        // Print suggested command on a special line for the shell to parse
                        println!("\n__WAZ_CMD__:{}", cmd);
                    }
                }
                None => {
                    eprintln!("No LLM provider configured. Set an API key or configure ~/.config/waz/config.toml");
                    std::process::exit(1);
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
    }
}
