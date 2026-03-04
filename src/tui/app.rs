use crate::config::Config;

/// TUI operating mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// TMP command palette (triggered by `/`)
    Tmp,
    /// AI chat mode (natural language)
    Ai,
    /// Shell history browser (triggered by `!`)
    Shell,
}

/// Application state for the TUI.
pub struct App {
    pub mode: Mode,
    pub input: String,
    pub cursor_pos: usize,
    pub should_quit: bool,
    pub output_command: Option<String>,

    // TMP mode state
    pub command_list: Vec<CommandEntry>,
    pub filtered_commands: Vec<usize>,
    pub selected_index: usize,
    pub selected_command: Option<usize>,
    pub token_values: Vec<String>,
    pub active_token: usize,
    pub editing_tokens: bool,

    // AI mode state
    pub ai_messages: Vec<AiMessage>,
    pub ai_loading: bool,
    pub ai_commands: Vec<AiCommand>,
    pub ai_selected_cmd: usize,
    pub ai_selecting: bool,

    // Shell mode state
    pub history_entries: Vec<String>,
    pub filtered_history: Vec<usize>,

    // Context
    pub cwd: String,
    pub config: Config,
    pub scroll_offset: u16,
}

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub command: String,
    pub description: String,
    pub tokens: Vec<TokenDef>,
    pub group: String,
}

#[derive(Debug, Clone)]
pub struct TokenDef {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub token_type: TokenType,
    pub default: Option<String>,
    pub values: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    String,
    Boolean,
    Enum,
    File,
    Number,
}

#[derive(Debug, Clone)]
pub struct AiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct AiCommand {
    pub cmd: String,
    pub desc: String,
    pub placeholders: Vec<String>,
}

impl App {
    pub fn new(mode: Mode, cwd: String, config: Config) -> Self {
        Self {
            mode,
            input: match mode {
                Mode::Tmp => "/".to_string(),
                Mode::Shell => "!".to_string(),
                Mode::Ai => String::new(),
            },
            cursor_pos: match mode {
                Mode::Tmp | Mode::Shell => 1,
                Mode::Ai => 0,
            },
            should_quit: false,
            output_command: None,
            command_list: Vec::new(),
            filtered_commands: Vec::new(),
            selected_index: 0,
            selected_command: None,
            token_values: Vec::new(),
            active_token: 0,
            editing_tokens: false,
            ai_messages: Vec::new(),
            ai_loading: false,
            ai_commands: Vec::new(),
            ai_selected_cmd: 0,
            ai_selecting: false,
            history_entries: Vec::new(),
            filtered_history: Vec::new(),
            cwd,
            config,
            scroll_offset: 0,
        }
    }

    /// Filter commands based on current input.
    pub fn filter_commands(&mut self) {
        let query = if self.input.starts_with('/') {
            &self.input[1..]
        } else {
            &self.input
        }.to_lowercase();

        self.filtered_commands = self.command_list.iter().enumerate()
            .filter(|(_, cmd)| {
                if query.is_empty() {
                    true
                } else {
                    cmd.command.to_lowercase().contains(&query)
                        || cmd.description.to_lowercase().contains(&query)
                }
            })
            .map(|(i, _)| i)
            .collect();

        // Reset selection if out of bounds
        if self.selected_index >= self.filtered_commands.len() {
            self.selected_index = 0;
        }
    }

    /// Filter history entries based on input.
    pub fn filter_history(&mut self) {
        let query = if self.input.starts_with('!') {
            &self.input[1..]
        } else {
            &self.input
        }.to_lowercase();

        self.filtered_history = self.history_entries.iter().enumerate()
            .filter(|(_, entry)| {
                if query.is_empty() {
                    true
                } else {
                    entry.to_lowercase().contains(&query)
                }
            })
            .map(|(i, _)| i)
            .collect();

        if self.selected_index >= self.filtered_history.len() {
            self.selected_index = 0;
        }
    }

    /// Select a command and prepare token editing.
    pub fn select_command(&mut self) {
        if self.filtered_commands.is_empty() {
            return;
        }
        let idx = self.filtered_commands[self.selected_index];
        self.selected_command = Some(idx);
        let cmd = &self.command_list[idx];

        // Pre-fill token values with defaults
        self.token_values = cmd.tokens.iter().map(|t| {
            t.default.clone().unwrap_or_default()
        }).collect();

        self.active_token = 0;
        self.editing_tokens = !cmd.tokens.is_empty();
    }

    /// Build the final command string from selected command + token values.
    pub fn build_command(&self) -> Option<String> {
        let idx = self.selected_command?;
        let cmd = &self.command_list[idx];
        let mut parts = vec![cmd.command.clone()];

        for (i, token) in cmd.tokens.iter().enumerate() {
            let value = self.token_values.get(i).cloned().unwrap_or_default();
            if value.is_empty() {
                continue;
            }
            match token.token_type {
                TokenType::Boolean => {
                    if value == "true" || value == "yes" {
                        parts.push(format!("--{}", token.name));
                    }
                }
                TokenType::Enum | TokenType::String | TokenType::File | TokenType::Number => {
                    if token.name.len() == 1 {
                        parts.push(format!("-{}", token.name));
                    } else {
                        parts.push(format!("--{}", token.name));
                    }
                    parts.push(value);
                }
            }
        }

        Some(parts.join(" "))
    }

    pub fn move_up(&mut self) {
        if self.editing_tokens {
            if self.active_token > 0 {
                self.active_token -= 1;
            }
        } else if self.ai_selecting {
            if self.ai_selected_cmd > 0 {
                self.ai_selected_cmd -= 1;
            }
        } else if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.editing_tokens {
            let max = self.token_values.len().saturating_sub(1);
            if self.active_token < max {
                self.active_token += 1;
            }
        } else if self.ai_selecting {
            if self.ai_selected_cmd + 1 < self.ai_commands.len() {
                self.ai_selected_cmd += 1;
            }
        } else {
            let max = match self.mode {
                Mode::Tmp => self.filtered_commands.len(),
                Mode::Shell => self.filtered_history.len(),
                _ => 0,
            };
            if self.selected_index + 1 < max {
                self.selected_index += 1;
            }
        }
    }
}
