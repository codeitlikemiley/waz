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

fn default_generated_by() -> String {
    "ai".to_string()
}
fn default_coverage() -> String {
    "partial".to_string()
}

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
    /// If true, whitespace-split the value and emit each piece (multi `git add` paths).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
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
    /// Re-resolve this source when the named sibling token changes (e.g. `"provider"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<String>,
}

fn default_parse_mode() -> String {
    "lines".to_string()
}

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
            let mut scored: Vec<(usize, u8)> = self
                .command_list
                .iter()
                .enumerate()
                .filter_map(|(i, cmd)| {
                    score_command_query(&cmd.command, &cmd.group, &query).map(|s| (i, s))
                })
                .collect();

            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_commands = scored.into_iter().map(|(i, _)| i).collect();
        }

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
        self.token_values = Self::default_token_values(cmd);

        self.active_token = 0;
        self.editing_tokens = !cmd.tokens.is_empty();
    }
    /// Build the final command string from selected command + token values.
    pub fn build_command(&self) -> Option<String> {
        let idx = self.selected_command?;
        Some(assemble_command(
            &self.command_list[idx],
            &self.token_values,
        ))
    }

    /// Default token values used when a command is selected (defaults, else a unique enum).
    pub fn default_token_values(cmd: &CommandEntry) -> Vec<String> {
        cmd.tokens
            .iter()
            .map(|t| {
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
            })
            .collect()
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

/// Serialize a TMP command + filled tokens to a shell argv string.
/// Flags first, then positionals — this order is part of tmp/1.
/// Score a TMP command against a query. Higher is better. `None` = no match.
/// Does not search descriptions.
pub fn score_command_query(command: &str, group: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_lowercase();
    let subcommand = command
        .strip_prefix(&format!("{group} "))
        .unwrap_or(command)
        .to_lowercase();
    let full_cmd = command.to_lowercase();

    if subcommand == query {
        Some(10)
    } else if subcommand.starts_with(&query) {
        Some(5)
    } else if full_cmd.starts_with(&query) {
        Some(4)
    } else if subcommand.contains(&query) {
        Some(3)
    } else if full_cmd.contains(&query) {
        Some(2)
    } else if query.len() >= 3 && edit_distance_at_most_one(&subcommand, &query) {
        Some(1)
    } else {
        None
    }
}

fn edit_distance_at_most_one(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 {
        return false;
    }
    if short.len() == long.len() {
        return short
            .iter()
            .zip(long.iter())
            .filter(|(x, y)| x != y)
            .count()
            == 1;
    }
    let mut i = 0;
    let mut j = 0;
    let mut skipped = false;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
        } else if !skipped {
            skipped = true;
            j += 1;
        } else {
            return false;
        }
    }
    true
}

