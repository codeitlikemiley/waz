//! Waz local managed-secrets client.
//!
//! Waz's upstream originally used server_api to adjust the cloud interface to maintain team/user managed keys.
//! Waz retains the `warp_managed_secrets` crate for reuse by local functions, but all cloud-managed keys
//! None of the actions are reachable: the query returns an empty collection, and the write action and OIDC token issuance return disabled errors.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use warp_managed_secrets::client::{
    ManagedSecretConfigs, ManagedSecretsClient, SecretOwner, TaskIdentityToken,
};
use warp_managed_secrets::{ManagedSecret, ManagedSecretType, ManagedSecretValue};

pub(crate) struct DisabledManagedSecretsClient;

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl ManagedSecretsClient for DisabledManagedSecretsClient {
    async fn get_managed_secret_configs(&self) -> Result<ManagedSecretConfigs> {
        Ok(ManagedSecretConfigs {
            user_secrets: None,
            team_secrets: HashMap::new(),
        })
    }

    async fn create_managed_secret(
        &self,
        _owner: SecretOwner,
        _name: String,
        _secret_type: ManagedSecretType,
        _encrypted_value: String,
        _description: Option<String>,
    ) -> Result<ManagedSecret> {
        Err(anyhow!("Cloud managed secrets disabled in Waz"))
    }

    async fn delete_managed_secret(&self, _owner: SecretOwner, _name: String) -> Result<()> {
        Err(anyhow!("Cloud managed secrets disabled in Waz"))
    }

    async fn update_managed_secret(
        &self,
        _owner: SecretOwner,
        _name: String,
        _encrypted_value: Option<String>,
        _description: Option<String>,
    ) -> Result<ManagedSecret> {
        Err(anyhow!("Cloud managed secrets disabled in Waz"))
    }

    async fn list_secrets(&self) -> Result<Vec<ManagedSecret>> {
        Ok(Vec::new())
    }

    async fn get_task_secrets(
        &self,
        _task_id: String,
    ) -> Result<HashMap<String, ManagedSecretValue>> {
        Ok(HashMap::new())
    }

    async fn issue_task_identity_token(
        &self,
        _options: warp_managed_secrets::client::IdentityTokenOptions,
    ) -> Result<TaskIdentityToken> {
        Err(anyhow!("Task identity token issuance disabled in Waz"))
    }
}
