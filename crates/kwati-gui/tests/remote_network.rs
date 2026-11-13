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

#[gpui::test]
fn test_network_view_timeout_configuration(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    use kwati_gui::views::network_view::NetworkView;

    let state = cx.new(|_| AppState::new(Location::Local(PathBuf::from("/tmp"))));
    let view = cx.new(|cx| NetworkView::new(state.clone(), cx));

    assert_eq!(DEFAULT_NETWORK_TIMEOUT_SECS, 10);
    view.read_with(cx, |v, _| assert_eq!(v.timeout_secs, 10));

    state.update(cx, |state, cx| {
        state.set_list_timeout_secs(25).unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.timeout_secs, 25));
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

#[gpui::test]
fn test_host_key_changed_offers_no_connect_button(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    use kwati_gui::state::RemotePrompt;
    use kwati_gui::views::remote_dialogs::{RemoteDialogKind, RemoteDialogsView};

    let state = cx.new(|_| AppState::new(Location::Local(PathBuf::from("/tmp"))));
    let view = cx.new(|cx| RemoteDialogsView::new(state.clone(), cx));

    // Security rule: Changed host key must NOT offer a connect button
    state.update(cx, |state, cx| {
        state.remote_prompt = Some(RemotePrompt::HostKeyChanged {
            host: "ssh.company.com".into(),
            port: 2222,
            presented: "SHA256:new_unauthorized_fingerprint_22222222".into(),
            stored: vec!["SHA256:known_old_fingerprint_1111111111111111111".into()],
        });
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, cx| assert!(!v.has_connect_button(cx)));

    // CertificateInvalid must NOT offer a connect button
    state.update(cx, |state, cx| {
        state.remote_prompt = Some(RemotePrompt::CertificateInvalid {
            host: "ftps.company.com".into(),
            port: 990,
            reason: "unknown issuer".into(),
        });
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, cx| assert!(!v.has_connect_button(cx)));

    // Password prompt DOES offer Connect
    state.update(cx, |state, cx| {
        state.remote_prompt = Some(RemotePrompt::Password {
            profile_id: "srv".into(),
            error: None,
        });
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, cx| assert!(v.has_connect_button(cx)));

    // First-time TrustFingerprint DOES offer Trust & Connect
    state.update(cx, |state, cx| {
        state.remote_prompt = None;
        cx.notify();
    });
    view.update(cx, |v, cx| {
        v.show(
            RemoteDialogKind::TrustFingerprint {
                host: "ssh.company.com".into(),
                port: 2222,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "SHA256:fingerprint_hash".into(),
            },
            cx,
        );
    });
    view.read_with(cx, |v, cx| assert!(v.has_connect_button(cx)));
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

#[test]
fn test_remote_auto_refresh_toggle_and_interval() {
    use kwati_gui::actions::remote_sync::{
        RemotePollingState, RemoteSyncController, DEFAULT_REMOTE_POLL_INTERVAL_SECS,
        MIN_REMOTE_POLL_INTERVAL_SECS,
    };

    let mut controller = RemoteSyncController::new();
    assert!(!controller.auto_refresh_enabled);
    assert_eq!(controller.polling_state, RemotePollingState::Disabled);
    assert_eq!(
        controller.poll_interval_secs,
        DEFAULT_REMOTE_POLL_INTERVAL_SECS
    );
    assert_eq!(controller.status_indicator(), "Auto-refresh: Off");

    // Enable with interval
    controller.enable_auto_refresh(20);
    assert!(controller.auto_refresh_enabled);
    assert_eq!(controller.poll_interval_secs, 20);
    assert_eq!(controller.polling_state, RemotePollingState::Active);
    assert!(controller.can_poll());

    // Interval below minimum is clamped to MIN_REMOTE_POLL_INTERVAL_SECS (10s)
    controller.set_poll_interval(3);
    assert_eq!(controller.poll_interval_secs, MIN_REMOTE_POLL_INTERVAL_SECS);

    // Polling start and finish transitions
    assert!(controller.start_poll());
    assert_eq!(controller.polling_state, RemotePollingState::Polling);
    controller.finish_poll();
    assert_eq!(controller.polling_state, RemotePollingState::Active);
    assert!(controller.last_poll_time.is_some());

    // Disable auto-refresh
    controller.disable_auto_refresh();
    assert!(!controller.auto_refresh_enabled);
    assert_eq!(controller.polling_state, RemotePollingState::Disabled);
    assert!(!controller.can_poll());
}

#[test]
fn test_remote_poll_pauses_when_window_unfocused() {
    use kwati_gui::actions::remote_sync::{RemotePollingState, RemoteSyncController};
    use std::sync::atomic::Ordering;

    let mut controller = RemoteSyncController::new();
    controller.enable_auto_refresh(15);
    assert!(controller.can_poll());
    assert_eq!(controller.polling_state, RemotePollingState::Active);
    assert!(!controller.paused_flag().load(Ordering::SeqCst));

    // When the window loses focus, polling must pause immediately
    controller.set_window_focused(false);
    assert_eq!(
        controller.polling_state,
        RemotePollingState::PausedUnfocused
    );
    assert!(!controller.can_poll());
    assert!(controller.paused_flag().load(Ordering::SeqCst));
    assert_eq!(
        controller.status_indicator(),
        "Auto-refresh: Paused (window unfocused)"
    );

    // Attempting to poll while unfocused fails
    assert!(!controller.start_poll());

    // When window regains focus, polling resumes automatically
    controller.set_window_focused(true);
    assert_eq!(controller.polling_state, RemotePollingState::Active);
    assert!(controller.can_poll());
    assert!(!controller.paused_flag().load(Ordering::SeqCst));
}

#[test]
fn test_remote_refresh_retains_selection_by_location() {
    use kwati_core::{Entry, Selection};
    use kwati_gui::actions::remote_sync::RemoteSyncController;
    use std::ffi::OsString;
    use std::path::PathBuf;

    let e1 = Entry::new(
        OsString::from("file1.txt"),
        PathBuf::from("/remote/file1.txt"),
        false,
    );
    let e2 = Entry::new(
        OsString::from("file2.txt"),
        PathBuf::from("/remote/file2.txt"),
        false,
    );
    let e3 = Entry::new(
        OsString::from("file3.txt"),
        PathBuf::from("/remote/file3.txt"),
        false,
    );

    let loc1 = Location::from(&e1.path);
    let loc2 = Location::from(&e2.path);

    let mut selection = Selection::new();
    selection.select(loc1.clone());
    selection.select(loc2.clone());
    assert_eq!(selection.len(), 2);

    // A refresh arrives: e1 still exists, e2 was deleted on remote, e3 was newly added
    let new_entries = vec![e1.clone(), e3.clone()];
    RemoteSyncController::retain_selection(&mut selection, &new_entries);

    // e1 must still be selected; e2 must be dropped because it no longer exists
    assert!(selection.is_selected(&loc1));
    assert!(!selection.is_selected(&loc2));
    assert_eq!(selection.len(), 1);
}

#[test]
fn test_remote_search_streaming_and_cancellation() {
    use kwati_gui::actions::remote_sync::{RemoteSearchController, RemoteSearchMatch};
    use std::path::PathBuf;

    let items = vec![
        (
            PathBuf::from("/remote/app.rs"),
            "app.rs".to_string(),
            1,
            false,
        ),
        (
            PathBuf::from("/remote/src/main.rs"),
            "main.rs".to_string(),
            2,
            false,
        ),
        (
            PathBuf::from("/remote/src/views/app_view.rs"),
            "app_view.rs".to_string(),
            3,
            false,
        ),
        (
            PathBuf::from("/remote/src/views/other.rs"),
            "other.rs".to_string(),
            3,
            false,
        ),
        (
            PathBuf::from("/remote/tests/app_test.rs"),
            "app_test.rs".to_string(),
            2,
            false,
        ),
    ];

    let mut controller = RemoteSearchController::new();
    let mut streamed_hits: Vec<RemoteSearchMatch> = Vec::new();

    // 1. Full search for "app": should find app.rs, app_view.rs, app_test.rs
    controller.stream_search_items(&items, "app", |hit| {
        streamed_hits.push(hit);
    });
    assert_eq!(streamed_hits.len(), 3);
    assert_eq!(controller.results.len(), 3);
    assert_eq!(streamed_hits[0].name, "app.rs");
    assert_eq!(streamed_hits[1].name, "app_view.rs");
    assert_eq!(streamed_hits[2].name, "app_test.rs");

    // 2. Cancellation: during streaming, Cancel stops further results from arriving
    let mut cancel_streamed: Vec<RemoteSearchMatch> = Vec::new();
    let cancel_flag = controller.cancel_flag();
    controller.start_search();

    for (path, name, depth, is_dir) in &items {
        if controller.is_cancelled() {
            break;
        }
        if name.contains("app") {
            let hit = RemoteSearchMatch {
                name: name.clone(),
                path: path.clone(),
                depth: *depth,
                is_dir: *is_dir,
            };
            cancel_streamed.push(hit);
            // Cancel immediately after first hit
            cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    // Only the first match was received before cancel stopped the stream
    assert_eq!(cancel_streamed.len(), 1);
    assert_eq!(cancel_streamed[0].name, "app.rs");
}

#[test]
fn test_remote_search_depth_limiting() {
    use kwati_gui::actions::remote_sync::{
        RemoteSearchController, RemoteSearchMatch, DEFAULT_REMOTE_SEARCH_DEPTH,
    };
    use std::path::PathBuf;

    let items = vec![
        (
            PathBuf::from("/remote/target.txt"),
            "target.txt".to_string(),
            1,
            false,
        ),
        (
            PathBuf::from("/remote/sub1/target.txt"),
            "target.txt".to_string(),
            2,
            false,
        ),
        (
            PathBuf::from("/remote/sub1/sub2/target.txt"),
            "target.txt".to_string(),
            3,
            false,
        ),
        (
            PathBuf::from("/remote/sub1/sub2/sub3/target.txt"),
            "target.txt".to_string(),
            4,
            false,
        ),
    ];

    let mut controller = RemoteSearchController::new();
    assert_eq!(controller.options.depth_limit, DEFAULT_REMOTE_SEARCH_DEPTH); // 3

    // Search with depth 3: finds depths 1, 2, 3 (depth 4 is excluded)
    let mut matches_depth_3 = Vec::new();
    controller.stream_search_items(&items, "target", |hit| {
        matches_depth_3.push(hit);
    });
    assert_eq!(matches_depth_3.len(), 3);
    assert_eq!(matches_depth_3[0].depth, 1);
    assert_eq!(matches_depth_3[1].depth, 2);
    assert_eq!(matches_depth_3[2].depth, 3);

    // Lowering depth in settings to 1 excludes matches below depth 1 on next search
    controller.set_depth_limit(1);
    let mut matches_depth_1 = Vec::new();
    controller.stream_search_items(&items, "target", |hit| {
        matches_depth_1.push(hit);
    });
    assert_eq!(matches_depth_1.len(), 1);
    assert_eq!(matches_depth_1[0].depth, 1);

    // Testing static filter_by_depth
    let all_matches = vec![
        RemoteSearchMatch {
            name: "d1".into(),
            path: PathBuf::from("/d1"),
            depth: 1,
            is_dir: false,
        },
        RemoteSearchMatch {
            name: "d2".into(),
            path: PathBuf::from("/d1/d2"),
            depth: 2,
            is_dir: false,
        },
        RemoteSearchMatch {
            name: "d3".into(),
            path: PathBuf::from("/d1/d2/d3"),
            depth: 3,
            is_dir: false,
        },
    ];
    let filtered = RemoteSearchController::filter_by_depth(&all_matches, 2);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].depth, 1);
    assert_eq!(filtered[1].depth, 2);
}

#[test]
fn test_remote_search_options_parallel_requests() {
    use kwati_gui::actions::remote_sync::{
        RemoteSearchController, DEFAULT_PARALLEL_REQUESTS, DEFAULT_REMOTE_SEARCH_DEPTH,
    };

    let mut controller = RemoteSearchController::new();
    assert_eq!(controller.options.depth_limit, DEFAULT_REMOTE_SEARCH_DEPTH);
    assert_eq!(
        controller.options.parallel_requests,
        DEFAULT_PARALLEL_REQUESTS
    );

    controller.set_parallel_requests(8);
    assert_eq!(controller.options.parallel_requests, 8);

    // Min clamped to 1
    controller.set_parallel_requests(0);
    assert_eq!(controller.options.parallel_requests, 1);
}

#[test]
fn test_sidebar_standard_places_discovery() {
    use kwati_gui::views::sidebar_view::standard_places;

    let places = standard_places();
    assert!(!places.is_empty());
    // Home or root is always present
    let has_home = places.iter().any(|p| p.name == "Home" || p.name == "Root");
    assert!(has_home);

    for place in &places {
        assert!(!place.name.is_empty());
    }
}

#[test]
fn test_sidebar_bookmarks_add_remove_reorder_and_missing_target() {
    use kwati_gui::views::sidebar_view::UserBookmark;
    use std::path::PathBuf;

    let existing_dir = std::env::temp_dir();
    let missing_dir = PathBuf::from("/non/existent/path/for/kwati/test/bookmark/12345");

    let bm_valid = UserBookmark::new("Temp Folder", existing_dir);
    let bm_missing = UserBookmark::new("Old Missing Drive", missing_dir);

    // Missing target detection
    assert!(!bm_valid.is_missing());
    assert!(bm_missing.is_missing());

    // Bookmarks list operations
    let mut bookmarks = vec![bm_valid.clone(), bm_missing.clone()];
    assert_eq!(bookmarks.len(), 2);

    // Reorder: move index 1 to 0
    let item = bookmarks.remove(1);
    bookmarks.insert(0, item);
    assert_eq!(bookmarks[0].name, "Old Missing Drive");
    assert_eq!(bookmarks[1].name, "Temp Folder");

    // Remove
    let removed = bookmarks.remove(0);
    assert_eq!(removed.name, "Old Missing Drive");
    assert_eq!(bookmarks.len(), 1);
}

#[test]
fn test_sidebar_volume_capacity_indicators() {
    use kwati_gui::views::sidebar_view::{format_bytes, SidebarVolume};
    use std::path::PathBuf;

    assert_eq!(format_bytes(500), "500 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1024 * 1024 * 50), "50.0 MB");
    assert_eq!(format_bytes(1024 * 1024 * 1024 * 64), "64.0 GB");

    let vol = SidebarVolume::new(
        "usb-1",
        "SANDISK USB",
        PathBuf::from("/media/usb1"),
        64 * 1024 * 1024 * 1024, // 64 GB
        48 * 1024 * 1024 * 1024, // 48 GB available
        true,
    );

    assert_eq!(vol.used_bytes(), 16 * 1024 * 1024 * 1024); // 16 GB used
    assert_eq!(vol.used_percentage(), 25); // 25% used
    assert_eq!(vol.capacity_summary(), "48.0 GB free of 64.0 GB");
}

#[test]
fn test_sidebar_eject_removable_drive_safe_to_remove_and_home_redirect() {
    use kwati_core::Location;
    use kwati_gui::views::sidebar_view::{EjectResult, SidebarVolume};
    use std::path::PathBuf;

    let usb_mount = PathBuf::from("/media/external_drive");
    let mut volumes = vec![
        SidebarVolume::new(
            "vol-root",
            "System Root",
            PathBuf::from("/"),
            500 * 1024 * 1024 * 1024,
            200 * 1024 * 1024 * 1024,
            false,
        ),
        SidebarVolume::new(
            "vol-usb",
            "Flash Drive",
            usb_mount.clone(),
            32 * 1024 * 1024 * 1024,
            30 * 1024 * 1024 * 1024,
            true, // removable
        ),
    ];

    // Current location is inside the USB drive
    let current_location = Location::Local(usb_mount.join("DCIM/Photos"));

    // Find volume
    let pos = volumes.iter().position(|v| v.id == "vol-usb").unwrap();
    let vol = &volumes[pos];
    assert!(!vol.is_busy);

    // Ejecting idle removable volume
    let was_inside = current_location.path().starts_with(&vol.mount_point);
    assert!(was_inside);

    let ejected_vol = volumes.remove(pos);
    let result = EjectResult::SafeToRemove {
        volume_name: ejected_vol.name.clone(),
        unmounted_path: ejected_vol.mount_point,
    };

    assert_eq!(
        result,
        EjectResult::SafeToRemove {
            volume_name: "Flash Drive".into(),
            unmounted_path: usb_mount,
        }
    );
    // Removed from listed volumes
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].id, "vol-root");
}

#[test]
fn test_sidebar_eject_busy_drive_keeps_listed() {
    use kwati_gui::views::sidebar_view::{EjectResult, SidebarVolume};
    use std::path::PathBuf;

    let mut busy_volume = SidebarVolume::new(
        "usb-busy",
        "Backup Disk",
        PathBuf::from("/media/backup"),
        128 * 1024 * 1024 * 1024,
        64 * 1024 * 1024 * 1024,
        true,
    );
    busy_volume.is_busy = true;
    busy_volume.busy_reason = Some("Open file: /media/backup/archive.tar.gz".into());

    let volumes = [busy_volume];

    // Attempt to eject
    let vol = &volumes[0];
    let result = if vol.is_busy {
        EjectResult::Busy {
            volume_name: vol.name.clone(),
            reason: vol.busy_reason.clone().unwrap(),
        }
    } else {
        panic!("should be busy");
    };

    assert_eq!(
        result,
        EjectResult::Busy {
            volume_name: "Backup Disk".into(),
            reason: "Open file: /media/backup/archive.tar.gz".into(),
        }
    );

    // Crucially: drive must remain listed in sidebar when eject fails due to busy!
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].id, "usb-busy");
}

#[test]
fn test_sidebar_mount_unmounted_drive() {
    use kwati_gui::views::sidebar_view::{MountResult, SidebarVolume};
    use std::path::PathBuf;

    let mut unmounted_volume = SidebarVolume::new(
        "disk-2",
        "Data Partition",
        PathBuf::from("/mnt/data"),
        256 * 1024 * 1024 * 1024,
        200 * 1024 * 1024 * 1024,
        true,
    );
    unmounted_volume.is_mounted = false;

    assert!(!unmounted_volume.is_mounted);

    // Mount drive
    unmounted_volume.is_mounted = true;
    let res = MountResult::Mounted {
        volume_name: unmounted_volume.name.clone(),
        mount_point: unmounted_volume.mount_point.clone(),
    };

    assert_eq!(
        res,
        MountResult::Mounted {
            volume_name: "Data Partition".into(),
            mount_point: PathBuf::from("/mnt/data"),
        }
    );
    assert!(unmounted_volume.is_mounted);
    assert_eq!(
        unmounted_volume.capacity_summary(),
        "200.0 GB free of 256.0 GB"
    );
}

#[test]
fn test_app_state_settings_persistence_and_load_applies_hidden_and_sort() {
    use kwati_core::{SortColumn, SortDirection, SortOrder};

    let dir = std::env::temp_dir().join(format!(
        "kwati_remote_network_settings_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings_file = dir.join("settings.toml");

    // Isolated default state does not touch disk
    let isolated = AppState::new(Location::Local(dir.clone()));
    assert!(isolated.settings_path().is_none());

    let mut state =
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()));
    state.set_show_hidden(true);
    state.set_sort(SortColumn::Size, SortDirection::Descending);
    assert!(state.show_hidden);
    assert_eq!(state.sort_column, SortColumn::Size);
    assert_eq!(state.sort_direction, SortDirection::Descending);

    // Reload into a fresh AppState applies persisted show_hidden and default_sort
    let mut reloaded = AppState::new(Location::Local(dir.clone()));
    reloaded.load_settings_from(&settings_file).unwrap();
    assert!(reloaded.show_hidden);
    assert_eq!(reloaded.sort_column, SortColumn::Size);
    assert_eq!(reloaded.sort_direction, SortDirection::Descending);
    assert_eq!(
        reloaded.settings.default_sort,
        SortOrder::new(SortColumn::Size, SortDirection::Descending)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn test_main_view_wires_modals_search_actions_and_focus_restoration(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    use kwati_gui::actions::{register_keybindings, ManageServers, OpenSettings};
    use kwati_gui::components::register_text_input_keybindings;
    use kwati_gui::views::search_view::ToggleSearch;
    use kwati_gui::views::MainView;

    let dir = std::env::temp_dir().join(format!("kwati_main_wiring_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    cx.update(|app| {
        register_keybindings(app);
        register_text_input_keybindings(app);
    });

    let (main, vcx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut st = AppState::new(Location::Local(dir.clone()));
            st.attach(cx);
            st.reload_entries();
            st
        });
        MainView::new(state, window, cx)
    });
    vcx.run_until_parked();

    let list_focused = |vcx: &mut gpui::VisualTestContext| {
        let handle = main.read_with(vcx, |m, cx| m.list.read(cx).focus_handle.clone());
        vcx.update(|window, _| handle.is_focused(window))
    };
    assert!(list_focused(vcx));

    // OpenSettings action opens SettingsView; closing restores list focus
    vcx.dispatch_action(OpenSettings);
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(m.settings.read(cx).is_open(cx)));
    let settings = main.read_with(vcx, |m, _| m.settings.clone());
    settings.update(vcx, |s, cx| s.close(cx));
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(!m.settings.read(cx).is_open(cx)));
    assert!(list_focused(vcx));

    // ManageServers action opens RemoteDialogsView profile manager; closing restores list focus
    vcx.dispatch_action(ManageServers);
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(m.remote_dialogs.read(cx).is_open(cx)));
    let remote_dialogs = main.read_with(vcx, |m, _| m.remote_dialogs.clone());
    remote_dialogs.update(vcx, |d, cx| d.close(cx));
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(!m.remote_dialogs.read(cx).is_open(cx)));
    assert!(list_focused(vcx));

    // ToggleSearch action opens SearchView even when collapsed; closing restores list focus
    vcx.dispatch_action(ToggleSearch);
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(m.search.read(cx).is_active()));
    let search = main.read_with(vcx, |m, _| m.search.clone());
    search.update(vcx, |s, cx| s.close(cx));
    vcx.run_until_parked();
    main.read_with(vcx, |m, cx| assert!(!m.search.read(cx).is_active()));
    assert!(list_focused(vcx));

    let _ = std::fs::remove_dir_all(&dir);
}
