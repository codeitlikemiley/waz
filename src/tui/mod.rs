pub mod app;
pub mod cargo_schema;
pub mod ui;

use std::io;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{App, Mode, TokenType};

/// Launch the TUI overlay. Returns the resolved command (if any).
pub fn launch(mode: Mode, cwd: String, query: Option<String>) -> io::Result<Option<String>> {
    let config = crate::config::Config::load();
    let mut app = App::new(mode, cwd.clone(), config);

    // Pre-fill input if query provided
    if let Some(q) = query {
        app.input = match mode {
            Mode::Ai => q.clone(),
            Mode::Tmp => format!("/{}", q),
            Mode::Shell => format!("!{}", q),
        };
        app.cursor_pos = app.input.len();
    }

    // Load context based on mode
    match mode {
        Mode::Tmp => load_tmp_commands(&mut app),
        Mode::Shell => load_history(&mut app, &cwd),
        Mode::Ai => {}
    }

    // Open /dev/tty for TUI rendering (so stdout remains free for command output).
    // When launched from a ZLE widget via $(command waz tui ...), stdout is captured.
    // Writing TUI escape codes to stdout would corrupt the shell buffer.
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    let mut tty_write = tty.try_clone()?;

    enable_raw_mode()?;
    execute!(tty_write, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(tty_write);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result.map(|_| app.output_command)
}

fn run_event_loop<W: io::Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Render
        terminal.draw(|f| ui::draw(f, app))?;

        if app.should_quit {
            break;
        }

        // Handle events
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Esc => {
                    if app.editing_tokens {
                        // Exit token editing, back to command list
                        app.editing_tokens = false;
                        app.selected_command = None;
                    } else if app.ai_selecting {
                        // Exit AI command selection, back to input
                        app.ai_selecting = false;
                        app.ai_selected_cmd = 0;
                    } else {
                        app.should_quit = true;
                    }
                }

                KeyCode::Up => app.move_up(),
                KeyCode::Down => app.move_down(),
                KeyCode::PageUp => {
                    app.scroll_offset = app.scroll_offset.saturating_sub(5);
                }
                KeyCode::PageDown => {
                    app.scroll_offset = app.scroll_offset.saturating_add(5);
                }

                KeyCode::Enter => {
                    handle_enter(app);
                }

                KeyCode::Tab => {
                    handle_tab(app);
                }

                KeyCode::Backspace => {
                    if app.editing_tokens {
                        // Delete from active token value
                        let val = &mut app.token_values[app.active_token];
                        val.pop();
                    } else if app.cursor_pos > 0 {
                        let min_pos = match app.mode {
                            Mode::Tmp | Mode::Shell => 1, // Keep the / or ! prefix
                            Mode::Ai => 0,
                        };
                        if app.cursor_pos > min_pos {
                            app.input.remove(app.cursor_pos - 1);
                            app.cursor_pos -= 1;
                            update_filter(app);
                        }
                    }
                }

                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                        app.should_quit = true;
                    } else if app.editing_tokens {
                        handle_token_char(app, c);
                    } else {
                        app.input.insert(app.cursor_pos, c);
                        app.cursor_pos += 1;
                        update_filter(app);
                    }
                }

                KeyCode::Left => {
                    let min_pos = match app.mode {
                        Mode::Tmp | Mode::Shell => 1,
                        Mode::Ai => 0,
                    };
                    if app.cursor_pos > min_pos {
                        app.cursor_pos -= 1;
                    }
                }

                KeyCode::Right => {
                    if app.cursor_pos < app.input.len() {
                        app.cursor_pos += 1;
                    }
                }

                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_enter(app: &mut App) {
    match app.mode {
        Mode::Tmp => {
            if app.editing_tokens {
                // Build and output the command
                if let Some(cmd) = app.build_command() {
                    app.output_command = Some(cmd);
                    app.should_quit = true;
                }
            } else {
                // Select command and enter token editing
                app.select_command();

                // If no tokens, run directly
                if !app.editing_tokens {
                    if let Some(idx) = app.selected_command {
                        let cmd = app.command_list[idx].command.clone();
                        app.output_command = Some(cmd);
                        app.should_quit = true;
                    }
                }
            }
        }
        Mode::Shell => {
            if !app.filtered_history.is_empty() {
                let idx = app.filtered_history[app.selected_index];
                let entry = app.history_entries[idx].clone();
                app.output_command = Some(entry);
                app.should_quit = true;
            }
        }
        Mode::Ai => {
            if app.ai_selecting {
                // User selected an AI command
                if !app.ai_commands.is_empty() {
                    let cmd = app.ai_commands[app.ai_selected_cmd].cmd.clone();
                    // If command has placeholders, output for editing
                    app.output_command = Some(cmd);
                    app.should_quit = true;
                }
            } else if !app.input.is_empty() {
                let query = app.input.clone();
                app.ai_messages.push(app::AiMessage {
                    role: "user".to_string(),
                    content: query.clone(),
                });
                app.input.clear();
                app.cursor_pos = 0;
                app.ai_loading = true;

                // Get recent commands for context
                let db_path = crate::get_db_path();
                let recent = crate::db::HistoryDb::open(&db_path)
                    .ok()
                    .and_then(|db| db.get_recent_by_cwd(&app.cwd, None, 10).ok())
                    .unwrap_or_default();

                // Call the LLM
                let result = crate::ask::ask_structured(
                    &app.config,
                    &query,
                    &app.cwd,
                    &recent,
                );

                app.ai_loading = false;

                match result {
                    Some(resp) => {
                        // Store explanation
                        app.ai_messages.push(app::AiMessage {
                            role: "assistant".to_string(),
                            content: resp.explanation,
                        });

                        // Store commands for selection
                        app.ai_commands = resp.commands.into_iter().map(|c| {
                            app::AiCommand {
                                cmd: c.cmd,
                                desc: c.desc,
                                placeholders: c.placeholders,
                            }
                        }).collect();

                        if !app.ai_commands.is_empty() {
                            app.ai_selecting = true;
                            app.ai_selected_cmd = 0;
                            app.selected_index = 0;
                        }
                    }
                    None => {
                        app.ai_messages.push(app::AiMessage {
                            role: "assistant".to_string(),
                            content: "No response from AI. Check your API keys.".to_string(),
                        });
                    }
                }
            }
        }
    }
}

