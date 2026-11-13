#![cfg(feature = "remote-tests")]
//! SMB backend against a real Samba or Windows server. Compiled only with the `remote-tests`
//! feature:
//!
//! ```text
//! KWATI_SMB_TEST_HOST=127.0.0.1 KWATI_SMB_TEST_PORT=1445 KWATI_SMB_TEST_USER=kwati \
//! KWATI_SMB_TEST_PASSWORD=secret KWATI_SMB_TEST_SHARE=share \
//! cargo test -p kwati-core --features remote-tests smb_
//! ```
//!
//! The user must be able to write to the share. The tests create and delete their own folder in
//! it. Set `KWATI_SMB_TEST_SMB1_PORT` to the port of a server limited to SMB1
//! (`server max protocol = NT1`) to also run `smb_refuses_smb1`.

use kwati_core::{
    copy_between, AuthMethod, Backend, BackendError, Confirmed, ConflictPolicy, ConnectionProfile,
    CopyOptions, CopyOutcome, EntryKind, LocalBackend, Location, Protocol, Secrets, SecretsError,
    SmbBackend,
};
use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const PROFILE_ID: &str = "test-smb";

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name} to run the remote tests"))
}

#[derive(Default)]
struct MapSecrets(Mutex<HashMap<String, String>>);

impl MapSecrets {
    fn with(password: &str) -> Self {
        let secrets = Self::default();
        secrets.set(PROFILE_ID, password).unwrap();
        secrets
    }
}

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

/// Counts reads of the password, to show a refused one is not retried.
struct CountingSecrets {
    inner: MapSecrets,
    reads: AtomicUsize,
}

impl Secrets for CountingSecrets {
    fn get(&self, id: &str) -> Result<Option<String>, SecretsError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(id)
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretsError> {
        self.inner.set(id, secret)
    }
    fn delete(&self, id: &str) -> Result<(), SecretsError> {
        self.inner.delete(id)
    }
}

fn profile(port: u16) -> ConnectionProfile {
    ConnectionProfile {
        id: PROFILE_ID.into(),
        name: "test".into(),
        protocol: Protocol::Smb,
        host: env("KWATI_SMB_TEST_HOST"),
        port,
        user: env("KWATI_SMB_TEST_USER"),
        start_path: format!("/{}", env("KWATI_SMB_TEST_SHARE")),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    }
}

fn port() -> u16 {
    env("KWATI_SMB_TEST_PORT").parse().unwrap()
}

fn remote(path: &str) -> Location {
    Location::Remote {
        profile_id: PROFILE_ID.into(),
        path: path.into(),
    }
}

/// Connects and returns a fresh folder in the test share, deleted on drop.
struct Server {
    backend: SmbBackend,
    dir: String,
}

