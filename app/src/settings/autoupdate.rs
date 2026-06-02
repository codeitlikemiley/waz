use settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

define_settings_group!(AutoupdateSettings, settings: [
    automatic_updates_enabled: AutomaticUpdatesEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        storage_key: "AutomaticUpdatesEnabled",
        toml_path: "updates.automatic_updates_enabled",
        description: "Whether Waz automatically checks for and downloads updates in the background.",
    },
]);