fn handle_tab(app: &mut App) {
    if app.editing_tokens {
        // Cycle token value for Enum/Boolean types
        let cmd_idx = match app.selected_command {
            Some(idx) => idx,
            None => return,
        };
        let token = &app.command_list[cmd_idx].tokens[app.active_token];

        match token.token_type {
            TokenType::Boolean => {
                let val = &app.token_values[app.active_token];
                app.token_values[app.active_token] = if val == "true" {
                    "false".to_string()
                } else {
                    "true".to_string()
                };
            }
            TokenType::Enum => {
                if let Some(values) = &token.values {
                    let current = &app.token_values[app.active_token];
                    let idx = values.iter().position(|v| v == current).unwrap_or(0);
                    let next = (idx + 1) % values.len();
                    app.token_values[app.active_token] = values[next].clone();
                }
            }
            _ => {
                // Move to next token
                let max = app.token_values.len().saturating_sub(1);
                if app.active_token < max {
                    app.active_token += 1;
                }
            }
        }
    } else if app.mode == Mode::Tmp {
        // Tab selects the highlighted command
        app.select_command();
    }
}

fn handle_token_char(app: &mut App, c: char) {
    let cmd_idx = match app.selected_command {
        Some(idx) => idx,
        None => return,
    };
    let token = &app.command_list[cmd_idx].tokens[app.active_token];

    match token.token_type {
        TokenType::Boolean => {
            // Toggle on space or y/n
            match c {
                ' ' | 'y' | 'Y' => app.token_values[app.active_token] = "true".to_string(),
                'n' | 'N' => app.token_values[app.active_token] = "false".to_string(),
                _ => {}
            }
        }
        _ => {
            app.token_values[app.active_token].push(c);
        }
    }
}

fn update_filter(app: &mut App) {
    match app.mode {
        Mode::Tmp => app.filter_commands(),
        Mode::Shell => app.filter_history(),
        Mode::Ai => {}
    }
}

