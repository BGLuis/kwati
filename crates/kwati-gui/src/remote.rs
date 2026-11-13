//! Saved servers and the connections open to them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use kwati_core::{
    connect_profile, known_hosts_path, profiles_path, AuthMethod, Backend, BackendError,
    ConnectionProfile, ConnectionProfiles, ConnectionState, HostKeyUnknown, KeyringSecrets,
    KnownHosts, Location, Protocol, ReconnectingBackend, Secrets, SecretsError,
};

type SharedSecrets = Arc<dyn Secrets + Send + Sync>;

/// The profiles, where they and their secrets are kept, and the open connections.
pub struct RemoteSessions {
    /// `None` keeps the profiles in memory only.
    profiles_path: Option<PathBuf>,
    profiles: ConnectionProfiles,
    secrets: SharedSecrets,
    known_hosts: KnownHosts,
    connections: HashMap<String, Arc<ReconnectingBackend>>,
}

impl RemoteSessions {
    pub fn new(
        profiles_path: Option<PathBuf>,
        profiles: ConnectionProfiles,
        secrets: SharedSecrets,
        known_hosts: KnownHosts,
    ) -> Self {
        Self {
            profiles_path,
            profiles,
            secrets,
            known_hosts,
            connections: HashMap::new(),
        }
    }

    /// No profiles and nothing persisted; what a fresh [`AppState`](crate::AppState) starts
    /// with, so tests never touch the user's files or keychain.
    pub fn in_memory() -> Self {
        Self::new(
            None,
            ConnectionProfiles::default(),
            Arc::new(SessionSecrets::default()),
            KnownHosts::new(std::env::temp_dir().join("kwati-unused-known_hosts")),
        )
    }

    /// The user's profiles file, the system keychain and the app's known-hosts file. Returns a
    /// message for the status bar when one of them cannot be used.
    pub fn load_native() -> (Self, Option<String>) {
        let mut problem = None;
        let secrets: SharedSecrets = match KeyringSecrets::native() {
            Ok(store) => Arc::new(store),
            Err(err) => {
                problem = Some(format!("Passwords cannot be saved: {err}"));
                Arc::new(SessionSecrets::default())
            }
        };
        let path = profiles_path();
        let profiles = match path.as_deref().map(ConnectionProfiles::load_from) {
            Some(Ok(profiles)) => profiles,
            Some(Err(err)) => {
                problem = Some(format!("Saved servers could not be read: {err}"));
                ConnectionProfiles::default()
            }
            None => ConnectionProfiles::default(),
        };
        let known_hosts = KnownHosts::new(
            known_hosts_path().unwrap_or_else(|| std::env::temp_dir().join("kwati_known_hosts")),
        );
        (Self::new(path, profiles, secrets, known_hosts), problem)
    }

    pub fn profiles(&self) -> &[ConnectionProfile] {
        &self.profiles.profiles
    }

    pub fn profile(&self, id: &str) -> Option<&ConnectionProfile> {
        self.profiles.profiles.iter().find(|p| p.id == id)
    }

    /// Adds or replaces `profile`. A `secret` replaces the saved password or passphrase; `None`
    /// keeps the one already saved.
    pub fn save_profile(
        &mut self,
        profile: ConnectionProfile,
        secret: Option<String>,
    ) -> Result<(), String> {
        let id = profile.id.clone();
        let pending = match &self.profiles_path {
            Some(path) => self
                .profiles
                .save_profile(path, profile, secret)
                .map_err(|err| format!("The server could not be saved: {err}"))?,
            None => {
                match self.profiles.profiles.iter_mut().find(|p| p.id == id) {
                    Some(existing) => *existing = profile,
                    None => self.profiles.profiles.push(profile),
                }
                if let Some(secret) = secret {
                    self.remember_password(&id, &secret)?;
                }
                None
            }
        };
        if let Some(pending) = pending {
            pending
                .store(self.secrets.as_ref())
                .map_err(|err| format!("The password could not be saved: {err}"))?;
        }
        // A changed address or user must not keep using the old session.
        self.disconnect(&id);
        Ok(())
    }

    /// Removes the profile and its saved secret, and closes its connection.
    pub fn remove_profile(&mut self, id: &str) -> Result<(), String> {
        match &self.profiles_path {
            Some(path) => self
                .profiles
                .remove_profile(path, id, self.secrets.as_ref())
                .map_err(|err| format!("The server could not be removed: {err}"))?,
            None => {
                self.secrets
                    .delete(id)
                    .map_err(|err| format!("The password could not be removed: {err}"))?;
                self.profiles.profiles.retain(|p| p.id != id);
            }
        }
        self.disconnect(id);
        Ok(())
    }

