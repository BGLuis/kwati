use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
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
    FtpError, FtpResult, FtpStream, RustlsConnector, RustlsFtpStream, Status, TlsStream,
    TransferStream,
};

use crate::{
    AuthenticationFailed, Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryKind,
    EntryMeta, Listing, Location, Protocol, Secrets,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const GREETING_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const IMPLICIT_FTPS_PORT: u16 = 990;

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
        FtpError::BadResponse => io::Error::new(
            InvalidData,
            "the server sent a malformed or non-FTP response",
        ),
        err @ FtpError::InvalidAddress(_) => io::Error::new(InvalidData, err),
    }
}

/// A 530 reply to the login means the user name or password was refused; anything else (a lost
/// connection, a server error) keeps the usual mapping.
fn login_error(profile: &ConnectionProfile, err: FtpError) -> io::Error {
    match &err {
        FtpError::UnexpectedResponse(reply) if reply.status.code() == 530 => {
            AuthenticationFailed::error(profile)
        }
        _ => ftp_error(err),
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

    fn clear_timeouts(&self) {
        session!(self, s => {
            let sock = s.get_ref();
            let _ = sock.set_read_timeout(None);
            let _ = sock.set_write_timeout(None);
        });
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
        Self::open_with_probe_timeout(profile, secrets, extra_roots, GREETING_PROBE_TIMEOUT)
    }

    fn open_with_probe_timeout(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
        extra_roots: Option<&Path>,
        probe_timeout: Duration,
    ) -> Result<Self, BackendError> {
        check_profile(profile)?;
        let password = secrets
            .get(&profile.id)
            .map_err(io::Error::from)?
            .ok_or_else(|| invalid("no password is saved for the profile"))?;
        let tls = tls_config(extra_roots)?;

        let mut session = open_session(profile, tls, probe_timeout)?;
        session
            .login(&profile.user, &password)
            .map_err(|err| login_error(profile, err))?;
        let mlsd = session.supports_mlsd();
        session.clear_timeouts();
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

enum GreetingProbe {
    Plaintext(Vec<u8>),
    Tls,
    NoGreeting(io::Error),
}

fn open_session(
    profile: &ConnectionProfile,
    tls: Arc<ClientConfig>,
    probe_timeout: Duration,
) -> Result<Session, BackendError> {
    if profile.protocol == Protocol::Ftps && profile.port == IMPLICIT_FTPS_PORT {
        for addr in resolve_addrs(profile)? {
            match connect_implicit(addr, profile, Arc::clone(&tls)) {
                Ok(session) => return Ok(session),
                Err(err) if err.certificate_invalid().is_some() => return Err(err),
                Err(_) => {}
            }
        }
    }
    open_explicit(profile, tls, probe_timeout)
}

fn open_explicit(
    profile: &ConnectionProfile,
    tls: Arc<ClientConfig>,
    probe_timeout: Duration,
) -> Result<Session, BackendError> {
    let (stream, addr) = connect_tcp(profile)?;
    let preview = match probe_greeting(&stream, probe_timeout)? {
        GreetingProbe::Tls => {
            drop(stream);
            if profile.protocol == Protocol::Ftps {
                return connect_implicit(addr, profile, tls);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the server at {}:{} expects TLS; use FTPS instead",
                    profile.host, profile.port
                ),
            )
            .into());
        }
        GreetingProbe::NoGreeting(reason) => {
            drop(stream);
            if profile.protocol == Protocol::Ftps {
                return connect_implicit(addr, profile, tls);
            }
            return Err(reason.into());
        }
        GreetingProbe::Plaintext(preview) => preview,
    };

    if let Some(err) = check_non_ftp_banner(profile, &preview) {
        return Err(err.into());
    }

    stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    let ftp = match RustlsFtpStream::connect_with_stream(stream) {
        Ok(stream) => stream,
        Err(FtpError::BadResponse) => return Err(invalid_greeting_error(profile, &preview).into()),
        Err(err) => return Err(ftp_error(err).into()),
    };

    match ftp.into_secure(RustlsConnector::from(tls), &profile.host) {
        Ok(secure) => Ok(Session::Secure(secure)),
        Err(err) => {
            if let Some(reason) = certificate_failure(&err) {
                return Err(certificate_invalid_error(profile, reason));
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
            connect_plain(addr)
        }
    }
}

fn connect_implicit(
    addr: SocketAddr,
    profile: &ConnectionProfile,
    tls: Arc<ClientConfig>,
) -> Result<Session, BackendError> {
    let mut stream =
        RustlsFtpStream::connect_secure_implicit(addr, RustlsConnector::from(tls), &profile.host)
            .map_err(|err| {
            if let Some(reason) = certificate_failure(&err) {
                certificate_invalid_error(profile, reason)
            } else {
                ftp_error(err).into()
            }
        })?;
    {
        let sock = stream.get_ref();
        sock.set_read_timeout(Some(CONNECT_TIMEOUT))?;
        sock.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    }
    for cmd in ["PBSZ 0", "PROT P"] {
        match stream.custom_command(cmd, &[Status::CommandOk]) {
            Ok(_) => {}
            Err(FtpError::UnexpectedResponse(reply))
                if matches!(reply.status.code(), 500 | 502 | 504) => {}
            Err(err) => return Err(ftp_error(err).into()),
        }
    }
    Ok(Session::Secure(stream))
}

fn connect_plain(addr: SocketAddr) -> Result<Session, BackendError> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    FtpStream::connect_with_stream(stream)
        .map(Session::Plain)
        .map_err(|err| ftp_error(err).into())
}

fn certificate_invalid_error(profile: &ConnectionProfile, reason: String) -> BackendError {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        CertificateInvalid {
            host: profile.host.clone(),
            port: profile.port,
            reason,
        },
    )
    .into()
}

