//! User interface language setting (persisted via settings.toml, applied to i18n loader at startup).
//!
//! Currently supports English, Simplified Chinese and Japanese. To add a new language just:
//!   1. `Language` plus variant
//!   2. `app/i18n/<locale>/warp.ftl` Create a new translation file
//!   3. `Display` + `to_locale_str` plus case
//!
//! The switch takes full effect after restart (rendered UI text will not be automatically reflowed and requires view reconstruction).
//! The settings page dropdown should be accompanied by a "It will take full effect after restarting Waz" prompt.

use enum_iterator::Sequence;
use serde::{Deserialize, Serialize};
use warp_core::settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Sequence,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "The language used in Waz's user interface.",
    rename_all = "snake_case"
)]
pub enum Language {
    /// Follow the system language; if the system locale is not a supported language, fallback to English.
    #[default]
    #[schemars(description = "System default")]
    System,
    #[schemars(description = "English")]
    English,
    #[schemars(description = "Simplified Chinese")]
    SimplifiedChinese,
    #[schemars(description = "Japanese")]
    Japanese,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Language::System => "System default",
            Language::English => "English",
            Language::SimplifiedChinese => "简体中文",
            Language::Japanese => "日本語",
        };
        write!(f, "{value}")
    }
}

impl Language {
    /// Convert to BCP-47 locale string, `System` returns `None` to perform system detection.
    pub fn to_locale_str(self) -> Option<&'static str> {
        match self {
            Language::System => None,
            Language::English => Some("en"),
            Language::SimplifiedChinese => Some("zh-CN"),
            Language::Japanese => Some("ja"),
        }
    }
}

define_settings_group!(LanguageSettings, settings: [
    language: LanguageState {
        type: Language,
        default: Language::System,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        storage_key: "Language",
        toml_path: "appearance.language",
        description: "The language used in Waz's user interface. Falls back to English when the chosen language is not fully translated.",
    },
]);