impl Server {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let profile = profile(port());
        let backend =
            SmbBackend::connect(&profile, &MapSecrets::with(&env("KWATI_SMB_TEST_PASSWORD")))
                .unwrap();
        let dir = format!(
            "{}/kwati-smb-{}-{}",
            profile.start_path,
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

fn kind(err: BackendError) -> ErrorKind {
    match err {
        BackendError::Io(err) => err.kind(),
        BackendError::Unsupported => ErrorKind::Unsupported,
    }
}

#[test]
fn smb_list_and_stat() {
    let s = Server::new();
    s.put("a.txt", b"hello");
    s.backend.create_dir(&s.at("sub")).unwrap();

    let listing = s.backend.list(&remote(&s.dir)).unwrap();
    let names: Vec<_> = listing.iter().map(|e| e.display_name()).collect();
    assert_eq!(names, ["sub", "a.txt"], "folders first, no . or ..");
    assert!(listing[0].is_dir() && !listing[1].is_dir());
    assert_eq!(listing[1].size, Some(5));

    let meta = s.backend.stat(&s.at("a.txt")).unwrap();
    assert_eq!((meta.kind, meta.len), (EntryKind::File, 5));
    assert_eq!(s.backend.stat(&s.at("sub")).unwrap().kind, EntryKind::Dir);
    assert_eq!(
        kind(s.backend.stat(&s.at("missing")).unwrap_err()),
        ErrorKind::NotFound
    );
}

#[test]
fn smb_read_write_truncates() {
    let s = Server::new();
    s.put("f", b"a long first content");
    s.put("f", b"short");
    assert_eq!(s.get("f"), b"short");

    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    s.put("big", &big);
    assert_eq!(
        s.get("big"),
        big,
        "content larger than one SMB read or write"
    );
}

#[test]
fn smb_rename_replaces_a_file() {
    let s = Server::new();
    s.put("from", b"new");
    s.put("to", b"old");
    s.backend.rename(&s.at("from"), &s.at("to")).unwrap();
    assert_eq!(s.get("to"), b"new");
    assert_eq!(
        kind(s.backend.stat(&s.at("from")).unwrap_err()),
        ErrorKind::NotFound
    );
}

#[test]
fn smb_create_dir_refuses_an_existing_name() {
    let s = Server::new();
    s.backend.create_dir(&s.at("d")).unwrap();
    assert_eq!(
        kind(s.backend.create_dir(&s.at("d")).unwrap_err()),
        ErrorKind::AlreadyExists
    );
}

#[test]
fn smb_delete_removes_a_folder_with_its_contents() {
    let s = Server::new();
    s.backend.create_dir(&s.at("d")).unwrap();
    s.backend.create_dir(&s.at("d/inner")).unwrap();
    s.put("d/inner/f", b"x");
    s.put("loose", b"y");

    s.backend
        .delete_permanently(&s.at("d"), Confirmed::user_confirmed())
        .unwrap();
    s.backend
        .delete_permanently(&s.at("loose"), Confirmed::user_confirmed())
        .unwrap();
    assert!(s.backend.list(&remote(&s.dir)).unwrap().is_empty());
    assert!(s
        .backend
        .trash(&s.at("x"))
        .unwrap_err()
        .is_trash_unavailable());
}

#[test]
fn smb_copy_from_local_and_back() {
    let s = Server::new();
    let local = std::env::temp_dir().join(format!("kwati_smb_copy_{}", std::process::id()));
    let _ = fs::remove_dir_all(&local);
    fs::create_dir_all(local.join("tree/nested")).unwrap();
    fs::write(local.join("tree/nested/f.txt"), b"payload").unwrap();
    let options = CopyOptions::new(ConflictPolicy::Skip, Default::default());

    let up = copy_between(
        &LocalBackend::default(),
        &Location::Local(local.join("tree")),
        &s.backend,
        &remote(&s.dir),
        &options,
        &mut |_| {},
    )
    .unwrap();
    assert!(matches!(up, CopyOutcome::Completed));
    assert_eq!(s.get("tree/nested/f.txt"), b"payload");

    let back: PathBuf = local.join("back");
    fs::create_dir_all(&back).unwrap();
    copy_between(
        &s.backend,
        &s.at("tree"),
        &LocalBackend::default(),
        &Location::Local(back.clone()),
        &options,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(back.join("tree/nested/f.txt")).unwrap(),
        b"payload"
    );
    let _ = fs::remove_dir_all(&local);
}

#[test]
fn smb_bad_credentials() {
    let secrets = CountingSecrets {
        inner: MapSecrets::with("certainly-not-the-password"),
        reads: AtomicUsize::new(0),
    };
    let err = SmbBackend::connect(&profile(port()), &secrets)
        .err()
        .expect("a wrong password must not connect");
    let failure = err
        .authentication_failed()
        .expect("reported as AuthenticationFailed");
    assert_eq!(failure.user, env("KWATI_SMB_TEST_USER"));
    assert_eq!(
        secrets.reads.load(Ordering::SeqCst),
        1,
        "the password is read once and never retried"
    );
}

#[test]
fn smb_refuses_smb1() {
    let Ok(port) = std::env::var("KWATI_SMB_TEST_SMB1_PORT") else {
        eprintln!("KWATI_SMB_TEST_SMB1_PORT is not set; skipping");
        return;
    };
    let err = SmbBackend::connect(
        &profile(port.parse().unwrap()),
        &MapSecrets::with(&env("KWATI_SMB_TEST_PASSWORD")),
    )
    .err()
    .expect("an SMB1-only server must be refused");
    assert_eq!(kind(err), ErrorKind::Unsupported);
}
