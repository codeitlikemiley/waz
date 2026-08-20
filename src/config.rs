use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Waz configuration stored at ~/.config/waz/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub llm: LlmConfig,

    #[serde(default)]
    pub generate: GenerateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Rotation strategy: "fallback", "round-robin", or "single"
    #[serde(default = "default_strategy")]
    pub strategy: String,

    /// Default provider name (used when strategy = "single")
    #[serde(default = "default_provider")]
    pub default: String,

    /// Provider order for rotation/fallback
    #[serde(default = "default_order")]
    pub order: Vec<String>,

    /// Timeout in seconds per LLM request
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Provider configurations
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// Provider name: "gemini", "grok", "anthropic", "openai", "ollama", or any custom id.
    pub name: String,

    /// Wire protocol: "openai" | "anthropic" | "gemini" | "ollama".
    /// Empty means infer from `name` (unknown names default to OpenAI-compatible).
    #[serde(default)]
    pub api: String,

    /// Base URL (auto-filled from defaults if omitted)
    #[serde(default)]
    pub base_url: String,

    /// API keys (supports multiple for rotation). Empty is allowed for Ollama
    /// and OpenAI-compatible local proxies.
    #[serde(default)]
    pub keys: Vec<String>,

    /// Model name (auto-filled from defaults if omitted)
    #[serde(default)]
    pub model: String,
}

fn default_strategy() -> String {
    "fallback".into()
}
fn default_provider() -> String {
    "gemini".into()
}
fn default_order() -> Vec<String> {
    vec![
        "gemini".into(),
        "grok".into(),
        "anthropic".into(),
        "codex".into(),
        "openai".into(),
        "glm".into(),
        "qwen".into(),
        "minimax".into(),
        "ollama".into(),
    ]
}
fn default_timeout() -> u64 {
    3
}

/// Config for `waz generate` / `waz schema` commands.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenerateConfig {
    /// Default LLM provider for generation (e.g. "gemini", "glm", "openai").
    /// Overridden by --provider flag.
    #[serde(default)]
    pub provider: Option<String>,

    /// Default model for generation (e.g. "gemini-2.5-pro-preview-05-06").
    /// Overridden by --model flag.
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            default: default_provider(),
            order: default_order(),
            timeout_secs: default_timeout(),
            providers: Vec::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            generate: GenerateConfig::default(),
        }
    }
}

/// Known provider defaults.
pub struct ProviderDefaults;

impl ProviderDefaults {
    pub fn base_url(name: &str) -> &'static str {
        match canonical_name(name).as_str() {
            "gemini" => "https://generativelanguage.googleapis.com/v1beta",
            "glm" => "https://api.z.ai/api/paas/v4",
            "qwen" => "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            "minimax" => "https://api.minimax.io/v1",
            "openai" => "https://api.openai.com/v1",
            "ollama" => "http://localhost:11434",
            "lmstudio" => "http://127.0.0.1:1234/v1",
            "llamacpp" => "http://127.0.0.1:8080/v1",
            "vllm" => "http://127.0.0.1:8000/v1",
            "grok" => "https://api.x.ai/v1",
            "anthropic" => "https://api.anthropic.com",
            "codex" => "https://chatgpt.com/backend-api/codex",
            "openrouter" => "https://openrouter.ai/api/v1",
            "groq" => "https://api.groq.com/openai/v1",
            "deepseek" => "https://api.deepseek.com",
            _ => "",
        }
    }

    pub fn model(name: &str) -> &'static str {
        match canonical_name(name).as_str() {
            "gemini" => "gemini-3.1-flash-lite-preview",
            "glm" => "glm-4.7",
            "qwen" => "qwen3.5-plus",
            "minimax" => "MiniMax-M2.5",
            "openai" => "gpt-4o-mini",
            "ollama" => "llama3.2",
            "lmstudio" => "local",
            "llamacpp" => "local",
            "vllm" => "local",
            "grok" => "grok-3-mini",
            "anthropic" => "claude-sonnet-4-5",
            "codex" => "gpt-5.4-mini",
            "openrouter" => "openai/gpt-4o-mini",
            "groq" => "llama-3.3-70b-versatile",
            "deepseek" => "deepseek-chat",
            _ => "",
        }
    }

    /// Map provider name to env var names to check.
    pub fn env_vars(name: &str) -> Vec<&'static str> {
        match canonical_name(name).as_str() {
            "gemini" => vec!["WAZ_GEMINI_KEY", "GEMINI_API_KEY"],
            "glm" => vec!["WAZ_GLM_KEY", "GLM_API_KEY"],
            "qwen" => vec!["WAZ_QWEN_KEY", "DASHSCOPE_API_KEY"],
            "minimax" => vec!["WAZ_MINIMAX_KEY", "MINIMAX_API_KEY"],
            "openai" => vec!["WAZ_OPENAI_KEY", "OPENAI_API_KEY"],
            "grok" => vec!["WAZ_GROK_KEY", "XAI_API_KEY", "GROK_API_KEY"],
            "anthropic" => vec!["WAZ_ANTHROPIC_KEY", "ANTHROPIC_API_KEY"],
            "openrouter" => vec!["WAZ_OPENROUTER_KEY", "OPENROUTER_API_KEY"],
            "groq" => vec!["WAZ_GROQ_KEY", "GROQ_API_KEY"],
            "deepseek" => vec!["WAZ_DEEPSEEK_KEY", "DEEPSEEK_API_KEY"],
            _ => vec![],
        }
    }

    pub fn known_names() -> &'static [&'static str] {
        &[
            "gemini",
            "grok",
            "anthropic",
            "codex",
            "openai",
            "glm",
            "qwen",
            "minimax",
            "ollama",
            "lmstudio",
            "llamacpp",
            "vllm",
            "openrouter",
            "groq",
            "deepseek",
        ]
    }
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

