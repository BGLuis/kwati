//! Context menu for entries and empty space (#55).

use std::borrow::Cow;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, MouseButton, Pixels, Point,
    TestAppContext, VisualTestContext,
};
use kwati_core::{
    AuthMethod, Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryMeta,
    FolderSettingsScope, Listing, Location, Protocol, ReconnectingBackend, RetryPolicy, Settings,
    SortColumn, SortDirection, SortOrder, ViewMode,
};
use kwati_gui::actions::operations::{clear_clipboard, execute_rename};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::AppState;
use kwati_gui::views::{ContextMenuAction, ContextMenuEntry, DialogKind, MainView};

fn clipboard() -> MutexGuard<'static, ()> {
    static CLIPBOARD_IN_USE: Mutex<()> = Mutex::new(());
    let guard = CLIPBOARD_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
    clear_clipboard();
    guard
}

fn fixture(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("kwati_test_ctxmenu_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("alpha.txt"), "alpha").unwrap();
    fs::write(root.join("bravo.txt"), "bravo").unwrap();
    root
}

fn open_window<'a>(
    cx: &'a mut TestAppContext,
    dir: &Path,
) -> (Entity<MainView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });
    let location = Location::Local(dir.to_path_buf());
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut state = AppState::new(location);
            state.attach(cx);
            state.reload_entries();
            state
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();
    (main, cx)
}

