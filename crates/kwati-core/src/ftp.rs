use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::{CertificateError, ClientConfig, RootCertStore};
use suppaftp::list::{File, ListParser};
use suppaftp::types::{FileType, Mode};
use suppaftp::{
    FtpError, FtpResult, FtpStream, RustlsConnector, RustlsFtpStream, TlsStream, TransferStream,
};

use crate::{
    Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryKind, EntryMeta, Listing,
    Location, Protocol, Secrets,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The server does not offer TLS and the profile does not allow plain FTP; nothing, not even the
/// user name, was sent to it.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::PermissionDenied`]; detect it with
/// [`BackendError::tls_unavailable`]. The interface may offer to retry with
/// [`ConnectionProfile::allow_insecure`] set, after warning that credentials then travel in clear
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsUnavailable {
    pub host: String,
    pub port: u16,
    /// What the server answered to the request to start TLS.
    pub reply: String,
}

impl fmt::Display for TlsUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} does not offer TLS ({}); plain FTP is not allowed for this profile",
            self.host, self.port, self.reply
        )
    }
}

impl std::error::Error for TlsUnavailable {}

/// The server's TLS certificate was rejected (unknown issuer, expired, wrong name, ...); nothing
/// was sent after the handshake. There is no way to bypass the check, not even with
/// [`ConnectionProfile::allow_insecure`].
///
/// Carried like [`TlsUnavailable`]; detect it with [`BackendError::certificate_invalid`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInvalid {
    pub host: String,
    pub port: u16,
    pub reason: String,
}

impl fmt::Display for CertificateInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the certificate of {}:{} was rejected: {}",
            self.host, self.port, self.reason
        )
    }
}

impl std::error::Error for CertificateInvalid {}

impl BackendError {
    /// The refusal behind this error, if the server offers no TLS and the profile forbids plain
    /// FTP.
    pub fn tls_unavailable(&self) -> Option<&TlsUnavailable> {
        self.ftp_cause()
    }

    /// The rejection behind this error, if the server's certificate was not accepted.
    pub fn certificate_invalid(&self) -> Option<&CertificateInvalid> {
        self.ftp_cause()
    }

    fn ftp_cause<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            BackendError::Io(err) => err.get_ref()?.downcast_ref(),
            BackendError::Unsupported => None,
        }
    }
}

/// Maps FTP failures to the [`io::ErrorKind`]s the rest of the crate reasons about, so a lost
/// connection is seen by [`is_dropped_connection`](crate::is_dropped_connection).
fn ftp_error(err: FtpError) -> io::Error {
    use io::ErrorKind::*;
    match err {
        FtpError::ConnectionError(err) => err,
        FtpError::SecureError(message) => io::Error::other(message),
        FtpError::UnexpectedResponse(response) => {
            let body = String::from_utf8_lossy(&response.body);
            let text = body.trim().to_lowercase();
            let denied = text.contains("denied")
                || text.contains("permission")
                || text.contains("not allowed");
            let kind = match response.status.code() {
                421 => ConnectionAborted,
                425 | 426 => ConnectionAborted,
                530 | 532 | 553 => PermissionDenied,
                450 | 550 if denied => PermissionDenied,
                450 | 550 => NotFound,
                452 | 552 => StorageFull,
                500 | 502 | 504 => Unsupported,
                501 => InvalidInput,
                _ => Other,
            };
            io::Error::new(kind, format!("{}: {}", response.status.code(), body.trim()))
        }
        FtpError::DataConnectionAlreadyOpen => busy(),
        err @ (FtpError::BadResponse | FtpError::InvalidAddress(_)) => {
            io::Error::new(InvalidData, err)
        }
    }
}

