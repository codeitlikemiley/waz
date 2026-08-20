//! Agent Plugins v1.0.0 client (https://agent-plugins.org/).
//!
//! Loads `plugin.json`, discovers `skills/*/SKILL.md`, and reads `mcp.json`.
//! Skills-only loading is enough for generate; MCP is launched via `waz mcp`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const PLUGIN_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

const BUNDLED_PLUGIN_JSON: &str = include_str!("../plugins/waz/plugin.json");
const BUNDLED_MCP_JSON: &str = include_str!("../plugins/waz/mcp.json");
const BUNDLED_TMP_SKILL: &str = include_str!("../plugins/waz/skills/tmp-schema/SKILL.md");
const BUNDLED_TMP_CONTRACT: &str =
    include_str!("../plugins/waz/skills/tmp-schema/references/tmp-contract.md");
const BUNDLED_TMP_USE: &str = include_str!("../plugins/waz/skills/tmp-use/SKILL.md");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plugin {
    pub root: PathBuf,
    pub source: String,
    pub manifest: PluginManifest,
    pub skills: Vec<Skill>,
    pub has_mcp: bool,
}

pub fn bundled_tmp_schema_prompt() -> String {
    format!("{BUNDLED_TMP_SKILL}\n\n{}", BUNDLED_TMP_CONTRACT)
}

pub fn plugins_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WAZ_PLUGINS_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("waz")
        .join("plugins")
}

fn waz_command() -> (String, Vec<String>) {
    if let Ok(exe) = std::env::current_exe() {
        (exe.display().to_string(), vec!["mcp".into()])
    } else {
        ("waz".into(), vec!["mcp".into()])
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Write MCP launch config into a known agent client. Does not require that
/// client to speak Agent Plugins — stdio MCP is the compatibility layer.
pub fn connect_client(client: &str) -> Result<String, String> {
    let _ = install_bundled();
    let (command, args) = waz_command();
    match client.trim().to_ascii_lowercase().as_str() {
        "gemini" | "gemini-cli" | "google" => {
            let path = std::env::var("WAZ_GEMINI_SETTINGS")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".gemini/settings.json"));
            merge_mcp_json(&path, "mcpServers", &command, &args)?;
            Ok(format!("gemini → {}", path.display()))
        }
        "claude" | "claude-code" | "anthropic" => {
            let path = std::env::var("WAZ_CLAUDE_CONFIG")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".claude.json"));
            merge_mcp_json(&path, "mcpServers", &command, &args)?;
            Ok(format!("claude → {}", path.display()))
        }
        "codex" | "chatgpt" | "openai" => {
            let path = std::env::var("WAZ_CODEX_CONFIG")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".codex/config.toml"));
            merge_mcp_toml(&path, &command, &args)?;
            Ok(format!("codex → {}", path.display()))
        }
        "cursor" => {
            let path = std::env::var("WAZ_CURSOR_MCP")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".cursor/mcp.json"));
            merge_mcp_json(&path, "mcpServers", &command, &args)?;
            Ok(format!("cursor → {}", path.display()))
        }
        "all" => {
            let mut lines = Vec::new();
            for name in ["gemini", "claude", "codex", "cursor"] {
                match connect_client(name) {
                    Ok(s) => lines.push(s),
                    Err(e) => lines.push(format!("{name}: skipped ({e})")),
                }
            }
            Ok(lines.join("\n"))
        }
        other => Err(format!(
            "unknown client '{other}'. Use: gemini, claude, codex, cursor, all"
        )),
    }
}

fn merge_mcp_json(path: &Path, key: &str, command: &str, args: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut root: serde_json::Value = if path.exists() {
        let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?
        }
    } else {
        serde_json::json!({})
    };
    if !root.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    let entry = serde_json::json!({
        "command": command,
        "args": args,
    });
    {
        let obj = root.as_object_mut().unwrap();
        let servers = obj.entry(key).or_insert_with(|| serde_json::json!({}));
        if !servers.is_object() {
            *servers = serde_json::json!({});
        }
        servers.as_object_mut().unwrap().insert("waz".into(), entry);
    }
    let body = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    fs::write(path, body + "\n").map_err(|e| e.to_string())
}

