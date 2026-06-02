use std::sync::LazyLock;

use async_trait::async_trait;

use super::{CliAgentPluginManager, PluginInstructionStep, PluginInstructions};

pub(super) struct DeepSeekPluginManager;

#[async_trait]
impl CliAgentPluginManager for DeepSeekPluginManager {
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
    title: crate::t_static!("cli-agent-plugin-deepseek-install-title"),
    subtitle: crate::t_static!("cli-agent-plugin-deepseek-install-subtitle"),
    steps: vec![PluginInstructionStep {
        description: crate::t_static!("cli-agent-plugin-deepseek-notification-step"),
        command: "[tui]\nnotification_condition = \"always\"",
        executable: false,
        link: None,
    }],
    post_install_notes: vec![crate::t_static!("cli-agent-plugin-deepseek-restart-note")],
});

static EMPTY_INSTRUCTIONS: LazyLock<PluginInstructions> = LazyLock::new(|| PluginInstructions {
    title: "",
    subtitle: "",
    steps: vec![],
    post_install_notes: vec![],
});
