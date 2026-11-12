#![cfg(feature = "remote-tests")]
//! SFTP backend against a real SSH server. Compiled only with the `remote-tests` feature:
//!
//! ```text
//! KWATI_SFTP_TEST_HOST=127.0.0.1 KWATI_SFTP_TEST_PORT=2222 KWATI_SFTP_TEST_USER=kwati \
//! KWATI_SFTP_TEST_KEY=/path/to/client_key KWATI_SFTP_TEST_ROOT=/writable/dir/on/server \
//! cargo test -p kwati-core --features remote-tests sftp_
//! ```
//!
//! The server must offer the `sftp` subsystem and accept the key (unencrypted) for the user;
//! `KWATI_SFTP_TEST_ROOT` is a directory the user may write to, as seen by the server. The tests
//! create and delete their own sub-folder in it. The host key tests disconnect before authenticating,
//! which OpenSSH 9.8+ penalises per source address, so start the server with
//! `PerSourcePenalties no`. Optionally set `KWATI_SFTP_TEST_ENCRYPTED_KEY`
//! and `KWATI_SFTP_TEST_PASSPHRASE` (same user, passphrase-protected key) to also run
//! `sftp_passphrase_key`.

use kwati_core::{
    copy_between, AuthMethod, Backend, BackendError, Confirmed, ConflictPolicy, ConnectionProfile,
    CopyOptions, CopyOutcome, EntryKind, KnownHosts, LocalBackend, Location, Protocol, Secrets,
    SecretsError, SftpBackend,
};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// OpenSSH drops connections beyond `MaxStartups` (10) unauthenticated handshakes at once.
static HANDSHAKES: Mutex<()> = Mutex::new(());

fn handshake() -> std::sync::MutexGuard<'static, ()> {
    HANDSHAKES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const PROFILE_ID: &str = "test-server";

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

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "kwati_sftp_test_{}_{}",
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

fn profile(key: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: PROFILE_ID.into(),
        name: "test".into(),
        protocol: Protocol::Sftp,
        host: env("KWATI_SFTP_TEST_HOST"),
        port: env("KWATI_SFTP_TEST_PORT").parse().unwrap(),
        user: env("KWATI_SFTP_TEST_USER"),
        start_path: env("KWATI_SFTP_TEST_ROOT"),
        auth: AuthMethod::PrivateKey,
        private_key_path: Some(key.into()),
        allow_insecure: false,
    }
}

fn remote(path: &str) -> Location {
    Location::Remote {
        profile_id: PROFILE_ID.into(),
        path: path.into(),
    }
}

/// Connects after trusting the server's key, and returns a fresh sub-folder of the test root.
struct Server {
    backend: SftpBackend,
    dir: String,
    _known_hosts: TempDir,
}

impl Server {
    fn new() -> Self {
        Self::with_key(&env("KWATI_SFTP_TEST_KEY"), &MapSecrets::default())
    }

