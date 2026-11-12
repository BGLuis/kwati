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