fn row_center(main: &Entity<MainView>, cx: &mut VisualTestContext, ix: usize) -> Point<Pixels> {
    main.read_with(cx, |main, cx| {
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

fn right_click(cx: &mut VisualTestContext, pos: Point<Pixels>) {
    cx.simulate_mouse_move(pos, None, Modifiers::none());
    cx.simulate_mouse_down(pos, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(pos, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
}

fn left_click(cx: &mut VisualTestContext, pos: Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_mouse_move(pos, None, modifiers);
    cx.simulate_mouse_down(pos, MouseButton::Left, modifiers);
    cx.simulate_mouse_up(pos, MouseButton::Left, modifiers);
    cx.run_until_parked();
}

fn selected_names(main: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        state
            .selected_entries()
            .into_iter()
            .map(|entry| entry.name().to_string_lossy().into_owned())
            .collect()
    })
}

fn menu_actions(main: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<ContextMenuAction> {
    main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .filter_map(|e| match e {
                ContextMenuEntry::Item { action, .. } => Some(*action),
                ContextMenuEntry::Submenu { .. } | ContextMenuEntry::Separator => None,
            })
            .collect()
    })
}

fn menu_submenu_labels(main: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .filter_map(|e| match e {
                ContextMenuEntry::Submenu { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect()
    })
}

fn submenu_entries_for(
    main: &Entity<MainView>,
    cx: &mut VisualTestContext,
    submenu_label: &str,
) -> Vec<ContextMenuEntry> {
    main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .find_map(|e| match e {
                ContextMenuEntry::Submenu { label, items } if label == submenu_label => {
                    Some(items.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    })
}

fn is_menu_open(main: &Entity<MainView>, cx: &mut VisualTestContext) -> bool {
    main.read_with(cx, |main, cx| main.context_menu.read(cx).is_open())
}

#[gpui::test]
fn right_click_unselected_entry_selects_only_it_and_opens_at_pointer(cx: &mut TestAppContext) {
    let root = fixture("entry");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    // Pre-select row 2 (bravo.txt), then right-click row 1 (alpha.txt).
    let r2 = row_center(&main, cx, 2);
    left_click(cx, r2, Modifiers::none());
    assert_eq!(selected_names(&main, cx), ["bravo.txt"]);

    let target = row_center(&main, cx, 1);
    right_click(cx, target);

    assert!(is_menu_open(&main, cx));
    assert_eq!(selected_names(&main, cx), ["alpha.txt"]);
    let menu_pos = main.read_with(cx, |main, cx| main.context_menu.read(cx).position());
    assert_eq!(menu_pos, target);
    assert_eq!(
        menu_actions(&main, cx),
        [
            ContextMenuAction::Open,
            ContextMenuAction::Rename,
            ContextMenuAction::Copy,
            ContextMenuAction::Cut,
            ContextMenuAction::Paste,
            ContextMenuAction::Remove,
        ]
    );

    // Right-clicking outside the menu card closes it and returns focus to the list.
    left_click(cx, point(px(10.), px(10.)), Modifiers::none());
    assert!(!is_menu_open(&main, cx));

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn right_click_inside_multi_selection_preserves_selection(cx: &mut TestAppContext) {
    let root = fixture("multi");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    let r1 = row_center(&main, cx, 1);
    let r2 = row_center(&main, cx, 2);
    left_click(cx, r1, Modifiers::none());
    left_click(cx, r2, Modifiers::secondary_key());
    assert_eq!(selected_names(&main, cx), ["alpha.txt", "bravo.txt"]);

    right_click(cx, r2);
    assert!(is_menu_open(&main, cx));
    assert_eq!(selected_names(&main, cx), ["alpha.txt", "bravo.txt"]);
    assert_eq!(
        menu_actions(&main, cx),
        [
            ContextMenuAction::Copy,
            ContextMenuAction::Cut,
            ContextMenuAction::Remove,
        ]
    );

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn right_click_empty_space_shows_folder_actions(cx: &mut TestAppContext) {
    let root = fixture("empty_space");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    let r1 = row_center(&main, cx, 1);
    left_click(cx, r1, Modifiers::none());
    assert_eq!(selected_names(&main, cx), ["alpha.txt"]);

    let r2 = row_center(&main, cx, 2);
    let empty = point(r2.x, r2.y + px(48.));
    right_click(cx, empty);

    assert!(is_menu_open(&main, cx));
    assert!(selected_names(&main, cx).is_empty());
    assert_eq!(
        menu_actions(&main, cx),
        [
            ContextMenuAction::Paste,
            ContextMenuAction::NewFolder,
            ContextMenuAction::Undo,
            ContextMenuAction::ToggleShowHidden,
            ContextMenuAction::ApplyFolderSettingsToAll,
            ContextMenuAction::Refresh,
        ]
    );
    let undo_item = main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .find_map(|entry| match entry {
                ContextMenuEntry::Item {
                    action: ContextMenuAction::Undo,
                    label,
                    enabled,
                    ..
                } => Some((label.to_string(), *enabled)),
                _ => None,
            })
    });
    assert_eq!(undo_item, Some(("Undo".to_string(), false)));
    assert_eq!(menu_submenu_labels(&main, cx), ["View", "Sort by"]);

    let view_items = submenu_entries_for(&main, cx, "View");
    assert!(view_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetViewMode(ViewMode::Details),
        label: Cow::Borrowed("Details"),
        shortcut: Some("Ctrl+1"),
        enabled: true,
        destructive: false,
        checked: true,
    }));
    assert!(view_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetViewMode(ViewMode::Grid),
        label: Cow::Borrowed("Grid"),
        shortcut: Some("Ctrl+2"),
        enabled: true,
        destructive: false,
        checked: false,
    }));
    assert!(view_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::ToggleSidebar,
        label: Cow::Borrowed("Show Sidebar"),
        shortcut: Some("F9"),
        enabled: true,
        destructive: false,
        checked: true,
    }));
    assert!(view_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::TogglePreview,
        label: Cow::Borrowed("Show Preview"),
        shortcut: Some("Alt+P"),
        enabled: true,
        destructive: false,
        checked: true,
    }));

    let sort_items = submenu_entries_for(&main, cx, "Sort by");
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortColumn(SortColumn::Name),
        label: Cow::Borrowed("Name"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: true,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortColumn(SortColumn::Created),
        label: Cow::Borrowed("Created"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: false,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortColumn(SortColumn::Accessed),
        label: Cow::Borrowed("Accessed"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: false,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortColumn(SortColumn::Type),
        label: Cow::Borrowed("Type"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: false,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortDirection(SortDirection::Ascending),
        label: Cow::Borrowed("A to Z"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: true,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::ToggleFoldersFirst,
        label: Cow::Borrowed("Folders First"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: true,
    }));
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::ToggleHiddenLast,
        label: Cow::Borrowed("Hidden Files Last"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: false,
    }));

    let _ = fs::remove_dir_all(&root);
}

fn row_of(main: &Entity<MainView>, cx: &mut VisualTestContext, name: &str) -> usize {
    main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .entries
            .iter()
            .position(|e| e.name().to_string_lossy() == name)
            .unwrap()
    })
}

