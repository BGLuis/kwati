//! Tests for Details, CompactList, and Grid view modes, shortcuts, virtualization,
//! 2D keyboard navigation, mouse/rubber-band selection, and status bar metadata.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
};
use kwati_core::{
    Entry, Location, MotionPreference, Settings, SortColumn, SortDirection, ViewMode,
};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::AppState;
use kwati_gui::tokens::{size as token_size, Fonts, BODY_SIZE};
use kwati_gui::views::list_view::{
    fit_two_line_label, grid_columns_for_width, COMPACT_ROW_HEIGHT, GRID_GAP, GRID_PAD_X,
    GRID_ROW_HEIGHT, GRID_TILE_HEIGHT, GRID_TILE_WIDTH,
};
use kwati_gui::views::settings_view::SettingsView;
use kwati_gui::views::{format_status_selection_metadata, MainView};

fn fixture(name: &str, count: usize) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "kwati_test_viewmodes_{name}_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("a_sub")).unwrap();
    for i in 0..count {
        fs::write(
            root.join(format!("file_{:02}.txt", i)),
            format!("payload_{i}"),
        )
        .unwrap();
    }
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
    cx.run_until_parked();
}

fn right_click_at(cx: &mut VisualTestContext, pos: Point<Pixels>) {
    cx.simulate_mouse_move(pos, None, Modifiers::none());
    cx.simulate_mouse_down(pos, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(pos, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
}

fn grid_tile_center(
    main: &Entity<MainView>,
    cx: &mut VisualTestContext,
    ix: usize,
) -> Point<Pixels> {
    main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        let cols = list.grid_columns().max(1);
        let row = ix / cols;
        let col = ix % cols;
        let scroll = list.scroll_handle.0.borrow();
        let bounds = scroll.base_handle.bounds();
        let offset = scroll.base_handle.offset();
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "grid has no painted bounds: {bounds:?}"
        );
        point(
            bounds.left()
                + GRID_PAD_X
                + (GRID_TILE_WIDTH + GRID_GAP) * (col as f32)
                + GRID_TILE_WIDTH / 2.0,
            bounds.top()
                + GRID_ROW_HEIGHT * (row as f32)
                + GRID_GAP / 2.0
                + GRID_TILE_HEIGHT / 2.0
                + offset.y,
        )
    })
}

fn compact_row_center(
    main: &Entity<MainView>,
    cx: &mut VisualTestContext,
    ix: usize,
) -> Point<Pixels> {
    main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        let scroll = list.scroll_handle.0.borrow();
        let bounds = scroll.base_handle.bounds();
        let offset = scroll.base_handle.offset();
        point(
            bounds.left() + px(80.),
            bounds.top() + COMPACT_ROW_HEIGHT * (ix as f32) + COMPACT_ROW_HEIGHT / 2.0 + offset.y,
        )
    })
}

#[test]
fn grid_columns_for_width_calculations() {
    assert_eq!(grid_columns_for_width(px(0.)), 4);
    assert_eq!(grid_columns_for_width(px(100.)), 1);
    assert_eq!(grid_columns_for_width(px(200.)), 1);
    // 480 px minus 24 px padding = 456 px -> 4 * 104 + 3 * 8 = 440 px <= 456 px
    assert_eq!(grid_columns_for_width(px(480.)), 4);
    // 524 px minus 24 px padding = 500 px -> 4 columns
    assert_eq!(grid_columns_for_width(px(524.)), 4);
    // 800 px minus 24 px padding = 776 px -> 7 * 104 + 6 * 8 = 776 px -> 7 columns
    assert_eq!(grid_columns_for_width(px(800.)), 7);
}

