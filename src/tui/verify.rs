//! Verification TUI for reviewing and approving generated schemas.
//!
//! Launch with: `waz generate <tool> --verify`
//! Allows human-in-the-loop review of each command, its tokens,
//! and data sources in a generated schema.

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use std::io;

use crate::tui::app::{SchemaFile, CommandEntry, TokenDef, TokenType};

// ──────────────────────────── State ────────────────────────────

/// Verification TUI pane focus.
#[derive(Debug, Clone, PartialEq)]
enum Pane {
    Commands,
    Tokens,
}

/// Which field of a token is being edited.
#[derive(Debug, Clone, PartialEq)]
enum EditField {
    Name,
    Description,
    Flag,
    DataSource,
}

/// Verification TUI state.
struct VerifyApp {
    schema: SchemaFile,
    tool: String,
    path: std::path::PathBuf,

    /// Currently selected command index.
    cmd_idx: usize,
    /// Currently selected token index within the selected command.
    tok_idx: usize,
    /// Which pane has focus.
    pane: Pane,

    /// If Some, we're editing a token field inline.
    editing: Option<EditField>,
    /// Buffer for inline editing.
    edit_buf: String,

    /// Data source test result (shown temporarily).
    ds_test_result: Option<String>,

    /// Status message shown at bottom.
    status: String,

    /// Whether to quit.
    quit: bool,
    /// Whether changes were saved.
    saved: bool,
}

impl VerifyApp {
    fn new(schema: SchemaFile, tool: String, path: std::path::PathBuf) -> Self {
        let status = format!("{} commands loaded", schema.commands.len());
        Self {
            schema,
            tool,
            path,
            cmd_idx: 0,
            tok_idx: 0,
            pane: Pane::Commands,
            editing: None,
            edit_buf: String::new(),
            ds_test_result: None,
            status,
            quit: false,
            saved: false,
        }
    }

    fn current_cmd(&self) -> Option<&CommandEntry> {
        self.schema.commands.get(self.cmd_idx)
    }

    fn current_cmd_mut(&mut self) -> Option<&mut CommandEntry> {
        self.schema.commands.get_mut(self.cmd_idx)
    }

    fn current_token(&self) -> Option<&TokenDef> {
        self.current_cmd().and_then(|c| c.tokens.get(self.tok_idx))
    }

    fn cmd_count(&self) -> usize {
        self.schema.commands.len()
    }

    fn tok_count(&self) -> usize {
        self.current_cmd().map(|c| c.tokens.len()).unwrap_or(0)
    }

    fn verified_count(&self) -> usize {
        self.schema.commands.iter().filter(|c| c.verified).count()
    }

    fn save(&mut self) -> Result<(), String> {
        // Update meta verification status
        let all_verified = self.schema.commands.iter().all(|c| c.verified);
        self.schema.meta.verified = all_verified;
        if all_verified {
            self.schema.meta.generated_by = "hybrid".to_string();
            self.schema.meta.verified_at = Some(chrono_today());
            self.schema.meta.coverage = "full".to_string();
        }

        let json = serde_json::to_string_pretty(&self.schema)
            .map_err(|e| format!("Serialize: {}", e))?;
        std::fs::write(&self.path, &json)
            .map_err(|e| format!("Write: {}", e))?;
        self.saved = true;
        self.status = format!("✅ Saved ({}/{} verified)", self.verified_count(), self.cmd_count());
        Ok(())
    }

