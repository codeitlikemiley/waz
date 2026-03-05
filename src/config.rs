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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider name: "gemini", "glm", "qwen", "minimax", "openai", "ollama"
    pub name: String,

    /// Base URL (auto-filled from defaults if omitted)
    #[serde(default)]
    pub base_url: String,

    /// API keys (supports multiple for rotation)
    #[serde(default)]
    pub keys: Vec<String>,

    /// Model name (auto-filled from defaults if omitted)
    #[serde(default)]
    pub model: String,
}

fn default_strategy() -> String { "fallback".into() }
fn default_provider() -> String { "gemini".into() }
fn default_order() -> Vec<String> {
    vec!["gemini".into(), "glm".into(), "qwen".into(), "minimax".into(), "ollama".into()]
}
fn default_timeout() -> u64 { 3 }

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
        match name {
            "gemini" => "https://generativelanguage.googleapis.com/v1beta",
            "glm" => "https://api.z.ai/api/paas/v4",
            "qwen" => "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            "minimax" => "https://api.minimax.io/v1",
            "openai" => "https://api.openai.com/v1",
            "ollama" => "http://localhost:11434",
            _ => "",
        }
    }

    pub fn model(name: &str) -> &'static str {
        match name {
            "gemini" => "gemini-3.1-flash-lite-preview",
            "glm" => "glm-4.7",
            "qwen" => "qwen3.5-plus",
            "minimax" => "MiniMax-M2.5",
            "openai" => "gpt-4o-mini",
            "ollama" => "llama3.2",
            _ => "",
        }
    }

    /// Map provider name to env var names to check.
    pub fn env_vars(name: &str) -> Vec<&'static str> {
        match name {
            "gemini" => vec!["WAZ_GEMINI_KEY", "GEMINI_API_KEY"],
            "glm" => vec!["WAZ_GLM_KEY", "GLM_API_KEY"],
            "qwen" => vec!["WAZ_QWEN_KEY", "DASHSCOPE_API_KEY"],
            "minimax" => vec!["WAZ_MINIMAX_KEY", "MINIMAX_API_KEY"],
            "openai" => vec!["WAZ_OPENAI_KEY", "OPENAI_API_KEY"],
            _ => vec![],
        }
    }
}

impl Config {
    /// Load config from ~/.config/waz/config.toml, then overlay env vars.
    pub fn load() -> Self {
        let path = Self::config_path();
        let mut config = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|content| toml::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Config::default()
        };

        // Auto-detect API keys from env vars for known providers
        let known = ["gemini", "glm", "qwen", "minimax", "openai"];
        for name in &known {
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
                        base_url: String::new(),
                        keys: vec![key],
                        model: String::new(),
                    });
                }
            }
        }

        // Fill in default base_url and model for any provider that doesn't specify them
        for provider in &mut config.llm.providers {
            if provider.base_url.is_empty() {
                provider.base_url = ProviderDefaults::base_url(&provider.name).to_string();
            }
            if provider.model.is_empty() {
                provider.model = ProviderDefaults::model(&provider.name).to_string();
            }
        }

        config
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("waz")
            .join("config.toml")
    }

    /// Get the rotation state file path.
    pub fn rotation_state_path() -> PathBuf {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local").join("share"))
            .join("waz");
        data_dir.join("rotation.json")
    }
}