fn upsert_local(
    config: &mut Config,
    name: &str,
    api: &str,
    base_url: Option<String>,
    model: Option<String>,
) {
    if let Some(existing) = config
        .llm
        .providers
        .iter_mut()
        .find(|p| canonical_name(&p.name) == name)
    {
        if existing.base_url.is_empty() {
            if let Some(base) = base_url {
                existing.base_url = base;
            }
        }
        if existing.model.is_empty() {
            if let Some(m) = model {
                existing.model = m;
            }
        }
        if existing.api.is_empty() {
            existing.api = api.to_string();
        }
        return;
    }
    config.llm.providers.push(ProviderConfig {
        name: name.to_string(),
        api: api.to_string(),
        base_url: base_url.unwrap_or_default(),
        model: model.unwrap_or_default(),
        keys: Vec::new(),
    });
}

/// Enable Ollama / LM Studio / llama.cpp / OPENAI_BASE_URL when env vars say so.
/// Not enabled by default — probing a dead localhost would add seconds of timeout.
fn enable_local_providers(config: &mut Config) {
    if env_enabled("WAZ_OLLAMA")
        || std::env::var("OLLAMA_HOST")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        || std::env::var("WAZ_OLLAMA_MODEL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    {
        upsert_local(
            config,
            "ollama",
            "ollama",
            std::env::var("OLLAMA_HOST").ok().filter(|s| !s.is_empty()),
            std::env::var("WAZ_OLLAMA_MODEL")
                .ok()
                .filter(|s| !s.is_empty()),
        );
    }

    if env_enabled("WAZ_LMSTUDIO")
        || std::env::var("WAZ_LMSTUDIO_MODEL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        || std::env::var("LM_STUDIO_BASE_URL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    {
        upsert_local(
            config,
            "lmstudio",
            "openai",
            std::env::var("LM_STUDIO_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            std::env::var("WAZ_LMSTUDIO_MODEL")
                .ok()
                .filter(|s| !s.is_empty()),
        );
    }

    if env_enabled("WAZ_LLAMACPP")
        || std::env::var("LLAMA_CPP_BASE_URL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        || std::env::var("WAZ_LLAMACPP_MODEL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    {
        upsert_local(
            config,
            "llamacpp",
            "openai",
            std::env::var("LLAMA_CPP_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            std::env::var("WAZ_LLAMACPP_MODEL")
                .ok()
                .filter(|s| !s.is_empty()),
        );
    }

    if env_enabled("WAZ_VLLM")
        || std::env::var("VLLM_BASE_URL")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    {
        upsert_local(
            config,
            "vllm",
            "openai",
            std::env::var("VLLM_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            std::env::var("WAZ_VLLM_MODEL")
                .ok()
                .filter(|s| !s.is_empty()),
        );
    }

    // LiteLLM / LocalAI / any OpenAI-compatible proxy
    if let Ok(base) = std::env::var("OPENAI_BASE_URL") {
        if !base.is_empty() {
            if let Some(existing) = config
                .llm
                .providers
                .iter_mut()
                .find(|p| canonical_name(&p.name) == "openai")
            {
                existing.base_url = base;
            } else {
                config.llm.providers.push(ProviderConfig {
                    name: "openai".into(),
                    api: "openai".into(),
                    base_url: base,
                    model: std::env::var("OPENAI_MODEL").unwrap_or_default(),
                    keys: std::env::var("OPENAI_API_KEY")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .into_iter()
                        .collect(),
                    ..Default::default()
                });
            }
        }
    }
}

/// If OAuth tokens exist, make sure the matching provider is in the rotation.
/// Does not write tokens into config.toml — they live in auth.json.
fn ensure_oauth_providers(config: &mut Config) {
    ensure_oauth_provider(config, "grok", "openai", "grok-4.6");
    ensure_oauth_provider(config, "anthropic", "anthropic", "claude-sonnet-4-5");
    ensure_oauth_provider(config, "codex", "codex", "gpt-5.4-mini");
}

fn ensure_oauth_provider(config: &mut Config, name: &str, api: &str, oauth_model: &str) {
    if !crate::oauth::has_provider(name) {
        return;
    }
    if let Some(existing) = config
        .llm
        .providers
        .iter_mut()
        .find(|p| canonical_name(&p.name) == name)
    {
        if existing.base_url.is_empty() {
            existing.base_url = ProviderDefaults::base_url(name).to_string();
        }
        if existing.api.is_empty() {
            existing.api = api.to_string();
        }
        if existing.model.is_empty() {
            existing.model = oauth_model.to_string();
        }
        return;
    }
    config.llm.providers.push(ProviderConfig {
        name: name.into(),
        api: api.into(),
        base_url: ProviderDefaults::base_url(name).into(),
        model: oauth_model.into(),
        keys: Vec::new(),
    });
}

/// Aliases: xai→grok, claude→anthropic, google→gemini.
pub fn canonical_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "xai" | "x-ai" => "grok".into(),
        "claude" => "anthropic".into(),
        "chatgpt" | "openai-codex" => "codex".into(),
        "google" => "gemini".into(),
        "lm-studio" | "lm_studio" => "lmstudio".into(),
        "llama.cpp" | "llama-cpp" | "llama_cpp" => "llamacpp".into(),
        other => other.to_string(),
    }
}

impl Config {
    /// Load config from ~/.config/waz/config.toml, then overlay env vars.
    pub fn load() -> Self {
        let mut config = Self::load_disk();

        // Auto-detect API keys from env vars for known providers
        for name in ProviderDefaults::known_names() {
            let env_key = ProviderDefaults::env_vars(name)
                .into_iter()
                .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()));

            if let Some(key) = env_key {
                // Find or create provider entry
                if let Some(provider) = config.llm.providers.iter_mut().find(|p| p.name == *name) {
                    // Only add if not already present
                    if !provider.keys.contains(&key) {
                        provider.keys.push(key);
                    }
                } else {
                    config.llm.providers.push(ProviderConfig {
                        name: name.to_string(),
                        keys: vec![key],
                        ..Default::default()
                    });
                }
            }
        }

        enable_local_providers(&mut config);
        ensure_oauth_providers(&mut config);

        // Fill in default base_url and model for any provider that doesn't specify them
        for provider in &mut config.llm.providers {
            if provider.base_url.is_empty() {
                provider.base_url = ProviderDefaults::base_url(&provider.name).to_string();
            }
            if provider.model.is_empty() {
                provider.model = ProviderDefaults::model(&provider.name).to_string();
            }
            if let Ok(host) = std::env::var("OLLAMA_HOST") {
                if canonical_name(&provider.name) == "ollama" && !host.is_empty() {
                    provider.base_url = host;
                }
            }
        }

        config
    }

    pub fn config_path() -> PathBuf {
        if let Ok(p) = std::env::var("WAZ_CONFIG_PATH") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("waz")
            .join("config.toml")
    }

    /// On-disk config only (no env keys, no OAuth injection). Used for save.
    pub fn load_disk() -> Self {
        let path = Self::config_path();
        if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|content| toml::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Config::default()
        }
    }

    pub fn save_disk(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        }
        let body = toml::to_string_pretty(self).map_err(|e| format!("serialize config: {e}"))?;
        fs::write(&path, body).map_err(|e| format!("write config: {e}"))
    }

    /// Get the rotation state file path.
    pub fn rotation_state_path() -> PathBuf {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local").join("share"))
            .join("waz");
        data_dir.join("rotation.json")
    }
}

