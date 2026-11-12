use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use russh::client::{self, Handle};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::File;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileType, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::{Builder, Runtime};

use crate::{
    AuthMethod, Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryKind, EntryMeta,
    Listing, Location, Protocol, Secrets,
};

/// The server presented a host key that is not in [`KnownHosts`]; nothing was sent to it.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::PermissionDenied`]; detect it with
/// [`BackendError::host_key_unknown`]. The interface shows `fingerprint` to the user and, only if
/// they agree, passes this value to [`KnownHosts::trust`], which stores the very key that was
/// presented, so the decision cannot be applied to a different key.
#[derive(Debug, Clone)]
pub struct HostKeyUnknown {
    pub host: String,
    pub port: u16,
    /// `SHA256:` followed by the base64 digest, as OpenSSH prints it.
    pub fingerprint: String,
    key: PublicKey,
}

impl fmt::Display for HostKeyUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown host key for {}:{} ({})",
            self.host, self.port, self.fingerprint
        )
    }
}

impl std::error::Error for HostKeyUnknown {}

/// The server presented a host key that differs from the stored one; nothing was sent to it.
///
/// Carried like [`HostKeyUnknown`]; detect it with [`BackendError::host_key_changed`]. There is
/// no way to accept it from here: the user must remove the old key from the known-hosts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyChanged {
    pub host: String,
    pub port: u16,
    /// The fingerprint of the key the server presented now.
    pub fingerprint: String,
}

impl fmt::Display for HostKeyChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "host key of {}:{} changed (now {}); the connection was refused",
            self.host, self.port, self.fingerprint
        )
    }
}

impl std::error::Error for HostKeyChanged {}

impl BackendError {
    /// The unknown host key behind this error, if that is why the connection was refused.
    pub fn host_key_unknown(&self) -> Option<&HostKeyUnknown> {
        self.inner_error()
    }

    /// The changed host key behind this error, if that is why the connection was refused.
    pub fn host_key_changed(&self) -> Option<&HostKeyChanged> {
        self.inner_error()
    }

    fn inner_error<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            BackendError::Io(err) => err.get_ref()?.downcast_ref(),
            BackendError::Unsupported => None,
        }
    }
}

/// `<config dir>/kwati/known_hosts`, beside the settings file, or `None` when the platform has no
/// config dir.
pub fn known_hosts_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kwati").join("known_hosts"))
}

/// The host keys the user trusts, kept in an OpenSSH-format `known_hosts` file of its own (never
/// `~/.ssh/known_hosts`).
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
}

enum Verdict {
    Trusted,
    Unknown,
    Changed,
}

impl KnownHosts {
    /// Uses the file at `path`, typically [`known_hosts_path`]. A missing file trusts nothing.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Stores the key of `unknown` for its host and port. Call it only after the user, shown
    /// [`HostKeyUnknown::fingerprint`], agreed.
    pub fn trust(&self, unknown: &HostKeyUnknown) -> io::Result<()> {
        keys::known_hosts::learn_known_hosts_path(
            &unknown.host,
            unknown.port,
            &unknown.key,
            &self.path,
        )
        .map_err(keys_error)
    }

    fn check(&self, host: &str, port: u16, key: &PublicKey) -> io::Result<Verdict> {
        match keys::check_known_hosts_path(host, port, key, &self.path) {
            Ok(true) => Ok(Verdict::Trusted),
            Ok(false) => Ok(Verdict::Unknown),
            Err(keys::Error::KeyChanged { .. }) => Ok(Verdict::Changed),
            Err(err) => Err(keys_error(err)),
        }
    }
}

fn keys_error(err: keys::Error) -> io::Error {
    match err {
        keys::Error::IO(err) => err,
        err => io::Error::new(io::ErrorKind::InvalidData, err),
    }
}

