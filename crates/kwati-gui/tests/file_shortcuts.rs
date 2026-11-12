//! Drives the real window with key presses, so these fail when a shortcut stops reaching the
//! operation behind it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use kwati_core::{volume_kind, ConflictPolicy, Location, VolumeKind};
use kwati_gui::actions::operations::{
    clear_clipboard, execute_copy, execute_undo, start_transfer, Transfer,
};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::AppState;
use kwati_gui::views::{DialogKind, MainView};

/// The clipboard is one per process, so tests that use it must not overlap.
fn clipboard() -> MutexGuard<'static, ()> {
    static CLIPBOARD_IN_USE: Mutex<()> = Mutex::new(());
    let guard = CLIPBOARD_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
    clear_clipboard();
    guard
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("kwati_test_keys_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("notes.txt"), "notes").unwrap();
    fs::write(root.join("other.txt"), "other").unwrap();
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
    cx.run_until_parked();
    (main, cx)
}

fn select(main: &Entity<MainView>, cx: &mut VisualTestContext, name: &str) {
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            let index = state
                .entries
                .iter()
                .position(|entry| entry.name() == name)
                .unwrap_or_else(|| panic!("{name} is not listed"));
            state.select_index(index);
        })
    });
}

fn go_to(main: &Entity<MainView>, cx: &mut VisualTestContext, dir: &Path) {
    let location = Location::Local(dir.to_path_buf());
    main.update(cx, |main, cx| {
        main.app_state
            .update(cx, |state, _| state.navigate_to(location))
    });
    cx.run_until_parked();
}

fn dialog(main: &Entity<MainView>, cx: &mut VisualTestContext) -> DialogKind {
    main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone())
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

fn status(main: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
    main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .status_message
            .clone()
            .unwrap_or_default()
    })
}