    /// Whether connecting needs a password typed now: password login with none saved.
    pub fn needs_password(&self, id: &str) -> bool {
        let Some(profile) = self.profile(id) else {
            return false;
        };
        profile.auth == AuthMethod::Password && !self.has_secret(id)
    }

    /// Whether a non-empty secret (password or key passphrase) is saved for `id`.
    pub fn has_secret(&self, id: &str) -> bool {
        matches!(self.secrets.get(id), Ok(Some(secret)) if !secret.is_empty())
    }

    /// The work that opens `id`, to run off the interface thread. `typed` is a password the user
    /// just entered; it lives only in this connection, so the next connection asks again unless
    /// it was saved.
    pub fn connector(
        &self,
        id: &str,
        typed: Option<String>,
    ) -> Option<impl FnOnce() -> Result<ReconnectingBackend, BackendError> + Send + 'static> {
        let profile = self.profile(id)?.clone();
        let secrets: SharedSecrets = match typed {
            Some(password) => Arc::new(TypedSecret {
                profile_id: profile.id.clone(),
                password,
                saved: self.secrets.clone(),
            }),
            None => self.secrets.clone(),
        };
        let known_hosts = self.known_hosts.clone();
        Some(move || connect_profile(&profile, secrets, known_hosts))
    }

    /// Saves a password the user chose to remember.
    pub fn remember_password(&self, id: &str, password: &str) -> Result<(), String> {
        self.secrets
            .set(id, password)
            .map_err(|err| format!("The password could not be saved: {err}"))
    }

    /// Records the host key the user agreed to trust.
    pub fn trust(&self, unknown: &HostKeyUnknown) -> Result<(), String> {
        self.known_hosts
            .trust(unknown)
            .map_err(|err| format!("The host key could not be saved: {err}"))
    }

    pub fn add_connection(&mut self, id: &str, backend: ReconnectingBackend) {
        self.connections.insert(id.to_owned(), Arc::new(backend));
    }

    pub fn disconnect(&mut self, id: &str) {
        self.connections.remove(id);
    }

    pub fn backend(&self, id: &str) -> Option<Arc<dyn Backend>> {
        self.connections
            .get(id)
            .map(|backend| backend.clone() as Arc<dyn Backend>)
    }

    pub fn state(&self, id: &str) -> Option<ConnectionState> {
        self.connections.get(id).map(|backend| backend.state())
    }

    pub fn is_connected(&self, id: &str) -> bool {
        self.connections.contains_key(id)
    }
}

/// Where a profile opens: its start path on its server.
pub fn start_location(profile: &ConnectionProfile) -> Location {
    let start = if profile.start_path.trim().is_empty() {
        "/"
    } else {
        profile.start_path.trim()
    };
    Location::Remote {
        profile_id: profile.id.clone(),
        path: start.into(),
    }
}

/// The port each protocol listens on unless the profile says otherwise.
pub fn default_port(protocol: Protocol) -> u16 {
    match protocol {
        Protocol::Sftp => 22,
        Protocol::Ftp | Protocol::Ftps => 21,
        Protocol::Smb => 445,
    }
}

pub fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Sftp => "SFTP",
        Protocol::Ftp => "FTP",
        Protocol::Ftps => "FTPS",
        Protocol::Smb => "SMB",
    }
}

/// A new, unique profile id.
pub fn new_profile_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("p{nanos:x}-{seq}")
}

/// Secrets kept in memory for this run only: the fallback when the keychain is unavailable.
#[derive(Default)]
pub struct SessionSecrets(Mutex<HashMap<String, String>>);

impl Secrets for SessionSecrets {
    fn get(&self, id: &str) -> Result<Option<String>, SecretsError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretsError> {
        self.0.lock().unwrap().insert(id.into(), secret.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), SecretsError> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

/// A password typed for one connection, in front of the saved secrets.
struct TypedSecret {
    profile_id: String,
    password: String,
    saved: SharedSecrets,
}

impl Secrets for TypedSecret {
    fn get(&self, id: &str) -> Result<Option<String>, SecretsError> {
        if id == self.profile_id {
            Ok(Some(self.password.clone()))
        } else {
            self.saved.get(id)
        }
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretsError> {
        self.saved.set(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), SecretsError> {
        self.saved.delete(id)
    }
}
