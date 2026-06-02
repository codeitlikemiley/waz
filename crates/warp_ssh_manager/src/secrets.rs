//! OS keychain wrapper — cross-platform using `keyring` crate.
//! Windows = Credential Manager / macOS = Keychain / Linux = Secret Service.
//!
//! Account key is formatted as `<node_uuid>:password` or `<node_uuid>:passphrase`,
//! independent of host/username, **renaming the node or changing host does not lose the password**.

use thiserror::Error;
use zeroize::Zeroizing;

const SERVICE: &str = "waz.ssh";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretKind {
    Password,
    Passphrase,
    RootPassword,
}

impl SecretKind {
    fn suffix(&self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "passphrase",
            SecretKind::RootPassword => "root_password",
        }
    }
}

#[derive(Debug, Error)]
pub enum SshSecretStoreError {
    /// The platform does not have an available keychain backend (common in Linux headless / WSL without
    /// Secret Service). The UI layer should prompt the user to use a private key instead.
    #[error("no keychain backend available on this platform")]
    NoBackend,
    #[error("keyring error: {0}")]
    Keyring(String),
}

impl From<keyring::Error> for SshSecretStoreError {
    fn from(e: keyring::Error) -> Self {
        match e {
            keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
                SshSecretStoreError::NoBackend
            }
            other => SshSecretStoreError::Keyring(other.to_string()),
        }
    }
}

/// Credential storage abstraction — `KeychainSecretStore` is the default implementation, mock is available for testing.
pub trait SshSecretStore: Send + Sync {
    fn set(&self, node_id: &str, kind: SecretKind, secret: &str)
    -> Result<(), SshSecretStoreError>;

    fn get(
        &self,
        node_id: &str,
        kind: SecretKind,
    ) -> Result<Option<Zeroizing<String>>, SshSecretStoreError>;

    fn delete(&self, node_id: &str, kind: SecretKind) -> Result<(), SshSecretStoreError>;
}

#[derive(Default, Clone, Copy, Debug)]
pub struct KeychainSecretStore;

fn account_key(node_id: &str, kind: SecretKind) -> String {
    format!("{node_id}:{}", kind.suffix())
}

impl SshSecretStore for KeychainSecretStore {
    fn set(
        &self,
        node_id: &str,
        kind: SecretKind,
        secret: &str,
    ) -> Result<(), SshSecretStoreError> {
        let entry = keyring::Entry::new(SERVICE, &account_key(node_id, kind))?;
        entry.set_password(secret)?;
        Ok(())
    }

    fn get(
        &self,
        node_id: &str,
        kind: SecretKind,
    ) -> Result<Option<Zeroizing<String>>, SshSecretStoreError> {
        let entry = keyring::Entry::new(SERVICE, &account_key(node_id, kind))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(Zeroizing::new(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, node_id: &str, kind: SecretKind) -> Result<(), SshSecretStoreError> {
        let entry = keyring::Entry::new(SERVICE, &account_key(node_id, kind))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! In-process memory mock, bypassing OS keychain — for CI / unit tests.

    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct InMemorySecretStore {
        inner: Mutex<HashMap<String, String>>,
    }

    impl SshSecretStore for InMemorySecretStore {
        fn set(
            &self,
            node_id: &str,
            kind: SecretKind,
            secret: &str,
        ) -> Result<(), SshSecretStoreError> {
            self.inner
                .lock()
                .unwrap()
                .insert(account_key(node_id, kind), secret.to_string());
            Ok(())
        }

        fn get(
            &self,
            node_id: &str,
            kind: SecretKind,
        ) -> Result<Option<Zeroizing<String>>, SshSecretStoreError> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .get(&account_key(node_id, kind))
                .cloned()
                .map(Zeroizing::new))
        }

        fn delete(&self, node_id: &str, kind: SecretKind) -> Result<(), SshSecretStoreError> {
            self.inner
                .lock()
                .unwrap()
                .remove(&account_key(node_id, kind));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::InMemorySecretStore;
    use super::*;

    #[test]
    fn set_get_delete_roundtrip() {
        let store = InMemorySecretStore::default();
        let node = "abc-123";
        store.set(node, SecretKind::Password, "hunter2").unwrap();
        let got = store.get(node, SecretKind::Password).unwrap().unwrap();
        assert_eq!(&*got, "hunter2");
        store.delete(node, SecretKind::Password).unwrap();
        assert!(store.get(node, SecretKind::Password).unwrap().is_none());
    }

    #[test]
    fn password_and_passphrase_have_separate_keys() {
        let store = InMemorySecretStore::default();
        store.set("n", SecretKind::Password, "pw").unwrap();
        store.set("n", SecretKind::Passphrase, "pp").unwrap();
        assert_eq!(
            &*store.get("n", SecretKind::Password).unwrap().unwrap(),
            "pw"
        );
        assert_eq!(
            &*store.get("n", SecretKind::Passphrase).unwrap().unwrap(),
            "pp"
        );
    }

    #[test]
    fn delete_missing_is_idempotent() {
        let store = InMemorySecretStore::default();
        // Should not error on absent key.
        store.delete("never-stored", SecretKind::Password).unwrap();
    }
}