fn bookmark_item_enabled(main: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<bool> {
    main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .find_map(|e| match e {
                ContextMenuEntry::Item {
                    action: ContextMenuAction::Bookmark,
                    enabled,
                    ..
                } => Some(*enabled),
                _ => None,
            })
    })
}

#[gpui::test]
fn bookmark_folder_adds_it_to_the_sidebar_and_it_survives_a_restart(cx: &mut TestAppContext) {
    let root = fixture("bookmark");
    let _clipboard = clipboard();
    let settings_file = root.join("settings.toml");
    let (main, cx) = open_window(cx, &root);
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            state.set_settings_path(Some(settings_file.clone()))
        })
    });

    // Files cannot be bookmarked.
    let ix = row_of(&main, cx, "alpha.txt");
    let file = row_center(&main, cx, ix);
    right_click(cx, file);
    assert_eq!(bookmark_item_enabled(&main, cx), None);
    cx.simulate_keystrokes("escape");

    let ix = row_of(&main, cx, "sub");
    let folder = row_center(&main, cx, ix);
    right_click(cx, folder);
    assert_eq!(bookmark_item_enabled(&main, cx), Some(true));
    // Open, Rename, Bookmark Folder.
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();

    let sub = root.join("sub");
    let has_sub = main.read_with(cx, |main, cx| {
        main.sidebar
            .read(cx)
            .places
            .iter()
            .any(|p| p.name == "sub" && p.path() == Some(sub.as_path()))
    });
    assert!(has_sub);

    let (disk, _) = kwati_core::Settings::load_from(&settings_file).unwrap();
    assert!(disk
        .resolved_places()
        .iter()
        .any(|p| p.path() == Some(sub.as_path())));

    // The same folder is offered again only as a disabled item.
    right_click(cx, folder);
    assert_eq!(bookmark_item_enabled(&main, cx), Some(false));

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn shift_f10_and_escape_toggle_context_menu_and_keyboard_runs_action(cx: &mut TestAppContext) {
    let root = fixture("keyboard");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    let r1 = row_center(&main, cx, 1);
    left_click(cx, r1, Modifiers::none());
    cx.simulate_keystrokes("shift-f10");
    assert!(is_menu_open(&main, cx));

    // Escape closes the menu and restores keyboard focus to the list.
    cx.simulate_keystrokes("escape");
    assert!(!is_menu_open(&main, cx));

    // Menu key also opens it; Down moves focus to Rename, Enter opens the Rename dialog.
    cx.simulate_keystrokes("menu");
    assert!(is_menu_open(&main, cx));
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();

    assert!(!is_menu_open(&main, cx));
    let dialog = main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert!(matches!(dialog, DialogKind::Rename { .. }));

    let _ = fs::remove_dir_all(&root);
}

fn menu_item_bounds(cx: &mut VisualTestContext, selector: &str) -> Point<Pixels> {
    cx.debug_bounds(Box::leak(selector.to_string().into_boxed_str()))
        .unwrap_or_else(|| panic!("missing bounds for {selector}"))
        .center()
}

#[cfg(windows)]
fn mark_hidden(path: &Path) {
    // Windows hides by file attribute, not by a leading dot.
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(path)
        .status()
        .expect("attrib command failed");
    assert!(status.success(), "attrib +h must succeed");
}

#[cfg(not(windows))]
fn mark_hidden(_path: &Path) {}

