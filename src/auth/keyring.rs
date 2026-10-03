//! [`SecretStore`] implementations: [`SystemKeyring`] for production and, with `test-util`,
//! `InMemoryStore` for tests.

use std::sync::Once;

use tracing::debug;

use crate::error::{Error, Result};

/// A platform credential store addressed by `(service, account)`.
pub trait SecretStore: Send + Sync {
    /// Returns the stored secret, or `Ok(None)` when there is no entry or no usable
    /// credential store (headless Linux, containers) — `hw` then falls back to other sources.
    fn get(&self, service: &str, account: &str) -> Result<Option<String>>;

    /// Stores `secret`, replacing any existing value.
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()>;

    /// Removes the entry; a missing entry is not an error, so `hw auth logout` is idempotent.
    fn delete(&self, service: &str, account: &str) -> Result<()>;
}

/// Production store backed by the OS keyring (Keychain, Credential Manager, keyutils).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemKeyring;

/// Installs the platform default store once. On failure no default is installed and
/// `keyring_core` reports `NoDefaultStore`, which `get`/`delete` treat as "no entry".
fn ensure_default_store() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        #[cfg(target_os = "macos")]
        match apple_native_keyring_store::keychain::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(err) => debug!("macOS keychain store unavailable: {err}"),
        }
        #[cfg(target_os = "windows")]
        match windows_native_keyring_store::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(err) => debug!("Windows credential store unavailable: {err}"),
        }
        #[cfg(target_os = "linux")]
        match linux_keyutils_keyring_store::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(err) => debug!("Linux keyutils store unavailable: {err}"),
        }
    });
}

/// Errors that mean "there is no usable keyring here" rather than a real failure.
fn is_unavailable(err: &keyring_core::Error) -> bool {
    matches!(
        err,
        keyring_core::Error::NoStorageAccess(_)
            | keyring_core::Error::PlatformFailure(_)
            | keyring_core::Error::NoDefaultStore
    )
}

fn keyring_error(err: keyring_core::Error) -> Error {
    Error::Keyring(err.to_string())
}

impl SecretStore for SystemKeyring {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        ensure_default_store();
        let entry = match keyring_core::Entry::new(service, account) {
            Ok(entry) => entry,
            Err(err) if is_unavailable(&err) => {
                debug!("keyring unavailable for {service}: {err}");
                return Ok(None);
            }
            Err(err) => return Err(keyring_error(err)),
        };
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(err) if is_unavailable(&err) => {
                debug!("keyring unavailable for {service}: {err}");
                Ok(None)
            }
            Err(err) => Err(keyring_error(err)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        ensure_default_store();
        // Every failure propagates: a silently dropped write would leave the user without a token.
        let entry = keyring_core::Entry::new(service, account).map_err(keyring_error)?;
        entry.set_password(secret).map_err(keyring_error)
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        ensure_default_store();
        let entry = match keyring_core::Entry::new(service, account) {
            Ok(entry) => entry,
            Err(err) if is_unavailable(&err) => return Ok(()),
            Err(err) => return Err(keyring_error(err)),
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(err) if is_unavailable(&err) => Ok(()),
            Err(err) => Err(keyring_error(err)),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub use memory::InMemoryStore;

#[cfg(any(test, feature = "test-util"))]
mod memory {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::SecretStore;
    use crate::error::{Error, Result};

    /// Thread-safe in-memory store; public so integration tests can inject it.
    #[derive(Debug, Default)]
    pub struct InMemoryStore {
        inner: Mutex<HashMap<(String, String), String>>,
    }

    impl InMemoryStore {
        /// Creates an empty store.
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        /// Number of stored entries.
        #[must_use]
        pub fn len(&self) -> usize {
            self.inner
                .lock()
                .map(|guard| guard.len())
                .unwrap_or_default()
        }

        /// Whether the store holds no entries.
        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }

        fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<(String, String), String>>> {
            self.inner
                .lock()
                .map_err(|_| Error::Keyring("in-memory store mutex poisoned".into()))
        }
    }

    impl SecretStore for InMemoryStore {
        fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
            Ok(self
                .lock()?
                .get(&(service.to_owned(), account.to_owned()))
                .cloned())
        }

        fn set(&self, service: &str, account: &str, secret: &str) -> Result<()> {
            self.lock()?
                .insert((service.to_owned(), account.to_owned()), secret.to_owned());
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<()> {
            self.lock()?
                .remove(&(service.to_owned(), account.to_owned()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_missing_is_none() {
        assert!(InMemoryStore::new().get("svc", "acct").unwrap().is_none());
    }

    #[test]
    fn set_then_get_round_trips() {
        let store = InMemoryStore::new();
        store.set("svc", "acct", "tok").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap().as_deref(), Some("tok"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn delete_is_idempotent() {
        let store = InMemoryStore::new();
        store.set("svc", "acct", "tok").unwrap();
        store.delete("svc", "acct").unwrap();
        store.delete("svc", "acct").unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn entries_are_scoped_by_service_and_account() {
        let store = InMemoryStore::new();
        store.set("hw:a", "x.hookah.work", "1").unwrap();
        store.set("hw:b", "x.hookah.work", "2").unwrap();
        assert_eq!(
            store.get("hw:a", "x.hookah.work").unwrap().as_deref(),
            Some("1")
        );
        assert!(store.get("hw:a", "y.hookah.work").unwrap().is_none());
    }
}
