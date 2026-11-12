use keyring_core::api::CredentialStoreApi;
use keyring_core::mock::{Cred, Store};
use keyring_core::Error;
use kwati_core::{
    AuthMethod, ConnectionProfile, ConnectionProfiles, KeyringSecrets, Protocol, Secrets,
    SecretsError,
};
use std::fs;
use std::path::PathBuf;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("kwati_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn profile(id: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.into(),
        name: "Backup box".into(),
        protocol: Protocol::Sftp,
        host: "backup.example.com".into(),
        port: 22,
        user: "luis".into(),
        start_path: "/".into(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    }
}

fn in_memory() -> KeyringSecrets {
    KeyringSecrets::new(Store::new().expect("mock store"))
}

#[test]
fn secrets_round_trip() {
    let secrets = in_memory();

    assert_eq!(secrets.get("a").unwrap(), None);
    secrets.set("a", "hunter2").unwrap();
    assert_eq!(secrets.get("a").unwrap().as_deref(), Some("hunter2"));

    secrets.set("a", "correct horse").unwrap();
    assert_eq!(secrets.get("a").unwrap().as_deref(), Some("correct horse"));

    secrets.delete("a").unwrap();
    assert_eq!(secrets.get("a").unwrap(), None);
    secrets.delete("a").unwrap();
}

#[test]
fn delete_profile_removes_secret() {
    let dir = TestDir::new("secrets_delete");
    let path = dir.path.join("connections.toml");
    let secrets = in_memory();
    let mut profiles = ConnectionProfiles::default();
    for (id, secret) in [("a", "secret-a"), ("b", "secret-b")] {
        let pending = profiles
            .save_profile(&path, profile(id), Some(secret.into()))
            .unwrap()
            .unwrap();
        pending.store(&secrets).unwrap();
    }

    profiles.remove_profile(&path, "a", &secrets).unwrap();

    assert_eq!(secrets.get("a").unwrap(), None);
    assert_eq!(secrets.get("b").unwrap().as_deref(), Some("secret-b"));
    let reloaded = ConnectionProfiles::load_from(&path).unwrap();
    assert_eq!(reloaded.profiles, vec![profile("b")]);
}

#[test]
fn secrets_no_file_fallback() {
    let dir = TestDir::new("secrets_no_fallback");
    let path = dir.path.join("connections.toml");
    let store = Store::new().expect("mock store");
    let secrets = KeyringSecrets::new(store.clone());
    store
        .build("kwati", "a", None)
        .unwrap()
        .as_any()
        .downcast_ref::<Cred>()
        .expect("mock credential")
        .set_error(Error::NoStorageAccess("no secret service".into()));

    let mut profiles = ConnectionProfiles::default();
    let pending = profiles
        .save_profile(&path, profile("a"), Some("s3cret-passw0rd".into()))
        .unwrap()
        .unwrap();
    let err = pending.store(&secrets).unwrap_err();

    assert!(matches!(err, SecretsError::Unavailable(_)), "{err:?}");
    assert_eq!(secrets.get("a").unwrap(), None);
    for entry in fs::read_dir(&dir.path).unwrap() {
        let text = fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&text).contains("s3cret-passw0rd"));
    }
}