fn busy() -> io::Error {
    io::Error::new(
        io::ErrorKind::ResourceBusy,
        "another transfer is still open on this FTP connection",
    )
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

/// The certificate rejection hidden in a failed TLS handshake, if that is what failed. rustls
/// hands it over inside an [`io::Error`] because the handshake runs on the first read or write.
fn certificate_failure(err: &FtpError) -> Option<String> {
    let FtpError::ConnectionError(err) = err else {
        return None;
    };
    match err.get_ref()?.downcast_ref::<rustls::Error>()? {
        rustls::Error::InvalidCertificate(reason) => Some(match reason {
            CertificateError::UnknownIssuer => "the issuer is not trusted".to_owned(),
            other => format!("{other:?}"),
        }),
        _ => None,
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

/// The path as it goes on the wire: absolute, UTF-8, `/`-separated, without a trailing `/`.
fn wire_path(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("remote paths must be valid UTF-8"))?;
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    };
    if !text.starts_with('/') {
        return Err(invalid("remote paths must be absolute"));
    }
    Ok(match text.trim_end_matches('/') {
        "" => "/".to_owned(),
        trimmed => trimmed.to_owned(),
    })
}

fn split_parent(path: &str) -> Option<(&str, &str)> {
    let (parent, name) = path.rsplit_once('/')?;
    (!name.is_empty()).then_some((if parent.is_empty() { "/" } else { parent }, name))
}

fn child(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// What `connect` needs from the profile, checked before any network traffic.
fn check_profile(profile: &ConnectionProfile) -> io::Result<()> {
    if !matches!(profile.protocol, Protocol::Ftp | Protocol::Ftps) {
        return Err(invalid("the profile is not an FTP or FTPS profile"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Remote {
    name: String,
    kind: EntryKind,
    len: u64,
}

/// Turns the lines of an `MLSD` or `LIST` reply into entries. `LIST` has no standard format, so
/// each line is tried as Unix style, then as DOS style; lines in neither (a `total` line, a
/// server's banner) and the `.` and `..` entries are dropped.
fn parse_listing(lines: &[String], mlsd: bool) -> Vec<Remote> {
    lines
        .iter()
        .filter_map(|line| {
            let parsed = if mlsd {
                ListParser::parse_mlsd(line)
            } else {
                ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line))
            };
            parsed.ok().and_then(remote_entry)
        })
        .collect()
}

fn remote_entry(file: File) -> Option<Remote> {
    let name = file.name();
    if matches!(name, "." | "..") || name.contains('/') {
        return None;
    }
    let kind = if file.is_directory() {
        EntryKind::Dir
    } else if file.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    let len = if kind == EntryKind::File {
        file.size() as u64
    } else {
        0
    };
    Some(Remote {
        name: name.to_owned(),
        kind,
        len,
    })
}

fn tls_config(extra_roots: Option<&Path>) -> io::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    if let Some(path) = extra_roots {
        let mut found = 0;
        for cert in CertificateDer::pem_file_iter(path).map_err(pem_error)? {
            roots
                .add(cert.map_err(pem_error)?)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            found += 1;
        }
        if found == 0 {
            return Err(invalid("the extra roots file holds no certificate"));
        }
    }
    if roots.is_empty() {
        return Err(io::Error::other(
            "no trusted root certificates are available on this system",
        ));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn pem_error(err: rustls::pki_types::pem::Error) -> io::Error {
    match err {
        rustls::pki_types::pem::Error::Io(err) => err,
        err => io::Error::new(io::ErrorKind::InvalidData, err),
    }
}

/// The control connection, plain or TLS-protected. The two have different types in `suppaftp`.
enum Session {
    Plain(FtpStream),
    Secure(RustlsFtpStream),
}

macro_rules! session {
    ($self:expr, $s:ident => $body:expr) => {
        match $self {
            Session::Plain($s) => $body,
            Session::Secure($s) => $body,
        }
    };
}

impl Session {
    fn login(&mut self, user: &str, password: &str) -> FtpResult<()> {
        session!(self, s => {
            s.login(user, password)?;
            s.set_mode(Mode::Passive);
            // Servers behind NAT often announce an unreachable address in the passive reply.
            s.set_passive_nat_workaround(true);
            s.transfer_type(FileType::Binary)
        })
    }

    fn supports_mlsd(&mut self) -> bool {
        session!(self, s => s.feat())
            .is_ok_and(|features| features.keys().any(|key| key.eq_ignore_ascii_case("MLST")))
    }

    fn mlsd(&mut self, path: &str) -> FtpResult<Vec<String>> {
        session!(self, s => s.mlsd(Some(path)))
    }

    fn list(&mut self, path: &str) -> FtpResult<Vec<String>> {
        session!(self, s => s.list(Some(path)))
    }

    fn mkdir(&mut self, path: &str) -> FtpResult<()> {
        session!(self, s => s.mkdir(path))
    }

    fn rm(&mut self, path: &str) -> FtpResult<()> {
        session!(self, s => s.rm(path))
    }

    fn rmdir(&mut self, path: &str) -> FtpResult<()> {
        session!(self, s => s.rmdir(path))
    }

    fn rename(&mut self, from: &str, to: &str) -> FtpResult<()> {
        session!(self, s => s.rename(from, to))
    }

    fn download(&mut self, path: &str, busy: &Arc<AtomicBool>) -> FtpResult<Box<dyn Read + Send>> {
        session!(self, s => Ok(Box::new(Transfer::start(s.retr_as_stream(path)?, busy))))
    }

    fn upload(&mut self, path: &str, busy: &Arc<AtomicBool>) -> FtpResult<Box<dyn Write + Send>> {
        session!(self, s => Ok(Box::new(Transfer::start(s.put_with_stream(path)?, busy))))
    }
}

struct Inner {
    session: Session,
    mlsd: bool,
}

impl Inner {
    fn read_dir(&mut self, path: &str) -> io::Result<Vec<Remote>> {
        let lines = if self.mlsd {
            self.session.mlsd(path)
        } else {
            self.session.list(path)
        };
        Ok(parse_listing(&lines.map_err(ftp_error)?, self.mlsd))
    }

    fn stat(&mut self, path: &str) -> io::Result<Remote> {
        let Some((parent, name)) = split_parent(path) else {
            return Ok(Remote {
                name: String::new(),
                kind: EntryKind::Dir,
                len: 0,
            });
        };
        self.read_dir(parent)?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("{path} does not exist"))
            })
    }

    fn exists(&mut self, path: &str) -> io::Result<Option<Remote>> {
        match self.stat(path) {
            Ok(entry) => Ok(Some(entry)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn delete_tree(&mut self, path: &str) -> io::Result<()> {
        for entry in self.read_dir(path)? {
            let path = child(path, &entry.name);
            match entry.kind {
                EntryKind::Dir => self.delete_tree(&path)?,
                EntryKind::File | EntryKind::Symlink => {
                    self.session.rm(&path).map_err(ftp_error)?
                }
            }
        }
        self.session.rmdir(path).map_err(ftp_error)
    }
}

/// An FTP or FTPS server, browsed and changed through the [`Backend`] trait.
///
/// The connection is blocking and not thread-safe, so calls are serialized by a mutex; call it
/// from a worker thread, never from the interface's executor. Passive mode and binary transfers
/// only. There is no trash on an FTP server: [`Backend::trash`] keeps its unavailable default.
///
/// Listings use `MLSD` when the server announces it and otherwise parse `LIST` output in the Unix
/// or DOS style. Other `LIST` formats yield no entries. Symlinks are listed as plain files, never
/// followed, and deleted as links.
///
/// [`Backend::open_read`] and [`Backend::open_write`] stream over a data connection that stays
/// open until the reader or writer is dropped. The control connection serves nothing else in the
/// meantime: every other call, including a second transfer on the same backend, fails with
/// [`io::ErrorKind::ResourceBusy`]. Dropping a writer ends the upload, but only
/// [`Write::flush`] reports whether the server accepted it, so flush before dropping; after the
/// flush the writer takes no more data.
pub struct FtpBackend {
    profile_id: String,
    inner: Mutex<Inner>,
    transferring: Arc<AtomicBool>,
}

impl FtpBackend {
    /// Connects as `profile` says, trusting the system's root certificates.
    ///
    /// Explicit FTPS (`AUTH TLS`) is tried first and the certificate is verified. If the server
    /// does not offer TLS, the connection is refused before the user name is sent with
    /// [`BackendError::tls_unavailable`], unless the profile has `allow_insecure`, in which case
    /// plain FTP is used. An invalid certificate fails with
    /// [`BackendError::certificate_invalid`] whatever the profile says. The password is read
    /// from `secrets` under the profile id and a missing one is an error.
    ///
    /// Accepts [`Protocol::Ftp`] and [`Protocol::Ftps`] profiles alike: both try TLS first.
    pub fn connect(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
    ) -> Result<Self, BackendError> {
        Self::open(profile, secrets, None)
    }

    /// Like [`connect`](Self::connect), but also trusts the certificates in the PEM file
    /// `extra_roots`, for a server with a private CA. The system roots stay trusted; there is no
    /// way to turn verification off.
    pub fn connect_trusting(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
        extra_roots: &Path,
    ) -> Result<Self, BackendError> {
        Self::open(profile, secrets, Some(extra_roots))
    }

    fn open(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
        extra_roots: Option<&Path>,
    ) -> Result<Self, BackendError> {
        check_profile(profile)?;
        let password = secrets
            .get(&profile.id)
            .map_err(io::Error::from)?
            .ok_or_else(|| invalid("no password is saved for the profile"))?;
        let tls = tls_config(extra_roots)?;

        let mut session = match dial(profile, RustlsFtpStream::connect_timeout)?
            .into_secure(RustlsConnector::from(tls), &profile.host)
        {
            Ok(secure) => Session::Secure(secure),
            Err(err) => {
                if let Some(reason) = certificate_failure(&err) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        CertificateInvalid {
                            host: profile.host.clone(),
                            port: profile.port,
                            reason,
                        },
                    )
                    .into());
                }
                let FtpError::UnexpectedResponse(reply) = err else {
                    return Err(ftp_error(err).into());
                };
                if !profile.allow_insecure {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        TlsUnavailable {
                            host: profile.host.clone(),
                            port: profile.port,
                            reply: format!(
                                "{} {}",
                                reply.status.code(),
                                String::from_utf8_lossy(&reply.body).trim()
                            ),
                        },
                    )
                    .into());
                }
                Session::Plain(dial(profile, FtpStream::connect_timeout)?)
            }
        };
        session.login(&profile.user, &password).map_err(ftp_error)?;
        let mlsd = session.supports_mlsd();
        Ok(Self {
            profile_id: profile.id.clone(),
            inner: Mutex::new(Inner { session, mlsd }),
            transferring: Arc::new(AtomicBool::new(false)),
        })
    }

    fn with<T>(
        &self,
        location: &Location,
        op: impl FnOnce(&mut Inner, &str) -> io::Result<T>,
    ) -> Result<T, BackendError> {
        let path = wire_path(remote_path(&self.profile_id, location)?)?;
        let mut inner: MutexGuard<'_, Inner> = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.transferring.load(Ordering::SeqCst) {
            return Err(busy().into());
        }
        Ok(op(&mut inner, &path)?)
    }
}

