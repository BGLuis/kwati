#![cfg(feature = "remote-tests")]
//! FTP and FTPS backend against real servers. Compiled only with the `remote-tests` feature:
//!
//! ```text
//! KWATI_FTP_TEST_HOST=127.0.0.1 KWATI_FTP_TEST_USER=kwati KWATI_FTP_TEST_PASSWORD=... \
//! KWATI_FTP_TEST_CA=/path/to/ca.pem \
//! KWATI_FTP_TEST_PORT=2121 KWATI_FTP_TEST_BADCERT_PORT=2122 KWATI_FTP_TEST_PLAIN_PORT=2123 \
//! cargo test -p kwati-core --features remote-tests ftp
//! ```
//!
//! Three servers with the same user and password are needed, each with its own passive port
//! range reachable from the test machine:
//!
//! - `_PORT`: explicit FTPS (`AUTH TLS`) with a certificate for `_HOST` signed by the CA in
//!   `_CA` (a PEM file, which the backend is told to trust in addition to the system roots).
//!   It should support `MLSD`; the user needs write access to `_ROOT` (default `/`, as the
//!   server sees it). The tests create and delete their own sub-folder there.
//! - `_BADCERT_PORT`: FTPS whose certificate is not signed by that CA.
//! - `_PLAIN_PORT`: a server that refuses `AUTH TLS`.

use kwati_core::{
    copy_between, AuthMethod, Backend, BackendError, Confirmed, ConflictPolicy, ConnectionProfile,
    CopyOptions, CopyOutcome, EntryKind, FtpBackend, LocalBackend, Location, Protocol, Secrets,
    SecretsError,
};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const PROFILE_ID: &str = "test-ftp";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name} to run the remote tests"))
}

#[derive(Default)]
struct MapSecrets(Mutex<HashMap<String, String>>);