#[gpui::test]
fn f2_renames_the_selected_entry_from_the_keyboard(cx: &mut TestAppContext) {
    let root = fixture("rename");
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("f2");
    assert!(matches!(dialog(&main, cx), DialogKind::Rename { .. }));

    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("renamed.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert_eq!(dialog(&main, cx), DialogKind::None);
    assert!(root.join("renamed.txt").exists());
    assert!(!root.join("notes.txt").exists());
    assert_eq!(selected_names(&main, cx), ["renamed.txt"]);
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn renaming_onto_an_existing_name_reports_and_changes_nothing(cx: &mut TestAppContext) {
    let root = fixture("rename_refused");
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("f2 ctrl-a");
    cx.simulate_input("other.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let DialogKind::Rename { error, .. } = dialog(&main, cx) else {
        panic!("the rename dialog stays open to show the error");
    };
    assert!(error.is_some());
    assert_eq!(fs::read_to_string(root.join("notes.txt")).unwrap(), "notes");
    assert_eq!(fs::read_to_string(root.join("other.txt")).unwrap(), "other");

    // Escape closes it and the list takes the keyboard back.
    cx.simulate_keystrokes("escape");
    assert_eq!(dialog(&main, cx), DialogKind::None);
    cx.simulate_keystrokes("f2");
    assert!(matches!(dialog(&main, cx), DialogKind::Rename { .. }));
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn ctrl_shift_n_creates_and_selects_a_folder(cx: &mut TestAppContext) {
    let root = fixture("new_folder");
    let (main, cx) = open_window(cx, &root);

    cx.simulate_keystrokes("ctrl-shift-n");
    assert!(matches!(dialog(&main, cx), DialogKind::NewFolder { .. }));

    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("made");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert!(root.join("made").is_dir());
    assert_eq!(selected_names(&main, cx), ["made"]);
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn copy_and_paste_runs_as_a_job_and_keeps_the_source(cx: &mut TestAppContext) {
    let root = fixture("copy_paste");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("ctrl-c");
    go_to(&main, cx, &root.join("sub"));
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();

    assert_eq!(
        fs::read_to_string(root.join("sub/notes.txt")).unwrap(),
        "notes"
    );
    assert!(root.join("notes.txt").exists());
    let running = main.read_with(cx, |main, cx| main.app_state.read(cx).active_jobs.len());
    assert_eq!(running, 0);
    assert_eq!(selected_names(&main, cx), ["notes.txt"]);
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn cut_and_paste_moves_the_entry(cx: &mut TestAppContext) {
    let root = fixture("cut_paste");
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("ctrl-x");
    go_to(&main, cx, &root.join("sub"));
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();

    assert!(root.join("sub/notes.txt").exists());
    assert!(!root.join("notes.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn pasting_onto_an_existing_name_asks_for_a_policy_first(cx: &mut TestAppContext) {
    let root = fixture("paste_conflict");
    fs::write(root.join("sub/notes.txt"), "already here").unwrap();
    let _clipboard = clipboard();
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("ctrl-c");
    go_to(&main, cx, &root.join("sub"));
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();
    assert!(matches!(
        dialog(&main, cx),
        DialogKind::ConflictResolution { .. }
    ));

    // Enter picks no policy for the user.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        fs::read_to_string(root.join("sub/notes.txt")).unwrap(),
        "already here"
    );

    main.update(cx, |main, cx| {
        main.dialogs.update(cx, |dialogs, cx| {
            dialogs.resolve_conflict(ConflictPolicy::KeepBoth, cx)
        })
    });
    cx.run_until_parked();

    assert_eq!(
        fs::read_to_string(root.join("sub/notes.txt")).unwrap(),
        "already here"
    );
    assert_eq!(fs::read_dir(root.join("sub")).unwrap().count(), 2);
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn delete_asks_first_and_escape_leaves_the_file(cx: &mut TestAppContext) {
    let root = fixture("delete_prompt");
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");

    cx.simulate_keystrokes("delete");
    let asked = dialog(&main, cx);
    if volume_kind(&root) == VolumeKind::Local {
        assert!(matches!(asked, DialogKind::ConfirmTrash { .. }));
    } else {
        // No trash on this volume: only the permanent-deletion prompt is honest, and Enter
        // must not confirm it.
        assert!(matches!(asked, DialogKind::ConfirmDelete { .. }));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(matches!(
            dialog(&main, cx),
            DialogKind::ConfirmDelete { .. }
        ));
    }

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(dialog(&main, cx), DialogKind::None);
    assert!(root.join("notes.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn ctrl_z_undoes_the_last_rename_and_says_so(cx: &mut TestAppContext) {
    let root = fixture("undo");
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");
    cx.simulate_keystrokes("f2 ctrl-a");
    cx.simulate_input("renamed.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(root.join("renamed.txt").exists());

    cx.simulate_keystrokes("ctrl-z");
    cx.run_until_parked();

    assert!(root.join("notes.txt").exists());
    assert!(!root.join("renamed.txt").exists());
    assert!(status(&main, cx).contains("Undid rename"));

    // Nothing is left to undo: the refusal is reported and nothing changes.
    cx.simulate_keystrokes("ctrl-z");
    assert!(status(&main, cx).contains("Nothing to undo"));
    assert!(root.join("notes.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn file_shortcuts_are_ignored_while_typing_in_the_address_bar(cx: &mut TestAppContext) {
    let root = fixture("address_bar");
    let (main, cx) = open_window(cx, &root);
    select(&main, cx, "notes.txt");
    main.update_in(cx, |main, window, cx| {
        main.header
            .read(cx)
            .address_input
            .read(cx)
            .focus_handle
            .focus(window)
    });

    cx.simulate_keystrokes("f2");
    assert_eq!(dialog(&main, cx), DialogKind::None);
    cx.simulate_keystrokes("ctrl-shift-n");
    assert_eq!(dialog(&main, cx), DialogKind::None);
    let _ = fs::remove_dir_all(&root);
}

#[gpui::test]
fn a_transfer_is_queued_as_a_job_instead_of_blocking_the_window(cx: &mut TestAppContext) {
    let root = fixture("queued_job");
    let (main, cx) = open_window(cx, &root);
    let transfer = Transfer::copy(
        vec![root.join("notes.txt")],
        root.join("sub"),
        ConflictPolicy::Skip,
    );

    let running = main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            start_transfer(state, transfer);
            state.active_jobs.len()
        })
    });
    assert_eq!(running, 1);
    assert!(!root.join("sub/notes.txt").exists());

    cx.run_until_parked();

    assert!(root.join("sub/notes.txt").exists());
    let running = main.read_with(cx, |main, cx| main.app_state.read(cx).active_jobs.len());
    assert_eq!(running, 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn undoing_a_copy_never_deletes_a_file_that_was_already_there() {
    let root = fixture("undo_copy_existing");
    fs::write(root.join("sub/notes.txt"), "already here").unwrap();
    let mut state = AppState::new(Location::Local(root.join("sub")));

    let sources = [root.join("notes.txt"), root.join("other.txt")];
    execute_copy(
        &mut state,
        &sources,
        &root.join("sub"),
        ConflictPolicy::Skip,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(root.join("sub/other.txt").exists());

    execute_undo(&mut state).unwrap();

    assert!(!root.join("sub/other.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("sub/notes.txt")).unwrap(),
        "already here"
    );
    let _ = fs::remove_dir_all(&root);
}