/// Opens the control connection to the profile's server, plain and with nothing sent yet.
fn dial<S>(
    profile: &ConnectionProfile,
    connect: fn(SocketAddr, Duration) -> FtpResult<S>,
) -> io::Result<S> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the host name has no address");
    for addr in (profile.host.as_str(), profile.port).to_socket_addrs()? {
        match connect(addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(err) => last = ftp_error(err),
        }
    }
    Err(last)
}

impl Backend for FtpBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        self.with(location, |inner, dir| {
            let entries = inner
                .read_dir(dir)?
                .into_iter()
                .map(|entry| {
                    let path = location.path().join(&entry.name);
                    Entry::new(entry.name.into(), path, entry.kind == EntryKind::Dir)
                })
                .collect();
            Ok(Listing::new(entries))
        })
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        self.with(location, |inner, path| {
            let entry = inner.stat(path)?;
            Ok(EntryMeta {
                kind: entry.kind,
                len: entry.len,
            })
        })
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        self.with(location, |inner, path| {
            inner
                .session
                .download(path, &self.transferring)
                .map_err(ftp_error)
        })
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        self.with(location, |inner, path| {
            inner
                .session
                .upload(path, &self.transferring)
                .map_err(ftp_error)
        })
    }

    /// Renames onto an existing file by removing it when the server refuses to replace it. That
    /// is not atomic: if the rename then fails, the file at `to` is already gone.
    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let to = wire_path(remote_path(&self.profile_id, to)?)?;
        self.with(from, |inner, from| {
            let Err(refused) = inner.session.rename(from, &to) else {
                return Ok(());
            };
            match inner.exists(&to)? {
                Some(existing) if existing.kind != EntryKind::Dir => {
                    inner.session.rm(&to).map_err(ftp_error)?;
                    inner.session.rename(from, &to).map_err(ftp_error)
                }
                _ => Err(ftp_error(refused)),
            }
        })
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        self.with(location, |inner, path| {
            if inner.exists(path)?.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "target already exists",
                ));
            }
            inner.session.mkdir(path).map_err(ftp_error)
        })
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        self.with(location, |inner, path| match inner.stat(path)?.kind {
            EntryKind::Dir => inner.delete_tree(path),
            EntryKind::File | EntryKind::Symlink => inner.session.rm(path).map_err(ftp_error),
        })
    }
}