impl Secrets for MapSecrets {
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

fn secrets() -> MapSecrets {
    let secrets = MapSecrets::default();
    secrets
        .set(PROFILE_ID, &env("KWATI_FTP_TEST_PASSWORD"))
        .unwrap();
    secrets
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "kwati_ftp_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root() -> String {
    std::env::var("KWATI_FTP_TEST_ROOT").unwrap_or_else(|_| "/".into())
}

fn profile(port_var: &str, allow_insecure: bool) -> ConnectionProfile {
    ConnectionProfile {
        id: PROFILE_ID.into(),
        name: "test".into(),
        protocol: Protocol::Ftps,
        host: env("KWATI_FTP_TEST_HOST"),
        port: env(port_var).parse().unwrap(),
        user: env("KWATI_FTP_TEST_USER"),
        start_path: root(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure,
    }
}

fn ca() -> PathBuf {
    env("KWATI_FTP_TEST_CA").into()
}

fn connect(port_var: &str, allow_insecure: bool) -> Result<FtpBackend, BackendError> {
    FtpBackend::connect_trusting(&profile(port_var, allow_insecure), &secrets(), &ca())
}

fn remote(path: &str) -> Location {
    Location::Remote {
        profile_id: PROFILE_ID.into(),
        path: path.into(),
    }
}

struct Server {
    backend: FtpBackend,
    dir: String,
}

impl Server {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let backend = connect("KWATI_FTP_TEST_PORT", false).unwrap();
        let dir = format!(
            "{}/kwati-ftp-{}-{}",
            root().trim_end_matches('/'),
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        );
        backend.create_dir(&remote(&dir)).unwrap();
        Self { backend, dir }
    }

    fn at(&self, name: &str) -> Location {
        remote(&format!("{}/{name}", self.dir))
    }

    fn put(&self, name: &str, content: &[u8]) {
        let mut w = self.backend.open_write(&self.at(name)).unwrap();
        w.write_all(content).unwrap();
        w.flush().unwrap();
    }

    fn get(&self, name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        self.backend
            .open_read(&self.at(name))
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self
            .backend
            .delete_permanently(&remote(&self.dir), Confirmed::user_confirmed());
    }
}

#[test]
fn ftp_list_and_stat() {
    let s = Server::new();
    s.put("a.txt", b"hello");
    s.backend.create_dir(&s.at("sub")).unwrap();

    let listing = s.backend.list(&remote(&s.dir)).unwrap();
    let names: Vec<_> = listing.iter().map(|e| e.display_name()).collect();
    assert_eq!(names, ["sub", "a.txt"], "folders first, no . or ..");
    assert!(listing[0].is_dir() && !listing[1].is_dir());

    let meta = s.backend.stat(&s.at("a.txt")).unwrap();
    assert_eq!((meta.kind, meta.len), (EntryKind::File, 5));
    assert_eq!(s.backend.stat(&s.at("sub")).unwrap().kind, EntryKind::Dir);
    assert_eq!(
        s.backend.stat(&s.at("missing")).unwrap_err(),
        BackendError::Io(std::io::ErrorKind::NotFound.into())
    );
}

#[test]
fn ftp_read_write_truncates() {
    let s = Server::new();
    s.put("f", b"a long first content");
    s.put("f", b"short");
    assert_eq!(s.get("f"), b"short");
}

#[test]
fn ftp_binary_roundtrip_keeps_every_byte() {
    let s = Server::new();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    s.put("big", &data);
    assert_eq!(s.get("big"), data);
    s.put("crlf", b"a\nb\r\nc\n");
    assert_eq!(s.get("crlf"), b"a\nb\r\nc\n", "no text-mode conversion");
}

#[test]
fn ftp_rename_replaces_file_and_moves_folder() {
    let s = Server::new();
    s.put("old", b"new content");
    s.put("target", b"to be replaced");
    s.backend.rename(&s.at("old"), &s.at("target")).unwrap();
    assert_eq!(s.get("target"), b"new content");
    assert!(s.backend.stat(&s.at("old")).is_err());

    s.backend.create_dir(&s.at("d1")).unwrap();
    s.backend.rename(&s.at("d1"), &s.at("d2")).unwrap();
    assert_eq!(s.backend.stat(&s.at("d2")).unwrap().kind, EntryKind::Dir);
}

#[test]
fn ftp_create_dir_refuses_existing() {
    let s = Server::new();
    s.backend.create_dir(&s.at("d")).unwrap();
    let err = s.backend.create_dir(&s.at("d")).unwrap_err();
    assert_eq!(
        err,
        BackendError::Io(std::io::ErrorKind::AlreadyExists.into())
    );
}

#[test]
fn ftp_delete_file_and_folder_with_contents() {
    let s = Server::new();
    s.put("f", b"x");
    s.backend
        .delete_permanently(&s.at("f"), Confirmed::user_confirmed())
        .unwrap();
    assert!(s.backend.stat(&s.at("f")).is_err());

    s.backend.create_dir(&s.at("tree")).unwrap();
    s.backend.create_dir(&s.at("tree/inner")).unwrap();
    s.put("tree/inner/deep", b"1");
    s.put("tree/top", b"2");
    s.backend
        .delete_permanently(&s.at("tree"), Confirmed::user_confirmed())
        .unwrap();
    assert!(s.backend.stat(&s.at("tree")).is_err());
}

#[test]
fn ftp_trash_is_unavailable() {
    let s = Server::new();
    s.put("f", b"x");
    assert!(s
        .backend
        .trash(&s.at("f"))
        .unwrap_err()
        .is_trash_unavailable());
    assert_eq!(s.get("f"), b"x");
}

#[test]
fn ftp_refuses_other_locations() {
    let s = Server::new();
    assert!(s
        .backend
        .list(&Location::Local("/".into()))
        .unwrap_err()
        .is_unsupported());
    let other = Location::Remote {
        profile_id: "other".into(),
        path: "/".into(),
    };
    assert!(s.backend.list(&other).unwrap_err().is_unsupported());
}

#[test]
fn ftp_one_transfer_at_a_time() {
    let s = Server::new();
    s.put("f", b"x");
    let reader = s.backend.open_read(&s.at("f")).unwrap();
    let err = s.backend.list(&remote(&s.dir)).unwrap_err();
    assert!(matches!(err, BackendError::Io(ref e) if e.kind() == std::io::ErrorKind::ResourceBusy));
    drop(reader);
    assert_eq!(s.backend.list(&remote(&s.dir)).unwrap().len(), 1);
}

#[test]
fn ftp_copy_between_local_backend() {
    let s = Server::new();
    s.backend.create_dir(&s.at("src")).unwrap();
    s.put("src/a.txt", b"alpha");
    s.backend.create_dir(&s.at("src/nested")).unwrap();
    s.put("src/nested/b.bin", &[7u8; 100_000]);

    let local = TempDir::new();
    let outcome = copy_between(
        &s.backend,
        &s.at("src"),
        &LocalBackend::new(),
        &Location::Local(local.0.clone()),
        &CopyOptions::new(ConflictPolicy::Replace, Arc::new(AtomicBool::new(false))),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(fs::read(local.0.join("src/a.txt")).unwrap(), b"alpha");
    assert_eq!(
        fs::read(local.0.join("src/nested/b.bin")).unwrap().len(),
        100_000
    );
}

#[test]
fn ftp_wrong_password_is_refused() {
    let secrets = MapSecrets::default();
    secrets.set(PROFILE_ID, "not the password").unwrap();
    let err = FtpBackend::connect_trusting(&profile("KWATI_FTP_TEST_PORT", false), &secrets, &ca())
        .err()
        .expect("must not log in");
    assert!(
        matches!(err, BackendError::Io(ref e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{err}"
    );
}

#[test]
fn ftp_refuses_plain() {
    let err = connect("KWATI_FTP_TEST_PLAIN_PORT", false)
        .err()
        .expect("a server without TLS must be refused");
    let refusal = err.tls_unavailable().expect("TlsUnavailable");
    assert_eq!(
        refusal.port,
        profile("KWATI_FTP_TEST_PLAIN_PORT", false).port
    );
    assert!(
        !kwati_core::is_dropped_connection(&err),
        "must not be retried"
    );
}

#[test]
fn ftp_plain_connects_only_with_allow_insecure() {
    let backend = connect("KWATI_FTP_TEST_PLAIN_PORT", true).unwrap();
    backend.list(&remote(&root())).unwrap();
}

#[test]
fn ftps_bad_certificate() {
    let err = connect("KWATI_FTP_TEST_BADCERT_PORT", true)
        .err()
        .expect("an untrusted certificate must not connect, even with allow_insecure");
    assert!(err.certificate_invalid().is_some(), "{err}");
    assert!(err.tls_unavailable().is_none());
}

#[test]
fn ftps_unknown_ca_is_not_trusted_by_default() {
    let err = FtpBackend::connect(&profile("KWATI_FTP_TEST_PORT", false), &secrets())
        .err()
        .expect("the test CA is not a system root");
    assert!(err.certificate_invalid().is_some(), "{err}");
}

#[test]
fn ftps_missing_extra_root_file_is_an_error() {
    let err = FtpBackend::connect_trusting(
        &profile("KWATI_FTP_TEST_PORT", false),
        &secrets(),
        Path::new("/nonexistent/ca.pem"),
    )
    .err()
    .unwrap();
    assert!(matches!(err, BackendError::Io(_)));
}
