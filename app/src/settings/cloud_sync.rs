use serde::{Deserialize, Serialize};
use settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

/// Cloud synchronization platform selection
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
#[serde(rename_all = "snake_case")]
pub enum SyncPlatformSetting {
    #[default]
    GitHub,
    Gitee,
}

impl SyncPlatformSetting {
    /// Convert to waz_sync::SyncPlatform
    pub fn to_sync_platform(self) -> waz_sync::SyncPlatform {
        match self {
            Self::GitHub => waz_sync::SyncPlatform::GitHub,
            Self::Gitee => waz_sync::SyncPlatform::Gitee,
        }
    }

    /// Get display name
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Gitee => "Gitee",
        }
    }
}

impl std::fmt::Display for SyncPlatformSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

define_settings_group!(CloudSyncSettings,
    settings: [
        sync_platform: SyncPlatform {
            type: SyncPlatformSetting,
            default: SyncPlatformSetting::GitHub,
            supported_platforms: SupportedPlatforms::ALL,
            sync_to_cloud: SyncToCloud::Never,
            private: false,
            storage_key: "CloudSyncPlatform",
            toml_path: "cloud_sync.sync_platform",
            description: "Cloud sync platform",
        },
    ]
);