fn merge_mcp_toml(path: &Path, command: &str, args: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut root: toml::Value = if path.exists() {
        let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if raw.trim().is_empty() {
            toml::Value::Table(toml::map::Map::new())
        } else {
            raw.parse::<toml::Value>()
                .map_err(|e| format!("{}: {e}", path.display()))?
        }
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let table = root
        .as_table_mut()
        .ok_or_else(|| format!("{} is not a TOML table", path.display()))?;
    let servers = table
        .entry("mcp_servers")
        .or_insert(toml::Value::Table(toml::map::Map::new()));
    let servers = servers
        .as_table_mut()
        .ok_or_else(|| "mcp_servers is not a table".to_string())?;
    let mut waz = toml::map::Map::new();
    waz.insert("command".into(), toml::Value::String(command.into()));
    waz.insert(
        "args".into(),
        toml::Value::Array(
            args.iter()
                .map(|a| toml::Value::String(a.clone()))
                .collect(),
        ),
    );
    servers.insert("waz".into(), toml::Value::Table(waz));
    let body = toml::to_string_pretty(&root).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

/// Copy the bundled waz plugin into the user plugins dir (idempotent).
pub fn install_bundled() -> Result<PathBuf, String> {
    let root = plugins_dir().join("waz");
    fs::create_dir_all(root.join("skills/tmp-schema/references"))
        .map_err(|e| format!("create plugin dir: {e}"))?;
    fs::write(root.join("plugin.json"), BUNDLED_PLUGIN_JSON)
        .map_err(|e| format!("write plugin.json: {e}"))?;
    fs::write(root.join("mcp.json"), BUNDLED_MCP_JSON)
        .map_err(|e| format!("write mcp.json: {e}"))?;
    fs::write(root.join("skills/tmp-schema/SKILL.md"), BUNDLED_TMP_SKILL)
        .map_err(|e| format!("write SKILL.md: {e}"))?;
    fs::write(
        root.join("skills/tmp-schema/references/tmp-contract.md"),
        BUNDLED_TMP_CONTRACT,
    )
    .map_err(|e| format!("write tmp-contract.md: {e}"))?;
    fs::create_dir_all(root.join("skills/tmp-use")).map_err(|e| format!("create tmp-use: {e}"))?;
    fs::write(root.join("skills/tmp-use/SKILL.md"), BUNDLED_TMP_USE)
        .map_err(|e| format!("write tmp-use skill: {e}"))?;
    Ok(root)
}

pub fn discover() -> Vec<Plugin> {
    let _ = install_bundled();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(plugins_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match load_plugin(&path, "user") {
                Ok(p) => {
                    seen.insert(p.manifest.name.clone());
                    out.push(p);
                }
                Err(e) => eprintln!("warning: skip plugin {}: {e}", path.display()),
            }
        }
    }
    if !seen.contains("waz") {
        if let Ok(p) = load_bundled_in_memory() {
            out.insert(0, p);
        }
    }
    out
}

fn load_bundled_in_memory() -> Result<Plugin, String> {
    let manifest: PluginManifest = serde_json::from_str(BUNDLED_PLUGIN_JSON)
        .map_err(|e| format!("bundled plugin.json: {e}"))?;
    validate_manifest(&manifest)?;
    Ok(Plugin {
        root: PathBuf::from("(bundled)"),
        source: "bundled".into(),
        manifest,
        skills: vec![
            parse_skill_md(
                "tmp-schema",
                BUNDLED_TMP_SKILL,
                PathBuf::from("(bundled)/skills/tmp-schema"),
            ),
            parse_skill_md(
                "tmp-use",
                BUNDLED_TMP_USE,
                PathBuf::from("(bundled)/skills/tmp-use"),
            ),
        ],
        has_mcp: true,
    })
}

pub fn load_plugin(root: &Path, source: &str) -> Result<Plugin, String> {
    let resolved = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let manifest_path = resolved.join("plugin.json");
    if !manifest_path.is_file() {
        return Err("missing plugin.json".into());
    }
    let raw = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
    let manifest: PluginManifest =
        serde_json::from_str(&raw).map_err(|e| format!("plugin.json: {e}"))?;
    validate_manifest(&manifest)?;
    if manifest.schema != PLUGIN_SCHEMA {
        return Err(format!("unsupported plugin $schema: {}", manifest.schema));
    }
    let skills = discover_skills(&resolved);
    let mcp_path = resolved.join("mcp.json");
    let has_mcp = if mcp_path.is_file() {
        match fs::read_to_string(&mcp_path) {
            Ok(s) => serde_json::from_str::<serde_json::Value>(&s)
                .ok()
                .and_then(|v| {
                    v.get("$schema")
                        .and_then(|s| s.as_str())
                        .map(|s| s == MCP_SCHEMA)
                })
                .unwrap_or(false),
            Err(_) => false,
        }
    } else {
        false
    };
    Ok(Plugin {
        root: resolved,
        source: source.into(),
        manifest,
        skills,
        has_mcp,
    })
}

fn validate_manifest(m: &PluginManifest) -> Result<(), String> {
    if m.name.is_empty() || m.name.len() > 64 {
        return Err("plugin name must be 1-64 characters".into());
    }
    let bytes = m.name.as_bytes();
    let first = *bytes.first().unwrap();
    let last = *bytes.last().unwrap();
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return Err("plugin name must start and end alphanumeric".into());
    }
    if m.name.contains("--") || m.name.contains("..") {
        return Err("plugin name cannot contain -- or ..".into());
    }
    if !m
        .name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err("plugin name must be lowercase alphanumeric, hyphen, or period".into());
    }
    Ok(())
}