/// True when this provider can actually serve a request (keys, OAuth, or opted-in local).
pub fn provider_is_ready(name: &str) -> bool {
    let want = canonical_name(name);
    if crate::oauth::has_provider(&want) {
        return true;
    }
    let config = Config::load();
    let Some(provider) = config
        .llm
        .providers
        .iter()
        .find(|p| canonical_name(&p.name) == want)
    else {
        return false;
    };
    if !provider.keys.is_empty() {
        return true;
    }
    match want.as_str() {
        "ollama" | "lmstudio" | "llamacpp" | "vllm" => true,
        "openai" => {
            let base = provider.base_url.to_ascii_lowercase();
            !base.is_empty() && (base.contains("127.0.0.1") || base.contains("localhost"))
        }
        _ => false,
    }
}

fn require_ready(name: &str) -> Result<String, String> {
    let want = canonical_name(name);
    if provider_is_ready(&want) {
        return Ok(want);
    }
    let hint = match want.as_str() {
        "grok" | "anthropic" | "codex" => format!("Run `waz login {want}` first."),
        "ollama" => "Set WAZ_OLLAMA=1 (or OLLAMA_HOST) first.".into(),
        "lmstudio" => "Set WAZ_LMSTUDIO=1 first.".into(),
        "llamacpp" => "Set WAZ_LLAMACPP=1 first.".into(),
        "vllm" => "Set WAZ_VLLM=1 first.".into(),
        _ => format!("Set an API key for {want} or add it under [[llm.providers]] first."),
    };
    Err(format!("cannot use {want}: provider is not set up. {hint}"))
}