fn load_tmp_commands(app: &mut App) {
    // Check project files first (immutable borrow of app.cwd)
    let has_cargo = std::path::Path::new(&app.cwd).join("Cargo.toml").exists();
    let has_npm = std::path::Path::new(&app.cwd).join("package.json").exists();
    let has_git = std::path::Path::new(&app.cwd).join(".git").exists()
        || find_git_root(std::path::Path::new(&app.cwd)).is_some();

    // Now load commands (mutable borrow of app)
    if has_cargo { cargo_schema::load(app); }
    if has_npm { load_npm_commands(app); }
    if has_git { load_git_commands(app); }

    app.filter_commands();
}

// load_cargo_commands removed — now in cargo_schema.rs

fn load_npm_commands(app: &mut App) {
    use app::{CommandEntry, TokenDef};

    // Read scripts from package.json
    let pkg_path = std::path::Path::new(&app.cwd).join("package.json");
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
            tokens: vec![],
        },
    ];

    if !scripts.is_empty() {
        commands.push(CommandEntry {
            command: "npm run".to_string(),
            description: "Run a script".to_string(),
            group: "npm".to_string(),
            tokens: vec![
                TokenDef {
                    name: "script".to_string(),
                    description: "Script to run".to_string(),
                    required: true,
                    token_type: TokenType::Enum,
                    default: None,
                    values: Some(scripts),
                    flag: None,
                },
            ],
        });
    }

    app.command_list.extend(commands);
}

fn load_git_commands(app: &mut App) {
    use app::{CommandEntry, TokenDef};

    // Get current branches
    let branches: Vec<String> = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&app.cwd)
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

    let commands = vec![
        CommandEntry {
            command: "git status".to_string(),
            description: "Show working tree status".to_string(),
            group: "git".to_string(),
            tokens: vec![],
        },
        CommandEntry {
            command: "git add".to_string(),
            description: "Stage files for commit".to_string(),
            group: "git".to_string(),
            tokens: vec![
                TokenDef {
                    name: "path".to_string(),
                    description: "File or directory to stage".to_string(),
                    required: true,
                    token_type: TokenType::File,
                    default: Some(".".to_string()),
                    values: None,
                    flag: None,
                },
            ],
        },
        CommandEntry {
            command: "git commit".to_string(),
            description: "Record changes to the repository".to_string(),
            group: "git".to_string(),
            tokens: vec![
                TokenDef {
                    name: "m".to_string(),
                    description: "Commit message".to_string(),
                    required: true,
                    token_type: TokenType::String,
                    default: None,
                    values: None,
                    flag: None,
                },
            ],
        },
        CommandEntry {
            command: "git checkout".to_string(),
            description: "Switch branches".to_string(),
            group: "git".to_string(),
            tokens: vec![
                TokenDef {
                    name: "branch".to_string(),
                    description: "Branch to switch to".to_string(),
                    required: true,
                    token_type: if branches.is_empty() { TokenType::String } else { TokenType::Enum },
                    default: None,
                    values: if branches.is_empty() { None } else { Some(branches.clone()) },
                    flag: None,
                },
            ],
        },
        CommandEntry {
            command: "git push".to_string(),
            description: "Push to remote".to_string(),
            group: "git".to_string(),
            tokens: vec![],
        },
        CommandEntry {
            command: "git pull".to_string(),
            description: "Pull from remote".to_string(),
            group: "git".to_string(),
            tokens: vec![],
        },
        CommandEntry {
            command: "git log".to_string(),
            description: "Show commit logs".to_string(),
            group: "git".to_string(),
            tokens: vec![
                TokenDef {
                    name: "n".to_string(),
                    description: "Number of commits to show".to_string(),
                    required: false,
                    token_type: TokenType::Number,
                    default: Some("10".to_string()),
                    values: None,
                    flag: None,
                },
                TokenDef {
                    name: "oneline".to_string(),
                    description: "Show in one-line format".to_string(),
                    required: false,
                    token_type: TokenType::Boolean,
                    default: Some("true".to_string()),
                    values: None,
                    flag: None,
                },
            ],
        },
    ];

    app.command_list.extend(commands);
}

fn load_history(app: &mut App, cwd: &str) {
    let db_path = crate::get_db_path();
    if let Ok(db) = crate::db::HistoryDb::open(&db_path) {
        if let Ok(entries) = db.get_recent_by_cwd(cwd, None, 50) {
            app.history_entries = entries;
        }
    }
    app.filter_history();
}




fn find_git_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = path;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