fn discover_skills(root: &Path) -> Vec<Skill> {
    let skills_dir = root.join("skills");
    let Ok(entries) = fs::read_dir(&skills_dir) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let Ok(body) = fs::read_to_string(&skill_md) else {
            continue;
        };
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("skill")
            .to_string();
        skills.push(parse_skill_md(&name, &body, dir));
    }
    skills
}

fn parse_skill_md(dir_name: &str, body: &str, dir: PathBuf) -> Skill {
    let (name, description, rest) = parse_frontmatter(body);
    Skill {
        name: name.unwrap_or_else(|| dir_name.to_string()),
        description: description.unwrap_or_default(),
        body: rest,
        dir,
    }
}

fn parse_frontmatter(body: &str) -> (Option<String>, Option<String>, String) {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None, body.to_string());
    }
    let after = &trimmed[3..];
    let Some(end) = after.find("\n---") else {
        return (None, None, body.to_string());
    };
    let fm = &after[..end];
    let rest = after[end + 4..].trim_start().to_string();
    let mut name = None;
    let mut description = None;
    let mut desc_buf = String::new();
    let mut in_desc = false;
    for line in fm.lines() {
        if in_desc {
            if let Some(rest) = line.strip_prefix("  ") {
                desc_buf.push(' ');
                desc_buf.push_str(rest.trim());
                continue;
            }
            in_desc = false;
            description = Some(desc_buf.trim().to_string());
            desc_buf.clear();
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            let v = v.trim();
            if v == ">" || v == "|" {
                in_desc = true;
            } else {
                description = Some(v.trim_matches('"').to_string());
            }
        }
    }
    if in_desc && !desc_buf.is_empty() {
        description = Some(desc_buf.trim().to_string());
    }
    (name, description, rest)
}