fn parse_strategy(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fallback" | "round-robin" | "single" => Ok(value.trim().to_ascii_lowercase()),
        _ => Err("llm.strategy must be fallback, round-robin, or single".into()),
    }
}

fn parse_order(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(canonical_name)
        .collect()
}

/// Effective public settings (no secrets) for `waz config`.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    pub path: String,
    pub strategy: String,
    pub default: String,
    pub default_ready: bool,
    pub order: Vec<String>,
    pub timeout_secs: u64,
    pub generate_provider: Option<String>,
    pub generate_model: Option<String>,
    pub providers: Vec<ConfigProviderView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigProviderView {
    pub name: String,
    pub api: String,
    pub model: String,
    pub base_url: String,
    pub ready: bool,
    pub auth: String,
}

pub fn view() -> ConfigView {
    let loaded = Config::load();
    let path = Config::config_path().display().to_string();
    let default = loaded.llm.default.clone();
    let providers = loaded
        .llm
        .providers
        .iter()
        .map(|p| {
            let name = canonical_name(&p.name);
            let auth = if crate::oauth::has_provider(&name) {
                "oauth"
            } else if !p.keys.is_empty() {
                "key"
            } else {
                "none"
            };
            ConfigProviderView {
                name: p.name.clone(),
                api: if p.api.is_empty() {
                    match crate::llm::api_kind(p) {
                        crate::llm::ApiKind::OpenAi => "openai".into(),
                        crate::llm::ApiKind::Anthropic => "anthropic".into(),
                        crate::llm::ApiKind::Gemini => "gemini".into(),
                        crate::llm::ApiKind::Ollama => "ollama".into(),
                        crate::llm::ApiKind::Codex => "codex".into(),
                    }
                } else {
                    p.api.clone()
                },
                model: p.model.clone(),
                base_url: p.base_url.clone(),
                ready: provider_is_ready(&name),
                auth: auth.into(),
            }
        })
        .collect();
    ConfigView {
        path,
        strategy: loaded.llm.strategy.clone(),
        default: default.clone(),
        default_ready: provider_is_ready(&default),
        order: loaded.llm.order.clone(),
        timeout_secs: loaded.llm.timeout_secs,
        generate_provider: loaded.generate.provider.clone(),
        generate_model: loaded.generate.model.clone(),
        providers,
    }
}

