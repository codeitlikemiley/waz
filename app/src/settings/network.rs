//! Global HTTP network proxy settings.
//!
//! See Issue #72. Provides a user-configurable global proxy setting, which is injected into
//! the egress points of `http_client::Client` and `websocket`, covering all outbound HTTP/WS requests
//! such as BYOP calls, autoupdates, conversation loading, MCP OAuth, cloud workflow fetch, etc.
//!
//! Three fields:
//! - `proxy_mode`: `system` / `custom` / `off` (default is `system`, equivalent to the
//!   existing behavior of reqwest).
//! - `proxy_url`: used in `Custom` mode, e.g., `http://proxy.corp:8080`.
//! - `proxy_no_proxy`: comma-separated list of host exceptions, e.g., `localhost,127.0.0.1,.internal`.
//!
//! Username / password are not here: username will be placed in a separate setting (or written in the URL),
//! and password goes to `managed_secrets` (same pattern as BYOP API keys), which is managed separately by the UI.
//!
//! To simplify the first version, the username field is also provided here; password is still managed by managed_secrets.

use serde::{Deserialize, Serialize};
use settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

/// User-visible proxy mode.
///
/// Corresponds one-to-one with `http_client::ProxyMode` / `websocket::ProxyMode`. The reason for
/// defining it separately is to decouple the configuration layer from the infrastructure layer,
/// and this type needs to implement traits required by the settings system such as `JsonSchema`.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "HTTP proxy mode: off completely disabled (default); system follows system/environment; custom uses explicit URL.",
    rename_all = "snake_case"
)]
pub enum ProxyMode {
    /// Force disable proxy, including environment variables. Default item; prevents unexpected system proxies detected by reqwest from interfering with local calls.
    #[default]
    Off,
    /// Follow system proxy / environment variables (reqwest default behavior).
    System,
    /// Use the URL entered by the user.
    Custom,
}

impl ProxyMode {
    /// Convert to `http_client::ProxyMode`.
    pub fn to_http_client_mode(self) -> http_client::ProxyMode {
        match self {
            ProxyMode::System => http_client::ProxyMode::System,
            ProxyMode::Custom => http_client::ProxyMode::Custom,
            ProxyMode::Off => http_client::ProxyMode::Off,
        }
    }

    /// Convert to `websocket::ProxyMode` (independent mirror, see comments at the top of websocket/proxy.rs).
    pub fn to_websocket_mode(self) -> websocket::ProxyMode {
        match self {
            ProxyMode::System => websocket::ProxyMode::System,
            ProxyMode::Custom => websocket::ProxyMode::Custom,
            ProxyMode::Off => websocket::ProxyMode::Off,
        }
    }
}

define_settings_group!(NetworkSettings, settings: [
    proxy_mode: ProxyModeSetting {
        type: ProxyMode,
        default: ProxyMode::Off,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "network.proxy_mode",
        description: "HTTP proxy mode: off (default) / system / custom.",
    },
    proxy_url: ProxyUrlSetting {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "network.proxy_url",
        description: "Proxy URL in Custom mode, e.g.: http://proxy.corp:8080.",
    },
    proxy_username: ProxyUsernameSetting {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "network.proxy_username",
        description: "Proxy username in Custom mode; empty means no basic auth or no username.",
    },
    proxy_no_proxy: ProxyNoProxySetting {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        toml_path: "network.proxy_no_proxy",
        description: "Comma-separated list of host exceptions, e.g.: localhost,127.0.0.1,.internal.",
    },
]);