    fn with_key(key: &str, secrets: &dyn Secrets) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let tmp = TempDir::new();
        let known_hosts = KnownHosts::new(tmp.0.join("known_hosts"));
        let profile = profile(key);
        let _one_handshake_at_a_time = handshake();
        let backend = match SftpBackend::connect(&profile, secrets, &known_hosts) {
            Err(err) => {
                let unknown = err.host_key_unknown().expect("first contact is unknown");
                known_hosts.trust(unknown).unwrap();
                SftpBackend::connect(&profile, secrets, &known_hosts).unwrap()
            }
            Ok(_) => panic!("an empty known_hosts must not connect"),
        };
        let dir = format!(
            "{}/kwati-sftp-{}-{}",
            profile.start_path.trim_end_matches('/'),
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        );
        drop(_one_handshake_at_a_time);
        backend.create_dir(&remote(&dir)).unwrap();
        Self {
            backend,
            dir,
            _known_hosts: tmp,
        }
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
fn sftp_list_and_stat() {
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
fn sftp_read_write_truncates() {
    let s = Server::new();
    s.put("f", b"a long first content");
    s.put("f", b"short");
    assert_eq!(s.get("f"), b"short");
}

#[test]
fn sftp_large_file_roundtrip() {
    let s = Server::new();
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    s.put("big", &data);
    assert_eq!(s.get("big"), data);
}

#[test]
fn sftp_rename_replaces_file_and_moves_folder() {
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
fn sftp_create_dir_refuses_existing() {
    let s = Server::new();
    s.backend.create_dir(&s.at("d")).unwrap();
    let err = s.backend.create_dir(&s.at("d")).unwrap_err();
    assert_eq!(
        err,
        BackendError::Io(std::io::ErrorKind::AlreadyExists.into())
    );
}

#[test]
fn sftp_delete_file_and_folder_with_contents() {
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

#[cfg(unix)]
#[test]
fn sftp_delete_symlink_never_follows() {
    let s = Server::new();
    if !std::path::Path::new(&s.dir).is_dir() {
        eprintln!("skipped: the server's files are not on this machine, so no symlink can be made");
        return;
    }
    s.backend.create_dir(&s.at("keep")).unwrap();
    s.put("keep/file", b"safe");
    let (link, target) = (s.at("link"), format!("{}/keep", s.dir));
    std::os::unix::fs::symlink(&target, link.path()).unwrap();
    assert_eq!(s.backend.stat(&link).unwrap().kind, EntryKind::Symlink);

    s.backend
        .delete_permanently(&link, Confirmed::user_confirmed())
        .unwrap();
    assert!(s.backend.stat(&link).is_err());
    assert_eq!(s.get("keep/file"), b"safe");
}

#[test]
fn sftp_trash_is_unavailable() {
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
fn sftp_refuses_other_locations() {
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
fn sftp_copy_between_local_backend() {
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
fn sftp_passphrase_key() {
    let (Ok(key), Ok(passphrase)) = (
        std::env::var("KWATI_SFTP_TEST_ENCRYPTED_KEY"),
        std::env::var("KWATI_SFTP_TEST_PASSPHRASE"),
    ) else {
        eprintln!("skipped: KWATI_SFTP_TEST_ENCRYPTED_KEY / _PASSPHRASE not set");
        return;
    };
    let tmp = TempDir::new();
    let known_hosts = KnownHosts::new(tmp.0.join("known_hosts"));
    let secrets = MapSecrets::default();
    let profile = profile(&key);
    let _one_handshake_at_a_time = handshake();
    secrets.set(PROFILE_ID, &passphrase).unwrap();
    let unknown = SftpBackend::connect(&profile, &secrets, &known_hosts)
        .err()
        .unwrap();
    known_hosts
        .trust(unknown.host_key_unknown().unwrap())
        .unwrap();

    secrets.delete(PROFILE_ID).unwrap();
    let err = SftpBackend::connect(&profile, &secrets, &known_hosts)
        .err()
        .unwrap();
    assert!(matches!(err, BackendError::Io(_)), "no passphrase: {err}");
    secrets.set(PROFILE_ID, "wrong").unwrap();
    assert!(SftpBackend::connect(&profile, &secrets, &known_hosts).is_err());

    secrets.set(PROFILE_ID, &passphrase).unwrap();
    let backend = SftpBackend::connect(&profile, &secrets, &known_hosts).unwrap();
    backend.list(&remote(&profile.start_path)).unwrap();
}

fn other_server_key_line(host: &str, port: u16) -> String {
    format!(
        "[{host}]:{port} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ\n"
    )
}

#[test]
fn sftp_host_key_changed() {
    let _one_handshake_at_a_time = handshake();
    let tmp = TempDir::new();
    let profile = profile(&env("KWATI_SFTP_TEST_KEY"));
    let path = tmp.0.join("known_hosts");
    fs::write(&path, other_server_key_line(&profile.host, profile.port)).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    let err = SftpBackend::connect(&profile, &MapSecrets::default(), &KnownHosts::new(&path))
        .err()
        .expect("a changed key must not connect");
    let changed = err.host_key_changed().expect("HostKeyChanged");
    assert!(changed.fingerprint.starts_with("SHA256:"));
    assert!(err.host_key_unknown().is_none());
    assert!(
        !kwati_core::is_dropped_connection(&err),
        "must not be retried"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "known_hosts untouched"
    );
}

#[test]
fn sftp_host_key_unknown() {
    let _one_handshake_at_a_time = handshake();
    let tmp = TempDir::new();
    let profile = profile(&env("KWATI_SFTP_TEST_KEY"));
    let known_hosts = KnownHosts::new(tmp.0.join("known_hosts"));
    let secrets = MapSecrets::default();

    let err = SftpBackend::connect(&profile, &secrets, &known_hosts)
        .err()
        .expect("an unknown key must not connect");
    let unknown = err.host_key_unknown().expect("HostKeyUnknown");
    assert!(unknown.fingerprint.starts_with("SHA256:"));
    assert_eq!(
        (unknown.host.as_str(), unknown.port),
        (profile.host.as_str(), profile.port)
    );
    assert!(
        !tmp.0.join("known_hosts").exists(),
        "nothing stored before the user decides"
    );

    known_hosts.trust(unknown).unwrap();
    let again = SftpBackend::connect(&profile, &secrets, &known_hosts).unwrap();
    again.list(&remote(&profile.start_path)).unwrap();

    let stored = fs::read_to_string(tmp.0.join("known_hosts")).unwrap();
    let b64 = stored.split_whitespace().nth(2).unwrap();
    let key = russh::keys::parse_public_key_base64(b64).unwrap();
    assert_eq!(
        key.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
        unknown.fingerprint
    );
}