    fn toggle_verified(&mut self) {
        if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
            cmd.verified = !cmd.verified;
            let state = if cmd.verified { "verified" } else { "unverified" };
            self.status = format!("Toggled '{}' → {}", cmd.command, state);
        }
    }

    fn delete_token(&mut self) {
        if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
            if self.tok_idx < cmd.tokens.len() {
                let name = cmd.tokens[self.tok_idx].name.clone();
                cmd.tokens.remove(self.tok_idx);
                if self.tok_idx > 0 && self.tok_idx >= cmd.tokens.len() {
                    self.tok_idx = cmd.tokens.len().saturating_sub(1);
                }
                self.status = format!("Deleted token '{}'", name);
            }
        }
    }

    fn add_token(&mut self) {
        if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
            cmd.tokens.push(TokenDef {
                name: "new_param".to_string(),
                description: "Description".to_string(),
                required: false,
                token_type: TokenType::String,
                default: None,
                values: None,
                flag: None,
                data_source: None,
            });
            self.tok_idx = cmd.tokens.len() - 1;
            self.pane = Pane::Tokens;
            self.status = "Added new token — edit it now".to_string();
        }
    }

    fn test_data_source(&mut self) {
        // Clone data source info up front to avoid borrow issues
        let ds_info = self.current_token().and_then(|tok| {
            tok.data_source.as_ref().map(|ds| {
                let label = ds.resolver.clone()
                    .or(ds.command.clone())
                    .unwrap_or_default();
                (label, tok.clone())
            })
        });

        let (label, test_tok) = match ds_info {
            Some((label, tok)) if !label.is_empty() => (label, tok),
            _ => {
                self.status = "No data source on this token".to_string();
                return;
            }
        };

        self.status = format!("Testing: {}...", label);

        let cwd = std::env::current_dir().unwrap_or_default();
        let cwd_str = cwd.to_string_lossy().to_string();
        let mut test_entry = CommandEntry {
            command: String::new(),
            description: String::new(),
            tokens: vec![test_tok],
            group: String::new(),
            verified: false,
        };
        crate::generate::resolve_data_sources_pub(&mut test_entry, &cwd_str);
        let resolved_tok = test_entry.tokens.remove(0);

        if let Some(vals) = resolved_tok.values {
            let preview = if vals.len() > 10 {
                format!("{} (and {} more)", vals[..10].join(", "), vals.len() - 10)
            } else {
                vals.join(", ")
            };
            self.ds_test_result = Some(format!("✅ {} values: {}", vals.len(), preview));
            self.status = format!("Data source returned {} values", vals.len());
        } else {
            self.ds_test_result = Some("⚠️  No values returned".to_string());
            self.status = "Data source returned no values".to_string();
        }
    }

    fn start_edit(&mut self, field: EditField) {
        let buf = self.current_token().map(|tok| {
            match field {
                EditField::Name => tok.name.clone(),
                EditField::Description => tok.description.clone(),
                EditField::Flag => tok.flag.clone().unwrap_or_default(),
                EditField::DataSource => {
                    tok.data_source.as_ref()
                        .and_then(|ds| ds.resolver.clone().or(ds.command.clone()))
                        .unwrap_or_default()
                }
            }
        });
        if let Some(buf) = buf {
            self.edit_buf = buf;
            self.editing = Some(field);
        }
    }

    fn finish_edit(&mut self) {
        if let Some(ref field) = self.editing.take() {
            if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
                if let Some(tok) = cmd.tokens.get_mut(self.tok_idx) {
                    match field {
                        EditField::Name => tok.name = self.edit_buf.clone(),
                        EditField::Description => tok.description = self.edit_buf.clone(),
                        EditField::Flag => {
                            tok.flag = if self.edit_buf.is_empty() { None } else { Some(self.edit_buf.clone()) };
                        }
                        EditField::DataSource => {
                            // Don't change data source from inline edit for now
                        }
                    }
                    self.status = format!("Updated {} → '{}'", format!("{:?}", field), self.edit_buf);
                }
            }
        }
        self.edit_buf.clear();
    }

    fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_buf.clear();
    }

    fn toggle_required(&mut self) {
        if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
            if let Some(tok) = cmd.tokens.get_mut(self.tok_idx) {
                tok.required = !tok.required;
                self.status = format!("'{}' required → {}", tok.name, tok.required);
            }
        }
    }

    fn cycle_token_type(&mut self) {
        if let Some(cmd) = self.schema.commands.get_mut(self.cmd_idx) {
            if let Some(tok) = cmd.tokens.get_mut(self.tok_idx) {
                tok.token_type = match tok.token_type {
                    TokenType::String => TokenType::Boolean,
                    TokenType::Boolean => TokenType::Enum,
                    TokenType::Enum => TokenType::File,
                    TokenType::File => TokenType::Number,
                    TokenType::Number => TokenType::String,
                };
                self.status = format!("'{}' type → {:?}", tok.name, tok.token_type);
            }
        }
    }
}

