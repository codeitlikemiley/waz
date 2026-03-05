pub mod app;
pub mod cargo_schema;
pub mod ui;
pub mod verify;

use std::io;
use std::sync::mpsc;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{App, Mode, TokenType};

/// Launch the TUI overlay. Returns the resolved command (if any).
pub fn launch(cwd: String, query: Option<String>, config_mode: bool) -> io::Result<Option<String>> {
    let config = crate::config::Config::load();
    let mut app = App::new(cwd.clone(), config);
    app.config_mode = config_mode;

    // Self mode: jump straight into TMP with waz commands loaded
    if config_mode {
        app.mode = Mode::Tmp;
        load_tmp_commands(&mut app);
        app.tmp_loaded = true;
    }

    // Pre-fill input if query provided (enters AI mode)
    if let Some(q) = query {
        app.mode = Mode::Ai;
        app.input = q.clone();
        app.cursor_pos = app.input.len();
    }

    // Open /dev/tty — the real terminal, regardless of how stdin/stdout are redirected.
    // When launched from a ZLE widget, all fds are redirected to /dev/tty by the widget.
    // When launched manually, stdin/stdout are already the terminal.
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
    // Channel for async AI results
    let (ai_tx, ai_rx) = mpsc::channel::<AiResult>();

    loop {
        // Render
        terminal.draw(|f| ui::draw(f, app))?;

        if app.should_quit {
            break;
        }

        // Check for AI results from background thread
        if app.ai_loading {
            if let Ok(result) = ai_rx.try_recv() {
                app.ai_loading = false;
                apply_ai_result(app, result);
            } else {
                // Advance spinner tick for animation
                app.spinner_tick = app.spinner_tick.wrapping_add(1);
            }
        }

        // Non-blocking event poll (100ms timeout for spinner animation)
        let timeout = if app.ai_loading {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_secs(60)
        };

        if !event::poll(timeout)? {
            continue; // No event — just redraw (for spinner)
        }

        // Handle events
        if let Event::Key(key) = event::read()? {
            // Ignore key releases (crossterm may send them)
            if key.kind == crossterm::event::KeyEventKind::Release {
                continue;
            }

            // Ignore SUPER (Cmd) modifier on char keys — macOS sends escape
            // sequences for Cmd+key that crossterm parses as chars (e.g. Cmd+Backspace → 'u')
            if key.modifiers.contains(KeyModifiers::SUPER) {
                // Handle Cmd+Backspace as "clear entire line"
                if key.code == KeyCode::Backspace {
                    if app.ai_editing_placeholders {
                        app.ai_placeholder_values[app.ai_active_placeholder].clear();
                    } else if app.editing_tokens {
                        app.token_values[app.active_token].clear();
                    } else {
                        app.input.clear();
                        app.cursor_pos = 0;
                        if app.mode != Mode::Empty {
                            update_filter(app);
                        }
                    }
                }
                continue;
            }

            match key.code {
                KeyCode::Esc => {
                    if app.ai_editing_placeholders {
                        // Exit placeholder editing → back to command selection
                        app.ai_editing_placeholders = false;
                        app.ai_placeholder_names.clear();
                        app.ai_placeholder_values.clear();
                        app.ai_editing_cmd.clear();
                        app.ai_selecting = true;
                    } else if app.editing_tokens {
                        // Exit token editing → back to command list
                        app.editing_tokens = false;
                        app.selected_command = None;
                    } else if app.ai_selecting {
                        // Exit command selection → keep AI conversation, allow retyping
                        app.ai_selecting = false;
                        app.ai_selected_cmd = 0;
                    } else if !app.ai_commands.is_empty() || !app.ai_messages.is_empty() {
                        // Clear AI conversation → fresh AI input
                        app.ai_commands.clear();
                        app.ai_messages.clear();
                        app.ai_selecting = false;
                        app.ai_selected_cmd = 0;
                        app.input.clear();
                        app.cursor_pos = 0;
                    } else if app.mode != Mode::Empty {
                        // Return to empty mode
                        app.reset_to_empty();
                    } else {
                        app.should_quit = true;
                    }
                }

                KeyCode::Tab => handle_tab(app),
                KeyCode::BackTab => {
                    // Shift+Tab — go to previous field (tokens or placeholders)
                    if app.editing_tokens {
                        if app.active_token > 0 {
                            app.active_token -= 1;
                        }
                    } else if app.ai_editing_placeholders {
                        if app.ai_active_placeholder > 0 {
                            app.ai_active_placeholder -= 1;
                        }
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
                    handle_enter(app, &ai_tx);
                }


                KeyCode::Backspace => {
                    if app.editing_tokens {
                        // Delete from active token value
                        let val = &mut app.token_values[app.active_token];
                        val.pop();
                    } else if app.ai_editing_placeholders {
                        // Delete from active placeholder value
                        app.ai_placeholder_values[app.ai_active_placeholder].pop();
                    } else if app.cursor_pos > 0 {
                        app.input.remove(app.cursor_pos - 1);
                        app.cursor_pos -= 1;

                        // If input is now empty, reset to Empty mode
                        if app.input.is_empty() {
                            app.reset_to_empty();
                        } else {
                            update_filter(app);
                        }
                    } else if app.mode != Mode::Empty && app.input.is_empty() {
                        // Backspace with no input in a mode → go back to Empty
                        app.reset_to_empty();
                    }
                }

                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        match c {
                            'c' => app.should_quit = true,
                            // Ctrl+U = clear line (macOS Cmd+Backspace sends \x15 = Ctrl+U)
                            'u' => {
                                if app.ai_editing_placeholders {
                                    app.ai_placeholder_values[app.ai_active_placeholder].clear();
                                } else if app.editing_tokens {
                                    app.token_values[app.active_token].clear();
                                } else {
                                    app.input.clear();
                                    app.cursor_pos = 0;
                                    if app.mode == Mode::Tmp {
                                        update_filter(app);
                                    } else if app.mode != Mode::Empty && app.input.is_empty() {
                                        app.reset_to_empty();
                                    }
                                }
                            }
                            // Ignore all other Ctrl combos to prevent stray chars
                            _ => {}
                        }
                    } else if app.editing_tokens {
                        handle_token_char(app, c);
                    } else if app.ai_editing_placeholders {
                        // Typing into the active placeholder field
                        app.ai_placeholder_values[app.ai_active_placeholder].push(c);
                    } else if app.ai_selecting && c.is_ascii_digit() {
                        // Number key selection in AI mode (1-9)
                        let num = c.to_digit(10).unwrap_or(0) as usize;
                        if num >= 1 && num <= app.ai_commands.len() {
                            let cmd = app.ai_commands[num - 1].cmd.clone();
                            let placeholders = extract_placeholders(&cmd);

                            if placeholders.is_empty() {
                                app.output_command = Some(cmd);
                                app.should_quit = true;
                            } else {
                                app.ai_editing_cmd = cmd;
                                app.ai_placeholder_values = vec![String::new(); placeholders.len()];
                                app.ai_placeholder_names = placeholders;
                                app.ai_active_placeholder = 0;
                                app.ai_editing_placeholders = true;
                                app.ai_selecting = false;
                            }
                        }
                    } else {
                        // If AI commands are showing but user starts typing,
                        // clear the old response and let them ask a new question
                        if app.mode == Mode::Ai && !app.ai_commands.is_empty() && !app.ai_selecting {
                            app.ai_commands.clear();
                            app.ai_selecting = false;
                        }
                        handle_char_input(app, c);
                    }
                }

                KeyCode::Left => {
                    if app.cursor_pos > 0 {
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

/// Handle character input with prefix-based mode switching.
fn handle_char_input(app: &mut App, c: char) {
    match app.mode {
        Mode::Empty => {
            // First character determines the mode
            match c {
                '/' => {
                    app.mode = Mode::Tmp;
                    // Lazy-load TMP commands on first entry
                    if !app.tmp_loaded {
                        load_tmp_commands(app);
                        app.tmp_loaded = true;
                    }
                    app.filter_commands();
                    // Don't add '/' to input — it's just the mode trigger
                }
                '!' => {
                    app.mode = Mode::Shell;
                    // Don't add '!' to input — it's just the mode trigger
                }
                _ => {
                    app.mode = Mode::Ai;
                    app.input.push(c);
                    app.cursor_pos += 1;
                }
            }
        }
        Mode::Tmp => {
            app.input.insert(app.cursor_pos, c);
            app.cursor_pos += 1;
            update_filter(app);
        }
        Mode::Shell => {
            app.input.insert(app.cursor_pos, c);
            app.cursor_pos += 1;
        }
        Mode::Ai => {
            app.input.insert(app.cursor_pos, c);
            app.cursor_pos += 1;
        }
    }
}

fn handle_enter(app: &mut App, ai_tx: &mpsc::Sender<AiResult>) {
    match app.mode {
        Mode::Empty => {
            // Nothing to do
        }
        Mode::Tmp => {
            if app.editing_tokens {
                // Validate: all required tokens must have values
                if let Some(cmd_idx) = app.selected_command {
                    let tokens = &app.command_list[cmd_idx].tokens;
                    for (i, token) in tokens.iter().enumerate() {
                        if token.required && app.token_values[i].trim().is_empty() {
                            // Jump to the first unfilled required token
                            app.active_token = i;
                            return;
                        }
                    }
                }
                // All required tokens filled — build and output
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
            if !app.input.is_empty() {
                // Output the raw shell command
                app.output_command = Some(app.input.clone());
                app.should_quit = true;
            }
        }
        Mode::Ai => {
            if app.ai_editing_placeholders {
                // Currently editing placeholders — Enter confirms and outputs
                let mut resolved = app.ai_editing_cmd.clone();
                for (i, name) in app.ai_placeholder_names.iter().enumerate() {
                    let val = &app.ai_placeholder_values[i];
                    resolved = resolved.replace(&format!("<{}>", name), val);
                }
                app.output_command = Some(resolved);
                app.should_quit = true;
            } else if app.ai_selecting {
                // User selected an AI command — check for placeholders
                if !app.ai_commands.is_empty() {
                    let cmd = app.ai_commands[app.ai_selected_cmd].cmd.clone();
                    let placeholders = extract_placeholders(&cmd);

                    if placeholders.is_empty() {
                        // No placeholders — output directly
                        app.output_command = Some(cmd);
                        app.should_quit = true;
                    } else {
                        // Has placeholders — enter editing mode
                        app.ai_editing_cmd = cmd;
                        app.ai_placeholder_values = vec![String::new(); placeholders.len()];
                        app.ai_placeholder_names = placeholders;
                        app.ai_active_placeholder = 0;
                        app.ai_editing_placeholders = true;
                        app.ai_selecting = false;
                    }
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
                app.spinner_tick = 0;

                // Spawn AI call in background thread
                let config = app.config.clone();
                let cwd = app.cwd.clone();
                let tx = ai_tx.clone();

                std::thread::spawn(move || {
                    // Get recent commands for context
                    let db_path = crate::get_db_path();
                    let recent = crate::db::HistoryDb::open(&db_path)
                        .ok()
                        .and_then(|db| db.get_recent_by_cwd(&cwd, None, 10).ok())
                        .unwrap_or_default();

                    // Try TMP resolve first (grounded in schemas + real data sources)
                    let resolve_result = crate::resolve::resolve(
                        &config,
                        &query,
                        &cwd,
                        None,
                    );

                    let used_resolve = if let Ok(ref res) = resolve_result {
                        res.confidence == "high" || res.confidence == "medium"
                    } else {
                        false
                    };

                    if used_resolve {
                        let res = resolve_result.unwrap();
                        let _ = tx.send(AiResult::Resolve(res));
                    } else {
                        let result = crate::ask::ask_structured(
                            &config,
                            &query,
                            &cwd,
                            &recent,
                        );
                        let _ = tx.send(AiResult::Ask(result));
                    }
                });
            }
        }
    }
}

/// Result type for async AI calls
enum AiResult {
    Resolve(crate::resolve::ResolveResult),
    Ask(Option<crate::ask::StructuredResponse>),
}

/// Apply async AI result to app state
fn apply_ai_result(app: &mut App, result: AiResult) {
    match result {
        AiResult::Resolve(res) => {
            app.ai_messages.push(app::AiMessage {
                role: "assistant".to_string(),
                content: format!("[TMP] {}", res.explanation),
            });
            app.ai_commands = vec![app::AiCommand {
                cmd: res.command,
                desc: res.tokens_filled.iter()
                    .map(|t| format!("{} = {}", t.name, t.value))
                    .collect::<Vec<_>>()
                    .join(", "),
                placeholders: vec![],
            }];
            app.ai_selecting = true;
            app.ai_selected_cmd = 0;
        }
        AiResult::Ask(result) => {
            match result {
                Some(resp) => {
                    app.ai_messages.push(app::AiMessage {
                        role: "assistant".to_string(),
                        content: resp.explanation,
                    });
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
                    }
                }
                None => {
                    app.ai_messages.push(app::AiMessage {
                        role: "assistant".to_string(),
                        content: "Sorry, I couldn't process that. Check your API key.".to_string(),
                    });
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
                    
                    // If this is a "provider" token, re-resolve the "model" token
                    let token_name = token.name.clone();
                    let new_value = values[next].clone();
                    if token_name == "provider" {
                        let cmd = &mut app.command_list[cmd_idx];
                        if let Some(model_idx) = cmd.tokens.iter().position(|t| t.name == "model") {
                            // Update the resolver to use the new provider
                            if let Some(ref mut ds) = cmd.tokens[model_idx].data_source {
                                ds.resolver = Some(format!("waz:models:{}", new_value));
                            }
                            // Re-resolve: fetch models from the provider's API
                            let cwd = app.cwd.clone();
                            crate::generate::resolve_data_sources_pub(&mut app.command_list[cmd_idx], &cwd);
                            // Reset model value to first available option
                            if let Some(ref vals) = app.command_list[cmd_idx].tokens[model_idx].values {
                                if !vals.is_empty() {
                                    app.token_values[model_idx] = vals[0].clone();
                                }
                            }
                        }
                    }
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
    } else if app.ai_editing_placeholders {
        // Move to next placeholder field
        let max = app.ai_placeholder_names.len().saturating_sub(1);
        if app.ai_active_placeholder < max {
            app.ai_active_placeholder += 1;
        }
    }
}

/// Extract unique `<placeholder>` names from a command string.
fn extract_placeholders(cmd: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut remaining = cmd;
    while let Some(start) = remaining.find('<') {
        if let Some(end) = remaining[start..].find('>') {
            let name = &remaining[start + 1..start + end];
            // Only treat as placeholder if it looks like a name (no spaces, not empty)
            if !name.is_empty() && !name.contains(' ') && name.len() < 30 {
                let name_str = name.to_string();
                if !placeholders.contains(&name_str) {
                    placeholders.push(name_str);
                }
            }
            remaining = &remaining[start + end + 1..];
        } else {
            break;
        }
    }
    placeholders
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
        _ => {}
    }
}

fn load_tmp_commands(app: &mut App) {
    if app.config_mode {
        // Config mode: load only the waz schema (bundled in binary)
        let waz_schema_bytes = include_str!("../../schemas/curated/waz.json");
        match serde_json::from_str::<crate::tui::app::SchemaFile>(waz_schema_bytes) {
            Ok(sf) => {
                app.command_list.extend(sf.commands);
            }
            Err(e) => {
                eprintln!("Failed to parse embedded waz schema: {}", e);
            }
        }
    } else {
        // Normal mode: load all schemas (curated + generated)
        let commands = crate::generate::load_all_schemas(&app.cwd);
        app.command_list.extend(commands);
    }
    app.filter_commands();
}