/// Decides on the server key during the handshake, before anything is authenticated. The reason
/// for a refusal is left in `refusal`, because russh reports a rejected key only as a generic
/// error.
struct HostKeyHandler {
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    refusal: Arc<Mutex<Option<io::Error>>>,
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        match self.verdict(key) {
            Ok(()) => Ok(true),
            Err(refusal) => {
                *self.refusal.lock().unwrap() = Some(refusal);
                Ok(false)
            }
        }
    }
}

impl HostKeyHandler {
    fn verdict(&self, key: &PublicKeyOrCertificate) -> io::Result<()> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "host certificates are not supported",
            ));
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let (host, port) = (self.host.clone(), self.port);
        match self.known_hosts.check(&host, port, key)? {
            Verdict::Trusted => Ok(()),
            Verdict::Unknown => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                HostKeyUnknown {
                    host,
                    port,
                    fingerprint,
                    key: key.clone(),
                },
            )),
            Verdict::Changed => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                HostKeyChanged {
                    host,
                    port,
                    fingerprint,
                },
            )),
        }
    }
}

/// Maps SFTP failures to the [`io::ErrorKind`]s the rest of the crate reasons about, so a lost
/// connection is seen by [`is_dropped_connection`](crate::is_dropped_connection).
fn sftp_error(err: SftpError) -> io::Error {
    use io::ErrorKind::*;
    match err {
        SftpError::Status(status) => {
            let kind = match status.status_code {
                StatusCode::NoSuchFile => NotFound,
                StatusCode::PermissionDenied => PermissionDenied,
                StatusCode::NoConnection | StatusCode::ConnectionLost => ConnectionAborted,
                StatusCode::OpUnsupported => Unsupported,
                _ => Other,
            };
            io::Error::new(
                kind,
                format!("{}: {}", status.status_code, status.error_message),
            )
        }
        SftpError::Timeout => io::Error::new(TimedOut, "the server did not answer in time"),
        SftpError::IO(message) => io::Error::new(BrokenPipe, message),
        err => io::Error::other(err),
    }
}

fn ssh_error(err: russh::Error) -> io::Error {
    use io::ErrorKind::*;
    match err {
        russh::Error::IO(err) => err,
        russh::Error::Disconnect => io::Error::new(ConnectionAborted, err),
        russh::Error::HUP | russh::Error::SendError => io::Error::new(ConnectionReset, err),
        russh::Error::ConnectionTimeout
        | russh::Error::KeepaliveTimeout
        | russh::Error::InactivityTimeout
        | russh::Error::Elapsed(_) => io::Error::new(TimedOut, err),
        err => io::Error::other(err),
    }
}

fn remote_path<'a>(profile_id: &str, location: &'a Location) -> Result<&'a Path, BackendError> {
    match location {
        Location::Remote {
            profile_id: id,
            path,
        } if id == profile_id => Ok(path),
        _ => Err(BackendError::Unsupported),
    }
}

