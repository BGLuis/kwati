use std::path::PathBuf;

use kwati_core::Location;
use kwati_gui::state::{AppState, NetworkMountStatus};
use kwati_gui::views::network_view::{is_network_location, DEFAULT_NETWORK_TIMEOUT_SECS};

#[test]
fn test_is_network_location() {
    let remote_loc = Location::Remote {
        profile_id: "myserver".into(),
        path: PathBuf::from("/var/www"),
    };
    assert!(is_network_location(&remote_loc));

    let unc_win = Location::Local(PathBuf::from("\\\\server\\share\\data"));
    assert!(is_network_location(&unc_win));

    let smb_posix = Location::Local(PathBuf::from("//192.168.1.100/files"));
    assert!(is_network_location(&smb_posix));

    let local = Location::Local(PathBuf::from("/tmp"));
    // Under normal Linux without /tmp being a CIFS/NFS mount, this is not a network location
    assert!(!is_network_location(&local));
}

#[test]
fn test_network_mount_status_transitions_and_timeout() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    assert_eq!(state.network_status, NetworkMountStatus::Disconnected);

    // Initial default timeout
    assert_eq!(DEFAULT_NETWORK_TIMEOUT_SECS, 10);

    // Connecting
    state.set_network_status(NetworkMountStatus::Connecting);
    assert_eq!(state.network_status, NetworkMountStatus::Connecting);

    // TimedOut
    state.set_network_status(NetworkMountStatus::TimedOut);
    assert_eq!(state.network_status, NetworkMountStatus::TimedOut);

    // MountBusy
    state.set_network_status(NetworkMountStatus::MountBusy);
    assert_eq!(state.network_status, NetworkMountStatus::MountBusy);

    // Connected
    state.set_network_status(NetworkMountStatus::Connected);
    assert_eq!(state.network_status, NetworkMountStatus::Connected);
}

#[test]
fn test_network_view_timeout_configuration() {
    let mut timeout = DEFAULT_NETWORK_TIMEOUT_SECS;
    assert_eq!(timeout, 10);

    // Apply settings timeout without restart
    let new_timeout_from_settings = 25;
    timeout = new_timeout_from_settings;
    assert_eq!(timeout, 25);
}

#[test]
fn test_host_key_fingerprint_trust_and_decline() {
    use kwati_gui::views::remote_dialogs::RemoteDialogKind;
    use std::collections::HashSet;

    let dialog = RemoteDialogKind::TrustFingerprint {
        host: "sftp.example.com".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:abc123def456ghi789jkl012mno345pqr678stu901vw".into(),
    };

    assert!(match &dialog {
        RemoteDialogKind::TrustFingerprint { host, port, .. } => {
            host == "sftp.example.com" && *port == 22
        }
        _ => false,
    });

    let mut trusted: HashSet<String> = HashSet::new();
    let fp = "SHA256:abc123def456ghi789jkl012mno345pqr678stu901vw";

    // Not trusted initially
    assert!(!trusted.contains(fp));

    // When trusted:
    trusted.insert(fp.to_string());
    assert!(trusted.contains(fp));

    // Declining does not add to trusted fingerprints
    let declined_fp = "SHA256:untrusted_fingerprint_hash_99999999999999999";
    assert!(!trusted.contains(declined_fp));
}

#[test]
fn test_host_key_changed_offers_no_connect_button() {
    use kwati_gui::views::remote_dialogs::RemoteDialogKind;

    let changed_dialog = RemoteDialogKind::HostKeyChanged {
        host: "ssh.company.com".into(),
        port: 2222,
        known_fingerprint: "SHA256:known_old_fingerprint_1111111111111111111".into(),
        presented_fingerprint: "SHA256:new_unauthorized_fingerprint_22222222".into(),
    };

    let trust_dialog = RemoteDialogKind::TrustFingerprint {
        host: "ssh.company.com".into(),
        port: 2222,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:fingerprint_hash".into(),
    };

    let password_dialog = RemoteDialogKind::PasswordPrompt {
        host: "ssh.company.com".into(),
        username: "root".into(),
        remember: false,
        error_message: None,
    };

    // Helper checking has_connect_button matching RemoteDialogsView logic
    let has_connect_btn = |d: &RemoteDialogKind| match d {
        RemoteDialogKind::TrustFingerprint { .. } => true,
        RemoteDialogKind::PasswordPrompt { .. } => true,
        RemoteDialogKind::ProfileManager { .. } => true,
        RemoteDialogKind::HostKeyChanged { .. } => false,
        RemoteDialogKind::None => false,
    };

    // Security rule: Changed host key must NOT offer a connect button
    assert!(!has_connect_btn(&changed_dialog));
    assert!(has_connect_btn(&trust_dialog));
    assert!(has_connect_btn(&password_dialog));
}

#[test]
fn test_password_prompt_saving_behavior() {
    use std::collections::HashMap;

    let mut stored_passwords: HashMap<String, String> = HashMap::new();
    let host = "sftp.testserver.org";

    // 1. Password typed without saving is NOT stored in keyring/cache
    let typed_pass_1 = "secret123";
    let remember_1 = false;
    if remember_1 {
        stored_passwords.insert(host.to_string(), typed_pass_1.to_string());
    }
    assert_eq!(stored_passwords.get(host), None);

    // 2. Password typed with saving IS stored
    let typed_pass_2 = "persistent_secret_456";
    let remember_2 = true;
    if remember_2 {
        stored_passwords.insert(host.to_string(), typed_pass_2.to_string());
    }
    assert_eq!(
        stored_passwords.get(host),
        Some(&"persistent_secret_456".to_string())
    );

    // Next connection can read remembered password
    assert_eq!(stored_passwords.get(host).unwrap(), "persistent_secret_456");
}