fn resolve_addrs(profile: &ConnectionProfile) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<_> = (profile.host.as_str(), profile.port)
        .to_socket_addrs()?
        .collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the host name has no address",
        ));
    }
    Ok(addrs)
}

/// Opens the raw TCP control connection to the profile's server, with nothing sent or read yet.
fn connect_tcp(profile: &ConnectionProfile) -> io::Result<(TcpStream, SocketAddr)> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the host name has no address");
    for addr in resolve_addrs(profile)? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok((stream, addr)),
            Err(err) => last = err,
        }
    }
    Err(last)
}

fn probe_greeting(stream: &TcpStream, timeout: Duration) -> io::Result<GreetingProbe> {
    stream.set_read_timeout(Some(timeout))?;
    let mut buf = [0u8; 256];
    match stream.peek(&mut buf) {
        Ok(0) => Ok(GreetingProbe::NoGreeting(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed before the server greeting",
        ))),
        Ok(n) if is_tls_record(&buf[..n]) => Ok(GreetingProbe::Tls),
        Ok(n) => Ok(GreetingProbe::Plaintext(buf[..n].to_vec())),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            Ok(GreetingProbe::NoGreeting(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the server greeting",
            )))
        }
        Err(err) => Err(err),
    }
}

fn is_tls_record(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && matches!(bytes[0], 0x15 | 0x16) && bytes[1] == 0x03 && bytes[2] <= 0x04
}

fn check_non_ftp_banner(profile: &ConnectionProfile, preview: &[u8]) -> Option<io::Error> {
    if preview.starts_with(b"SSH-") {
        let banner = sanitized_banner_line(preview);
        return Some(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the server at {}:{} speaks SSH/SFTP ({banner}); use SFTP instead",
                profile.host, profile.port
            ),
        ));
    }
    if preview.starts_with(b"HTTP/") {
        return Some(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the server at {}:{} responded with HTTP instead of FTP/FTPS",
                profile.host, profile.port
            ),
        ));
    }
    None
}

