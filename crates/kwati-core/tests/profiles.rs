use kwati_core::{AuthMethod, ConnectionProfile, ConnectionProfiles, Protocol};
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

fn profile(id: &str, auth: AuthMethod) -> ConnectionProfile {
    ConnectionProfile {
        id: id.into(),
        name: "Backup box".into(),
        protocol: Protocol::Sftp,
        host: "backup.example.com".into(),
        port: 2222,
        user: "luis".into(),
        start_path: "/srv/data".into(),
        auth,
        private_key_path: Some(PathBuf::from("/home/luis/.ssh/id_ed25519")),
        allow_insecure: true,
    }
}

#[test]
fn profiles_round_trip() {
    let dir = TestDir::new("profiles_round_trip");
    let path = dir.path.join("connections.toml");
    let profiles = ConnectionProfiles {
        profiles: vec![
            profile("a", AuthMethod::PrivateKey),
            ConnectionProfile {
                protocol: Protocol::Smb,
                private_key_path: None,
                allow_insecure: false,
                ..profile("b", AuthMethod::Password)
            },
        ],
    };

    profiles.save_to(&path).unwrap();
    assert_eq!(ConnectionProfiles::load_from(&path).unwrap(), profiles);
}

#[test]
fn profiles_hold_no_secret() {
    let dir = TestDir::new("profiles_hold_no_secret");
    let path = dir.path.join("connections.toml");
    let mut profiles = ConnectionProfiles::default();

    let pending = profiles
        .save_profile(
            &path,
            profile("a", AuthMethod::Password),
            Some("hunter2-passphrase".into()),
        )
        .unwrap()
        .expect("the secret is handed back for the secret store");
    assert_eq!(pending.profile_id, "a");
    assert_eq!(pending.secret, "hunter2-passphrase");

    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("hunter2"));
    assert_eq!(ConnectionProfiles::load_from(&path).unwrap(), profiles);
}

#[test]
fn profiles_corrupt_kept() {
    let dir = TestDir::new("profiles_corrupt_kept");
    let path = dir.path.join("connections.toml");
    let corrupt = "[[profiles]\nid = ";
    fs::write(&path, corrupt).unwrap();

    assert!(ConnectionProfiles::load_from(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
}
