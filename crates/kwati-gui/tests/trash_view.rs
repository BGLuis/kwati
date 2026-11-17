use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{px, size, AppContext as _, Entity, TestAppContext, VisualTestContext};
use kwati_core::{
    is_trash_browsing_supported, list_trash, purge_trash_items, trash, Confirmed, Location,
    SortDirection, TrashItem,
};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::{AppState, TrashSortColumn};
use kwati_gui::views::dialogs::confirm_empty_trash_message;
use kwati_gui::views::{DialogKind, MainView};

struct TrashFixture {
    root: PathBuf,
}

impl TrashFixture {
    fn new(name: &str) -> Self {
        let raw =
            std::env::temp_dir().join(format!("kwati_gui_trash_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&raw);
        fs::create_dir_all(&raw).unwrap();
        let root = raw.canonicalize().unwrap_or(raw);
        Self { root }
    }
}

impl Drop for TrashFixture {
    fn drop(&mut self) {
        if is_trash_browsing_supported() {
            if let Ok(items) = list_trash() {
                let ours: Vec<TrashItem> = items
                    .into_iter()
                    .filter(|item| item.original_parent == self.root)
                    .collect();
                if !ours.is_empty() {
                    let _ = purge_trash_items(&ours, Confirmed::user_confirmed());
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn open_window<'a>(
    cx: &'a mut TestAppContext,
    dir: &std::path::Path,
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

#[gpui::test]
fn sidebar_opens_system_trash_when_browsing_unsupported(cx: &mut TestAppContext) {
    let fixture = TrashFixture::new("macos_open");
    let (main, cx) = open_window(cx, &fixture.root);

    let system_opened = Arc::new(AtomicBool::new(false));
    let flag = system_opened.clone();
    main.update(cx, |main, cx| {
        main.sidebar.update(cx, |sidebar, cx| {
            sidebar.open_trash_with(
                false,
                move || {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                },
                cx,
            );
        });
    });
    cx.run_until_parked();

    assert!(system_opened.load(Ordering::SeqCst));
    let trash_active = main.read_with(cx, |main, cx| main.app_state.read(cx).trash_active);
    assert!(!trash_active, "in-app trash view must stay closed on macOS");

    main.update(cx, |main, cx| {
        main.sidebar.update(cx, |sidebar, cx| {
            sidebar.open_trash_with(
                false,
                || Err(std::io::Error::other("finder unavailable")),
                cx,
            );
        });
    });
    cx.run_until_parked();
    let status = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .status_message
            .clone()
            .unwrap_or_default()
    });
    assert!(status.contains("Cannot open system trash"));
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
#[gpui::test]
fn trash_view_restores_selected_item_to_original_folder(cx: &mut TestAppContext) {
    let fixture = TrashFixture::new("gui_restore");
    let file = fixture.root.join("restorable.txt");
    fs::write(&file, b"gui payload").unwrap();
    trash(&file).unwrap();
    assert!(!file.exists());

    let (main, cx) = open_window(cx, &fixture.root);

    main.update(cx, |main, cx| {
        main.sidebar
            .update(cx, |sidebar, cx| sidebar.open_trash(cx));
    });
    cx.run_until_parked();

    let (trash_active, addr_text, item) = main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        let addr = main
            .header
            .read(cx)
            .address_input
            .read(cx)
            .text()
            .to_string();
        let item = state
            .trash_items
            .iter()
            .find(|i| i.original_path() == file)
            .cloned();
        (state.trash_active, addr, item)
    });
    assert!(trash_active);
    assert_eq!(addr_text, "Trash");
    let item = item.expect("trashed item should appear in TrashView");
    assert_eq!(item.original_parent, fixture.root);
    assert!(item.deleted_at().is_some());

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.select_trash_only(item.id.clone());
            cx.notify();
        });
        main.trash
            .update(cx, |trash, cx| trash.restore_selected(cx));
    });
    cx.run_until_parked();

