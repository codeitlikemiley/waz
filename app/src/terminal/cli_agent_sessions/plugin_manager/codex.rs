use std::sync::LazyLock;

use async_trait::async_trait;

use super::{CliAgentPluginManager, PluginInstructionStep, PluginInstructions};

pub(super) struct CodexPluginManager;

#[async_trait]
impl CliAgentPluginManager for CodexPluginManager {
    fn minimum_plugin_version(&self) -> &'static str {
        "0.0.0"
    }

    fn can_auto_install(&self) -> bool {
        false
    }

    fn supports_update(&self) -> bool {
        false
    }

    fn install_instructions(&self) -> &'static PluginInstructions {
        &INSTALL_INSTRUCTIONS
    }

    fn update_instructions(&self) -> &'static PluginInstructions {
        &EMPTY_INSTRUCTIONS
    }
}

static INSTALL_INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: crate::t_static!("cli-agent-plugin-codex-install-title"),
    subtitle: crate::t_static!("cli-agent-plugin-codex-install-subtitle"),
    steps: vec![
        PluginInstructionStep {
            description: crate::t_static!("cli-agent-plugin-codex-update-step"),
            command: "",
            executable: false,
            link: Some("https://developers.openai.com/codex/cli#upgrade"),
        },
        PluginInstructionStep {
            description: crate::t_static!("cli-agent-plugin-codex-notification-step"),
            command: "[tui]\nnotification_condition = \"always\"",
            executable: false,
            link: None,
        },
    ],
    post_install_notes: vec![crate::t_static!("cli-agent-plugin-codex-restart-note")],
});

static EMPTY_INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "",
    subtitle: "",
    steps: vec![],
    post_install_notes: vec![],
});

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