#[test]
fn status_bar_metadata_when_size_and_date_columns_hidden() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(86400 * 365 + 3600 * 14);
    let dir = Entry::new(OsString::from("docs"), PathBuf::from("/tmp/docs"), true)
        .with_modified(modified);
    let file = Entry::new(
        OsString::from("report.txt"),
        PathBuf::from("/tmp/report.txt"),
        false,
    )
    .with_size(2048)
    .with_modified(modified);
    state.set_entries(vec![dir.clone(), file.clone()]);

    // Details mode already shows Size and Modified columns, so no extra metadata is needed.
    assert_eq!(state.view_mode, ViewMode::Details);
    assert_eq!(format_status_selection_metadata(&state), "");

    // Grid and CompactList surface sort state when nothing is selected, and entry details on single selection.
    for mode in [ViewMode::Grid, ViewMode::CompactList] {
        state.set_view_mode(mode);
        state.clear_selection();
        assert_eq!(format_status_selection_metadata(&state), "Sort: Name ▲");

        state.select_index(1);
        assert_eq!(
            format_status_selection_metadata(&state),
            "report.txt · 2.0 KB · 1971-01-01 14:00"
        );

        state.select_index(0);
        assert_eq!(
            format_status_selection_metadata(&state),
            "docs · Folder · 1971-01-01 14:00"
        );

        state.select_all_entries();
        assert_eq!(format_status_selection_metadata(&state), "Sort: Name ▲");
    }

    state.clear_selection();
    state.set_sort(SortColumn::Size, SortDirection::Descending);
    state.show_hidden = true;
    assert_eq!(
        format_status_selection_metadata(&state),
        "Sort: Size ▼ · Hidden"
    );
}

