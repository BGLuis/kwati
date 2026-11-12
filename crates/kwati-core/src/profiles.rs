use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Sftp,
    Ftp,
    Ftps,
    Smb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Password,
    PrivateKey,
}

/// A saved server. It never holds a password or key passphrase: those are kept in the secret
/// store under [`ConnectionProfile::id`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub start_path: String,
    pub auth: AuthMethod,
    pub private_key_path: Option<PathBuf>,
    pub allow_insecure: bool,
}

/// A password or key passphrase that must be stored under `profile_id` in the secret store.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingSecret {
    pub profile_id: String,
    pub secret: String,
}

impl fmt::Debug for PendingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingSecret")
            .field("profile_id", &self.profile_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The saved connection profiles, stored as `connections.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfiles {
    pub profiles: Vec<ConnectionProfile>,
}

/// `<config dir>/kwati/connections.toml`, beside the settings file, or `None` when the platform
/// has no config dir.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kwati").join("connections.toml"))
}

impl ConnectionProfiles {
    /// Reads the profiles from `path`. A missing file yields an empty list; a file that cannot be
    /// parsed is an error, so the caller never overwrites it by accident.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err),
        };
        toml::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    /// Atomically replaces the file at `path`, like [`Settings::save_to`](crate::Settings::save_to).
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let text =
            toml::to_string(self).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_atomic(path, &text, || Ok(()))
    }

    /// Adds or replaces `profile` (matched by id) and saves the list to `path`. The `secret`, if
    /// any, is not written: it is returned for the caller to hand to the secret store.
    pub fn save_profile(
        &mut self,
        path: &Path,
        profile: ConnectionProfile,
        secret: Option<String>,
    ) -> io::Result<Option<PendingSecret>> {
        let pending = secret.map(|secret| PendingSecret {
            profile_id: profile.id.clone(),
            secret,
        });
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
        self.save_to(path)?;
        Ok(pending)
    }
}