/// An open data connection as a blocking `Read` or `Write`. It owns the right to use the control
/// connection: `transferring` stays set until the transfer is finished.
struct Transfer<T: TlsStream> {
    stream: Option<TransferStream<T>>,
    transferring: Arc<AtomicBool>,
}

impl<T: TlsStream> Transfer<T> {
    fn start(stream: TransferStream<T>, transferring: &Arc<AtomicBool>) -> Self {
        transferring.store(true, Ordering::SeqCst);
        Self {
            stream: Some(stream),
            transferring: transferring.clone(),
        }
    }

    fn stream(&mut self) -> io::Result<&mut TransferStream<T>> {
        self.stream.as_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the transfer is already finished",
            )
        })
    }

    fn finish(&mut self) -> io::Result<()> {
        let result = self
            .stream
            .take()
            .map_or(Ok(()), |stream| stream.finish().map_err(ftp_error));
        self.transferring.store(false, Ordering::SeqCst);
        result
    }
}

impl<T: TlsStream> Read for Transfer<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream()?.read(buf)
    }
}

impl<T: TlsStream> Write for Transfer<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(stream) = self.stream.as_mut() {
            stream.flush()?;
        }
        self.finish()
    }
}

impl<T: TlsStream> Drop for Transfer<T> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{is_dropped_connection, AuthMethod};
    use suppaftp::types::Response;
    use suppaftp::Status;

    fn profile(protocol: Protocol) -> ConnectionProfile {
        ConnectionProfile {
            id: "p".into(),
            name: "n".into(),
            protocol,
            host: "h".into(),
            port: 21,
            user: "u".into(),
            start_path: "/".into(),
            auth: AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        }
    }

    fn reply(code: u32, body: &str) -> FtpError {
        FtpError::UnexpectedResponse(Response {
            status: Status::from(code),
            body: body.as_bytes().to_vec(),
        })
    }

    fn kind_of(err: FtpError) -> io::ErrorKind {
        ftp_error(err).kind()
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn only_ftp_and_ftps_profiles_are_accepted() {
        assert!(check_profile(&profile(Protocol::Ftp)).is_ok());
        assert!(check_profile(&profile(Protocol::Ftps)).is_ok());
        assert!(check_profile(&profile(Protocol::Sftp)).is_err());
        assert!(check_profile(&profile(Protocol::Smb)).is_err());
    }

    #[test]
    fn a_profile_of_another_protocol_is_refused_before_any_network_traffic() {
        struct NoSecrets;
        impl Secrets for NoSecrets {
            fn get(&self, _: &str) -> Result<Option<String>, crate::SecretsError> {
                panic!("the secret must not be read for a wrong profile")
            }
            fn set(&self, _: &str, _: &str) -> Result<(), crate::SecretsError> {
                unreachable!()
            }
            fn delete(&self, _: &str) -> Result<(), crate::SecretsError> {
                unreachable!()
            }
        }
        let err = FtpBackend::connect(&profile(Protocol::Sftp), &NoSecrets)
            .err()
            .unwrap();
        assert_eq!(err, BackendError::Io(io::ErrorKind::InvalidInput.into()));
    }

    #[test]
    fn a_missing_password_is_an_error_not_an_anonymous_login() {
        struct Empty;
        impl Secrets for Empty {
            fn get(&self, _: &str) -> Result<Option<String>, crate::SecretsError> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), crate::SecretsError> {
                unreachable!()
            }
            fn delete(&self, _: &str) -> Result<(), crate::SecretsError> {
                unreachable!()
            }
        }
        let err = FtpBackend::connect(&profile(Protocol::Ftps), &Empty)
            .err()
            .unwrap();
        assert_eq!(err, BackendError::Io(io::ErrorKind::InvalidInput.into()));
    }

    #[test]
    fn locations_of_other_backends_are_unsupported() {
        let remote = |id: &str| Location::Remote {
            profile_id: id.into(),
            path: "/".into(),
        };
        assert!(remote_path("p", &remote("p")).is_ok());
        assert!(remote_path("p", &remote("q")).unwrap_err().is_unsupported());
        assert!(remote_path("p", &Location::Local("/".into()))
            .unwrap_err()
            .is_unsupported());
    }

    #[test]
    fn wire_paths_are_absolute_and_normalised() {
        assert_eq!(wire_path(Path::new("/a/b/")).unwrap(), "/a/b");
        assert_eq!(wire_path(Path::new("/")).unwrap(), "/");
        assert_eq!(wire_path(Path::new("//")).unwrap(), "/");
        assert!(wire_path(Path::new("relative/dir")).is_err());
        assert!(wire_path(Path::new("")).is_err());
    }

    #[test]
    fn the_parent_of_a_path_is_split_off_and_the_root_has_none() {
        assert_eq!(split_parent("/a/b"), Some(("/a", "b")));
        assert_eq!(split_parent("/a"), Some(("/", "a")));
        assert_eq!(split_parent("/"), None);
    }

    #[test]
    fn replies_map_to_the_kinds_the_crate_reasons_about() {
        use io::ErrorKind::*;
        assert_eq!(kind_of(reply(550, "No such file or directory")), NotFound);
        assert_eq!(kind_of(reply(550, "Permission denied")), PermissionDenied);
        assert_eq!(kind_of(reply(530, "Login incorrect")), PermissionDenied);
        assert_eq!(kind_of(reply(552, "Disk full")), StorageFull);
        assert_eq!(kind_of(reply(502, "not implemented")), Unsupported);
        assert_eq!(kind_of(reply(421, "Timeout")), ConnectionAborted);
        assert_eq!(kind_of(reply(599, "?")), Other);
        assert_eq!(kind_of(FtpError::DataConnectionAlreadyOpen), ResourceBusy);
        assert_eq!(kind_of(FtpError::BadResponse), InvalidData);
    }

    #[test]
    fn a_lost_connection_is_seen_as_dropped_but_a_refusal_is_not() {
        let reset = io::Error::from(io::ErrorKind::ConnectionReset);
        let lost = BackendError::Io(ftp_error(FtpError::ConnectionError(reset)));
        assert!(is_dropped_connection(&lost));
        assert!(is_dropped_connection(&BackendError::Io(ftp_error(reply(
            421, "bye"
        )))));
        assert!(!is_dropped_connection(&BackendError::Io(ftp_error(reply(
            550, "no"
        )))));
    }

    #[test]
    fn the_reply_text_survives_in_the_message() {
        assert!(ftp_error(reply(550, "No such file"))
            .to_string()
            .contains("No such file"));
    }

    #[test]
    fn an_untrusted_issuer_is_a_certificate_failure_and_other_tls_errors_are_not() {
        let tls = |err: rustls::Error| {
            FtpError::ConnectionError(io::Error::new(io::ErrorKind::InvalidData, err))
        };
        let issuer = tls(rustls::Error::InvalidCertificate(
            CertificateError::UnknownIssuer,
        ));
        assert!(certificate_failure(&issuer).is_some());
        let expired = tls(rustls::Error::InvalidCertificate(CertificateError::Expired));
        assert!(certificate_failure(&expired).is_some());
        assert!(certificate_failure(&tls(rustls::Error::HandshakeNotComplete)).is_none());
        let plain = FtpError::ConnectionError(io::Error::from(io::ErrorKind::ConnectionReset));
        assert!(certificate_failure(&plain).is_none());
        assert!(certificate_failure(&reply(500, "no")).is_none());
    }

    #[test]
    fn typed_refusals_are_found_through_the_backend_error() {
        let refusal = TlsUnavailable {
            host: "h".into(),
            port: 21,
            reply: "500 no".into(),
        };
        let err = BackendError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            refusal.clone(),
        ));
        assert_eq!(err.tls_unavailable(), Some(&refusal));
        assert!(err.certificate_invalid().is_none());
        assert!(!is_dropped_connection(&err));
        assert!(BackendError::Unsupported.tls_unavailable().is_none());
    }

    #[test]
    fn mlsd_lines_give_kinds_and_sizes_and_skip_the_dot_entries() {
        let entries = parse_listing(
            &lines(&[
                "type=cdir;modify=20201019151930; .",
                "type=pdir;modify=20201019151930; ..",
                "type=dir;modify=20201019151930; sub dir",
                "type=file;size=1234;modify=20201019151930; a b.txt",
                "type=OS.unix=socket;size=0; sock",
            ]),
            true,
        );
        assert_eq!(
            entries,
            [
                Remote {
                    name: "sub dir".into(),
                    kind: EntryKind::Dir,
                    len: 0
                },
                Remote {
                    name: "a b.txt".into(),
                    kind: EntryKind::File,
                    len: 1234
                },
            ]
        );
    }

    #[test]
    fn unix_list_lines_are_parsed_and_the_total_line_is_ignored() {
        let entries = parse_listing(
            &lines(&[
                "total 12",
                "drwxr-xr-x    2 ftp      ftp          4096 Nov 05  2018 docs",
                "-rw-r--r--    1 ftp      ftp           512 Nov 05  2018 readme.txt",
                "lrwxrwxrwx    1 ftp      ftp             4 Nov 05  2018 link -> docs",
                "drwxr-xr-x    2 ftp      ftp          4096 Nov 05  2018 .",
                "drwxr-xr-x    2 ftp      ftp          4096 Nov 05  2018 ..",
            ]),
            false,
        );
        assert_eq!(
            entries,
            [
                Remote {
                    name: "docs".into(),
                    kind: EntryKind::Dir,
                    len: 0
                },
                Remote {
                    name: "readme.txt".into(),
                    kind: EntryKind::File,
                    len: 512
                },
                Remote {
                    name: "link".into(),
                    kind: EntryKind::Symlink,
                    len: 0
                },
            ]
        );
    }

    #[test]
    fn dos_list_lines_are_the_fallback_when_unix_parsing_fails() {
        let entries = parse_listing(
            &lines(&[
                "10-19-20  03:19PM       <DIR>          pub",
                "04-08-14  03:09PM                  403 readme.txt",
                "this line is in no known format",
            ]),
            false,
        );
        assert_eq!(
            entries,
            [
                Remote {
                    name: "pub".into(),
                    kind: EntryKind::Dir,
                    len: 0
                },
                Remote {
                    name: "readme.txt".into(),
                    kind: EntryKind::File,
                    len: 403
                },
            ]
        );
    }

    #[test]
    fn extra_roots_must_be_a_readable_pem_with_a_certificate() {
        let dir = std::env::temp_dir().join(format!("kwati_ftp_roots_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();
        assert!(tls_config(Some(&empty)).is_err());
        assert!(tls_config(Some(&dir.join("missing.pem"))).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