/// The path as it goes on the wire: SFTP paths are UTF-8 with `/` separators.
fn wire_path(path: &Path) -> io::Result<String> {
    let text = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote paths must be valid UTF-8",
        )
    })?;
    Ok(if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

/// What `connect` needs from the profile, checked before any network traffic.
fn check_profile(profile: &ConnectionProfile) -> io::Result<()> {
    if profile.protocol != Protocol::Sftp {
        return Err(invalid("the profile is not an SFTP profile"));
    }
    if profile.auth == AuthMethod::PrivateKey && profile.private_key_path.is_none() {
        return Err(invalid(
            "the profile uses a private key but names no key file",
        ));
    }
    Ok(())
}

/// An SFTP server, browsed and changed through the [`Backend`] trait.
///
/// The SSH library is asynchronous, but `Backend` is synchronous and must never run on the
/// interface's executor. So the connection lives on a private single-thread Tokio runtime owned
/// by this backend, and every call blocks the calling thread until that runtime answers. Call it
/// from a worker thread, never from inside an async task.
///
/// There is no trash on an SFTP server: [`Backend::trash`] keeps its unavailable default.
pub struct SftpBackend {
    profile_id: String,
    sftp: SftpSession,
    _session: Handle<HostKeyHandler>,
    rt: Arc<Runtime>,
}

impl SftpBackend {
    /// Connects and authenticates as `profile` says.
    ///
    /// Password auth reads the password from `secrets` under the profile id, and a missing one is
    /// an error. Key auth loads the file named by the profile and, when `secrets` holds a value
    /// for the profile id, uses it as the key's passphrase.
    ///
    /// The server's host key is checked against `known_hosts` before authenticating, so no
    /// credential is sent to an unverified server. A key that is not stored fails with
    /// [`BackendError::host_key_unknown`], one that differs from the stored key with
    /// [`BackendError::host_key_changed`].
    pub fn connect(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
        known_hosts: &KnownHosts,
    ) -> Result<Self, BackendError> {
        check_profile(profile)?;
        let credential = Credential::load(profile, secrets)?;
        let rt = Arc::new(
            Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("kwati-sftp")
                .enable_all()
                .build()?,
        );
        let refusal = Arc::new(Mutex::new(None));
        let handler = HostKeyHandler {
            host: profile.host.clone(),
            port: profile.port,
            known_hosts: known_hosts.clone(),
            refusal: refusal.clone(),
        };
        let opened = rt.block_on(open(profile, credential, handler));
        let (session, sftp) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                let refused = refusal.lock().unwrap().take();
                return Err(refused.unwrap_or(err).into());
            }
        };
        Ok(Self {
            profile_id: profile.id.clone(),
            sftp,
            _session: session,
            rt,
        })
    }

    fn path(&self, location: &Location) -> Result<String, BackendError> {
        Ok(wire_path(remote_path(&self.profile_id, location)?)?)
    }

    fn block<T>(
        &self,
        op: impl std::future::Future<Output = Result<T, SftpError>>,
    ) -> Result<T, BackendError> {
        Ok(self.rt.block_on(op).map_err(sftp_error)?)
    }

    fn delete_tree(&self, path: &str) -> Result<(), BackendError> {
        let entries = self.block(self.sftp.read_dir(path))?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = format!("{}/{name}", path.trim_end_matches('/'));
            if entry.file_type() == FileType::Dir {
                self.delete_tree(&child)?;
            } else {
                self.block(self.sftp.remove_file(child))?;
            }
        }
        self.block(self.sftp.remove_dir(path))
    }

    fn exists(&self, path: &str) -> Result<Option<FileType>, BackendError> {
        match self.block(self.sftp.symlink_metadata(path)) {
            Ok(meta) => Ok(Some(meta.file_type())),
            Err(BackendError::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

enum Credential {
    Password(String),
    Key(Arc<keys::PrivateKey>),
}

impl Credential {
    fn load(profile: &ConnectionProfile, secrets: &dyn Secrets) -> io::Result<Self> {
        let secret = secrets.get(&profile.id)?;
        match (profile.auth, &profile.private_key_path) {
            (AuthMethod::Password, _) => secret
                .map(Self::Password)
                .ok_or_else(|| invalid("no password is saved for the profile")),
            (AuthMethod::PrivateKey, Some(path)) => keys::load_secret_key(path, secret.as_deref())
                .map(|key| Self::Key(Arc::new(key)))
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err)),
            (AuthMethod::PrivateKey, None) => unreachable!("check_profile rejects it"),
        }
    }
}

async fn open(
    profile: &ConnectionProfile,
    credential: Credential,
    handler: HostKeyHandler,
) -> io::Result<(Handle<HostKeyHandler>, SftpSession)> {
    let config = Arc::new(client::Config::default());
    let mut session = client::connect(config, (profile.host.as_str(), profile.port), handler)
        .await
        .map_err(ssh_error)?;
    let auth = match credential {
        Credential::Password(password) => session
            .authenticate_password(&profile.user, password)
            .await
            .map_err(ssh_error)?,
        Credential::Key(key) => {
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(ssh_error)?
                .flatten();
            session
                .authenticate_publickey(&profile.user, PrivateKeyWithHashAlg::new(key, hash))
                .await
                .map_err(ssh_error)?
        }
    };
    if !auth.success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the server rejected the credentials",
        ));
    }
    let channel = session.channel_open_session().await.map_err(ssh_error)?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(ssh_error)?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(sftp_error)?;
    Ok((session, sftp))
}

