//! Context menu for entries and empty space (#55).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, MouseButton, Pixels, Point,
    TestAppContext, VisualTestContext,
};
use kwati_core::Location;
use kwati_gui::actions::operations::clear_clipboard;
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
            .selection
            .selected_in_order(&state.entries)
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
                ContextMenuEntry::Separator => None,
            })
            .collect()
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
            ContextMenuAction::Refresh,
        ]
    );

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
