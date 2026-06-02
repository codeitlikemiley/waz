//! Global HTTP proxy configuration.
//!
//! See Issue #72: Waz needs a globally configurable proxy setting to uniformly cover all
//! outbound HTTP requests (BYOP model list pulling, autoupdate, loading conversations, etc.).
//!
//! Design Highlights:
//! - Three modes of [`ProxyMode`]: `System` / `Custom` / `Off`.
//! - `System` falls back to the default behavior of reqwest; the workspace's reqwest has enabled
//!   `system-proxy` + `macos-system-configuration` features, so macOS reads from
//!   SystemConfiguration, Windows reads from WinINET, Linux reads from the `HTTP_PROXY` environment variable, etc.,
//!   without requiring custom implementation.
//! - `Custom` explicitly specifies URL / basic auth / no_proxy lists.
//! - `Off` calls [`reqwest::ClientBuilder::no_proxy`], completely disabling proxy settings (including environment variables).
//!
//! The application injects configuration through [`set_global_proxy_config`] at startup / settings change,
//! and all subsequent [`crate::Client::new`] calls will read this global value and apply it to reqwest.
//!
//! reqwest does not support runtime proxy switching for already constructed `Client`s, so callers must
//! reconstruct Client instances after settings changes (e.g. `AutoupdateState::new(http_client::Client::new())`).

use std::sync::{OnceLock, RwLock};

/// Global proxy mode.
///
/// The default is `Off` to avoid a `Client` constructed during cold start from using an
/// unexpected system proxy detected by reqwest before settings are injected in the app layer. app::ProxyMode shares the same default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyMode {
    /// Disable proxy, including environment variables. Default item.
    #[default]
    Off,
    /// Completely follow system / environment variables (reqwest default behavior).
    System,
    /// Use the proxy explicitly configured in [`ProxyConfig::url`].
    Custom,
}

impl ProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::System => "system",
            ProxyMode::Custom => "custom",
            ProxyMode::Off => "off",
        }
    }

    pub fn from_str_lenient(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "system" => ProxyMode::System,
            "custom" => ProxyMode::Custom,
            // off / disabled / none / unknown all fall back to Off (default), avoiding accidental use of system proxy.
            _ => ProxyMode::Off,
        }
    }
}

/// Parsed global proxy configuration.
///
/// `username` is stored in plaintext in settings.toml, while `password` is saved separately
/// through `managed_secrets` (same mode as BYOP API key), injected into [`Self::password`] by the caller before assembling this struct.
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// Example: `http://proxy.corp:8080`. Only active under [`ProxyMode::Custom`].
    pub url: String,
    pub username: String,
    pub password: String,
    /// Comma-separated list of hosts; empty string indicates no exceptions.
    pub no_proxy: String,
}

impl ProxyConfig {
    /// Applies this configuration to `reqwest::ClientBuilder`.
    ///
    /// On error (`Custom` mode but invalid URL), warns in logs and falls back to reqwest default behavior,
    /// preventing `Client::new()` from panicking.
    pub fn apply(&self, mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        match self.mode {
            ProxyMode::System => builder,
            ProxyMode::Off => builder.no_proxy(),
            ProxyMode::Custom => {
                let trimmed = self.url.trim();
                if trimmed.is_empty() {
                    log::warn!("HTTP proxy set to Custom but URL is empty, falling back to reqwest default (system proxy)");
                    return builder;
                }

                let proxy_result = reqwest::Proxy::all(trimmed);
                let mut proxy = match proxy_result {
                    Ok(p) => p,
                    Err(err) => {
                        log::warn!("HTTP proxy URL '{trimmed}' is invalid ({err}), falling back to reqwest default");
                        return builder;
                    }
                };

                if !self.username.is_empty() || !self.password.is_empty() {
                    proxy = proxy.basic_auth(&self.username, &self.password);
                }

                if !self.no_proxy.trim().is_empty() {
                    if let Some(no_proxy) = reqwest::NoProxy::from_string(self.no_proxy.trim()) {
                        proxy = proxy.no_proxy(Some(no_proxy));
                    }
                }

                builder = builder.proxy(proxy);
                builder
            }
        }
    }
}

static GLOBAL_PROXY_CONFIG: OnceLock<RwLock<ProxyConfig>> = OnceLock::new();

fn slot() -> &'static RwLock<ProxyConfig> {
    GLOBAL_PROXY_CONFIG.get_or_init(|| RwLock::new(ProxyConfig::default()))
}

/// Installs a new global proxy configuration.
///
/// Only affects `Client`s constructed after this call. Once a `reqwest::Client` is constructed, its proxy
/// cannot be switched, so the application layer needs to reconstruct all shared Client instances after settings changes.
pub fn set_global_proxy_config(cfg: ProxyConfig) {
    let lock = slot();
    if let Ok(mut guard) = lock.write() {
        *guard = cfg;
    } else {
        log::error!("Failed to write global HTTP proxy configuration: RwLock is poisoned");
    }
}

/// Reads the current global proxy configuration (returns default value if not set).
pub fn current_proxy_config() -> ProxyConfig {
    let lock = slot();
    match lock.read() {
        Ok(guard) => guard.clone(),
        Err(err) => {
            log::error!("Failed to read global HTTP proxy configuration: RwLock is poisoned ({err})");
            ProxyConfig::default()
        }
    }
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