    assert_eq!(fs::read(&file).unwrap(), b"gui payload");
    let still_in_trash = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .trash_items
            .iter()
            .any(|i| i.id == item.id)
    });
    assert!(!still_in_trash, "restored item should leave the trash view");

    // Returning to the original folder shows the restored file.
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, _| {
            assert!(state.go_back());
        });
    });
    cx.run_until_parked();

    let (trash_active, restored_listed) = main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        (
            state.trash_active,
            state.entries.iter().any(|e| e.name() == "restorable.txt"),
        )
    });
    assert!(!trash_active);
    assert!(restored_listed);
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
#[gpui::test]
fn trash_view_restore_conflict_shows_both_names_and_changes_nothing(cx: &mut TestAppContext) {
    let fixture = TrashFixture::new("gui_conflict");
    let file = fixture.root.join("clash.txt");
    fs::write(&file, b"trashed version").unwrap();
    trash(&file).unwrap();
    fs::write(&file, b"existing version").unwrap();

    let (main, cx) = open_window(cx, &fixture.root);
    main.update(cx, |main, cx| {
        main.sidebar
            .update(cx, |sidebar, cx| sidebar.open_trash(cx));
    });
    cx.run_until_parked();

    let item = main
        .read_with(cx, |main, cx| {
            main.app_state
                .read(cx)
                .trash_items
                .iter()
                .find(|i| i.original_path() == file)
                .cloned()
        })
        .expect("trashed item should appear in TrashView");

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.select_trash_only(item.id.clone());
            cx.notify();
        });
        main.trash
            .update(cx, |trash, cx| trash.restore_selected(cx));
    });
    cx.run_until_parked();

    let (dialog, status, still_in_trash) = main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        (
            main.dialogs.read(cx).current_dialog.clone(),
            state.status_message.clone().unwrap_or_default(),
            state.trash_items.iter().any(|i| i.id == item.id),
        )
    });

    assert_eq!(
        dialog,
        DialogKind::RestoreConflict {
            item_name: "clash.txt".to_string(),
            destination: file.clone(),
        }
    );
    assert!(status.contains("clash.txt"));
    assert!(status.contains(&file.display().to_string()));
    assert_eq!(fs::read(&file).unwrap(), b"existing version");
    assert!(still_in_trash);
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
#[gpui::test]
fn empty_trash_requires_confirmation_and_cancel_keeps_items(cx: &mut TestAppContext) {
    let fixture = TrashFixture::new("gui_empty");
    let first = fixture.root.join("first.txt");
    let second = fixture.root.join("second.txt");
    fs::write(&first, b"one").unwrap();
    fs::write(&second, b"two").unwrap();
    trash(&first).unwrap();
    trash(&second).unwrap();

    let (main, cx) = open_window(cx, &fixture.root);
    main.update(cx, |main, cx| {
        main.sidebar
            .update(cx, |sidebar, cx| sidebar.open_trash(cx));
    });
    cx.run_until_parked();

    // Scope the view's items to this fixture so parallel tests / host trash items are untouched.
    let our_items: Vec<TrashItem> = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .trash_items
            .iter()
            .filter(|item| item.original_parent == fixture.root)
            .cloned()
            .collect()
    });
    assert_eq!(our_items.len(), 2);

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.set_trash_items(our_items.clone());
            cx.notify();
        });
        main.trash
            .update(cx, |trash, cx| trash.request_empty_trash(cx));
    });
    cx.run_until_parked();

    let dialog = main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert!(matches!(
        &dialog,
        DialogKind::ConfirmEmptyTrash { items } if items.len() == 2
    ));
    assert!(confirm_empty_trash_message(2).contains("cannot be undone"));

    // Pressing Enter must never confirm Empty Trash.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let still_open = main.read_with(cx, |main, cx| {
        matches!(
            main.dialogs.read(cx).current_dialog,
            DialogKind::ConfirmEmptyTrash { .. }
        )
    });
    assert!(still_open);

    // Cancelling (Escape) closes the dialog and keeps every item.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let after_cancel_dialog =
        main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert_eq!(after_cancel_dialog, DialogKind::None);
    let listed = list_trash().unwrap();
    assert!(our_items
        .iter()
        .all(|ours| listed.iter().any(|item| item.id == ours.id)));

    // Confirming via the button handler purges the items.
    main.update(cx, |main, cx| {
        main.trash
            .update(cx, |trash, cx| trash.request_empty_trash(cx));
    });
    cx.run_until_parked();
    main.update(cx, |main, cx| {
        main.dialogs
            .update(cx, |dialogs, cx| dialogs.confirm_empty_trash(cx));
    });
    cx.run_until_parked();

    let after_confirm_dialog =
        main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert_eq!(after_confirm_dialog, DialogKind::None);
    let remaining = list_trash().unwrap();
    assert!(our_items
        .iter()
        .all(|ours| !remaining.iter().any(|item| item.id == ours.id)));
}

#[test]
fn trash_sorting_and_selection_work_in_app_state() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let items = vec![
        TrashItem {
            id: "1".into(),
            name: "zeta.txt".into(),
            original_parent: PathBuf::from("/b"),
            time_deleted: 100,
        },
        TrashItem {
            id: "2".into(),
            name: "alpha.txt".into(),
            original_parent: PathBuf::from("/a"),
            time_deleted: 300,
        },
        TrashItem {
            id: "3".into(),
            name: "mid.txt".into(),
            original_parent: PathBuf::from("/c"),
            time_deleted: 200,
        },
    ];
    state.set_trash_items(items);

    state.toggle_trash_sort(TrashSortColumn::Name);
    assert_eq!(state.trash_sort_direction, SortDirection::Ascending);
    let names: Vec<String> = state.trash_items.iter().map(|i| i.display_name()).collect();
    assert_eq!(names, vec!["alpha.txt", "mid.txt", "zeta.txt"]);

    state.select_trash_index(0);
    state.select_trash_range_to(2, false);
    assert_eq!(state.trash_selection.len(), 3);

    state.toggle_trash_item("2".into());
    assert_eq!(state.trash_selection.len(), 2);

    state.clear_trash_selection();
    assert!(state.trash_selection.is_empty());
}