fn chrono_today() -> String {
    // Simple date without chrono dependency
    let output = std::process::Command::new("date").arg("+%Y-%m-%d").output();
    output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "2026-03-05".to_string())
}

// ──────────────────────────── Launch ────────────────────────────

/// Launch the verification TUI for a schema.
pub fn launch(tool: &str) -> io::Result<()> {
    let schema_path = crate::generate::schemas_dir().join(format!("{}.json", tool));
    if !schema_path.exists() {
        eprintln!("❌ No schema found for '{}'. Run `waz generate {}` first.", tool, tool);
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&schema_path)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let schema: SchemaFile = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Parse error: {}", e)))?;

    let mut app = VerifyApp::new(schema, tool.to_string(), schema_path);

    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Event loop
    while !app.quit {
        terminal.draw(|f| draw(f, &app))?;

        if let Event::Key(key) = event::read()? {
            handle_key(&mut app, key.code, key.modifiers);
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if app.saved {
        eprintln!("✅ Schema saved ({}/{} commands verified)", app.verified_count(), app.cmd_count());
    }

    Ok(())
}

// ──────────────────────────── Key handling ────────────────────────────

fn handle_key(app: &mut VerifyApp, code: KeyCode, modifiers: KeyModifiers) {
    // If editing, handle edit input
    if app.editing.is_some() {
        match code {
            KeyCode::Enter => app.finish_edit(),
            KeyCode::Esc => app.cancel_edit(),
            KeyCode::Backspace => { app.edit_buf.pop(); }
            KeyCode::Char(c) => app.edit_buf.push(c),
            _ => {}
        }
        return;
    }

    match code {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,

        // Save
        KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Err(e) = app.save() {
                app.status = format!("❌ Save failed: {}", e);
            }
        }
        KeyCode::Char('s') => {
            if let Err(e) = app.save() {
                app.status = format!("❌ Save failed: {}", e);
            }
        }

        // Navigation
        KeyCode::Up | KeyCode::Char('k') => {
            match app.pane {
                Pane::Commands => {
                    if app.cmd_idx > 0 {
                        app.cmd_idx -= 1;
                        app.tok_idx = 0;
                        app.ds_test_result = None;
                    }
                }
                Pane::Tokens => {
                    if app.tok_idx > 0 {
                        app.tok_idx -= 1;
                        app.ds_test_result = None;
                    }
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            match app.pane {
                Pane::Commands => {
                    if app.cmd_idx + 1 < app.cmd_count() {
                        app.cmd_idx += 1;
                        app.tok_idx = 0;
                        app.ds_test_result = None;
                    }
                }
                Pane::Tokens => {
                    if app.tok_idx + 1 < app.tok_count() {
                        app.tok_idx += 1;
                        app.ds_test_result = None;
                    }
                }
            }
        }

        // Switch panes
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            if app.pane == Pane::Commands && app.tok_count() > 0 {
                app.pane = Pane::Tokens;
                app.tok_idx = 0;
            }
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            app.pane = Pane::Commands;
        }

        // Toggle verified (Space or Enter on Commands pane)
        KeyCode::Char(' ') | KeyCode::Enter if app.pane == Pane::Commands => {
            app.toggle_verified();
        }

        // Token actions (when in Tokens pane)
        KeyCode::Char('n') if app.pane == Pane::Tokens => app.start_edit(EditField::Name),
        KeyCode::Char('d') if app.pane == Pane::Tokens => app.start_edit(EditField::Description),
        KeyCode::Char('f') if app.pane == Pane::Tokens => app.start_edit(EditField::Flag),
        KeyCode::Char('r') if app.pane == Pane::Tokens => app.toggle_required(),
        KeyCode::Char('t') if app.pane == Pane::Tokens => app.cycle_token_type(),
        KeyCode::Char('x') if app.pane == Pane::Tokens => app.test_data_source(),
        KeyCode::Delete if app.pane == Pane::Tokens => app.delete_token(),

        // Add token
        KeyCode::Char('a') => app.add_token(),

        // Verify all
        KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
            for cmd in &mut app.schema.commands {
                cmd.verified = true;
            }
            app.status = "All commands marked as verified".to_string();
        }

        _ => {}
    }
}

// ──────────────────────────── Drawing ────────────────────────────

fn draw(f: &mut Frame, app: &VerifyApp) {
    let area = f.area();
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(10),   // main content
            Constraint::Length(3), // detail/ds test
            Constraint::Length(2), // footer
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_main(f, app, chunks[1]);
    draw_detail(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let verified = app.verified_count();
    let total = app.cmd_count();
    let pct = if total > 0 { (verified * 100) / total } else { 0 };
    let all_ok = verified == total;

    let status_icon = if all_ok { "✅" } else { "🔍" };

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                format!(" {} Schema Verification: {} ", status_icon, app.tool),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{} | {}/{} verified ({}%) | {} ",
                    app.schema.meta.version, verified, total, pct,
                    if all_ok { "COMPLETE" } else { "IN PROGRESS" }),
                Style::default().fg(if all_ok { Color::Green } else { Color::DarkGray }),
            ),
        ]),
    ])
    .block(Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if all_ok { Color::Green } else { Color::Cyan })));

    f.render_widget(header, area);
}

