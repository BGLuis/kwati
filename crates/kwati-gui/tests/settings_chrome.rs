use std::path::PathBuf;

use gpui::{AppContext as _, TestAppContext};
use kwati_core::{
    AuthMethod, ConnectionProfile, Location, Protocol, Settings, SortColumn, SortDirection,
    SortOrder,
};
use kwati_gui::state::AppState;
use kwati_gui::views::header_view::{
    is_network_mount_badge_visible, remote_header_info, HeaderView, RemoteBadgeState,
};
use kwati_gui::views::network_view::NetworkView;
use kwati_gui::views::settings_view::{SettingKey, SettingsView};
use kwati_gui::views::sidebar_view::{server_items, ServerConnectionStatus, SidebarView};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kwati_gui_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[gpui::test]
fn settings_view_applies_and_persists_changes(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_persist");
    let settings_file = dir.join("settings.toml");
    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    view.update(cx, |view, cx| {
        view.open(cx);
        assert!(view.is_open(cx));
        view.toggle_bool_setting(SettingKey::ShowHidden, cx)
            .unwrap();
        view.set_sort_column(SortColumn::Size, cx).unwrap();
        view.set_sort_direction(SortDirection::Descending, cx)
            .unwrap();
        view.toggle_bool_setting(SettingKey::RemotePollEnabled, cx)
            .unwrap();
        view.set_numeric_input(SettingKey::ListTimeoutSecs, "22", cx)
            .unwrap();
        view.set_numeric_input(SettingKey::RefreshIntervalSecs, "12", cx)
            .unwrap();
        view.set_numeric_input(SettingKey::RemotePollIntervalSecs, "45", cx)
            .unwrap();
        view.set_numeric_input(SettingKey::RemoteSearchDepth, "5", cx)
            .unwrap();
        view.set_numeric_input(SettingKey::RemoteSearchParallel, "4", cx)
            .unwrap();
    });

    state.read_with(cx, |state, _| {
        assert!(state.show_hidden);
        assert_eq!(state.sort_column, SortColumn::Size);
        assert_eq!(state.sort_direction, SortDirection::Descending);
        assert!(state.settings.show_hidden);
        assert_eq!(
            state.settings.default_sort,
            SortOrder::new(SortColumn::Size, SortDirection::Descending)
        );
        assert!(state.settings.remote_poll_enabled);
        assert_eq!(state.settings.list_timeout_secs, 22);
        assert_eq!(state.settings.refresh_interval_secs, 12);
        assert_eq!(state.settings.remote_poll_interval_secs, 45);
        assert_eq!(state.settings.remote_search_depth, 5);
        assert_eq!(state.settings.remote_search_parallel, 4);
    });

    let (disk, warnings) = Settings::load_from(&settings_file).unwrap();
    assert!(warnings.is_empty());
    assert!(disk.show_hidden);
    assert_eq!(
        disk.default_sort,
        SortOrder::new(SortColumn::Size, SortDirection::Descending)
    );
    assert!(disk.remote_poll_enabled);
    assert_eq!(disk.list_timeout_secs, 22);
    assert_eq!(disk.refresh_interval_secs, 12);
    assert_eq!(disk.remote_poll_interval_secs, 45);
    assert_eq!(disk.remote_search_depth, 5);
    assert_eq!(disk.remote_search_parallel, 4);

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn settings_view_rejects_out_of_range_inline_and_preserves_saved(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_reject");
    let settings_file = dir.join("settings.toml");
    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    view.update(cx, |view, cx| {
        assert!(view
            .set_numeric_input(SettingKey::ListTimeoutSecs, "0", cx)
            .is_err());
        assert!(view.field_error(SettingKey::ListTimeoutSecs).is_some());

        assert!(view
            .set_numeric_input(SettingKey::RefreshIntervalSecs, "abc", cx)
            .is_err());
        assert!(view.field_error(SettingKey::RefreshIntervalSecs).is_some());

        // Minimum remote poll interval is 10s
        assert!(view
            .set_numeric_input(SettingKey::RemotePollIntervalSecs, "5", cx)
            .is_err());
        assert!(view
            .field_error(SettingKey::RemotePollIntervalSecs)
            .is_some());

        assert!(view
            .set_numeric_input(SettingKey::RemoteSearchDepth, "0", cx)
            .is_err());
        assert!(view.field_error(SettingKey::RemoteSearchDepth).is_some());

        assert!(view
            .set_numeric_input(SettingKey::RemoteSearchParallel, "99", cx)
            .is_err());
        assert!(view.field_error(SettingKey::RemoteSearchParallel).is_some());
    });

    // Saved defaults must remain untouched.
    state.read_with(cx, |state, _| {
        assert_eq!(state.settings, Settings::default());
    });

    // Providing a valid value clears the inline error.
    view.update(cx, |view, cx| {
        view.set_numeric_input(SettingKey::RemotePollIntervalSecs, "15", cx)
            .unwrap();
        assert!(view
            .field_error(SettingKey::RemotePollIntervalSecs)
            .is_none());
    });
    state.read_with(cx, |state, _| {
        assert_eq!(state.settings.remote_poll_interval_secs, 15);
    });

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn settings_view_reset_to_defaults_restores_all_and_persists(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_reset");
    let settings_file = dir.join("settings.toml");
    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    view.update(cx, |view, cx| {
        view.toggle_bool_setting(SettingKey::ShowHidden, cx)
            .unwrap();
        view.set_sort_column(SortColumn::Date, cx).unwrap();
        view.set_numeric_input(SettingKey::ListTimeoutSecs, "30", cx)
            .unwrap();
        let _ = view.set_numeric_input(SettingKey::RemoteSearchDepth, "0", cx);
        assert!(view.field_error(SettingKey::RemoteSearchDepth).is_some());

        view.reset_to_defaults(cx).unwrap();
        assert!(view.field_error(SettingKey::RemoteSearchDepth).is_none());
        let timeout_text = view
            .input_entity(SettingKey::ListTimeoutSecs)
            .unwrap()
            .read(cx)
            .text()
            .to_string();
        assert_eq!(timeout_text, "10");
    });

    state.read_with(cx, |state, _| {
        assert!(!state.show_hidden);
        assert_eq!(state.sort_column, SortColumn::Name);
        assert_eq!(state.sort_direction, SortDirection::Ascending);
        assert_eq!(state.settings, Settings::default());
    });

    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert_eq!(disk, Settings::default());

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn settings_view_escape_closes_modal(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_escape");
    let state = cx.new(|_| AppState::new(Location::Local(dir.clone())));
    let (view, cx) = cx.add_window_view(|_, cx| SettingsView::new(state.clone(), cx));

    view.update_in(cx, |view, window, cx| {
        view.open(cx);
        view.focus(window, cx);
        assert!(view.is_open(cx));
    });

    cx.simulate_keystrokes("escape");

    state.read_with(cx, |state, _| {
        assert!(!state.settings_open);
    });

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn sidebar_servers_and_bookmarks_sync_with_state(cx: &mut TestAppContext) {
    let dir = temp_dir("sidebar_sync");
    let settings_file = dir.join("settings.toml");
    let profile = ConnectionProfile {
        id: "srv-1".into(),
        name: "Prod SFTP".into(),
        protocol: Protocol::Sftp,
        host: "files.example.com".into(),
        port: 22,
        user: "deploy".into(),
        start_path: "/var/www".into(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    };

    let state = cx.new(|_| {
        let mut s = AppState::new(Location::Local(dir.clone()))
            .with_settings_path(Some(settings_file.clone()));
        s.remote.save_profile(profile.clone(), None).unwrap();
        s
    });
    let sidebar = cx.new(|cx| SidebarView::new(state.clone(), cx));

    state.read_with(cx, |state, _| {
        let items = server_items(state);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Prod SFTP");
        assert_eq!(items[0].protocol, "SFTP");
        assert_eq!(items[0].status, ServerConnectionStatus::Disconnected);
    });

    sidebar.update(cx, |sidebar, cx| {
        sidebar
            .add_bookmark_in_state("Workspace", dir.clone(), cx)
            .unwrap();
        assert_eq!(sidebar.bookmarks.len(), 1);
        assert_eq!(sidebar.bookmarks[0].name, "Workspace");

        sidebar.rename_bookmark_in_state(0, "Projeto", cx).unwrap();
        assert_eq!(sidebar.bookmarks[0].name, "Projeto");

        sidebar.connect_server("srv-1", cx);
    });

    state.read_with(cx, |state, _| {
        assert_eq!(state.settings.bookmarks.len(), 1);
        assert_eq!(state.settings.bookmarks[0].name, "Projeto");
        assert!(state.remote_prompt.is_some());
    });

    sidebar.update(cx, |sidebar, cx| {
        sidebar.remove_bookmark_in_state(0, cx).unwrap();
        assert!(sidebar.bookmarks.is_empty());
    });

    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert!(disk.bookmarks.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn header_view_remote_and_network_mount_indicators(cx: &mut TestAppContext) {
    let dir = temp_dir("header_state");
    let profile = ConnectionProfile {
        id: "srv-sftp".into(),
        name: "Backup".into(),
        protocol: Protocol::Sftp,
        host: "backup.internal".into(),
        port: 22,
        user: "ops".into(),
        start_path: "/srv".into(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    };

    let state = cx.new(|_| {
        let mut s = AppState::new(Location::Local(dir.clone()));
        s.remote.save_profile(profile, None).unwrap();
        s
    });
    let header = cx.new(|cx| HeaderView::new(state.clone(), cx));

    state.read_with(cx, |state, _| {
        // Local directory: neither remote badge nor network-mount badge
        assert!(remote_header_info(state).is_none());
        assert!(!is_network_mount_badge_visible(state));
    });

    // Local UNC / network share path: shows network-mount badge
    state.update(cx, |state, _| {
        state.current_location = Location::Local(PathBuf::from("//nas.local/shared"));
    });
    state.read_with(cx, |state, _| {
        assert!(remote_header_info(state).is_none());
        assert!(is_network_mount_badge_visible(state));
    });

    // Remote location (disconnected): shows protocol + host + Desconectado + Atualizar + Reconectar
    state.update(cx, |state, _| {
        state.current_location = Location::Remote {
            profile_id: "srv-sftp".into(),
            path: PathBuf::from("/srv"),
        };
    });
    cx.run_until_parked();
    header.read_with(cx, |h, cx| {
        assert_eq!(h.address_input.read(cx).text(), "/srv");
    });
    state.read_with(cx, |state, _| {
        assert!(!is_network_mount_badge_visible(state));
        let info = remote_header_info(state).expect("remote header info");
        assert_eq!(info.protocol, "SFTP");
        assert_eq!(info.host, "backup.internal");
        assert_eq!(info.state, RemoteBadgeState::Disconnected);
        assert!(info.show_refresh);
        assert!(info.show_reconnect);
        assert!(!info.show_disconnect);
    });

    // Connecting/reconnecting state
    state.update(cx, |state, _| {
        state.connecting = Some("srv-sftp".into());
    });
    state.read_with(cx, |state, _| {
        let info_reconnecting = remote_header_info(state).expect("reconnecting info");
        assert_eq!(info_reconnecting.state, RemoteBadgeState::Reconnecting);
        assert!(!info_reconnecting.show_reconnect);
        assert!(info_reconnecting.show_disconnect);
    });

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn network_view_reads_timeout_from_state_and_triggers_retry_and_cancel(cx: &mut TestAppContext) {
    let dir = temp_dir("network_view");
    let profile = ConnectionProfile {
        id: "srv-1".into(),
        name: "Remote".into(),
        protocol: Protocol::Sftp,
        host: "remote.test".into(),
        port: 22,
        user: "user".into(),
        start_path: "/".into(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    };

    let state = cx.new(|_| {
        let mut s = AppState::new(Location::Remote {
            profile_id: "srv-1".into(),
            path: PathBuf::from("/data"),
        });
        s.remote.save_profile(profile, None).unwrap();
        s
    });
    let network_view = cx.new(|cx| NetworkView::new(state.clone(), cx));

    state.update(cx, |state, _| {
        state.set_list_timeout_secs(25).unwrap();
    });
    cx.run_until_parked();

    network_view.read_with(cx, |nv, cx| {
        assert_eq!(nv.timeout_secs, 25);
        assert_eq!(nv.effective_timeout_secs(state.read(cx)), 25);
    });

    // Retry on disconnected remote calls reconnect_current -> prompts for password when none saved
    network_view.update(cx, |nv, cx| {
        nv.retry(cx);
    });
    state.read_with(cx, |state, _| {
        assert!(state.remote_prompt.is_some());
    });

    // Cancel clears in-flight connection and sets status
    state.update(cx, |state, _| {
        state.connecting = Some("srv-1".into());
    });
    network_view.update(cx, |nv, cx| {
        nv.cancel(cx);
    });
    state.read_with(cx, |state, _| {
        assert!(state.connecting.is_none());
        assert_eq!(
            state.status_message.as_deref(),
            Some("Network operation cancelled")
        );
    });

    let _ = std::fs::remove_dir_all(&dir);
}
