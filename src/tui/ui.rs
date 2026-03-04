use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect, Alignment},
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

    // Main layout: header + content + input + footer
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
        Mode::Empty => None,
        Mode::Tmp => Some(("TMP Mode", Color::Cyan)),
        Mode::Ai => Some(("AI Mode", Color::Yellow)),
        Mode::Shell => Some(("Shell Mode", Color::Green)),
    };

    let mut spans = vec![
        Span::styled("🔮 waz", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ];

    if let Some((label, color)) = mode_label {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("[{}]", label),
            Style::default().fg(color),
        ));
    }

    let header = Paragraph::new(Line::from(spans))
        .block(Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)));

    f.render_widget(header, area);
}

fn draw_content(f: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Empty => draw_empty_content(f, area),
        Mode::Tmp => draw_tmp_content(f, app, area),
        Mode::Ai => draw_ai_content(f, app, area),
        Mode::Shell => draw_shell_content(f, app, area),
    }
}

fn draw_empty_content(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("  /", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("   Command palette", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  !", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("   Shell command", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  …", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled("   Just type for AI", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Left);
    f.render_widget(paragraph, area);
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
    } else if app.filtered_commands.is_empty() {
        // No commands found — show helpful message
        let msg = if app.command_list.is_empty() {
            "  No tools detected in this directory.\n  Navigate to a project with Cargo.toml, package.json, or .git"
        } else {
            "  No commands match your filter."
        };
        let lines: Vec<Line> = msg.lines().map(|l| {
            Line::from(Span::styled(l, Style::default().fg(Color::DarkGray)))
        }).collect();
        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, area);
    } else {
        draw_command_list(f, app, area, false);
    }
}

fn draw_command_list(f: &mut Frame, app: &App, area: Rect, dimmed: bool) {
    // Build items with group headers
    let mut items: Vec<ListItem> = Vec::new();
    let mut last_group: Option<String> = None;

    for (i, &cmd_idx) in app.filtered_commands.iter().enumerate() {
        let cmd = &app.command_list[cmd_idx];

        // Insert group header when group changes
        if last_group.as_ref() != Some(&cmd.group) {
            if last_group.is_some() {
                // Spacer between groups
                items.push(ListItem::new(Line::from("")));
            }
            let header = Line::from(vec![
                Span::styled(
                    format!("  {}", cmd.group),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            items.push(ListItem::new(header));
            last_group = Some(cmd.group.clone());
        }

        let is_selected = i == app.selected_index;

        let style = if dimmed {
            Style::default().fg(Color::DarkGray)
        } else if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let prefix = if is_selected && !dimmed { "  ▸ " } else { "    " };

        // Show subcommand only (strip the group prefix)
        let display_name = cmd.command
            .strip_prefix(&format!("{} ", cmd.group))
            .unwrap_or(&cmd.command);

        let line = Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(display_name, style),
            Span::styled(
                format!("  — {}", cmd.description),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        items.push(ListItem::new(line));
    }

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

    if app.ai_messages.is_empty() && !app.ai_loading {
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
                lines.push(Line::from(""));
            }
            "assistant" => {
                lines.push(Line::from(Span::styled(
                    "  🔮 waz:",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                // Render explanation as multi-line
                for line in msg.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", line),
                        Style::default().fg(Color::White),
                    )));
                }
                lines.push(Line::from(""));
            }
            _ => {}
        }
    }

    // Render AI-suggested commands as a selectable list
    if !app.ai_commands.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Commands:",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));

        for (i, cmd) in app.ai_commands.iter().enumerate() {
            let is_selected = app.ai_selecting && i == app.ai_selected_cmd;

            let prefix = if is_selected { "  ▸ " } else { "    " };
            let style = if is_selected {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            lines.push(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("[{}] ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::styled(&cmd.cmd, style),
            ]));

            // Show description for selected command
            if is_selected && !cmd.desc.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(&cmd.desc, Style::default().fg(Color::DarkGray)),
                ]));
            }

            // Show placeholders warning for selected command
            if is_selected && !cmd.placeholders.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(
                        format!("⚠ placeholders: {}", cmd.placeholders.join(", ")),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        }
    }

    // Render AI placeholder editing form
    if app.ai_editing_placeholders {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ⌨ Fill in placeholders:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        // Show the command with live preview
        let mut preview = app.ai_editing_cmd.clone();
        for (i, name) in app.ai_placeholder_names.iter().enumerate() {
            let val = &app.ai_placeholder_values[i];
            if !val.is_empty() {
                preview = preview.replace(&format!("<{}>", name), val);
            }
        }
        lines.push(Line::from(vec![
            Span::styled("  → ", Style::default().fg(Color::Green)),
            Span::styled(preview, Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(""));

        // Show placeholder input fields
        for (i, name) in app.ai_placeholder_names.iter().enumerate() {
            let is_active = i == app.ai_active_placeholder;
            let val = &app.ai_placeholder_values[i];

            let label_style = if is_active {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let val_style = if is_active {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let cursor = if is_active { "█" } else { "" };

            lines.push(Line::from(vec![
                Span::styled(format!("    {}: ", name), label_style),
                Span::styled(val, val_style),
                Span::styled(cursor, Style::default().fg(Color::Green)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Tab next field  Enter run  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));
    }

    if app.ai_loading {
        lines.push(Line::from(Span::styled(
            "  ⏳ Thinking...",
            Style::default().fg(Color::Yellow),
        )));
    }

    let paragraph = Paragraph::new(lines).scroll((app.scroll_offset, 0));
    f.render_widget(paragraph, area);
}

fn draw_shell_content(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if app.input.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Type a shell command and press Enter to run it...",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  → ", Style::default().fg(Color::Green)),
            Span::styled(
                &app.input,
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let input_style = Style::default().fg(Color::White);

    // Mode prefix (visual-only, not part of editable input)
    let (prefix, prefix_style, prefix_len) = match app.mode {
        Mode::Empty => ("❯ ", Style::default().fg(Color::Green), 2),
        Mode::Tmp => ("❯ /", Style::default().fg(Color::Cyan), 3),
        Mode::Shell => ("❯ !", Style::default().fg(Color::Green), 3),
        Mode::Ai => ("❯ ", Style::default().fg(Color::Yellow), 2),
    };

    let input_widget = Paragraph::new(Line::from(vec![
        Span::styled(prefix, prefix_style),
        Span::styled(&app.input, input_style),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(input_widget, area);

    // Position cursor (offset by visual prefix width)
    f.set_cursor_position((
        area.x + prefix_len + app.cursor_pos as u16,
        area.y + 1,
    ));
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let help = match app.mode {
        Mode::Empty => Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Cyan)),
            Span::styled(" commands  ", Style::default().fg(Color::DarkGray)),
            Span::styled("!", Style::default().fg(Color::Green)),
            Span::styled(" shell  ", Style::default().fg(Color::DarkGray)),
            Span::styled("text", Style::default().fg(Color::Yellow)),
            Span::styled(" ai  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ]),
        Mode::Tmp => Line::from(vec![
            Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
            Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::styled(" fill  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" run  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" back", Style::default().fg(Color::DarkGray)),
        ]),
        Mode::Shell => Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" run  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" back", Style::default().fg(Color::DarkGray)),
        ]),
        Mode::Ai => Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" send  ", Style::default().fg(Color::DarkGray)),
            Span::styled("↑↓", Style::default().fg(Color::Cyan)),
            Span::styled(" select cmd  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled(" back", Style::default().fg(Color::DarkGray)),
        ]),
    };

    f.render_widget(Paragraph::new(help), area);
}