#[gpui::test]
fn empty_space_context_menu_submenus_apply_per_folder_and_persist(cx: &mut TestAppContext) {
    let root = fixture("per_folder");
    let _clipboard = clipboard();
    let dir_a = root.join("folder_a");
    let dir_b = root.join("folder_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    fs::write(dir_a.join("small.txt"), "1").unwrap();
    fs::write(dir_a.join("large.txt"), "1234567890").unwrap();
    let secret_a = dir_a.join(".secret_a");
    fs::write(&secret_a, "hidden").unwrap();
    mark_hidden(&secret_a);
    fs::write(dir_b.join("only_b.txt"), "b").unwrap();
    let secret_b = dir_b.join(".secret_b");
    fs::write(&secret_b, "hidden").unwrap();
    mark_hidden(&secret_b);
    let settings_file = root.join("settings.toml");

    let (main, cx) = open_window(cx, &dir_a);
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            state.set_settings_path(Some(settings_file.clone()));
        });
    });

    // Open empty-space context menu and hover "View" submenu (entry index 4), click "Grid" (subitem 1).
    let empty = point(px(500.), px(400.));
    right_click(cx, empty);
    assert!(is_menu_open(&main, cx));
    let view_row = menu_item_bounds(cx, "context-menu-item-4");
    cx.simulate_mouse_move(view_row, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        main.read_with(cx, |m, cx| m.context_menu.read(cx).open_submenu()),
        Some(4)
    );
    let grid_item = menu_item_bounds(cx, "context-menu-subitem-1");
    left_click(cx, grid_item, Modifiers::none());

    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert_eq!(s.view_mode, ViewMode::Grid);
        assert_eq!(s.settings.view_mode, ViewMode::Details);
    });

    // Open via Shift+F10 with no selection and use keyboard into "Sort by" -> "Size" -> "Descending".
    cx.simulate_keystrokes("shift-f10");
    assert!(is_menu_open(&main, cx));
    // Items: Paste(0, disabled), New Folder(1), Undo(2, disabled), View(4), Sort by(5), Show Hidden(6).
    // Initial focus is on New Folder(1). Down -> View(4), Down -> Sort by(5), Right -> Name(0), Down -> Size(1), Enter.
    cx.simulate_keystrokes("down down right down enter");
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        assert_eq!(m.app_state.read(cx).sort_column, SortColumn::Size);
    });

    cx.simulate_keystrokes("shift-f10");
    // Open Sort by(5) -> Down x7 -> Descending(8) -> Enter.
    cx.simulate_keystrokes("down down right down down down down down down down enter");
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        assert_eq!(
            m.app_state.read(cx).sort_direction,
            SortDirection::Descending
        );
    });

    // Verify contextual direction labels update to Smallest/Largest first when SortColumn::Size is active.
    right_click(cx, empty);
    let sort_items = submenu_entries_for(&main, cx, "Sort by");
    assert!(sort_items.contains(&ContextMenuEntry::Item {
        action: ContextMenuAction::SetSortDirection(SortDirection::Descending),
        label: Cow::Borrowed("Largest first"),
        shortcut: None,
        enabled: true,
        destructive: false,
        checked: true,
    }));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Toggle "Folders First" off and "Hidden Files Last" on via Sort by submenu.
    right_click(cx, empty);
    let sort_row = menu_item_bounds(cx, "context-menu-item-5");
    cx.simulate_mouse_move(sort_row, None, Modifiers::none());
    cx.run_until_parked();
    let folders_first_item = menu_item_bounds(cx, "context-menu-subitem-10");
    left_click(cx, folders_first_item, Modifiers::none());

    right_click(cx, empty);
    let sort_row = menu_item_bounds(cx, "context-menu-item-5");
    cx.simulate_mouse_move(sort_row, None, Modifiers::none());
    cx.run_until_parked();
    let hidden_last_item = menu_item_bounds(cx, "context-menu-subitem-11");
    left_click(cx, hidden_last_item, Modifiers::none());

    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert!(!s.folders_first);
        assert!(s.hidden_last);
        assert!(s.settings.folders_first);
        assert!(!s.settings.hidden_last);
    });

    // Toggle "Show Hidden Files" from context menu.
    right_click(cx, empty);
    let hidden_row = menu_item_bounds(cx, "context-menu-item-6");
    left_click(cx, hidden_row, Modifiers::none());
    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert!(s.show_hidden);
        assert!(!s.settings.show_hidden);
        assert!(s.entries.iter().any(|e| e.name() == ".secret_a"));
    });

    // Navigating to folder_b keeps global defaults (Details, Name Asc, hidden false).
    main.update(cx, |m, cx| {
        m.app_state
            .update(cx, |s, _| s.navigate_to(Location::Local(dir_b.clone())));
    });
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert_eq!(s.view_mode, ViewMode::Details);
        assert_eq!(s.sort_column, SortColumn::Name);
        assert_eq!(s.sort_direction, SortDirection::Ascending);
        assert!(!s.show_hidden);
        assert!(!s.entries.iter().any(|e| e.name() == ".secret_b"));
    });

    // Navigating back to folder_a restores its per-folder settings.
    main.update(cx, |m, cx| {
        m.app_state
            .update(cx, |s, _| s.navigate_to(Location::Local(dir_a.clone())));
    });
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert_eq!(s.view_mode, ViewMode::Grid);
        assert_eq!(s.sort_column, SortColumn::Size);
        assert_eq!(s.sort_direction, SortDirection::Descending);
        assert!(s.show_hidden);
    });

    // Persisted settings load properly into a fresh AppState starting at folder_a.
    let mut reloaded = AppState::new(Location::Local(dir_a.clone()));
    reloaded.load_settings_from(&settings_file).unwrap();
    assert_eq!(reloaded.view_mode, ViewMode::Grid);
    assert_eq!(reloaded.sort_column, SortColumn::Size);
    assert_eq!(reloaded.sort_direction, SortDirection::Descending);
    assert!(reloaded.show_hidden);

    // Clicking "Apply to All Folders" (index 7) in folder_a applies settings globally.
    right_click(cx, empty);
    let apply_all_row = menu_item_bounds(cx, "context-menu-item-7");
    left_click(cx, apply_all_row, Modifiers::none());
    let (disk, warnings) = Settings::load_from(&settings_file).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(disk.view_mode, ViewMode::Grid);
    assert_eq!(
        disk.default_sort,
        SortOrder::new(SortColumn::Size, SortDirection::Descending)
    );
    assert!(disk.show_hidden);
    assert!(disk.folder_preferences.is_empty());

    // Navigating to folder_b now uses those promoted global settings.
    main.update(cx, |m, cx| {
        m.app_state
            .update(cx, |s, _| s.navigate_to(Location::Local(dir_b.clone())));
    });
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert_eq!(s.view_mode, ViewMode::Grid);
        assert_eq!(s.sort_column, SortColumn::Size);
        assert_eq!(s.sort_direction, SortDirection::Descending);
        assert!(s.show_hidden);
    });

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn settings_scope_all_folders_applies_context_menu_changes_globally(cx: &mut TestAppContext) {
    let root = fixture("scope_all");
    let _clipboard = clipboard();
    let dir_a = root.join("folder_a");
    let dir_b = root.join("folder_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    let settings_file = root.join("settings.toml");

    let (main, cx) = open_window(cx, &dir_a);
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            state.set_settings_path(Some(settings_file.clone()));
        });
        main.settings.update(cx, |sv, cx| {
            sv.set_folder_settings_scope(FolderSettingsScope::AllFolders, cx)
                .unwrap();
        });
    });

    let empty = point(px(500.), px(400.));
    right_click(cx, empty);
    let view_row = menu_item_bounds(cx, "context-menu-item-4");
    cx.simulate_mouse_move(view_row, None, Modifiers::none());
    cx.run_until_parked();
    let compact_item = menu_item_bounds(cx, "context-menu-subitem-2");
    left_click(cx, compact_item, Modifiers::none());

    main.read_with(cx, |m, cx| {
        let s = m.app_state.read(cx);
        assert_eq!(s.view_mode, ViewMode::CompactList);
        assert_eq!(s.settings.view_mode, ViewMode::CompactList);
        assert!(s.settings.folder_preferences.is_empty());
    });

    main.update(cx, |m, cx| {
        m.app_state
            .update(cx, |s, _| s.navigate_to(Location::Local(dir_b.clone())));
    });
    cx.run_until_parked();
    main.read_with(cx, |m, cx| {
        assert_eq!(m.app_state.read(cx).view_mode, ViewMode::CompactList);
    });

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn empty_space_context_menu_undo_names_operation_and_restores_rename(cx: &mut TestAppContext) {
    let root = fixture("undo_ctxmenu");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    let original = root.join("alpha.txt");
    let renamed = main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            execute_rename(state, &original, "alpha-renamed.txt").unwrap()
        })
    });
    cx.run_until_parked();
    assert!(renamed.exists());
    assert!(!original.exists());

    // Open empty-space context menu and verify the Undo item shows the top operation label.
    let empty = point(px(500.), px(400.));
    right_click(cx, empty);
    assert!(is_menu_open(&main, cx));

    let undo_item = main.read_with(cx, |main, cx| {
        main.context_menu
            .read(cx)
            .entries()
            .iter()
            .find_map(|entry| match entry {
                ContextMenuEntry::Item {
                    action: ContextMenuAction::Undo,
                    label,
                    enabled,
                    ..
                } => Some((label.to_string(), *enabled)),
                _ => None,
            })
    });
    assert_eq!(undo_item, Some(("Undo rename".to_string(), true)));

    // Navigate down from New Folder to Undo rename and press Enter.
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();

    assert!(!is_menu_open(&main, cx));
    assert!(original.exists());
    assert!(!renamed.exists());
    let status = main.read_with(cx, |main, cx| {
        main.app_state.read(cx).status_message.clone()
    });
    assert_eq!(status.as_deref(), Some("Undid rename of 'alpha.txt'"));

    let _ = fs::remove_dir_all(&root);
}