pub fn skill_text(plugin_name: &str, skill_name: &str) -> Option<String> {
    if plugin_name == "waz" && skill_name == "tmp-schema" {
        return Some(bundled_tmp_schema_prompt());
    }
    let plugins = discover();
    let p = plugins.iter().find(|p| p.manifest.name == plugin_name)?;
    let s = p.skills.iter().find(|s| s.name == skill_name)?;
    let mut extra = String::new();
    let refs = s.dir.join("references");
    if let Ok(entries) = fs::read_dir(&refs) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Ok(body) = fs::read_to_string(&path) {
                    extra.push_str("\n\n");
                    extra.push_str(&body);
                }
            }
        }
    }
    Some(format!("{}{extra}", s.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid() {
        let m: PluginManifest = serde_json::from_str(BUNDLED_PLUGIN_JSON).unwrap();
        validate_manifest(&m).unwrap();
        assert_eq!(m.name, "waz");
        assert_eq!(m.schema, PLUGIN_SCHEMA);
    }

    #[test]
    fn bundled_mcp_schema_matches() {
        let v: serde_json::Value = serde_json::from_str(BUNDLED_MCP_JSON).unwrap();
        assert_eq!(v["$schema"], MCP_SCHEMA);
        assert_eq!(v["mcpServers"]["waz"]["type"], "stdio");
        assert_eq!(v["mcpServers"]["waz"]["command"], "waz");
    }

    #[test]
    fn tmp_schema_skill_has_frontmatter() {
        let (name, desc, body) = parse_frontmatter(BUNDLED_TMP_SKILL);
        assert_eq!(name.as_deref(), Some("tmp-schema"));
        assert!(desc.unwrap().contains("TMP"));
        assert!(body.contains("JSON array"));
    }

    #[test]
    fn reject_invalid_plugin_name() {
        let m = PluginManifest {
            schema: PLUGIN_SCHEMA.into(),
            name: "My-Plugin".into(),
            version: None,
            description: None,
            license: None,
            keywords: vec![],
        };
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn install_bundled_writes_skills_and_mcp() {
        let dir = std::env::temp_dir().join(format!("waz-plug-{}", uuid::Uuid::new_v4()));
        std::env::set_var("WAZ_PLUGINS_DIR", dir.to_str().unwrap());
        let root = install_bundled().unwrap();
        assert!(root.join("plugin.json").is_file());
        assert!(root.join("mcp.json").is_file());
        assert!(root.join("skills/tmp-schema/SKILL.md").is_file());
        assert!(root.join("skills/tmp-use/SKILL.md").is_file());
        let plugins = discover();
        assert!(plugins.iter().any(|p| p.manifest.name == "waz"));
        assert!(plugins
            .iter()
            .any(|p| p.skills.iter().any(|s| s.name == "tmp-use")));
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("WAZ_PLUGINS_DIR");
    }

    #[test]
    fn connect_gemini_merges_mcp_servers_without_clobber() {
        let dir = std::env::temp_dir().join(format!("waz-gem-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        fs::write(
            &settings,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#,
        )
        .unwrap();
        std::env::set_var("WAZ_GEMINI_SETTINGS", settings.to_str().unwrap());
        let msg = connect_client("gemini").unwrap();
        assert!(msg.contains("gemini"));
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["waz"]["args"][0], "mcp");
        std::env::remove_var("WAZ_GEMINI_SETTINGS");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_codex_writes_toml_table() {
        let dir = std::env::temp_dir().join(format!("waz-codex-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");
        std::env::set_var("WAZ_CODEX_CONFIG", cfg.to_str().unwrap());
        connect_client("openai").unwrap();
        let raw = fs::read_to_string(&cfg).unwrap();
        assert!(raw.contains("[mcp_servers.waz]") || raw.contains("mcp_servers"));
        assert!(raw.contains("mcp"));
        std::env::remove_var("WAZ_CODEX_CONFIG");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_rejects_unknown_client() {
        let err = connect_client("notepad").unwrap_err();
        assert!(err.contains("gemini"));
    }
}
