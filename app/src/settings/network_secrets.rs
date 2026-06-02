//! `ProxyCredentials`: Saves the proxy Basic Auth password to the OS keystore (see Issue #72).
//!
//! Only the password is saved; non-sensitive fields such as username and URL remain in the settings.toml of `NetworkSettings`.
//! The design form is consistent with `crate::ai::agent_providers::AgentProviderSecrets`: based on
//! `warpui_extras::secure_storage` (macOS Keychain / Windows DPAPI / Linux Keyring).
//!
//! Note: The proxy has only one global password, so there is only one key-value pair in storage, and the value is the raw password
//! string (no longer using a JSON map).

use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "ProxyPassword";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyCredentialsEvent {
    /// Password value changed (can be empty).
    PasswordChanged,
}

/// Singleton: Manages the Basic Auth password for the global HTTP proxy.
pub struct ProxyCredentials {
    password: String,
}

impl ProxyCredentials {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self {
            password: Self::load_from_storage(ctx),
        }
    }

    /// Read the current password; returns an empty string if there is no value.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Set / update the password. Passing an empty string is equivalent to deletion.
    pub fn set_password(&mut self, password: String, ctx: &mut ModelContext<Self>) {
        if self.password == password {
            return;
        }
        self.password = password;
        self.persist(ctx);
        ctx.emit(ProxyCredentialsEvent::PasswordChanged);
    }

    fn load_from_storage(ctx: &mut ModelContext<Self>) -> String {
        match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(value) => value,
            Err(secure_storage::Error::NotFound) => String::new(),
            Err(e) => {
                log::error!("Failed to read proxy password: {e:#}");
                String::new()
            }
        }
    }

    fn persist(&self, ctx: &mut ModelContext<Self>) {
        if self.password.is_empty() {
            // Empty string means "no password"; delete failures are tolerated, only logged.
            // Avoid let-chain (app crate is Rust 2021), evaluate in two steps.
            if let Err(e) = ctx.secure_storage().remove_value(SECURE_STORAGE_KEY) {
                if !matches!(e, secure_storage::Error::NotFound) {
                    log::error!("Failed to remove proxy password: {e:#}");
                }
            }
            return;
        }
        if let Err(e) = ctx
            .secure_storage()
            .write_value(SECURE_STORAGE_KEY, &self.password)
        {
            log::error!("Failed to write proxy password: {e:#}");
        }
    }
}

impl Entity for ProxyCredentials {
    type Event = ProxyCredentialsEvent;
}

impl SingletonEntity for ProxyCredentials {}
