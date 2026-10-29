//! Account secrets. Never stored in SQLite or written to logs.

use std::collections::HashMap;
use std::sync::Mutex;

/// Where account passwords/tokens live. Implementations may block; the
/// engine calls them on Tokio's blocking pool.
pub trait CredentialStore: Send + Sync {
    fn get(&self, account_id: &str) -> Result<Option<String>, String>;
    fn set(&self, account_id: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account_id: &str) -> Result<(), String>;
}

/// In-memory store for tests and environments without a keyring.
#[derive(Default)]
pub struct MemoryCredentials {
    secrets: Mutex<HashMap<String, String>>,
}

impl CredentialStore for MemoryCredentials {
    fn get(&self, account_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .secrets
            .lock()
            .map_err(|e| e.to_string())?
            .get(account_id)
            .cloned())
    }

    fn set(&self, account_id: &str, secret: &str) -> Result<(), String> {
        self.secrets
            .lock()
            .map_err(|e| e.to_string())?
            .insert(account_id.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, account_id: &str) -> Result<(), String> {
        self.secrets
            .lock()
            .map_err(|e| e.to_string())?
            .remove(account_id);
        Ok(())
    }
}

/// The OS credential store: Secret Service on Linux (KWallet on KDE
/// Plasma, GNOME Keyring elsewhere), Keychain on macOS, Credential Manager
/// on Windows.
#[cfg(feature = "keyring")]
pub struct KeyringCredentials {
    service: String,
}

#[cfg(feature = "keyring")]
impl KeyringCredentials {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    fn entry(&self, account_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(&self.service, account_id).map_err(|e| describe(&e))
    }
}

#[cfg(feature = "keyring")]
fn describe(e: &keyring::Error) -> String {
    match e {
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_) => {
            "the system keyring (Secret Service) is not available".to_owned()
        }
        other => format!("keyring error: {other}"),
    }
}

#[cfg(feature = "keyring")]
impl CredentialStore for KeyringCredentials {
    fn get(&self, account_id: &str) -> Result<Option<String>, String> {
        match self.entry(account_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(describe(&e)),
        }
    }

    fn set(&self, account_id: &str, secret: &str) -> Result<(), String> {
        self.entry(account_id)?
            .set_password(secret)
            .map_err(|e| describe(&e))
    }

    fn delete(&self, account_id: &str) -> Result<(), String> {
        match self.entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(describe(&e)),
        }
    }
}