#[cfg(target_os = "linux")]
fn xdg_env_lock() -> MutexGuard<'static, ()> {
    static XDG_ENV_LOCK: Mutex<()> = Mutex::new(());
    XDG_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(target_os = "linux")]
struct XdgEnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

#[cfg(target_os = "linux")]
impl XdgEnvGuard {
    fn isolate(data_home: &Path, config_home: &Path, sys_data: &Path, sys_config: &Path) -> Self {
        let vars = [
            ("XDG_DATA_HOME", data_home.as_os_str()),
            ("XDG_CONFIG_HOME", config_home.as_os_str()),
            ("XDG_DATA_DIRS", sys_data.as_os_str()),
            ("XDG_CONFIG_DIRS", sys_config.as_os_str()),
            ("XDG_CURRENT_DESKTOP", std::ffi::OsStr::new("")),
        ];
        let mut saved = Vec::with_capacity(vars.len());
        for (key, val) in vars {
            saved.push((key, std::env::var_os(key)));
            std::env::set_var(key, val);
        }
        Self { saved }
    }
}

#[cfg(target_os = "linux")]
impl Drop for XdgEnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain(..).rev() {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn write_executable_script(path: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let script = format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", marker.display());
    fs::write(path, script).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[derive(Clone)]
struct StubRemoteBackend {
    entries: Vec<Entry>,
}

impl Backend for StubRemoteBackend {
    fn list(&self, _location: &Location) -> Result<Listing, BackendError> {
        Ok(Listing::new(self.entries.clone()))
    }

    fn stat(&self, _location: &Location) -> Result<EntryMeta, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn open_read(&self, _location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn open_write(&self, _location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn rename(&self, _from: &Location, _to: &Location) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    fn delete_permanently(
        &self,
        _location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }
}

#[gpui::test]
#[cfg(target_os = "linux")]
fn single_local_file_has_open_with_submenu_while_dir_and_multi_do_not(cx: &mut TestAppContext) {
    let _xdg_lock = xdg_env_lock();
    let _clipboard = clipboard();
    let root = fixture("open_with_submenu");
    let data_home = root.join("xdg_data");
    let config_home = root.join("xdg_config");
    let sys_data = root.join("sys_data");
    let sys_config = root.join("sys_config");
    fs::create_dir_all(data_home.join("mime")).unwrap();
    fs::create_dir_all(data_home.join("applications")).unwrap();
    fs::create_dir_all(&config_home).unwrap();
    fs::create_dir_all(&sys_data).unwrap();
    fs::create_dir_all(&sys_config).unwrap();
    let _env = XdgEnvGuard::isolate(&data_home, &config_home, &sys_data, &sys_config);

    fs::write(
        data_home.join("mime/globs2"),
        "50:application/x-kwatictx:*.kwatictx\n",
    )
    .unwrap();
    fs::write(
        data_home.join("applications/alpha.desktop"),
        "[Desktop Entry]\nType=Application\nName=Alpha Editor\nExec=/bin/true %f\nMimeType=application/x-kwatictx;\n",
    )
    .unwrap();
    fs::write(
        data_home.join("applications/zulu-def.desktop"),
        "[Desktop Entry]\nType=Application\nName=Zulu Viewer\nExec=/bin/true %f\nNoDisplay=true\nMimeType=application/x-kwatictx;\n",
    )
    .unwrap();
    fs::write(
        data_home.join("applications/hidden-other.desktop"),
        "[Desktop Entry]\nType=Application\nName=Hidden Other\nExec=/bin/true %f\nNoDisplay=true\nMimeType=application/x-kwatictx;\n",
    )
    .unwrap();
    fs::write(
        config_home.join("mimeapps.list"),
        "[Default Applications]\napplication/x-kwatictx=zulu-def.desktop;\n",
    )
    .unwrap();

    fs::write(root.join("note.kwatictx"), "note").unwrap();
    fs::write(root.join("plain.unknown_kwati_999"), "raw").unwrap();

    let (main, cx) = open_window(cx, &root);

    // 1. Single local file with registered apps: "Open With" submenu has default first, non-default,
    // separator, and "Other application...", excluding non-default NoDisplay=true entries.
    let note_ix = row_of(&main, cx, "note.kwatictx");
    let note_pos = row_center(&main, cx, note_ix);
    right_click(cx, note_pos);
    assert_eq!(menu_submenu_labels(&main, cx), ["Open With"]);
    assert_eq!(
        submenu_entries_for(&main, cx, "Open With"),
        vec![
            ContextMenuEntry::Item {
                action: ContextMenuAction::OpenWithApp(0),
                label: Cow::Owned("Zulu Viewer (Default)".to_string()),
                shortcut: None,
                enabled: true,
                destructive: false,
                checked: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::OpenWithApp(1),
                label: Cow::Owned("Alpha Editor".to_string()),
                shortcut: None,
                enabled: true,
                destructive: false,
                checked: false,
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item {
                action: ContextMenuAction::OpenWithOther,
                label: Cow::Borrowed("Other application..."),
                shortcut: None,
                enabled: true,
                destructive: false,
                checked: false,
            },
        ]
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // 2. Single local file with no registered apps: only "Other application..." (no separator).
    let unknown_ix = row_of(&main, cx, "plain.unknown_kwati_999");
    let unknown_pos = row_center(&main, cx, unknown_ix);
    right_click(cx, unknown_pos);
    assert_eq!(menu_submenu_labels(&main, cx), ["Open With"]);
    assert_eq!(
        submenu_entries_for(&main, cx, "Open With"),
        vec![ContextMenuEntry::Item {
            action: ContextMenuAction::OpenWithOther,
            label: Cow::Borrowed("Other application..."),
            shortcut: None,
            enabled: true,
            destructive: false,
            checked: false,
        }]
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // 3. Single local directory does NOT include "Open With".
    let sub_ix = row_of(&main, cx, "sub");
    let sub_pos = row_center(&main, cx, sub_ix);
    right_click(cx, sub_pos);
    assert!(menu_submenu_labels(&main, cx).is_empty());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // 4. Multi-selection does NOT include "Open With".
    let alpha_ix = row_of(&main, cx, "alpha.txt");
    let bravo_ix = row_of(&main, cx, "bravo.txt");
    let alpha_pos = row_center(&main, cx, alpha_ix);
    let bravo_pos = row_center(&main, cx, bravo_ix);
    left_click(cx, alpha_pos, Modifiers::none());
    left_click(cx, bravo_pos, Modifiers::secondary_key());
    right_click(cx, bravo_pos);
    assert!(menu_submenu_labels(&main, cx).is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn remote_file_context_menu_does_not_include_open_with(cx: &mut TestAppContext) {
    let _clipboard = clipboard();
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });

    let profile_id = "nas-ctx-test";
    let remote_dir = Location::Remote {
        profile_id: profile_id.to_owned(),
        path: PathBuf::from("/share"),
    };
    let remote_file = Entry::new(
        "remote.txt".into(),
        PathBuf::from("/share/remote.txt"),
        false,
    )
    .with_size(12);
    let backend = StubRemoteBackend {
        entries: vec![remote_file],
    };

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut st = AppState::new(remote_dir.clone());
            st.attach(cx);
            let profile = ConnectionProfile {
                id: profile_id.to_owned(),
                name: "NAS".to_owned(),
                protocol: Protocol::Smb,
                host: "nas.local".to_owned(),
                port: 445,
                user: "luis".to_owned(),
                start_path: "/share".to_owned(),
                auth: AuthMethod::Password,
                private_key_path: None,
                allow_insecure: false,
            };
            let _ = st.remote.save_profile(profile, None);
            let reconnecting = ReconnectingBackend::new(
                move || Ok(Box::new(backend.clone())),
                RetryPolicy::new(Duration::from_millis(1), 1),
            )
            .unwrap();
            st.remote.add_connection(profile_id, reconnecting);
            st.reload_entries();
            st
        });
        MainView::new(state, window, cx)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        cx.run_until_parked();
        let loaded = main.read_with(cx, |main, cx| !main.app_state.read(cx).entries.is_empty());
        if loaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();

    let row_ix = row_of(&main, cx, "remote.txt");
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.select_index(row_ix);
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-f10");
    cx.run_until_parked();
    assert!(is_menu_open(&main, cx));
    assert_eq!(selected_names(&main, cx), ["remote.txt"]);
    assert_eq!(
        menu_actions(&main, cx),
        [
            ContextMenuAction::Open,
            ContextMenuAction::Rename,
            ContextMenuAction::Copy,
            ContextMenuAction::Cut,
            ContextMenuAction::Paste,
            ContextMenuAction::Remove,
        ]
    );
    assert!(menu_submenu_labels(&main, cx).is_empty());
}

#[gpui::test]
#[cfg(target_os = "linux")]
fn open_with_submenu_launches_desktop_script_and_reports_missing_executable(
    cx: &mut TestAppContext,
) {
    let _xdg_lock = xdg_env_lock();
    let _clipboard = clipboard();
    let root = fixture("open_with_launch");
    let data_home = root.join("xdg_data");
    let config_home = root.join("xdg_config");
    let sys_data = root.join("sys_data");
    let sys_config = root.join("sys_config");
    fs::create_dir_all(data_home.join("mime")).unwrap();
    fs::create_dir_all(data_home.join("applications")).unwrap();
    fs::create_dir_all(&config_home).unwrap();
    fs::create_dir_all(&sys_data).unwrap();
    fs::create_dir_all(&sys_config).unwrap();
    let _env = XdgEnvGuard::isolate(&data_home, &config_home, &sys_data, &sys_config);

    let marker = root.join("launched_arg.txt");
    let script = root.join("open_script.sh");
    write_executable_script(&script, &marker);

    fs::write(
        data_home.join("mime/globs2"),
        "50:application/x-kwatirun:*.kwatirun\n",
    )
    .unwrap();
    fs::write(
        data_home.join("applications/good.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Good Runner\nExec={} %f\nMimeType=application/x-kwatirun;\n",
            script.display()
        ),
    )
    .unwrap();
    fs::write(
        data_home.join("applications/broken.desktop"),
        "[Desktop Entry]\nType=Application\nName=Broken Runner\nExec=/nonexistent/kwati/missing_exec_999 %f\nMimeType=application/x-kwatirun;\n",
    )
    .unwrap();
    fs::write(
        config_home.join("mimeapps.list"),
        "[Default Applications]\napplication/x-kwatirun=good.desktop;\n",
    )
    .unwrap();

    let target = root.join("sample.kwatirun");
    fs::write(&target, "payload").unwrap();

    let (main, cx) = open_window(cx, &root);
    let ix = row_of(&main, cx, "sample.kwatirun");
    let target_pos = row_center(&main, cx, ix);

    // Launch valid .desktop script via "Open With" submenu (index 2: Open, Rename, Open With).
    right_click(cx, target_pos);
    let open_with_row = menu_item_bounds(cx, "context-menu-item-2");
    cx.simulate_mouse_move(open_with_row, None, Modifiers::none());
    cx.run_until_parked();
    let good_subitem = menu_item_bounds(cx, "context-menu-subitem-0");
    left_click(cx, good_subitem, Modifiers::none());

    // Wait briefly for the spawned script to write the marker file.
    let mut recorded = String::new();
    for _ in 0..100 {
        if let Ok(content) = fs::read_to_string(&marker) {
            if !content.is_empty() {
                recorded = content;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(recorded, target.display().to_string());

    // Launch broken .desktop entry and verify error surfaces in state.status_message.
    right_click(cx, target_pos);
    let open_with_row = menu_item_bounds(cx, "context-menu-item-2");
    cx.simulate_mouse_move(open_with_row, None, Modifiers::none());
    cx.run_until_parked();
    let broken_subitem = menu_item_bounds(cx, "context-menu-subitem-1");
    left_click(cx, broken_subitem, Modifiers::none());
    cx.run_until_parked();

    let status = main.read_with(cx, |main, cx| {
        main.app_state.read(cx).status_message.clone()
    });
    let status = status.expect("expected error in status_message");
    assert!(
        status.contains("Broken Runner") && status.contains("not found"),
        "unexpected status message: {status}"
    );

    let _ = fs::remove_dir_all(&root);
}
