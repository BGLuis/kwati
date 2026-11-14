//! Multi-selection, rubber-band, status size, and multi-item operations (#54).

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
use kwati_gui::views::{format_selection_summary, DialogKind, MainView};

fn clipboard() -> MutexGuard<'static, ()> {
    static CLIPBOARD_IN_USE: Mutex<()> = Mutex::new(());
    let guard = CLIPBOARD_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
    clear_clipboard();
    guard
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("kwati_test_multi_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("alpha.txt"), "aaaa").unwrap(); // 4 bytes
    fs::write(root.join("bravo.txt"), "bbbbbb").unwrap(); // 6 bytes
    fs::write(root.join("charlie.txt"), "cccccccccc").unwrap(); // 10 bytes
    fs::write(root.join("delta.txt"), "dd").unwrap(); // 2 bytes
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

fn click_at(cx: &mut VisualTestContext, pos: Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_mouse_move(pos, None, modifiers);
    cx.simulate_mouse_down(pos, MouseButton::Left, modifiers);
    cx.simulate_mouse_up(pos, MouseButton::Left, modifiers);
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
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "list has no painted bounds: {bounds:?}"
        );
        point(
            bounds.left() + px(80.),
            bounds.top() + row_h * (ix as f32) + row_h / 2.0 + offset.y,
        )
    })
}

fn list_bounds(main: &Entity<MainView>, cx: &mut VisualTestContext) -> gpui::Bounds<Pixels> {
    main.read_with(cx, |main, cx| {
        main.list
            .read(cx)
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
    })
}

#[gpui::test]
fn ctrl_a_selects_all_and_escape_clears_selection(cx: &mut TestAppContext) {
    let root = fixture("ctrl_a_escape");
    let (main, cx) = open_window(cx, &root);

    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(
        selected_names(&main, cx),
        ["sub", "alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"]
    );

    cx.simulate_keystrokes("escape");
    assert!(selected_names(&main, cx).is_empty());

    // Ctrl-A inside the address bar selects text, not files.
    main.update_in(cx, |main, window, cx| {
        main.header
            .read(cx)
            .address_input
            .read(cx)
            .focus_handle
            .focus(window);
    });
    cx.simulate_keystrokes("ctrl-a");
    assert!(selected_names(&main, cx).is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn ctrl_click_toggles_and_shift_click_selects_range_from_anchor(cx: &mut TestAppContext) {
    let root = fixture("modifiers");
    let (main, cx) = open_window(cx, &root);

    let secondary = Modifiers::secondary_key();
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };

    // Entries: 0=sub, 1=alpha.txt, 2=bravo.txt, 3=charlie.txt, 4=delta.txt
    let p1 = row_center(&main, cx, 1);
    let p3 = row_center(&main, cx, 3);
    let p4 = row_center(&main, cx, 4);

    // Click row 1 (alpha.txt) -> anchor=1
    click_at(cx, p1, Modifiers::none());
    assert_eq!(selected_names(&main, cx), ["alpha.txt"]);

    // Ctrl/Cmd-click row 3 (charlie.txt) -> toggles on, anchor moves to 3
    click_at(cx, p3, secondary);
    assert_eq!(selected_names(&main, cx), ["alpha.txt", "charlie.txt"]);

    // Ctrl/Cmd-click row 1 again -> toggles off
    click_at(cx, p1, secondary);
    assert_eq!(selected_names(&main, cx), ["charlie.txt"]);

    // Click row 1 (alpha.txt) to set anchor=1, then Shift-click row 3 -> range [1..=3]
    click_at(cx, p1, Modifiers::none());
    click_at(cx, p3, shift);
    assert_eq!(
        selected_names(&main, cx),
        ["alpha.txt", "bravo.txt", "charlie.txt"]
    );

    // Shift-click row 4 while anchor stays at row 1 -> range [1..=4]
    click_at(cx, p4, shift);
    assert_eq!(
        selected_names(&main, cx),
        ["alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"]
    );

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn rubber_band_drag_selects_exact_touched_rows(cx: &mut TestAppContext) {
    let root = fixture("rubber_band");
    let (main, cx) = open_window(cx, &root);

    let bounds = list_bounds(&main, cx);
    let r2 = row_center(&main, cx, 2);
    let r3 = row_center(&main, cx, 3);
    // Start in empty space below row 4 (index 4 is the last row out of 5).
    let empty_y = row_center(&main, cx, 4).y + px(40.);
    let start = point(bounds.left() + px(40.), empty_y);
    let drag_end = point(bounds.left() + px(220.), r2.y);

    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(start.x + px(6.), r3.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(drag_end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(drag_end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    assert_eq!(
        selected_names(&main, cx),
        ["bravo.txt", "charlie.txt", "delta.txt"]
    );

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn three_selected_entries_report_count_and_total_size(cx: &mut TestAppContext) {
    let root = fixture("status_size");
    let (main, cx) = open_window(cx, &root);

    let p1 = row_center(&main, cx, 1);
    let p3 = row_center(&main, cx, 3);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };

    click_at(cx, p1, Modifiers::none());
    click_at(cx, p3, shift);

    let (count, total_size) = main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        (state.selected_count(), state.selected_total_size())
    });
    assert_eq!(count, 3);
    assert_eq!(total_size, 4 + 6 + 10);
    assert_eq!(
        format_selection_summary(count, total_size),
        "3 selected (20 B)"
    );

    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn three_selected_entries_copy_paste_and_prompt_once_on_delete(cx: &mut TestAppContext) {
    let root = fixture("multi_ops");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);

    let p1 = row_center(&main, cx, 1);
    let p3 = row_center(&main, cx, 3);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };

    click_at(cx, p1, Modifiers::none());
    click_at(cx, p3, shift);

    // Copy 3 items and paste into sub/
    cx.simulate_keystrokes("ctrl-c");
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            state.navigate_to(Location::Local(root.join("sub")))
        })
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();

    assert!(root.join("sub/alpha.txt").exists());
    assert!(root.join("sub/bravo.txt").exists());
    assert!(root.join("sub/charlie.txt").exists());

    // Select all 3 copied files inside sub/ and press Delete -> asks once with all 3 targets.
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_keystrokes("delete");
    let asked = main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    match asked {
        DialogKind::ConfirmTrash { locations } | DialogKind::ConfirmDelete { locations } => {
            assert_eq!(locations.len(), 3);
        }
        other => panic!("expected single confirmation dialog for 3 items, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&root);
}
