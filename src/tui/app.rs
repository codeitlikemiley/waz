use crate::config::Config;
use crate::context::RuntimeContext;

/// TUI operating mode — determined by the first character typed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// No prefix typed yet — show instructions
    Empty,
    /// TMP command palette (triggered by `/`)
    Tmp,
    /// AI chat mode (natural language — any text without prefix)
    Ai,
    /// Shell command mode (triggered by `!`)
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

    // AI placeholder editing state
    pub ai_editing_placeholders: bool,
    pub ai_placeholder_names: Vec<String>,
    pub ai_placeholder_values: Vec<String>,
    pub ai_active_placeholder: usize,
    pub ai_editing_cmd: String,

    // Context
    pub cwd: String,
    pub config: Config,
    pub runtime_context: Option<RuntimeContext>,
    pub scroll_offset: u16,
    pub spinner_tick: usize,
    pub ai_status: String,

    /// Whether TMP commands have been loaded (lazy loading on first `/`)
    pub tmp_loaded: bool,
    pub config_mode: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchemaFile {
    #[serde(default)]
    pub meta: SchemaMeta,
    pub commands: Vec<CommandEntry>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SchemaMeta {
    /// Tool name (e.g. "cargo", "brew")
    #[serde(default)]
    pub tool: String,
    /// Schema version (auto-incremented on regeneration)
    #[serde(default)]
    pub version: u32,
    /// Who generated this: "human", "ai", or "hybrid" (AI-generated, human-verified)
    #[serde(default = "default_generated_by")]
    pub generated_by: String,
    /// Model used for AI generation (e.g. "gemini-2.5-pro-preview-05-06")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_with: Option<String>,
    /// Whether all commands have been human-verified
    #[serde(default)]
    pub verified: bool,
    /// Date of last verification (ISO format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    /// "full" or "partial" coverage of the tool's commands
    #[serde(default = "default_coverage")]
    pub coverage: String,
    /// waz version that created this schema
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waz_version: Option<String>,
    /// Requires a project file to be present (e.g. "Cargo.toml", "package.json")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_file: Option<String>,
    /// Requires a specific runtime file kind (e.g. "cargo_project", "single_file_script")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_file_kind: Option<String>,
    /// Requires a binary on PATH (e.g. "git", "bun")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_binary: Option<String>,
    /// Custom keywords for AI query matching (e.g. ["postgres", "postgresql", "database", "db"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

fn default_generated_by() -> String { "ai".to_string() }
fn default_coverage() -> String { "partial".to_string() }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandEntry {
    pub command: String,
    pub description: String,
    pub tokens: Vec<TokenDef>,
    pub group: String,
    /// Whether this specific command has been human-verified
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    pub token_type: TokenType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    /// CLI flag override (e.g. "-p", "--bin", "-F"). If None, derives from name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// Dynamic data source: run a shell command or built-in resolver at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<DataSource>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataSource {
    /// Shell command to execute (e.g. "brew list --formula")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Built-in resolver name (e.g. "cargo:bins", "git:branches", "npm:scripts")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
    /// How to parse output: "lines" (split by newline) or "words" (split by whitespace)
    #[serde(default = "default_parse_mode")]
    pub parse: String,
}

fn default_parse_mode() -> String { "lines".to_string() }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub fn new(cwd: String, config: Config, runtime_context: Option<RuntimeContext>) -> Self {
        Self {
            mode: Mode::Empty,
            input: String::new(),
            cursor_pos: 0,
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
            ai_editing_placeholders: false,
            ai_placeholder_names: Vec::new(),
            ai_placeholder_values: Vec::new(),
            ai_active_placeholder: 0,
            ai_editing_cmd: String::new(),
            cwd,
            config,
            scroll_offset: 0,
            spinner_tick: 0,
            ai_status: String::new(),
            tmp_loaded: false,
            config_mode: false,
            runtime_context,
        }
    }

    /// Reset back to Empty mode, clearing all state.
    pub fn reset_to_empty(&mut self) {
        self.mode = Mode::Empty;
        self.input.clear();
        self.cursor_pos = 0;
        self.selected_index = 0;
        self.selected_command = None;
        self.editing_tokens = false;
        self.token_values.clear();
        self.active_token = 0;
        self.filtered_commands.clear();
        self.ai_selecting = false;
        self.ai_selected_cmd = 0;
        self.ai_editing_placeholders = false;
        self.ai_placeholder_names.clear();
        self.ai_placeholder_values.clear();
        self.ai_active_placeholder = 0;
        self.ai_editing_cmd.clear();
        self.scroll_offset = 0;
    }

    /// Filter commands based on current input, prioritizing subcommand name matches.
    pub fn filter_commands(&mut self) {
        let query = self.input.to_lowercase();

        if query.is_empty() {
            self.filtered_commands = (0..self.command_list.len()).collect();
        } else {
            // Score each command for relevance — higher is better
            let mut scored: Vec<(usize, u8)> = self.command_list.iter().enumerate()
                .filter_map(|(i, cmd)| {
                    let subcommand = cmd.command
                        .strip_prefix(&format!("{} ", cmd.group))
                        .unwrap_or(&cmd.command)
                        .to_lowercase();
                    let full_cmd = cmd.command.to_lowercase();

                    // Scoring: prioritize subcommand name over description
                    if subcommand == query {
                        Some((i, 10)) // Exact subcommand match
                    } else if subcommand.starts_with(&query) {
                        Some((i, 5)) // Subcommand starts with query
                    } else if subcommand.contains(&query) {
                        Some((i, 3)) // Subcommand contains query
                    } else if full_cmd.starts_with(&query) {
                        Some((i, 4)) // Full command starts with query (e.g. "git commit")
                    } else if full_cmd.contains(&query) {
                        Some((i, 2)) // Full command contains query
                    } else {
                        None // Don't match on description alone — too noisy
                    }
                })
                .collect();

            // Sort by score descending so best matches come first
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_commands = scored.into_iter().map(|(i, _)| i).collect();
        }

        // Always reset selection when filter changes
        self.selected_index = 0;
    }

    /// Select a command and prepare token editing.
pub fn select_command(&mut self) {
    if self.filtered_commands.is_empty() {
        return;
    }
    let idx = self.filtered_commands[self.selected_index];
    self.selected_command = Some(idx);
    
    // Lazily resolve data sources when a command is first selected
    let cwd = self.cwd.clone();
    let runtime_context = self.runtime_context.clone();
    crate::generate::resolve_data_sources_pub_ctx(
        &mut self.command_list[idx],
        &cwd,
        runtime_context.as_ref(),
    );
    
    let cmd = &self.command_list[idx];

    // Pre-fill token values with defaults, then fall back to single resolved values.
    self.token_values = cmd.tokens.iter().map(|t| {
        if let Some(default) = &t.default {
            default.clone()
        } else if let Some(values) = &t.values {
            if values.len() == 1 {
                values[0].clone()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }).collect();

    self.active_token = 0;
    self.editing_tokens = !cmd.tokens.is_empty();
}
    /// Build the final command string from selected command + token values.
pub fn build_command(&self) -> Option<String> {
    let idx = self.selected_command?;
    let cmd = &self.command_list[idx];
    let mut parts = vec![cmd.command.clone()];
    let mut positional_args: Vec<String> = Vec::new();

    for (i, token) in cmd.tokens.iter().enumerate() {
        let value = self.token_values.get(i).cloned().unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        match token.token_type {
            TokenType::Boolean => {
                if value == "true" || value == "yes" {
                    if let Some(ref f) = token.flag {
                        parts.push(f.clone());
                    }
                    // No flag = positional boolean (skip, not meaningful)
                }
            }
            TokenType::Enum | TokenType::String | TokenType::File | TokenType::Number => {
                if let Some(ref f) = token.flag {
                    // Flagged argument: --flag value
                    parts.push(f.clone());
                    parts.push(value);
                } else {
                    // Positional argument: just the value (no flag prefix)
                    positional_args.push(value);
                }
            }
        }
    }

    // Positional args go at the end (after all flags)
    parts.extend(positional_args);

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
                _ => 0,
            };
            if self.selected_index + 1 < max {
                self.selected_index += 1;
            }
        }
    }
}