pub fn assemble_command(cmd: &CommandEntry, token_values: &[String]) -> String {
    let mut parts = vec![cmd.command.clone()];
    let mut positional_args: Vec<String> = Vec::new();

    for (i, token) in cmd.tokens.iter().enumerate() {
        let value = token_values.get(i).cloned().unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        match token.token_type {
            TokenType::Boolean => {
                if value == "true" || value == "yes" {
                    if let Some(ref f) = token.flag {
                        parts.push(f.clone());
                    }
                }
            }
            TokenType::Enum | TokenType::String | TokenType::File | TokenType::Number => {
                let pieces: Vec<&str> = if token.repeat {
                    value.split_whitespace().filter(|s| !s.is_empty()).collect()
                } else {
                    vec![value.as_str()]
                };
                for piece in pieces {
                    if let Some(ref f) = token.flag {
                        parts.push(f.clone());
                        parts.push(piece.to_string());
                    } else {
                        positional_args.push(piece.to_string());
                    }
                }
            }
        }
    }

    parts.extend(positional_args);
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn token(
        name: &str,
        token_type: TokenType,
        flag: Option<&str>,
        default: Option<&str>,
    ) -> TokenDef {
        TokenDef {
            name: name.to_string(),
            description: name.to_string(),
            required: false,
            token_type,
            default: default.map(|s| s.to_string()),
            values: None,
            flag: flag.map(|s| s.to_string()),
            data_source: None,
            repeat: false,
        }
    }

    fn app_with(cmd: CommandEntry, values: Vec<&str>) -> App {
        let mut app = App::new("/tmp".into(), Config::default(), None);
        app.command_list.push(cmd);
        app.filtered_commands = vec![0];
        app.selected_index = 0;
        app.selected_command = Some(0);
        app.token_values = values.into_iter().map(|s| s.to_string()).collect();
        app
    }

    #[test]
    fn build_command_git_commit_omits_false_booleans() {
        let cmd = CommandEntry {
            command: "git commit".into(),
            description: "Record changes".into(),
            group: "git".into(),
            verified: true,
            tokens: vec![
                token("message", TokenType::String, Some("-m"), None),
                token("amend", TokenType::Boolean, Some("--amend"), Some("false")),
                token(
                    "no-edit",
                    TokenType::Boolean,
                    Some("--no-edit"),
                    Some("false"),
                ),
            ],
        };
        let app = app_with(cmd, vec!["fix login", "false", "false"]);
        assert_eq!(
            app.build_command().as_deref(),
            Some("git commit -m fix login")
        );
    }

    #[test]
    fn build_command_git_commit_amend() {
        let cmd = CommandEntry {
            command: "git commit".into(),
            description: "Record changes".into(),
            group: "git".into(),
            verified: true,
            tokens: vec![
                token("message", TokenType::String, Some("-m"), None),
                token("amend", TokenType::Boolean, Some("--amend"), Some("false")),
            ],
        };
        let app = app_with(cmd, vec!["fix", "true"]);
        assert_eq!(
            app.build_command().as_deref(),
            Some("git commit -m fix --amend")
        );
    }

    #[test]
    fn build_command_cargo_run_bin_flag() {
        let cmd = CommandEntry {
            command: "cargo run".into(),
            description: "Run".into(),
            group: "cargo".into(),
            verified: true,
            tokens: vec![
                token("bin", TokenType::Enum, Some("--bin"), None),
                token(
                    "release",
                    TokenType::Boolean,
                    Some("--release"),
                    Some("false"),
                ),
            ],
        };
        let app = app_with(cmd, vec!["waz", "false"]);
        assert_eq!(app.build_command().as_deref(), Some("cargo run --bin waz"));
    }

    #[test]
    fn build_command_positional_after_flags() {
        let cmd = CommandEntry {
            command: "git add".into(),
            description: "Stage".into(),
            group: "git".into(),
            verified: true,
            tokens: vec![
                token("force", TokenType::Boolean, Some("-f"), Some("false")),
                token("path", TokenType::File, None, Some(".")),
            ],
        };
        let app = app_with(cmd, vec!["true", "src/main.rs"]);
        assert_eq!(
            app.build_command().as_deref(),
            Some("git add -f src/main.rs")
        );
    }

    #[test]
    fn build_command_repeat_splits_positionals() {
        let mut path = token("path", TokenType::File, None, None);
        path.repeat = true;
        let cmd = CommandEntry {
            command: "git add".into(),
            description: "Stage".into(),
            group: "git".into(),
            verified: true,
            tokens: vec![path],
        };
        let app = app_with(cmd, vec!["src/a.rs src/b.rs"]);
        assert_eq!(
            app.build_command().as_deref(),
            Some("git add src/a.rs src/b.rs")
        );
    }

    #[test]
    fn build_command_without_repeat_keeps_one_positional() {
        let cmd = CommandEntry {
            command: "echo".into(),
            description: "Echo".into(),
            group: "echo".into(),
            verified: true,
            tokens: vec![token("msg", TokenType::String, None, None)],
        };
        let app = app_with(cmd, vec!["foo bar"]);
        assert_eq!(app.build_command().as_deref(), Some("echo foo bar"));
    }

    #[test]
    fn build_command_repeat_repeats_flag() {
        let mut feat = token("features", TokenType::Enum, Some("-F"), None);
        feat.repeat = true;
        let cmd = CommandEntry {
            command: "cargo build".into(),
            description: "Build".into(),
            group: "cargo".into(),
            verified: true,
            tokens: vec![feat],
        };
        let app = app_with(cmd, vec!["json yaml"]);
        assert_eq!(
            app.build_command().as_deref(),
            Some("cargo build -F json -F yaml")
        );
    }

    #[test]
    fn score_fuzzy_commit_typo() {
        assert_eq!(score_command_query("git commit", "git", "comit"), Some(1));
        assert_eq!(score_command_query("git commit", "git", "commit"), Some(10));
        assert!(score_command_query("git commit", "git", "xyzzy").is_none());
        // Queries shorter than 3 do not use fuzzy; "zz" is not a substring either.
        assert!(score_command_query("git commit", "git", "zz").is_none());
    }

    #[test]
    fn curated_cargo_schema_run_flags() {
        let schema: SchemaFile =
            serde_json::from_str(include_str!("../../schemas/curated/cargo.json")).unwrap();
        assert_eq!(schema.commands.len(), 12);
        let run = schema
            .commands
            .iter()
            .find(|c| c.command == "cargo run")
            .unwrap();
        assert_eq!(
            run.tokens
                .iter()
                .find(|t| t.name == "bin")
                .unwrap()
                .flag
                .as_deref(),
            Some("--bin")
        );
        assert_eq!(
            run.tokens
                .iter()
                .find(|t| t.name == "release")
                .unwrap()
                .flag
                .as_deref(),
            Some("--release")
        );
    }
}