pub fn get_value(key: &str) -> Result<String, String> {
    let v = view();
    match normalize_key(key).as_str() {
        "llm.strategy" => Ok(v.strategy),
        "llm.default" => Ok(v.default),
        "llm.order" => Ok(v.order.join(", ")),
        "llm.timeout_secs" => Ok(v.timeout_secs.to_string()),
        "generate.provider" => Ok(v.generate_provider.unwrap_or_default()),
        "generate.model" => Ok(v.generate_model.unwrap_or_default()),
        "path" => Ok(v.path),
        other if other.starts_with("llm.providers.") => get_provider_field(other),
        _ => Err(format!(
            "unknown key '{key}'. Try: llm.strategy, llm.default, llm.order, llm.timeout_secs, generate.provider, generate.model"
        )),
    }
}

fn get_provider_field(key: &str) -> Result<String, String> {
    // llm.providers.<name>.model | .base_url | .api
    let rest = key.trim_start_matches("llm.providers.");
    let (name, field) = rest
        .rsplit_once('.')
        .ok_or_else(|| format!("unknown key '{key}'"))?;
    let want = canonical_name(name);
    let loaded = Config::load();
    let p = loaded
        .llm
        .providers
        .iter()
        .find(|p| canonical_name(&p.name) == want)
        .ok_or_else(|| format!("provider {want} is not set up"))?;
    match field {
        "model" => Ok(p.model.clone()),
        "base_url" => Ok(p.base_url.clone()),
        "api" => Ok(p.api.clone()),
        "name" => Ok(p.name.clone()),
        _ => Err(format!(
            "unknown provider field '{field}' (model, base_url, api)"
        )),
    }
}