fn invalid_greeting_error(profile: &ConnectionProfile, preview: &[u8]) -> io::Error {
    let snippet = sanitized_banner_line(preview);
    if snippet.is_empty() {
        ftp_error(FtpError::BadResponse)
    } else {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the server at {}:{} sent an invalid FTP greeting ({snippet})",
                profile.host, profile.port
            ),
        )
    }
}

fn sanitized_banner_line(bytes: &[u8]) -> String {
    let first_line = bytes
        .split(|&b| b == b'\r' || b == b'\n')
        .next()
        .unwrap_or(bytes);
    let text = String::from_utf8_lossy(first_line);
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect()
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
    fn a_refused_login_is_an_authentication_failure_but_other_errors_are_not() {
        let refused = BackendError::from(login_error(
            &profile(Protocol::Ftp),
            reply(530, "Login incorrect."),
        ));
        let failure = refused.authentication_failed().unwrap();
        assert_eq!((failure.host.as_str(), failure.user.as_str()), ("h", "u"));

        let dropped = BackendError::from(login_error(
            &profile(Protocol::Ftp),
            reply(421, "Service not available"),
        ));
        assert!(dropped.authentication_failed().is_none());
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

    struct FixedPassword;
    impl Secrets for FixedPassword {
        fn get(&self, _: &str) -> Result<Option<String>, crate::SecretsError> {
            Ok(Some("secret".into()))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), crate::SecretsError> {
            unreachable!()
        }
        fn delete(&self, _: &str) -> Result<(), crate::SecretsError> {
            unreachable!()
        }
    }

    fn local_ftps_profile(port: u16) -> ConnectionProfile {
        ConnectionProfile {
            host: "127.0.0.1".into(),
            port,
            ..profile(Protocol::Ftps)
        }
    }

    fn serve_once_banner(banner: &'static [u8]) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.write_all(banner);
            let _ = stream.flush();
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf);
        });
        (port, handle)
    }

    #[test]
    fn ssh_banner_on_ftps_reports_ssh_suggestion() {
        let (port, handle) = serve_once_banner(b"SSH-2.0-SFTPGo_2.7.4\r\n");
        let err = FtpBackend::connect(&local_ftps_profile(port), &FixedPassword)
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(msg.contains("SSH/SFTP"), "{msg}");
        assert!(msg.contains("SSH-2.0-SFTPGo_2.7.4"), "{msg}");
        assert!(msg.contains("use SFTP instead"), "{msg}");
        handle.join().unwrap();
    }

    #[test]
    fn http_banner_on_ftps_reports_http_mismatch() {
        let (port, handle) = serve_once_banner(b"HTTP/1.1 400 Bad Request\r\n");
        let err = FtpBackend::connect(&local_ftps_profile(port), &FixedPassword)
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(msg.contains("HTTP"), "{msg}");
        handle.join().unwrap();
    }

    #[test]
    fn invalid_ftp_greeting_reports_greeting_snippet() {
        let (port, handle) = serve_once_banner(b"HELLO WORLD\r\n");
        let err = FtpBackend::connect(&local_ftps_profile(port), &FixedPassword)
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(msg.contains("invalid FTP greeting"), "{msg}");
        assert!(msg.contains("HELLO WORLD"), "{msg}");
        handle.join().unwrap();
    }

    // Test CA and leaf certificate for 127.0.0.1 / localhost (valid until 2098).
    const TEST_CA_PEM: &str = "\
-----BEGIN CERTIFICATE-----\n\
MIIBmDCCAT2gAwIBAgIUc6MvWArCjonRJKWjCAHwjLqA9qEwCgYIKoZIzj0EAwIw\n\
GDEWMBQGA1UEAwwNS3dhdGkgVGVzdCBDQTAgFw0yNjExMTMwNzIzNDdaGA8yMDk4\n\
MDExOTA3MjM0N1owGDEWMBQGA1UEAwwNS3dhdGkgVGVzdCBDQTBZMBMGByqGSM49\n\
AgEGCCqGSM49AwEHA0IABEb0y3CNBwQ+ti/z9k3sxMuhlMmdrM7b1qdi/Lmw2Irr\n\
T8/VVSa94dsGsU6KeeHfYNhr/UFi8NqgY0lNDZHDh+ejYzBhMB0GA1UdDgQWBBSf\n\
P+BJ0fR8vS6k4LTy9Flho10Q1TAfBgNVHSMEGDAWgBSfP+BJ0fR8vS6k4LTy9Flh\n\
o10Q1TAPBgNVHRMBAf8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjAKBggqhkjOPQQD\n\
AgNJADBGAiEA2Z3IwP0mZNU7dFGTyKbZiITL54rA9rot17qrC6P6m5MCIQDlgdSr\n\
SmqfA7yeIQzfnsL9jpoOQ1wXDfvsHA3FJGHZ2Q==\n\
-----END CERTIFICATE-----\n";

    const TEST_CERT_PEM: &str = "\
-----BEGIN CERTIFICATE-----\n\
MIIBvDCCAWOgAwIBAgIUScZmPrcFE+59XrdV6sXnd8mK+nswCgYIKoZIzj0EAwIw\n\
GDEWMBQGA1UEAwwNS3dhdGkgVGVzdCBDQTAgFw0yNjExMTMwNzIzNDdaGA8yMDk4\n\
MDExOTA3MjM0N1owFDESMBAGA1UEAwwJMTI3LjAuMC4xMFkwEwYHKoZIzj0CAQYI\n\
KoZIzj0DAQcDQgAExZHO1Cep/NacpzREEDYXghdJcwYqxCzqdonhpJaKZ03WbaLA\n\
0w0coUGSIfvDGt6ZUBTmljDTlenVW5MKXaTu96OBjDCBiTAJBgNVHRMEAjAAMAsG\n\
A1UdDwQEAwIFoDATBgNVHSUEDDAKBggrBgEFBQcDATAaBgNVHREEEzARhwR/AAAB\n\
gglsb2NhbGhvc3QwHQYDVR0OBBYEFNiOy6zqEFtALt9j7EuE1sXpThxPMB8GA1Ud\n\
IwQYMBaAFJ8/4EnR9Hy9LqTgtPL0WWGjXRDVMAoGCCqGSM49BAMCA0cAMEQCIG6m\n\
FnNKnqfH3BGNjRC9pHUiwkbRGKTVfUtRsMbNCbu3AiABdfgHeG6/jip6c8nIlrNF\n\
pO9WNuT85yHFeDJ+QLswhw==\n\
-----END CERTIFICATE-----\n";

    const TEST_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgtRZFkyhBHCcxBvBM\n\
fJCWG321a90dxnvQ6tTHMQAHQXShRANCAATFkc7UJ6n81pynNEQQNheCF0lzBirE\n\
LOp2ieGklopnTdZtosDTDRyhQZIh+8Ma3plQFOaWMNOV6dVbkwpdpO73\n\
-----END PRIVATE KEY-----\n";

    fn test_server_tls_config() -> Arc<rustls::ServerConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let certs: Vec<_> = CertificateDer::pem_slice_iter(TEST_CERT_PEM.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap();
        let key =
            rustls::pki_types::PrivateKeyDer::from_pem_slice(TEST_KEY_PEM.as_bytes()).unwrap();
        let cfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
        Arc::new(cfg)
    }

    fn spawn_implicit_ftps_server(
        server_cfg: Arc<rustls::ServerConfig>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            // First accept may be the explicit probe that times out before closing.
            loop {
                let Ok((sock, _)) = listener.accept() else {
                    return;
                };
                let conn = rustls::ServerConnection::new(Arc::clone(&server_cfg)).unwrap();
                let mut tls = rustls::StreamOwned::new(conn, sock);
                let mut peek = [0u8; 1];
                // If the client closed without sending ClientHello, wait for the implicit connect.
                if tls.sock.peek(&mut peek).ok() == Some(0) {
                    continue;
                }
                if tls.write_all(b"220 Implicit FTPS ready\r\n").is_err() {
                    return;
                }
                let _ = tls.flush();
                let mut reader = BufReader::new(tls);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let cmd = line.trim();
                    let reply = if cmd.starts_with("PBSZ ") || cmd.starts_with("PROT ") {
                        "200 OK\r\n".to_string()
                    } else if cmd.starts_with("USER ") {
                        "331 Password required\r\n".to_string()
                    } else if cmd.starts_with("PASS ") {
                        "230 Logged in\r\n".to_string()
                    } else if cmd.starts_with("TYPE ") {
                        "200 Type set\r\n".to_string()
                    } else if cmd == "FEAT" {
                        "211-Features:\r\n MLST size*;type*;\r\n211 End\r\n".to_string()
                    } else if cmd == "PASV" {
                        let data_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                        let data_port = data_listener.local_addr().unwrap().port();
                        let hi = data_port / 256;
                        let lo = data_port % 256;
                        let reply = format!("227 Entering Passive Mode (127,0,0,1,{hi},{lo})\r\n");
                        let _ = reader.get_mut().write_all(reply.as_bytes());
                        let _ = reader.get_mut().flush();
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let _ = reader
                            .get_mut()
                            .write_all(b"150 Opening data connection\r\n");
                        let _ = reader.get_mut().flush();
                        let (data_sock, _) = data_listener.accept().unwrap();
                        let data_conn =
                            rustls::ServerConnection::new(Arc::clone(&server_cfg)).unwrap();
                        let mut data_tls = rustls::StreamOwned::new(data_conn, data_sock);
                        data_tls
                            .write_all(b"type=file;size=5; hello.txt\r\n")
                            .unwrap();
                        data_tls.flush().unwrap();
                        data_tls.conn.send_close_notify();
                        let _ = data_tls.conn.write_tls(&mut data_tls.sock);
                        let _ = reader.get_mut().write_all(b"226 Transfer complete\r\n");
                        let _ = reader.get_mut().flush();
                        line.clear();
                        continue;
                    } else {
                        "502 Command not implemented\r\n".to_string()
                    };
                    if reader.get_mut().write_all(reply.as_bytes()).is_err() {
                        break;
                    }
                    let _ = reader.get_mut().flush();
                    line.clear();
                }
                return;
            }
        });
        (port, handle)
    }

    #[test]
    fn implicit_ftps_connects_and_lists_without_explicit_mode_setting() {
        let dir = std::env::temp_dir().join(format!("kwati_implicit_ftps_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ca = dir.join("ca.pem");
        std::fs::write(&ca, TEST_CA_PEM).unwrap();

        let (port, handle) = spawn_implicit_ftps_server(test_server_tls_config());
        let backend = FtpBackend::open_with_probe_timeout(
            &local_ftps_profile(port),
            &FixedPassword,
            Some(&ca),
            Duration::from_millis(100),
        )
        .unwrap();
        let listing = backend
            .list(&Location::Remote {
                profile_id: "p".into(),
                path: "/".into(),
            })
            .unwrap();
        let names: Vec<_> = listing.iter().map(|e| e.display_name()).collect();
        assert_eq!(names, ["hello.txt"]);
        drop(backend);
        handle.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn implicit_ftps_untrusted_cert_is_rejected_with_certificate_invalid() {
        let (port, handle) = spawn_implicit_ftps_server(test_server_tls_config());
        let err = FtpBackend::open_with_probe_timeout(
            &local_ftps_profile(port),
            &FixedPassword,
            None,
            Duration::from_millis(100),
        )
        .err()
        .unwrap();
        assert!(err.certificate_invalid().is_some(), "got: {err}");
        handle.join().unwrap();
    }
}
