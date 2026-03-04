use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::app::{App, Mode, TokenType};

/// Render the TUI overlay.
pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    // Semi-transparent overlay effect: clear the area
    f.render_widget(Clear, area);

    // Main layout: header + content + input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // header
            Constraint::Min(5),    // content
            Constraint::Length(3), // input
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_content(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mode_label = match app.mode {
        Mode::Tmp => ("TMP Mode", Color::Cyan),
        Mode::Ai => ("AI Mode", Color::Yellow),
        Mode::Shell => ("Shell Mode", Color::Green),
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled("🔮 waz", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", mode_label.0),
            Style::default().fg(mode_label.1),
        ),
    ]))
    .block(Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray)));

    f.render_widget(header, area);
}

fn draw_content(f: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Tmp => draw_tmp_content(f, app, area),
        Mode::Ai => draw_ai_content(f, app, area),
        Mode::Shell => draw_shell_content(f, app, area),
    }
}

fn draw_tmp_content(f: &mut Frame, app: &App, area: Rect) {
    if app.editing_tokens {
        // Split: left = command info, right = token form
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        // Left: command list (dimmed)
        draw_command_list(f, app, chunks[0], true);

        // Right: token form
        draw_token_form(f, app, chunks[1]);
    } else {
        draw_command_list(f, app, area, false);
    }
}

fn draw_command_list(f: &mut Frame, app: &App, area: Rect, dimmed: bool) {
    let items: Vec<ListItem> = app.filtered_commands.iter().enumerate().map(|(i, &cmd_idx)| {
        let cmd = &app.command_list[cmd_idx];
        let is_selected = i == app.selected_index;

        let style = if dimmed {
            Style::default().fg(Color::DarkGray)
        } else if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let prefix = if is_selected && !dimmed { "▸ " } else { "  " };

        let line = Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(&cmd.command, style),
            Span::styled(
                format!("  — {}", cmd.description),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        ListItem::new(line)
    }).collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(list, area);
}

fn draw_token_form(f: &mut Frame, app: &App, area: Rect) {
    let cmd_idx = match app.selected_command {
        Some(idx) => idx,
        None => return,
    };
    let cmd = &app.command_list[cmd_idx];

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                format!("▸ {}", cmd.command),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Tokens:",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
    ];

    for (i, token) in cmd.tokens.iter().enumerate() {
        let is_active = i == app.active_token;
        let value = app.token_values.get(i).cloned().unwrap_or_default();

        let req_marker = if token.required {
            Span::styled("*", Style::default().fg(Color::Red))
        } else {
            Span::raw(" ")
        };

        let name_style = if is_active {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };

        let value_display = match &token.token_type {
            TokenType::Boolean => {
                if value == "true" { "☑ yes".to_string() } else { "☐ no".to_string() }
            }
            TokenType::Enum => {
                if value.is_empty() {
                    if let Some(vals) = &token.values {
                        format!("[{}]", vals.join("|"))
                    } else {
                        "[___]".to_string()
                    }
                } else {
                    value.clone()
                }
            }
            _ => {
                if value.is_empty() { "[___]".to_string() } else { value.clone() }
            }
        };

        let cursor = if is_active { "▸" } else { " " };

        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", cursor), Style::default().fg(Color::Yellow)),
            req_marker,
            Span::styled(format!("{}: ", token.name), name_style),
            Span::styled(
                value_display,
                if is_active {
                    Style::default().fg(Color::White).add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ]));

        // Show description for active token
        if is_active {
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(
                    &token.description,
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    // Show the resolved command preview
    if let Some(preview) = app.build_command() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  → ", Style::default().fg(Color::Green)),
            Span::styled(preview, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE),
    );

    f.render_widget(paragraph, area);
}

fn draw_ai_content(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if app.ai_messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Type a question and press Enter...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    for msg in &app.ai_messages {
        match msg.role.as_str() {
            "user" => {
                lines.push(Line::from(vec![
                    Span::styled("  You: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw(&msg.content),
                ]));
            }
            "assistant" => {
                lines.push(Line::from(vec![
                    Span::styled("  🔮  ", Style::default().fg(Color::Yellow)),
                    Span::raw(&msg.content),
                ]));
            }
            _ => {}
        }
        lines.push(Line::from(""));
    }

    if app.ai_loading {
        lines.push(Line::from(Span::styled(
            "  ⏳ Thinking...",
            Style::default().fg(Color::Yellow),
        )));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}

fn draw_shell_content(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app.filtered_history.iter().enumerate().map(|(i, &hist_idx)| {
        let entry = &app.history_entries[hist_idx];
        let is_selected = i == app.selected_index;

        let style = if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let prefix = if is_selected { "▸ " } else { "  " };
        ListItem::new(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(entry.as_str(), style),
        ]))
    }).collect();

    let list = List::new(items).block(Block::default().borders(Borders::NONE));
    f.render_widget(list, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let input_style = Style::default().fg(Color::White);

    let input_widget = Paragraph::new(Line::from(vec![
        Span::styled("❯ ", Style::default().fg(Color::Green)),
        Span::styled(&app.input, input_style),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(input_widget, area);

    // Position cursor
    f.set_cursor_position((
        area.x + 2 + app.cursor_pos as u16,
        area.y + 1,
    ));
}

fn draw_footer(f: &mut Frame, _app: &App, area: Rect) {
    let help = Line::from(vec![
        Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Tab", Style::default().fg(Color::Cyan)),
        Span::styled(" fill  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled(" run  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);

    f.render_widget(Paragraph::new(help), area);
}