fn draw_main(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // commands list
            Constraint::Percentage(60), // tokens detail
        ])
        .split(area);

    draw_commands_list(f, app, cols[0]);
    draw_tokens_detail(f, app, cols[1]);
}

fn draw_commands_list(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let is_focused = app.pane == Pane::Commands;

    let items: Vec<ListItem> = app.schema.commands.iter().enumerate().map(|(i, cmd)| {
        let icon = if cmd.verified { "✅" } else { "○ " };
        let selected = i == app.cmd_idx;
        let tok_count = cmd.tokens.len();

        let style = if selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(if is_focused { Color::DarkGray } else { Color::Black })
        } else {
            Style::default().fg(if cmd.verified { Color::Green } else { Color::Gray })
        };

        ListItem::new(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(if cmd.verified { Color::Green } else { Color::DarkGray })),
            Span::styled(&cmd.command, style),
            Span::styled(format!("  [{}]", tok_count), Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();

    let border_color = if is_focused { Color::Cyan } else { Color::DarkGray };
    let list = List::new(items)
        .block(Block::default()
            .title(" Commands ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)));

    f.render_widget(list, area);
}

fn draw_tokens_detail(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let is_focused = app.pane == Pane::Tokens;

    let cmd = match app.current_cmd() {
        Some(c) => c,
        None => {
            let empty = Paragraph::new("No command selected")
                .block(Block::default().title(" Tokens ").borders(Borders::ALL));
            f.render_widget(empty, area);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();

    // Command description at top
    lines.push(Line::from(vec![
        Span::styled("  Desc: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&cmd.description, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(""));

    if cmd.tokens.is_empty() {
        lines.push(Line::from(Span::styled("  No tokens defined", Style::default().fg(Color::DarkGray))));
    } else {
        for (i, tok) in cmd.tokens.iter().enumerate() {
            let selected = i == app.tok_idx && is_focused;
            let bg = if selected { Color::DarkGray } else { Color::Reset };

            // Token header line
            let req_label = if tok.required { "req" } else { "opt" };
            let type_label = format!("{:?}", tok.token_type);
            let flag_label = tok.flag.as_deref().unwrap_or("positional");

            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}. ", i + 1),
                    Style::default().fg(Color::DarkGray).bg(bg),
                ),
                Span::styled(
                    &tok.name,
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD).bg(bg),
                ),
                Span::styled(
                    format!("  [{}]", type_label),
                    Style::default().fg(Color::Yellow).bg(bg),
                ),
                Span::styled(
                    format!("  {}", req_label),
                    Style::default().fg(if tok.required { Color::Red } else { Color::DarkGray }).bg(bg),
                ),
                Span::styled(
                    format!("  {}", flag_label),
                    Style::default().fg(Color::Magenta).bg(bg),
                ),
            ]));

            // Description line
            lines.push(Line::from(vec![
                Span::styled("     ", Style::default().bg(bg)),
                Span::styled(&tok.description, Style::default().fg(Color::Gray).bg(bg)),
            ]));

            // Data source line (if any)
            if let Some(ref ds) = tok.data_source {
                let src = ds.resolver.as_deref()
                    .or(ds.command.as_deref())
                    .unwrap_or("none");
                lines.push(Line::from(vec![
                    Span::styled("     📊 ", Style::default().bg(bg)),
                    Span::styled(src, Style::default().fg(Color::Blue).bg(bg)),
                ]));
            }

            // Values preview (if any)
            if let Some(ref vals) = tok.values {
                let preview = if vals.len() > 5 {
                    format!("{} (+{} more)", vals[..5].join(", "), vals.len() - 5)
                } else {
                    vals.join(", ")
                };
                lines.push(Line::from(vec![
                    Span::styled("     ▸ ", Style::default().bg(bg)),
                    Span::styled(preview, Style::default().fg(Color::DarkGray).bg(bg)),
                ]));
            }

            // Edit buffer (if editing this token)
            if selected {
                if let Some(ref field) = app.editing {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("     ✏️  {:?}: ", field),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::styled(
                            format!("{}▌", app.edit_buf),
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
            }

            lines.push(Line::from(""));
        }
    }

    let border_color = if is_focused { Color::Cyan } else { Color::DarkGray };
    let para = Paragraph::new(lines)
        .block(Block::default()
            .title(format!(" Tokens ({}) ", cmd.tokens.len()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)))
        .wrap(Wrap { trim: true });

    f.render_widget(para, area);
}

fn draw_detail(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let msg = if let Some(ref result) = app.ds_test_result {
        result.clone()
    } else {
        app.status.clone()
    };

    let para = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::raw(&msg),
    ]))
    .block(Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Status "));

    f.render_widget(para, area);
}

fn draw_footer(f: &mut Frame, app: &VerifyApp, area: Rect) {
    let keys = if app.editing.is_some() {
        vec![
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ]
    } else if app.pane == Pane::Commands {
        vec![
            ("Space", "toggle ✅"),
            ("→/Tab", "tokens"),
            ("a", "add token"),
            ("s", "save"),
            ("Ctrl+V", "verify all"),
            ("q", "quit"),
        ]
    } else {
        vec![
            ("n", "edit name"),
            ("d", "edit desc"),
            ("f", "edit flag"),
            ("r", "req/opt"),
            ("t", "cycle type"),
            ("x", "test ds"),
            ("Del", "delete"),
            ("←", "back"),
            ("s", "save"),
            ("q", "quit"),
        ]
    };

    let spans: Vec<Span> = keys.iter().flat_map(|(k, v)| {
        vec![
            Span::styled(format!(" {} ", k), Style::default().fg(Color::Black).bg(Color::DarkGray)),
            Span::styled(format!(" {} ", v), Style::default().fg(Color::DarkGray)),
        ]
    }).collect();

    let footer = Paragraph::new(Line::from(spans));
    f.render_widget(footer, area);
}