impl Backend for SftpBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let dir = self.path(location)?;
        let mut entries = Vec::new();
        for item in self.block(self.sftp.read_dir(dir.as_str()))? {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let path = location.path().join(&name);
            let is_dir = match item.file_type() {
                FileType::Dir => true,
                FileType::Symlink => self
                    .rt
                    .block_on(self.sftp.metadata(item.path()))
                    .is_ok_and(|meta| meta.file_type() == FileType::Dir),
                _ => false,
            };
            entries.push(Entry::new(name.into(), path, is_dir));
        }
        Ok(Listing::new(entries))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        let meta = self.block(self.sftp.symlink_metadata(self.path(location)?))?;
        let kind = match meta.file_type() {
            FileType::Dir => EntryKind::Dir,
            FileType::File => EntryKind::File,
            _ => EntryKind::Symlink,
        };
        let len = if kind == EntryKind::File {
            meta.len()
        } else {
            0
        };
        Ok(EntryMeta { kind, len })
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        let file = self.block(self.sftp.open(self.path(location)?))?;
        Ok(Box::new(BlockingFile::new(&self.rt, file)))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        let file = self.block(self.sftp.create(self.path(location)?))?;
        Ok(Box::new(BlockingFile::new(&self.rt, file)))
    }

    /// SFTP version 3 refuses to rename onto an existing name, so an existing file or symlink at
    /// `to` is removed first. That is not atomic: if the rename then fails, `to` is already gone.
    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let (from, to) = (self.path(from)?, self.path(to)?);
        if matches!(self.exists(&to)?, Some(kind) if kind != FileType::Dir) {
            self.block(self.sftp.remove_file(to.as_str()))?;
        }
        self.block(self.sftp.rename(from, to))
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        let path = self.path(location)?;
        if self.exists(&path)?.is_some() {
            return Err(
                io::Error::new(io::ErrorKind::AlreadyExists, "target already exists").into(),
            );
        }
        self.block(self.sftp.create_dir(path))
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        let path = self.path(location)?;
        match self.exists(&path)? {
            Some(FileType::Dir) => self.delete_tree(&path),
            _ => self.block(self.sftp.remove_file(path)),
        }
    }
}

/// An open remote file as a blocking `Read` or `Write`.
struct BlockingFile {
    rt: Arc<Runtime>,
    file: Option<File>,
}

impl BlockingFile {
    fn new(rt: &Arc<Runtime>, file: File) -> Self {
        Self {
            rt: rt.clone(),
            file: Some(file),
        }
    }

    fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("the file is open until dropped")
    }
}

impl Read for BlockingFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rt = self.rt.clone();
        rt.block_on(self.file().read(buf))
    }
}

impl Write for BlockingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let rt = self.rt.clone();
        rt.block_on(self.file().write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        let rt = self.rt.clone();
        rt.block_on(self.file().flush())
    }
}