pub fn set_value(key: &str, value: &str) -> Result<String, String> {
    let key = normalize_key(key);
    let value = value.trim();
    if value.is_empty() && key != "generate.provider" && key != "generate.model" {
        return Err("value cannot be empty".into());
    }
    let mut disk = Config::load_disk();
    match key.as_str() {
        "llm.strategy" => {
            disk.llm.strategy = parse_strategy(value)?;
        }
        "llm.default" => {
            disk.llm.default = require_ready(value)?;
        }
        "llm.order" => {
            let order = parse_order(value);
            if order.is_empty() {
                return Err("llm.order cannot be empty".into());
            }
            disk.llm.order = order;
        }
        "llm.timeout_secs" => {
            let n: u64 = value
                .parse()
                .map_err(|_| "llm.timeout_secs must be a positive integer".to_string())?;
            if n == 0 {
                return Err("llm.timeout_secs must be >= 1".into());
            }
            disk.llm.timeout_secs = n;
        }
        "generate.provider" => {
            if value.is_empty() {
                disk.generate.provider = None;
            } else {
                disk.generate.provider = Some(require_ready(value)?);
            }
        }
        "generate.model" => {
            disk.generate.model = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
        other if other.starts_with("llm.providers.") => {
            set_provider_field(&mut disk, other, value)?;
        }
        _ => {
            return Err(format!(
                "unknown key '{key}'. Try: llm.strategy, llm.default, llm.order, llm.timeout_secs, generate.provider, generate.model"
            ));
        }
    }
    disk.save_disk()?;
    get_value(&key)
}

fn set_provider_field(disk: &mut Config, key: &str, value: &str) -> Result<(), String> {
    let rest = key.trim_start_matches("llm.providers.");
    let (name, field) = rest
        .rsplit_once('.')
        .ok_or_else(|| format!("unknown key '{key}'"))?;
    let want = canonical_name(name);
    if !matches!(field, "model" | "base_url" | "api") {
        return Err(format!(
            "unknown provider field '{field}' (model, base_url, api)"
        ));
    }
    if let Some(existing) = disk
        .llm
        .providers
        .iter_mut()
        .find(|p| canonical_name(&p.name) == want)
    {
        match field {
            "model" => existing.model = value.to_string(),
            "base_url" => existing.base_url = value.to_string(),
            "api" => existing.api = value.to_string(),
            _ => {}
        }
        return Ok(());
    }
    disk.llm.providers.push(ProviderConfig {
        name: want,
        api: if field == "api" {
            value.to_string()
        } else {
            String::new()
        },
        base_url: if field == "base_url" {
            value.to_string()
        } else {
            String::new()
        },
        model: if field == "model" {
            value.to_string()
        } else {
            String::new()
        },
        keys: Vec::new(),
    });
    Ok(())
}

/// Pin a provider: strategy=single and llm.default=<provider>. Provider must be set up.
pub fn use_provider(name: &str) -> Result<String, String> {
    let want = require_ready(name)?;
    let mut disk = Config::load_disk();
    disk.llm.strategy = "single".into();
    disk.llm.default = want.clone();
    if !disk.llm.order.iter().any(|n| canonical_name(n) == want) {
        disk.llm.order.insert(0, want.clone());
    }
    disk.save_disk()?;
    Ok(want)
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

pub fn format_view(view: &ConfigView) -> String {
    let mut lines = vec![
        format!("path = {}", view.path),
        format!("llm.strategy = {}", view.strategy),
        format!(
            "llm.default = {}{}",
            view.default,
            if view.default_ready {
                ""
            } else {
                "  # not set up"
            }
        ),
        format!("llm.order = {}", view.order.join(", ")),
        format!("llm.timeout_secs = {}", view.timeout_secs),
    ];
    if let Some(p) = &view.generate_provider {
        lines.push(format!("generate.provider = {p}"));
    }
    if let Some(m) = &view.generate_model {
        lines.push(format!("generate.model = {m}"));
    }
    if view.providers.is_empty() {
        lines.push("providers = (none)".into());
    } else {
        lines.push("providers:".into());
        for p in &view.providers {
            let ready = if p.ready { "ready" } else { "not-ready" };
            lines.push(format!(
                "  - {}  {}  {}  {}  {}",
                p.name, p.auth, ready, p.model, p.base_url
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_config<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("waz-config-{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&dir);
        let cfg = dir.join("config.toml");
        let auth = dir.join("auth.json");
        let old_cfg = std::env::var("WAZ_CONFIG_PATH").ok();
        let old_auth = std::env::var("WAZ_AUTH_PATH").ok();
        std::env::set_var("WAZ_CONFIG_PATH", cfg.to_str().unwrap());
        std::env::set_var("WAZ_AUTH_PATH", auth.to_str().unwrap());
        f();
        match old_cfg {
            Some(v) => std::env::set_var("WAZ_CONFIG_PATH", v),
            None => std::env::remove_var("WAZ_CONFIG_PATH"),
        }
        match old_auth {
            Some(v) => std::env::set_var("WAZ_AUTH_PATH", v),
            None => std::env::remove_var("WAZ_AUTH_PATH"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_strategy_persists() {
        with_temp_config(|| {
            assert_eq!(set_value("llm.strategy", "single").unwrap(), "single");
            assert_eq!(get_value("llm.strategy").unwrap(), "single");
        });
    }

    #[test]
    fn set_rejects_unknown_strategy() {
        with_temp_config(|| {
            let err = set_value("llm.strategy", "fastest").unwrap_err();
            assert!(err.contains("fallback"), "{err}");
        });
    }

    #[test]
    fn set_default_rejects_provider_that_is_not_set_up() {
        with_temp_config(|| {
            let err = set_value("llm.default", "gemini").unwrap_err();
            assert!(err.contains("not set up"), "{err}");
        });
    }

    #[test]
    fn use_provider_rejects_when_not_logged_in() {
        with_temp_config(|| {
            let err = use_provider("grok").unwrap_err();
            assert!(err.contains("waz login grok"), "{err}");
        });
    }

    #[test]
    fn use_provider_pins_when_oauth_exists() {
        with_temp_config(|| {
            let auth = std::env::var("WAZ_AUTH_PATH").unwrap();
            fs::write(
                &auth,
                r#"{
                  "grok": {
                    "access_token": "a",
                    "refresh_token": "r",
                    "expires_at": "2099-01-01T00:00:00Z",
                    "source": "oauth"
                  }
                }"#,
            )
            .unwrap();
            let name = use_provider("xai").unwrap();
            assert_eq!(name, "grok");
            assert_eq!(get_value("llm.strategy").unwrap(), "single");
            assert_eq!(get_value("llm.default").unwrap(), "grok");
        });
    }
}