#[gpui::test]
fn settings_view_mode_persists_and_loads(cx: &mut TestAppContext) {
    let dir = fixture("persist", 2);
    let settings_file = dir.join("settings.toml");

    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let settings_view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    settings_view.update(cx, |view, cx| {
        view.set_view_mode(ViewMode::Grid, cx).unwrap();
    });
    state.read_with(cx, |s, _| {
        assert_eq!(s.view_mode, ViewMode::Grid);
        assert_eq!(s.settings.view_mode, ViewMode::Grid);
    });

    let (disk, warnings) = Settings::load_from(&settings_file).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(disk.view_mode, ViewMode::Grid);

    // Reloading into a fresh AppState restores the persisted view mode.
    let mut restored = AppState::new(Location::Local(dir.clone()));
    restored.load_settings_from(&settings_file).unwrap();
    assert_eq!(restored.view_mode, ViewMode::Grid);

    settings_view.update(cx, |view, cx| {
        view.reset_to_defaults(cx).unwrap();
    });
    state.read_with(cx, |s, _| {
        assert_eq!(s.view_mode, ViewMode::Details);
        assert_eq!(s.settings.view_mode, ViewMode::Details);
    });

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn shortcuts_and_header_switcher_switch_view_modes(cx: &mut TestAppContext) {
    let dir = fixture("shortcuts", 6);
    let (main, cx) = open_window(cx, &dir);

    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Details);
        assert_eq!(main.list.read(cx).row_height, token_size::ROW);
    });

    // Ctrl+2 -> Grid
    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Grid);
        assert_eq!(main.list.read(cx).row_height, GRID_ROW_HEIGHT);
        assert!(main.list.read(cx).grid_columns() >= 1);
    });

    // Ctrl+3 -> CompactList
    cx.simulate_keystrokes("ctrl-3");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::CompactList);
        assert_eq!(main.list.read(cx).row_height, COMPACT_ROW_HEIGHT);
    });

    // Ctrl+1 -> Details
    cx.simulate_keystrokes("ctrl-1");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Details);
        assert_eq!(main.list.read(cx).row_height, token_size::ROW);
    });

    // Clicking HeaderView switcher segments updates AppState and ListView
    let grid_btn = cx
        .debug_bounds("#view-mode-grid")
        .expect("#view-mode-grid bounds");
    click_at(cx, grid_btn.center(), Modifiers::none());
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Grid);
        assert_eq!(main.list.read(cx).row_height, GRID_ROW_HEIGHT);
    });

    let compact_btn = cx
        .debug_bounds("#view-mode-compact")
        .expect("#view-mode-compact bounds");
    click_at(cx, compact_btn.center(), Modifiers::none());
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::CompactList);
        assert_eq!(main.list.read(cx).row_height, COMPACT_ROW_HEIGHT);
    });

    let details_btn = cx
        .debug_bounds("#view-mode-details")
        .expect("#view-mode-details bounds");
    click_at(cx, details_btn.center(), Modifiers::none());
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Details);
        assert_eq!(main.list.read(cx).row_height, token_size::ROW);
    });

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn grid_mode_keyboard_navigation_2d_arrows_and_shift_selection(cx: &mut TestAppContext) {
    // 1 folder ("a_sub") + 9 files ("file_00.txt"..="file_08.txt") = 10 entries.
    let dir = fixture("grid_keynav", 9);
    let (main, cx) = open_window(cx, &dir);

    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();

    let cols = main.read_with(cx, |main, cx| main.list.read(cx).grid_columns());
    assert_eq!(
        cols, 4,
        "1024px window with default sidebar & preview yields 4 grid columns"
    );

    // Down from no selection selects index 0 ("a_sub")
    cx.simulate_keystrokes("down");
    assert_eq!(selected_names(&main, cx), ["a_sub"]);

    // Up on row 0 stays on row 0
    cx.simulate_keystrokes("up");
    assert_eq!(selected_names(&main, cx), ["a_sub"]);

    // Right moves across columns
    cx.simulate_keystrokes("right");
    assert_eq!(selected_names(&main, cx), ["file_00.txt"]);
    cx.simulate_keystrokes("right right");
    assert_eq!(selected_names(&main, cx), ["file_02.txt"]); // index 3

    // Down from index 3 moves to index 3 + 4 = 7 ("file_06.txt")
    cx.simulate_keystrokes("down");
    assert_eq!(selected_names(&main, cx), ["file_06.txt"]);

    // Down from index 7 (row 1, col 3) to shorter row 2 [8, 9] clamps to index 9 ("file_08.txt")
    cx.simulate_keystrokes("down");
    assert_eq!(selected_names(&main, cx), ["file_08.txt"]);

    // Down on last row stays on index 9
    cx.simulate_keystrokes("down");
    assert_eq!(selected_names(&main, cx), ["file_08.txt"]);

    // Left moves back to index 8 ("file_07.txt")
    cx.simulate_keystrokes("left");
    assert_eq!(selected_names(&main, cx), ["file_07.txt"]);

    // Up moves from index 8 to index 4 ("file_03.txt")
    cx.simulate_keystrokes("up");
    assert_eq!(selected_names(&main, cx), ["file_03.txt"]);

    // Home -> index 0, Shift+Right -> [0, 1], Shift+Down -> [0..=5]
    cx.simulate_keystrokes("home");
    assert_eq!(selected_names(&main, cx), ["a_sub"]);
    cx.simulate_keystrokes("shift-right");
    assert_eq!(selected_names(&main, cx), ["a_sub", "file_00.txt"]);
    cx.simulate_keystrokes("shift-down");
    assert_eq!(
        selected_names(&main, cx),
        [
            "a_sub",
            "file_00.txt",
            "file_01.txt",
            "file_02.txt",
            "file_03.txt",
            "file_04.txt"
        ]
    );

    // Home then Enter enters "a_sub", Backspace returns to root
    cx.simulate_keystrokes("home enter");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(
            main.app_state.read(cx).current_location,
            Location::Local(dir.join("a_sub"))
        );
    });
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(
            main.app_state.read(cx).current_location,
            Location::Local(dir.clone())
        );
    });

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn grid_mode_mouse_multi_select_rubber_band_context_menu_and_double_click(cx: &mut TestAppContext) {
    let dir = fixture("grid_mouse", 7); // 0=a_sub, 1..=7=file_00..file_06
    let (main, cx) = open_window(cx, &dir);

    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();

    let t1 = grid_tile_center(&main, cx, 1);
    let t2 = grid_tile_center(&main, cx, 2);
    let t5 = grid_tile_center(&main, cx, 5);

    // Click tile 1 -> selects file_00.txt
    click_at(cx, t1, Modifiers::none());
    assert_eq!(selected_names(&main, cx), ["file_00.txt"]);

    // Ctrl-click tile 5 (row 1, col 1) -> toggles both
    click_at(cx, t5, Modifiers::secondary_key());
    assert_eq!(selected_names(&main, cx), ["file_00.txt", "file_04.txt"]);

    // Shift-click from tile 1 -> range [1..=2]
    click_at(cx, t1, Modifiers::none());
    click_at(
        cx,
        t2,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(selected_names(&main, cx), ["file_00.txt", "file_01.txt"]);

    // Right-click tile 5 -> selects only tile 5 and opens context menu
    right_click_at(cx, t5);
    main.read_with(cx, |main, cx| {
        assert!(main.context_menu.read(cx).is_open());
    });
    assert_eq!(selected_names(&main, cx), ["file_04.txt"]);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Rubber-band drag from empty space below row 1 up through row 1 cols 1..=2 (indices 5, 6)
    let list_bounds = main.read_with(cx, |main, cx| {
        main.list
            .read(cx)
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
    });
    let t6 = grid_tile_center(&main, cx, 6);
    let start = point(t6.x, list_bounds.top() + GRID_ROW_HEIGHT * 2.0 + px(24.));
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(point(t5.x, t5.y), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(point(t5.x, t5.y), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(selected_names(&main, cx), ["file_04.txt", "file_05.txt"]);

    // Double-click folder tile 0 ("a_sub") navigates into it
    let t0 = grid_tile_center(&main, cx, 0);
    cx.simulate_mouse_move(t0, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        position: t0,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: t0,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(
            main.app_state.read(cx).current_location,
            Location::Local(dir.join("a_sub"))
        );
    });

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn compact_list_mode_navigation_selection_and_reduced_motion(cx: &mut TestAppContext) {
    let dir = fixture("compact", 5);
    let (main, cx) = open_window(cx, &dir);

    cx.simulate_keystrokes("ctrl-3");
    cx.run_until_parked();

    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::CompactList);
        assert_eq!(main.list.read(cx).row_height, COMPACT_ROW_HEIGHT);
    });

    let r1 = compact_row_center(&main, cx, 1);
    let r3 = compact_row_center(&main, cx, 3);
    click_at(cx, r1, Modifiers::none());
    assert_eq!(selected_names(&main, cx), ["file_00.txt"]);

    click_at(cx, r3, Modifiers::secondary_key());
    assert_eq!(selected_names(&main, cx), ["file_00.txt", "file_02.txt"]);

    // Reduced motion renders cleanly in CompactList and Grid without errors
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            state
                .update_settings(|s| {
                    s.motion_preference = MotionPreference::Reduced;
                    Ok(())
                })
                .unwrap();
        });
    });
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("ctrl-2");
    cx.run_until_parked();
    main.read_with(cx, |main, cx| {
        assert_eq!(main.app_state.read(cx).view_mode, ViewMode::Grid);
    });

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn grid_mode_virtualizes_100k_entries(cx: &mut TestAppContext) {
    let dir = fixture("grid_100k", 0);
    let (main, cx) = open_window(cx, &dir);

    let entries: Vec<Entry> = (0..100_000)
        .map(|i| {
            Entry::new(
                OsString::from(format!("entry_{:06}.txt", i)),
                dir.join(format!("entry_{:06}.txt", i)),
                i % 10 == 0,
            )
            .with_size(i as u64)
        })
        .collect();

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.set_view_mode(ViewMode::Grid);
            state.set_entries(entries);
            cx.notify();
        });
    });
    cx.run_until_parked();

    let (cols, total_rows) = main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        (list.grid_columns(), list.scrollbar_state.total_items)
    });
    assert_eq!(total_rows, 100_000usize.div_ceil(cols));

    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    assert_eq!(selected_names(&main, cx), ["entry_099999.txt"]);

    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    assert_eq!(selected_names(&main, cx), ["entry_000000.txt"]);

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn grid_two_line_labels_fit_within_tile_width_and_ellipsize_on_overflow(cx: &mut TestAppContext) {
    let dir = fixture("grid_labels", 0);
    let (_main, cx) = open_window(cx, &dir);

    let max_width = px(92.);
    let window_handle = cx.windows()[0];
    cx.update_window(window_handle, |_, window, cx| {
        let font = gpui::font(Fonts::get(cx).sans);
        let measure = |s: &str| {
            window
                .text_system()
                .shape_line(
                    s.to_string().into(),
                    BODY_SIZE,
                    &[gpui::TextRun {
                        len: s.len(),
                        font: font.clone(),
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width
        };

        // Short name stays on one line untouched.
        let (l1, l2) = fit_two_line_label("notes.txt", max_width, font.clone(), BODY_SIZE, window);
        assert_eq!(l1, "notes.txt");
        assert_eq!(l2, None);

        // Multi-word name where line 1 wraps early and line 2 still overflows gets truncated with "…".
        let (l1, l2) = fit_two_line_label(
            "My VeryLongDocumentName.pdf",
            max_width,
            font.clone(),
            BODY_SIZE,
            window,
        );
        let l2 = l2.expect("should wrap onto second line");
        assert!(measure(&l1) <= max_width, "line 1 overflowed: {l1:?}");
        assert!(measure(&l2) <= max_width, "line 2 overflowed: {l2:?}");
        assert!(l2.ends_with('…'), "expected ellipsis on line 2, got {l2:?}");

        // Single long token without spaces also fits both lines and ends with "…".
        let (l1, l2) = fit_two_line_label(
            "relatorio_financeiro_anual_2026_final.pdf",
            max_width,
            font.clone(),
            BODY_SIZE,
            window,
        );
        let l2 = l2.expect("long token should wrap onto second line");
        assert!(measure(&l1) <= max_width, "line 1 overflowed: {l1:?}");
        assert!(measure(&l2) <= max_width, "line 2 overflowed: {l2:?}");
        assert!(l2.ends_with('…'), "expected ellipsis on line 2, got {l2:?}");
    })
    .unwrap();

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn scrollbar_follows_scroll_and_supports_track_click_and_drag(cx: &mut TestAppContext) {
    let dir = fixture("scrollbar_sync", 120);
    let (main, cx) = open_window(cx, &dir);

    let (list_bounds, track_bounds, initial_offset, initial_ratio) =
        main.read_with(cx, |main, cx| {
            let list = main.list.read(cx);
            (
                list.scroll_handle.0.borrow().base_handle.bounds(),
                list.scrollbar_drag.borrow().track_bounds,
                list.scrollbar_state.current_offset,
                list.scrollbar_state.scroll_ratio(),
            )
        });
    assert!(track_bounds.size.height > px(0.));
    assert_eq!(initial_offset, 0);
    assert_eq!(initial_ratio, 0.0);

    // 1. Mouse wheel scroll down -> scrollbar_state follows page scroll
    let list_center = list_bounds.center();
    cx.simulate_mouse_move(list_center, None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: list_center,
        delta: ScrollDelta::Pixels(point(px(0.), px(-420.))),
        ..Default::default()
    });
    cx.run_until_parked();

    let (wheel_scroll_y, wheel_offset, wheel_ratio) = main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        (
            list.scroll_handle.0.borrow().base_handle.offset().y,
            list.scrollbar_state.current_offset,
            list.scrollbar_state.scroll_ratio(),
        )
    });
    assert!(wheel_scroll_y < px(0.), "wheel should scroll list down");
    assert!(
        wheel_offset > 0 && wheel_ratio > 0.0,
        "scrollbar must follow wheel scroll: offset={wheel_offset}, ratio={wheel_ratio}"
    );

    // 2. Keyboard End / Home -> scrollbar_state follows keyboard scroll
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    let end_ratio = main.read_with(cx, |main, cx| {
        main.list.read(cx).scrollbar_state.scroll_ratio()
    });
    assert!(
        end_ratio > 0.9,
        "scrollbar must reach near bottom on End key, got {end_ratio}"
    );

    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let home_offset = main.read_with(cx, |main, cx| {
        main.list.read(cx).scrollbar_state.current_offset
    });
    assert_eq!(home_offset, 0, "scrollbar must return to top on Home key");

    // 3. Dragging the thumb down while cursor moves horizontally outside the 10px track
    let track_x = track_bounds.center().x;
    let thumb_grab = point(track_x, track_bounds.top() + px(8.));
    cx.simulate_mouse_move(thumb_grab, None, Modifiers::none());
    cx.simulate_mouse_down(thumb_grab, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(track_x - px(48.), track_bounds.top() + px(180.)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(track_x - px(48.), track_bounds.top() + px(180.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    let (drag_scroll_y, drag_offset, still_dragging) = main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        (
            list.scroll_handle.0.borrow().base_handle.offset().y,
            list.scrollbar_state.current_offset,
            list.scrollbar_drag.borrow().is_dragging,
        )
    });
    assert!(drag_scroll_y < px(0.), "thumb drag must scroll list down");
    assert!(drag_offset > 0, "scrollbar offset must update on drag");
    assert!(!still_dragging, "mouse up must end scrollbar drag");

    // 4. Clicking on the lower track jumps near the bottom
    let lower_track = point(track_x, track_bounds.bottom() - px(24.));
    cx.simulate_mouse_move(lower_track, None, Modifiers::none());
    cx.simulate_mouse_down(lower_track, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(lower_track, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    let jump_ratio = main.read_with(cx, |main, cx| {
        main.list.read(cx).scrollbar_state.scroll_ratio()
    });
    assert!(
        jump_ratio > 0.7,
        "clicking lower track must jump near bottom, got {jump_ratio}"
    );

    let _ = fs::remove_dir_all(&dir);
}
