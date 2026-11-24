use std::path::PathBuf;

use gpui::{point, px, size, AppContext as _, Modifiers, MouseButton, TestAppContext};
use kwati_core::{
    group_for_type_key, AuthMethod, ConnectionProfile, IconConfig, IconGroup, IconSource, Location,
    PlaceEntry, Protocol, Settings, SortColumn, SortDirection, SortOrder, ICON_TYPE_SPECS,
};
use kwati_gui::state::AppState;
use kwati_gui::views::header_view::{
    is_network_mount_badge_visible, remote_header_info, HeaderView, RemoteBadgeState,
};
use kwati_gui::views::network_view::NetworkView;
use kwati_gui::views::settings_view::{SettingKey, SettingsView};
use kwati_gui::views::sidebar_view::{server_items, ServerConnectionStatus, SidebarView};
use kwati_gui::views::{ContextMenuAction, ContextMenuEntry, DialogKind, MainView};

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
        view.toggle_bool_setting(SettingKey::ShowSidebar, cx)
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
        assert!(state.panels.sidebar_open);
        assert_eq!(state.sort_column, SortColumn::Name);
        assert_eq!(state.sort_direction, SortDirection::Ascending);
        assert_eq!(state.settings, Settings::default());
    });

    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert_eq!(disk, Settings::default());

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn settings_show_sidebar_toggle_hides_it_and_persists(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_show_sidebar");
    let settings_file = dir.join("settings.toml");
    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    view.update(cx, |view, cx| {
        view.toggle_bool_setting(SettingKey::ShowSidebar, cx)
            .unwrap();
    });

    state.read_with(cx, |state, _| assert!(!state.panels.sidebar_open));
    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert!(!disk.show_sidebar);

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
        let initial_len = sidebar.places.len();
        assert!(sidebar.places.iter().any(|p| p.is_trash()));

        sidebar
            .add_place_in_state("Workspace", dir.clone(), cx)
            .unwrap();
        assert_eq!(sidebar.places.len(), initial_len + 1);
        let added_ix = sidebar
            .places
            .iter()
            .position(|p| p.path() == Some(dir.as_path()))
            .unwrap();
        assert_eq!(sidebar.places[added_ix].name, "Workspace");

        sidebar
            .rename_place_in_state(added_ix, "Projeto", cx)
            .unwrap();
        assert_eq!(sidebar.places[added_ix].name, "Projeto");

        // Trash cannot be removed, only moved.
        let trash_ix = sidebar.places.iter().position(|p| p.is_trash()).unwrap();
        assert!(sidebar.remove_place_in_state(trash_ix, cx).is_err());
        assert!(sidebar.reorder_place_in_state(trash_ix, 0, cx).is_ok());
        assert!(sidebar.places[0].is_trash());

        sidebar.connect_server("srv-1", cx);
    });

    state.read_with(cx, |state, _| {
        let places = state.places();
        assert!(places[0].is_trash());
        assert!(places
            .iter()
            .any(|p| p.name() == "Projeto" && p.path() == Some(dir.as_path())));
        assert!(state.remote_prompt.is_some());
    });

    sidebar.update(cx, |sidebar, cx| {
        let proj_ix = sidebar
            .places
            .iter()
            .position(|p| p.path() == Some(dir.as_path()))
            .unwrap();
        sidebar.remove_place_in_state(proj_ix, cx).unwrap();
        assert!(!sidebar
            .places
            .iter()
            .any(|p| p.path() == Some(dir.as_path())));

        // Even standard places (everything except Trash) can be removed.
        let first_folder_ix = sidebar.places.iter().position(|p| !p.is_trash()).unwrap();
        let removed_path = sidebar.places[first_folder_ix]
            .path()
            .unwrap()
            .to_path_buf();
        sidebar.remove_place_in_state(first_folder_ix, cx).unwrap();
        assert!(!sidebar
            .places
            .iter()
            .any(|p| p.path() == Some(removed_path.as_path())));
    });

    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert!(!disk
        .resolved_places()
        .iter()
        .any(|p| p.path() == Some(dir.as_path())));
    assert!(disk.resolved_places().iter().any(|p| p.is_trash()));

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
    state.update(cx, |state, cx| {
        state.current_location = Location::Remote {
            profile_id: "srv-sftp".into(),
            path: PathBuf::from("/srv"),
        };
        cx.notify();
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

    state.update(cx, |state, cx| {
        state.set_list_timeout_secs(25).unwrap();
        cx.notify();
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

#[gpui::test]
fn settings_view_icons_overrides_presets_and_reset_keep_saved_presets(cx: &mut TestAppContext) {
    let dir = temp_dir("settings_icons_full");
    let settings_file = dir.join("settings.toml");
    let icons_file = dir.join("icons.toml");
    let custom_png = dir.join("custom.png");
    let pdf_svg = dir.join("pdf.svg");
    std::fs::write(&custom_png, b"png").unwrap();
    std::fs::write(&pdf_svg, b"<svg/>").unwrap();

    assert!(SettingsView::is_valid_icon_image_path(&custom_png));
    assert!(SettingsView::is_valid_icon_image_path(
        &dir.join("UPPER.SVG")
    ));
    assert!(!SettingsView::is_valid_icon_image_path(
        &dir.join("notes.txt")
    ));

    // Verify per-type specs carry their owning group next to them for the per-type list.
    let pdf_spec = ICON_TYPE_SPECS
        .iter()
        .find(|s| s.key == "file-pdf")
        .expect("file-pdf spec");
    assert_eq!(pdf_spec.group, IconGroup::Documents);
    assert_eq!(group_for_type_key("file-pdf"), IconGroup::Documents);

    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone()))
            .with_settings_path(Some(settings_file.clone()))
            .with_icons_path(Some(icons_file.clone()))
    });
    let (view, cx) = cx.add_window_view(|_, cx| SettingsView::new(state.clone(), cx));

    view.update_in(cx, |view, window, cx| {
        view.open(cx);
        view.set_installed_themes_for_test(Vec::new(), cx);
        view.focus(window, cx);

        // Base + group + type overrides
        view.set_icon_base(IconSource::Theme("Breeze".into()), cx)
            .unwrap();
        view.set_icon_group_override(
            IconGroup::Images,
            Some(IconSource::Image(custom_png.clone())),
            cx,
        )
        .unwrap();
        view.set_icon_type_override("file-pdf", Some(IconSource::Image(pdf_svg.clone())), cx)
            .unwrap();

        // Save preset, modify, apply, rename, delete
        view.save_icon_preset("Escritorio", cx).unwrap();
        view.set_icon_type_override("file-pdf", None, cx).unwrap();
    });

    // Reset to inherit removes the per-type override from icons.toml
    let disk = IconConfig::load_from(&icons_file).unwrap();
    assert!(!disk.active.types.contains_key("file-pdf"));
    assert_eq!(
        disk.active.groups.get(&IconGroup::Images),
        Some(&IconSource::Image(custom_png.clone()))
    );
    assert!(disk.presets.contains_key("Escritorio"));

    view.update(cx, |view, cx| {
        view.apply_icon_preset("Escritorio", cx).unwrap();
    });
    let disk_after_apply = IconConfig::load_from(&icons_file).unwrap();
    assert_eq!(
        disk_after_apply.active.types.get("file-pdf"),
        Some(&IconSource::Image(pdf_svg.clone()))
    );

    view.update(cx, |view, cx| {
        view.rename_icon_preset("Escritorio", "Trabalho", cx)
            .unwrap();
    });
    let disk_after_rename = IconConfig::load_from(&icons_file).unwrap();
    assert!(!disk_after_rename.presets.contains_key("Escritorio"));
    assert!(disk_after_rename.presets.contains_key("Trabalho"));

    // Restaurar padrões returns every icon to built-in and keeps saved presets
    view.update(cx, |view, cx| {
        view.reset_to_defaults(cx).unwrap();
    });
    let disk_after_reset = IconConfig::load_from(&icons_file).unwrap();
    assert_eq!(disk_after_reset.active.base, IconSource::BuiltIn);
    assert!(disk_after_reset.active.groups.is_empty());
    assert!(disk_after_reset.active.types.is_empty());
    assert!(disk_after_reset.presets.contains_key("Trabalho"));

    view.update(cx, |view, cx| {
        assert!(view.delete_icon_preset("Trabalho", cx).unwrap());
    });
    let disk_after_delete = IconConfig::load_from(&icons_file).unwrap();
    assert!(!disk_after_delete.presets.contains_key("Trabalho"));

    // Verify keyboard focus order was populated on render and Tab cycles focus into the modal
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.focus_handles_count() > 10);
    });
    cx.simulate_keystrokes("tab");
    cx.simulate_keystrokes("escape");
    state.read_with(cx, |state, _| {
        assert!(!state.settings_open);
    });

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn sidebar_unified_places_add_remove_and_drag_reorder_with_trash_pinned(cx: &mut TestAppContext) {
    let dir = temp_dir("sidebar_drag_places");
    let custom = dir.join("custom_folder");
    std::fs::create_dir_all(&custom).unwrap();
    let settings_file = dir.join("settings.toml");

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|_| {
            AppState::new(Location::Local(custom.clone()))
                .with_settings_path(Some(settings_file.clone()))
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // 1. Click "+" button in Places header to add the current folder (`custom_folder`).
    let add_bounds = cx
        .debug_bounds("sidebar-add-place")
        .expect("add button should be visible for unpinned local folder");
    let add_center = add_bounds.center();
    cx.simulate_mouse_move(add_center, None, Modifiers::none());
    cx.simulate_mouse_down(add_center, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(add_center, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    let (trash_ix, custom_ix) = main.read_with(cx, |main, cx| {
        let places = &main.sidebar.read(cx).places;
        let trash_ix = places.iter().position(|p| p.is_trash()).unwrap();
        let custom_ix = places
            .iter()
            .position(|p| p.path() == Some(custom.as_path()))
            .unwrap();
        (trash_ix, custom_ix)
    });
    assert!(custom_ix > trash_ix || places_contain_custom(&main, cx, &custom));

    // 2. Drag Trash from `trash_ix` to row 0 (top of Places).
    let from_bounds = cx
        .debug_bounds(Box::leak(format!("place-row-{trash_ix}").into_boxed_str()))
        .expect("trash row bounds");
    let to_bounds = cx.debug_bounds("place-row-0").expect("top row bounds");
    let from_pos = from_bounds.center();
    let to_pos = to_bounds.center();

    cx.simulate_mouse_move(from_pos, None, Modifiers::none());
    cx.simulate_mouse_down(from_pos, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(from_pos.x, from_pos.y - px(8.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(to_pos, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to_pos, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    // Dragging Trash must move it to index 0 WITHOUT navigating into Trash.
    main.read_with(cx, |main, cx| {
        assert!(!main.app_state.read(cx).trash_active);
        assert!(main.sidebar.read(cx).places[0].is_trash());
    });

    // 3. Right-clicking Trash (row 0) opens context menu WITHOUT Remove from Places;
    // right-clicking row 1 (folder) offers Remove from Places, and clicking it removes the item.
    let row0 = cx.debug_bounds("place-row-0").unwrap().center();
    cx.simulate_mouse_move(row0, None, Modifiers::none());
    cx.simulate_mouse_down(row0, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row0, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        let menu = main.context_menu.read(cx);
        assert!(menu.is_open());
        assert!(!menu.entries().iter().any(|e| matches!(
            e,
            ContextMenuEntry::Item {
                action: ContextMenuAction::SidebarRemove(_),
                ..
            }
        )));
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    let removed_path = main.read_with(cx, |main, cx| {
        main.sidebar.read(cx).places[1]
            .path()
            .unwrap()
            .to_path_buf()
    });
    let row1 = cx.debug_bounds("place-row-1").unwrap().center();
    cx.simulate_mouse_move(row1, None, Modifiers::none());
    cx.simulate_mouse_down(row1, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row1, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    let remove_ix = main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .position(|e| {
                matches!(
                    e,
                    ContextMenuEntry::Item {
                        action: ContextMenuAction::SidebarRemove(1),
                        ..
                    }
                )
            })
            .expect("SidebarRemove entry in context menu")
    });
    let remove_btn = cx
        .debug_bounds(Box::leak(
            format!("context-menu-item-{remove_ix}").into_boxed_str(),
        ))
        .expect("context menu item bounds")
        .center();
    cx.simulate_mouse_move(remove_btn, None, Modifiers::none());
    cx.simulate_mouse_down(remove_btn, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(remove_btn, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    main.read_with(cx, |main, cx| {
        let places = &main.sidebar.read(cx).places;
        assert!(!places
            .iter()
            .any(|p| p.path() == Some(removed_path.as_path())));
        assert!(places[0].is_trash());
    });

    // 4. Order and removal survive reload from disk.
    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    let disk_places = disk.resolved_places();
    assert_eq!(disk_places[0], PlaceEntry::Trash);
    assert!(!disk_places
        .iter()
        .any(|p| p.path() == Some(removed_path.as_path())));
    assert!(disk_places
        .iter()
        .any(|p| p.path() == Some(custom.as_path())));

    let _ = std::fs::remove_dir_all(&dir);
}

fn places_contain_custom(
    main: &gpui::Entity<MainView>,
    cx: &gpui::VisualTestContext,
    custom: &std::path::Path,
) -> bool {
    main.read_with(cx, |main, cx| {
        main.sidebar
            .read(cx)
            .places
            .iter()
            .any(|p| p.path() == Some(custom))
    })
}

fn list_row_center(
    main: &gpui::Entity<MainView>,
    cx: &mut gpui::VisualTestContext,
    name: &str,
) -> gpui::Point<gpui::Pixels> {
    main.read_with(cx, |main, cx| {
        let ix = main
            .app_state
            .read(cx)
            .entries
            .iter()
            .position(|e| e.name().to_string_lossy() == name)
            .expect("entry in list");
        let list = main.list.read(cx);
        let entry_count = main.app_state.read(cx).entries.len().max(1);
        let state = list.scroll_handle.0.borrow();
        let bounds = state.base_handle.bounds();
        let offset = state.base_handle.offset();
        let row_h = state
            .last_item_size
            .map(|s| s.contents.height / (entry_count as f32))
            .filter(|h| *h > px(0.))
            .unwrap_or(list.row_height);
        point(
            bounds.left() + px(80.),
            bounds.top() + row_h * (ix as f32) + row_h / 2.0 + offset.y,
        )
    })
}

fn pointer_drag(
    cx: &mut gpui::VisualTestContext,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) {
    cx.simulate_mouse_move(from, None, Modifiers::none());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(from.x - px(8.), from.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn drag_from_list_to_places_pins_folders_moves_files_and_trashes(cx: &mut TestAppContext) {
    let dir = temp_dir("drag_list_to_places");
    let workspace = dir.join("workspace");
    let sub_folder = workspace.join("projetos");
    let drop_target = dir.join("dest_place");
    std::fs::create_dir_all(&sub_folder).unwrap();
    std::fs::create_dir_all(&drop_target).unwrap();
    std::fs::write(workspace.join("doc.txt"), "hello").unwrap();
    std::fs::write(workspace.join("trashme.txt"), "bye").unwrap();
    let settings_file = dir.join("settings.toml");

    let (main, cx) = cx.add_window_view(|window, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![
            PlaceEntry::folder("Destino", drop_target.clone()),
            PlaceEntry::Trash,
        ]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(workspace.clone()))
                .with_settings_path(Some(settings_file.clone()));
            s.settings = settings;
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // 1. Dragging a folder from the file list onto the Places header pins it into Places.
    let folder_pos = list_row_center(&main, cx, "projetos");
    let header_pos = cx
        .debug_bounds("sidebar-places-header")
        .expect("places header bounds")
        .center();
    pointer_drag(cx, folder_pos, header_pos);
    assert!(places_contain_custom(&main, cx, &sub_folder));

    let (disk, _) = Settings::load_from(&settings_file).unwrap();
    assert!(disk
        .resolved_places()
        .iter()
        .any(|p| p.path() == Some(sub_folder.as_path())));

    // 2. Dragging a file from the file list onto a Place folder moves the file there.
    let doc_pos = list_row_center(&main, cx, "doc.txt");
    let dest_row_pos = cx
        .debug_bounds("place-row-0")
        .expect("Destino row bounds")
        .center();
    pointer_drag(cx, doc_pos, dest_row_pos);
    assert!(drop_target.join("doc.txt").exists());
    assert!(!workspace.join("doc.txt").exists());

    // 3. Dragging a file from the file list onto Trash in Places opens ConfirmTrash.
    let trash_file_pos = list_row_center(&main, cx, "trashme.txt");
    let trash_row_pos = cx
        .debug_bounds("place-row-1")
        .expect("Trash row bounds")
        .center();
    pointer_drag(cx, trash_file_pos, trash_row_pos);
    let dialog = main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert!(matches!(
        dialog,
        DialogKind::ConfirmTrash { locations } if locations == vec![Location::Local(workspace.join("trashme.txt"))]
    ));

    let _ = std::fs::remove_dir_all(&dir);
}