impl Drop for BlockingFile {
    fn drop(&mut self) {
        if let Some(mut file) = self.file.take() {
            let _ = self.rt.block_on(file.shutdown());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::is_dropped_connection;
    use russh_sftp::protocol::Status;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    struct TempFile(PathBuf);

    impl TempFile {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!(
                "kwati_known_hosts_{}_{}/known_hosts",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            )))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn key(b64: &str) -> PublicKey {
        keys::parse_public_key_base64(b64).unwrap()
    }

    fn unknown(host: &str, port: u16, key: PublicKey) -> HostKeyUnknown {
        HostKeyUnknown {
            host: host.into(),
            port,
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            key,
        }
    }

    fn status(code: StatusCode) -> SftpError {
        SftpError::Status(Status {
            id: 0,
            status_code: code,
            error_message: "msg".into(),
            language_tag: String::new(),
        })
    }

    fn profile() -> ConnectionProfile {
        ConnectionProfile {
            id: "p".into(),
            name: "n".into(),
            protocol: Protocol::Sftp,
            host: "h".into(),
            port: 22,
            user: "u".into(),
            start_path: "/".into(),
            auth: AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        }
    }

    #[test]
    fn known_hosts_default_path_is_beside_settings() {
        assert_eq!(
            known_hosts_path().map(|p| p.parent().map(Path::to_path_buf)),
            crate::settings_path().map(|p| p.parent().map(Path::to_path_buf))
        );
        assert_eq!(
            known_hosts_path().unwrap().file_name().unwrap(),
            "known_hosts"
        );
    }

    #[test]
    fn known_hosts_unknown_until_trusted_then_trusted_for_that_port_only() {
        let file = TempFile::new();
        let hosts = KnownHosts::new(&file.0);
        let k = key(KEY_A);
        assert!(matches!(
            hosts.check("h", 2222, &k).unwrap(),
            Verdict::Unknown
        ));

        hosts.trust(&unknown("h", 2222, k.clone())).unwrap();
        assert!(matches!(
            hosts.check("h", 2222, &k).unwrap(),
            Verdict::Trusted
        ));
        assert!(matches!(
            hosts.check("h", 22, &k).unwrap(),
            Verdict::Unknown
        ));
        assert!(matches!(
            hosts.check("other", 2222, &k).unwrap(),
            Verdict::Unknown
        ));
    }

    #[test]
    fn known_hosts_different_key_for_a_trusted_host_is_changed() {
        let file = TempFile::new();
        let hosts = KnownHosts::new(&file.0);
        hosts.trust(&unknown("h", 22, key(KEY_A))).unwrap();
        assert!(matches!(
            hosts.check("h", 22, &key(KEY_B)).unwrap(),
            Verdict::Changed
        ));
    }

    #[test]
    fn trusting_stores_the_presented_key_not_a_caller_chosen_one() {
        let file = TempFile::new();
        let hosts = KnownHosts::new(&file.0);
        let presented = unknown("h", 22, key(KEY_A));
        hosts.trust(&presented).unwrap();
        let stored = fs::read_to_string(&file.0).unwrap();
        assert!(stored.contains(KEY_A) && !stored.contains(KEY_B));
    }

    #[test]
    fn known_hosts_unreadable_file_is_an_error_not_trust() {
        let file = TempFile::new();
        fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        fs::write(&file.0, "h ssh-ed25519 not-base64!!\n").unwrap();
        assert!(KnownHosts::new(&file.0)
            .check("h", 22, &key(KEY_A))
            .is_err());
    }

    #[test]
    fn host_key_errors_are_found_through_backend_error() {
        let k = key(KEY_A);
        let unknown_err: BackendError =
            io::Error::new(io::ErrorKind::PermissionDenied, unknown("h", 22, k.clone())).into();
        assert_eq!(
            unknown_err.host_key_unknown().unwrap().fingerprint,
            k.fingerprint(HashAlg::Sha256).to_string()
        );
        assert!(unknown_err.host_key_changed().is_none());

        let changed = HostKeyChanged {
            host: "h".into(),
            port: 22,
            fingerprint: "SHA256:x".into(),
        };
        let changed_err: BackendError =
            io::Error::new(io::ErrorKind::PermissionDenied, changed.clone()).into();
        assert_eq!(changed_err.host_key_changed(), Some(&changed));
        assert!(changed_err.host_key_unknown().is_none());
        assert!(
            !is_dropped_connection(&changed_err),
            "a refused key must not be retried"
        );
        assert!(BackendError::Unsupported.host_key_unknown().is_none());
        assert!(BackendError::from(io::Error::other("x"))
            .host_key_changed()
            .is_none());
    }

    #[test]
    fn sftp_error_kinds_feed_the_reconnect_logic() {
        let kind = |e| BackendError::from(sftp_error(e));
        assert_eq!(
            sftp_error(status(StatusCode::NoSuchFile)).kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            sftp_error(status(StatusCode::PermissionDenied)).kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!is_dropped_connection(&kind(status(
            StatusCode::NoSuchFile
        ))));
        assert!(!is_dropped_connection(&kind(status(StatusCode::Failure))));
        assert!(is_dropped_connection(&kind(status(
            StatusCode::ConnectionLost
        ))));
        assert!(is_dropped_connection(&kind(status(
            StatusCode::NoConnection
        ))));
        assert!(is_dropped_connection(&kind(SftpError::Timeout)));
        assert!(is_dropped_connection(&kind(SftpError::IO("closed".into()))));
    }

    #[test]
    fn ssh_error_kinds_feed_the_reconnect_logic() {
        let kind = |e| BackendError::from(ssh_error(e));
        assert!(is_dropped_connection(&kind(russh::Error::Disconnect)));
        assert!(is_dropped_connection(&kind(russh::Error::HUP)));
        assert!(is_dropped_connection(&kind(
            russh::Error::InactivityTimeout
        )));
        assert!(!is_dropped_connection(&kind(
            russh::Error::NotAuthenticated
        )));
        assert_eq!(
            ssh_error(russh::Error::IO(io::ErrorKind::ConnectionRefused.into())).kind(),
            io::ErrorKind::ConnectionRefused
        );
    }

    #[test]
    fn only_remote_locations_of_the_same_profile_are_accepted() {
        let remote = |id: &str| Location::Remote {
            profile_id: id.into(),
            path: "/a".into(),
        };
        assert_eq!(remote_path("p", &remote("p")).unwrap(), Path::new("/a"));
        assert!(remote_path("p", &remote("q")).unwrap_err().is_unsupported());
        assert!(remote_path("p", &Location::Local("/a".into()))
            .unwrap_err()
            .is_unsupported());
    }

    #[test]
    fn profile_must_be_sftp_and_key_auth_needs_a_key_file() {
        assert!(check_profile(&profile()).is_ok());

        let ftp = ConnectionProfile {
            protocol: Protocol::Ftp,
            ..profile()
        };
        assert!(check_profile(&ftp).is_err());

        let keyless = ConnectionProfile {
            auth: AuthMethod::PrivateKey,
            ..profile()
        };
        assert!(check_profile(&keyless).is_err());
    }

    #[test]
    fn password_auth_without_a_saved_password_fails_before_connecting() {
        struct NoSecrets;
        impl Secrets for NoSecrets {
            fn get(&self, _: &str) -> Result<Option<String>, crate::SecretsError> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), crate::SecretsError> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), crate::SecretsError> {
                Ok(())
            }
        }
        let err = SftpBackend::connect(&profile(), &NoSecrets, &KnownHosts::new("unused"))
            .err()
            .unwrap();
        assert_eq!(err, BackendError::Io(io::ErrorKind::InvalidInput.into()));
        assert!(err.to_string().contains("no password"));
    }

    #[test]
    fn wire_paths_are_utf8_with_forward_slashes() {
        assert_eq!(wire_path(Path::new("/a/b c")).unwrap(), "/a/b c");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bad = Path::new(std::ffi::OsStr::from_bytes(b"/\xff"));
            assert!(wire_path(bad).is_err());
        }
    }
}
