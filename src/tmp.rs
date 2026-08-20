//! Headless TMP for agents and CI: list, show, and build commands without the TUI.

use crate::context::RuntimeContext;
use crate::generate;
use crate::tui::app::{assemble_command, score_command_query, App, CommandEntry, TokenDef};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct TmpList {
    pub cwd: String,
    pub count: usize,
    pub commands: Vec<TmpCommandSummary>,
}

#[derive(Debug, Serialize)]
pub struct TmpCommandSummary {
    pub command: String,
    pub group: String,
    pub description: String,
    pub token_count: usize,
}

#[derive(Debug, Serialize)]
pub struct TmpShow {
    pub cwd: String,
    pub command: String,
    pub group: String,
    pub description: String,
    pub tokens: Vec<TokenDef>,
}

#[derive(Debug, Serialize)]
pub struct TmpBuild {
    pub cwd: String,
    pub command: String,
    pub argv: String,
    pub tokens_filled: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub version: String,
    pub db_path: String,
    pub schemas_dir: String,
    pub curated_embedded: usize,
    pub commands_loaded: usize,
    pub cwd: String,
    pub grok_oauth: crate::oauth::AuthStatus,
    pub oauth: crate::oauth::AllAuthStatus,
}

pub fn list(cwd: &str, query: Option<&str>) -> TmpList {
    let mut commands = load_commands(cwd);
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        let q = q.to_lowercase();
        commands.retain(|c| score_command_query(&c.command, &c.group, &q).is_some());
    }
    let summaries: Vec<TmpCommandSummary> = commands
        .iter()
        .map(|c| TmpCommandSummary {
            command: c.command.clone(),
            group: c.group.clone(),
            description: c.description.clone(),
            token_count: c.tokens.len(),
        })
        .collect();
    TmpList {
        cwd: cwd.to_string(),
        count: summaries.len(),
        commands: summaries,
    }
}

pub fn show(cwd: &str, command: &str) -> Result<TmpShow, String> {
    let mut cmd = find_command(cwd, command)?;
    let ctx = RuntimeContext::detect(cwd, None, None);
    generate::resolve_data_sources_pub_ctx(&mut cmd, cwd, Some(&ctx));
    Ok(TmpShow {
        cwd: cwd.to_string(),
        command: cmd.command.clone(),
        group: cmd.group.clone(),
        description: cmd.description.clone(),
        tokens: cmd.tokens,
    })
}

pub fn build(cwd: &str, command: &str, sets: &[String]) -> Result<TmpBuild, String> {
    let mut cmd = find_command(cwd, command)?;
    let ctx = RuntimeContext::detect(cwd, None, None);
    generate::resolve_data_sources_pub_ctx(&mut cmd, cwd, Some(&ctx));

    let mut values = App::default_token_values(&cmd);
    let mut filled = HashMap::new();
    for (i, token) in cmd.tokens.iter().enumerate() {
        if !values[i].is_empty() {
            filled.insert(token.name.clone(), values[i].clone());
        }
    }

    let mut set_count: HashMap<String, usize> = HashMap::new();
    for spec in sets {
        let (name, value) = spec
            .split_once('=')
            .ok_or_else(|| format!("invalid --set '{spec}' (expected name=value)"))?;
        let idx = cmd
            .tokens
            .iter()
            .position(|t| t.name == name)
            .ok_or_else(|| format!("unknown token '{name}' on {}", cmd.command))?;
        let n = set_count.entry(name.to_string()).or_insert(0);
        if cmd.tokens[idx].repeat && *n > 0 && !values[idx].is_empty() {
            values[idx] = format!("{} {value}", values[idx]);
        } else {
            values[idx] = value.to_string();
        }
        *n += 1;
        filled.insert(name.to_string(), values[idx].clone());
    }

    for (i, token) in cmd.tokens.iter().enumerate() {
        if token.required && values[i].is_empty() {
            return Err(format!(
                "missing required token '{}' on {}",
                token.name, cmd.command
            ));
        }
    }

    Ok(TmpBuild {
        cwd: cwd.to_string(),
        command: cmd.command.clone(),
        argv: assemble_command(&cmd, &values),
        tokens_filled: filled,
    })
}

pub fn doctor(cwd: &str, db_path: &str) -> DoctorReport {
    let commands = load_commands(cwd);
    DoctorReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_path: db_path.to_string(),
        schemas_dir: generate::schemas_dir().display().to_string(),
        curated_embedded: generate::curated_schema_count(),
        commands_loaded: commands.len(),
        cwd: cwd.to_string(),
        grok_oauth: crate::oauth::status_for("grok"),
        oauth: crate::oauth::status_all(),
    }
}

fn load_commands(cwd: &str) -> Vec<CommandEntry> {
    let ctx = RuntimeContext::detect(cwd, None, None);
    generate::load_all_schemas_with_context(cwd, Some(&ctx))
}

fn find_command(cwd: &str, command: &str) -> Result<CommandEntry, String> {
    let commands = load_commands(cwd);
    commands
        .into_iter()
        .find(|c| c.command == command)
        .ok_or_else(|| format!("no TMP command '{command}' loaded for {cwd}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cargo_run_sets_bin() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        let result = build(cwd, "cargo run", &["bin=waz".into()]).unwrap();
        assert!(
            result.argv.contains("--bin waz"),
            "argv should include --bin waz, got {}",
            result.argv
        );
        assert_eq!(
            result.tokens_filled.get("bin").map(String::as_str),
            Some("waz")
        );
    }

    #[test]
    fn list_includes_cargo_in_this_repo() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        let listed = list(cwd, Some("cargo run"));
        assert!(listed.commands.iter().any(|c| c.command == "cargo run"));
    }

    #[test]
    fn list_fuzzy_matches_commit_typo() {
        let cwd = env!("CARGO_MANIFEST_DIR");
        let listed = list(cwd, Some("comit"));
        assert!(
            listed.commands.iter().any(|c| c.command == "git commit"),
            "expected git commit for query comit, got {:?}",
            listed
                .commands
                .iter()
                .map(|c| c.command.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn build_git_add_repeat_appends() {
        let dir = std::env::temp_dir().join(format!("waz-tmp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("git.json"),
            include_str!("../schemas/curated/git.json"),
        )
        .unwrap();
        let old = std::env::var("WAZ_SCHEMAS_DIR").ok();
        std::env::set_var("WAZ_SCHEMAS_DIR", dir.to_str().unwrap());
        let result = build(".", "git add", &["path=a.rs".into(), "path=b.rs".into()]);
        match old {
            Some(v) => std::env::set_var("WAZ_SCHEMAS_DIR", v),
            None => std::env::remove_var("WAZ_SCHEMAS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        let result = result.unwrap();
        assert_eq!(result.argv, "git add a.rs b.rs");
        assert_eq!(
            result.tokens_filled.get("path").map(String::as_str),
            Some("a.rs b.rs")
        );
    }
}
