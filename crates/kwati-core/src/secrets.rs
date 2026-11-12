use std::fmt;
use std::io;
use std::sync::Arc;

use keyring_core::{CredentialStore, Entry, Error};

const SERVICE: &str = "kwati";

/// Why a secret could not be read, written, or deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsError {
    /// The system credential store cannot be reached (locked, not running, or unsupported). The
    /// secret is not saved anywhere else.
    Unavailable(String),
    /// The store was reached but refused the operation.
    Failed(String),
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "credential store unavailable: {reason}"),
            Self::Failed(reason) => write!(f, "credential store error: {reason}"),
        }
    }
}

impl std::error::Error for SecretsError {}

impl From<Error> for SecretsError {
    fn from(err: Error) -> Self {
        match err {
            Error::NoStorageAccess(_) | Error::PlatformFailure(_) | Error::NoDefaultStore => {
                Self::Unavailable(err.to_string())
            }
            err => Self::Failed(err.to_string()),
        }
    }
}

impl From<SecretsError> for io::Error {
    fn from(err: SecretsError) -> Self {
        io::Error::other(err)
    }
}

/// Passwords and key passphrases, keyed by [`ConnectionProfile::id`](crate::ConnectionProfile::id).
pub trait Secrets: Send + Sync {
    /// The secret saved for `profile_id`, or `None` when there is none.
    fn get(&self, profile_id: &str) -> Result<Option<String>, SecretsError>;

    /// Saves `secret` for `profile_id`, replacing any previous one.
    fn set(&self, profile_id: &str, secret: &str) -> Result<(), SecretsError>;

    /// Removes the secret for `profile_id`. Succeeds when there is none.
    fn delete(&self, profile_id: &str) -> Result<(), SecretsError>;
}

/// [`Secrets`] kept in a `keyring-core` credential store. It never writes a secret anywhere else.
pub struct KeyringSecrets {
    store: Arc<CredentialStore>,
}

impl KeyringSecrets {
    /// Uses `store`, for example the in-memory `keyring_core::mock::Store` in tests.
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    /// Uses the operating system credential store: Secret Service on Linux, Credential Manager on
    /// Windows, Keychain on macOS.
    pub fn native() -> Result<Self, SecretsError> {
        Ok(Self::new(native_store()?))
    }

    fn entry(&self, profile_id: &str) -> Result<Entry, SecretsError> {
        Ok(self.store.build(SERVICE, profile_id, None)?)
    }
}

impl Secrets for KeyringSecrets {
    fn get(&self, profile_id: &str) -> Result<Option<String>, SecretsError> {
        match self.entry(profile_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(Error::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn set(&self, profile_id: &str, secret: &str) -> Result<(), SecretsError> {
        Ok(self.entry(profile_id)?.set_password(secret)?)
    }

    fn delete(&self, profile_id: &str) -> Result<(), SecretsError> {
        match self.entry(profile_id)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn native_store() -> Result<Arc<CredentialStore>, SecretsError> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn native_store() -> Result<Arc<CredentialStore>, SecretsError> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn native_store() -> Result<Arc<CredentialStore>, SecretsError> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn native_store() -> Result<Arc<CredentialStore>, SecretsError> {
    Err(SecretsError::Unavailable(
        "no credential store for this platform".into(),
    ))
}
