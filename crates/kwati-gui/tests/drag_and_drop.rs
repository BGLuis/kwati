use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    point, px, size, AppContext, ExternalPaths, FileDropEvent, Modifiers, MouseButton, Point,
    TestAppContext,
};
use kwati_core::{Location, PlaceEntry, Settings, ViewMode};
use kwati_gui::state::AppState;
use kwati_gui::views::drag_drop::{
    is_copy_modifier_for_os, mount_point_for, resolve_drop_operation_for_os,
    validate_external_drop, validate_internal_drop, DropOperation, DropRejection,
};
use kwati_gui::views::list_view::{DragEntries, DraggedEntry};
use kwati_gui::views::sidebar_view::SidebarVolume;
use kwati_gui::views::{DialogKind, MainView};
use kwati_gui::IconName;

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "kwati_gui_dnd_{label}_{}_{}",
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_modifier() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::alt()
    } else {
        Modifiers::control()
    }
}

fn list_row_center(
    main: &gpui::Entity<MainView>,
    cx: &mut gpui::VisualTestContext,
    name: &str,
) -> Point<gpui::Pixels> {
    main.read_with(cx, |main, cx| {
        let state = main.app_state.read(cx);
        let ix = state
            .entries
            .iter()
            .position(|e| e.display_name() == name)
            .unwrap_or_else(|| panic!("entry '{name}' not found"));
        let list = main.list.read(cx);
        let scroll = list.scroll_handle.0.borrow();
        let bounds = scroll.base_handle.bounds();
        let offset = scroll.base_handle.offset();
        if state.view_mode == ViewMode::Grid {
            let cols = list.grid_columns().max(1);
            let row = ix / cols;
            let col = ix % cols;
            let x = bounds.left()
                + kwati_gui::views::list_view::GRID_PAD_X
                + (kwati_gui::views::list_view::GRID_TILE_WIDTH
                    + kwati_gui::views::list_view::GRID_GAP)
                    * (col as f32)
                + kwati_gui::views::list_view::GRID_TILE_WIDTH / 2.0;
            let y = bounds.top()
                + kwati_gui::views::list_view::GRID_ROW_HEIGHT * (row as f32)
                + kwati_gui::views::list_view::GRID_GAP / 2.0
                + kwati_gui::views::list_view::GRID_TILE_HEIGHT / 2.0
                + offset.y;
            point(x, y)
        } else {
            let row_h = list.row_height;
            point(
                bounds.left() + px(80.),
                bounds.top() + row_h * (ix as f32) + row_h / 2.0 + offset.y,
            )
        }
    })
}

fn pointer_drag_with_modifiers(
    cx: &mut gpui::VisualTestContext,
    from: Point<gpui::Pixels>,
    to: Point<gpui::Pixels>,
    drop_modifiers: Modifiers,
) {
    cx.simulate_mouse_move(from, None, Modifiers::none());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(from.x + px(8.), from.y),
        MouseButton::Left,
        drop_modifiers,
    );
    cx.simulate_mouse_move(to, MouseButton::Left, drop_modifiers);
    cx.simulate_mouse_up(to, MouseButton::Left, drop_modifiers);
    cx.run_until_parked();
}

#[test]
fn same_volume_moves_and_cross_volume_copies() {
    let dir = temp_dir("policy_volumes");
    let disk_a = dir.join("disk_a");
    let disk_b = dir.join("disk_b");
    let sub_a = disk_a.join("sub");
    std::fs::create_dir_all(&sub_a).unwrap();
    std::fs::create_dir_all(&disk_b).unwrap();
    let file_a = disk_a.join("file.txt");
    std::fs::write(&file_a, "x").unwrap();

    let mounts = vec![dir.clone(), disk_a.clone(), disk_b.clone()];
    assert_eq!(
        mount_point_for(&file_a, &mounts),
        disk_a.canonicalize().unwrap()
    );
    assert_eq!(
        mount_point_for(&disk_b, &mounts),
        disk_b.canonicalize().unwrap()
    );

    let sources = vec![Location::Local(file_a.clone())];
    assert_eq!(
        resolve_drop_operation_for_os(
            &sources,
            &Location::Local(sub_a.clone()),
            Modifiers::none(),
            false,
            &mounts
        ),
        DropOperation::Move
    );
    assert_eq!(
        resolve_drop_operation_for_os(
            &sources,
            &Location::Local(disk_b.clone()),
            Modifiers::none(),
            false,
            &mounts
        ),
        DropOperation::Copy
    );

    // Remote <-> Local and Remote(A) <-> Remote(B) default to Copy; same Remote profile defaults to Move.
    let remote_a_file = Location::Remote {
        profile_id: "srv-a".into(),
        path: PathBuf::from("/docs/a.txt"),
    };
    let remote_a_dir = Location::Remote {
        profile_id: "srv-a".into(),
        path: PathBuf::from("/archive"),
    };
    let remote_b_dir = Location::Remote {
        profile_id: "srv-b".into(),
        path: PathBuf::from("/archive"),
    };
    assert_eq!(
        resolve_drop_operation_for_os(
            std::slice::from_ref(&remote_a_file),
            &remote_a_dir,
            Modifiers::none(),
            false,
            &mounts
        ),
        DropOperation::Move
    );
    assert_eq!(
        resolve_drop_operation_for_os(
            std::slice::from_ref(&remote_a_file),
            &remote_b_dir,
            Modifiers::none(),
            false,
            &mounts
        ),
        DropOperation::Copy
    );
    assert_eq!(
        resolve_drop_operation_for_os(
            &[remote_a_file],
            &Location::Local(disk_a.clone()),
            Modifiers::none(),
            false,
            &mounts
        ),
        DropOperation::Copy
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_modifier_respects_os_conventions() {
    assert!(is_copy_modifier_for_os(Modifiers::control(), false));
    assert!(!is_copy_modifier_for_os(Modifiers::alt(), false));
    assert!(is_copy_modifier_for_os(Modifiers::alt(), true));
    assert!(!is_copy_modifier_for_os(Modifiers::control(), true));

    let dir = temp_dir("policy_modifiers");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let src = vec![Location::Local(dir.join("a.txt"))];
    let mounts = vec![dir.clone()];

    assert_eq!(
        resolve_drop_operation_for_os(
            &src,
            &Location::Local(sub.clone()),
            Modifiers::control(),
            false,
            &mounts
        ),
        DropOperation::Copy
    );
    assert_eq!(
        resolve_drop_operation_for_os(&src, &Location::Local(sub), Modifiers::alt(), true, &mounts),
        DropOperation::Copy
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validate_internal_and_external_drop_refuses_self_and_subfolders() {
    let dir = temp_dir("policy_self_sub");
    let parent = dir.join("parent");
    let child = parent.join("child");
    let grandchild = child.join("grandchild");
    let sibling = dir.join("sibling");
    std::fs::create_dir_all(&grandchild).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();

    let dragged_parent = DragEntries {
        items: vec![DraggedEntry {
            location: Location::Local(parent.clone()),
            is_dir: true,
            name: "parent".into(),
            icon: IconName::Folder,
        }],
    };

    for op in [DropOperation::Move, DropOperation::Copy] {
        assert_eq!(
            validate_internal_drop(&dragged_parent, &Location::Local(parent.clone()), op),
            Err(DropRejection::IntoSelfOrSubfolder)
        );
        assert_eq!(
            validate_internal_drop(&dragged_parent, &Location::Local(child.clone()), op),
            Err(DropRejection::IntoSelfOrSubfolder)
        );
        assert_eq!(
            validate_internal_drop(&dragged_parent, &Location::Local(grandchild.clone()), op),
            Err(DropRejection::IntoSelfOrSubfolder)
        );
        assert_eq!(
            validate_internal_drop(&dragged_parent, &Location::Local(sibling.clone()), op),
            Ok(())
        );
    }

    // Dropping into the same parent folder is refused for Move, allowed for Copy.
    assert_eq!(
        validate_internal_drop(
            &dragged_parent,
            &Location::Local(dir.clone()),
            DropOperation::Move
        ),
        Err(DropRejection::AlreadyInFolder)
    );
    assert_eq!(
        validate_internal_drop(
            &dragged_parent,
            &Location::Local(dir.clone()),
            DropOperation::Copy
        ),
        Ok(())
    );

    // External folder drops also refuse self and subfolders.
    assert_eq!(
        validate_external_drop(
            std::slice::from_ref(&parent),
            &Location::Local(parent.clone())
        ),
        Err(DropRejection::IntoSelfOrSubfolder)
    );
    assert_eq!(
        validate_external_drop(std::slice::from_ref(&parent), &Location::Local(grandchild)),
        Err(DropRejection::IntoSelfOrSubfolder)
    );
    assert_eq!(
        validate_external_drop(&[parent], &Location::Local(sibling)),
        Ok(())
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drag_two_selected_files_onto_folder_row_moves_on_same_volume(cx: &mut TestAppContext) {
    let dir = temp_dir("multi_move_row");
    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let keep = dir.join("keep.txt");
    std::fs::write(&a, "alpha").unwrap();
    std::fs::write(&b, "beta").unwrap();
    std::fs::write(&keep, "stay").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dir.clone()));
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.selection.clear();
            state.toggle_select_location(Location::Local(a.clone()));
            state.toggle_select_location(Location::Local(b.clone()));
            cx.notify();
        });
    });
    cx.run_until_parked();

    let from = list_row_center(&main, cx, "a.txt");
    let to = list_row_center(&main, cx, "dest");
    pointer_drag_with_modifiers(cx, from, to, Modifiers::none());

    assert_eq!(
        std::fs::read_to_string(dest.join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(std::fs::read_to_string(dest.join("b.txt")).unwrap(), "beta");
    assert!(!a.exists(), "a.txt must have moved out of workspace");
    assert!(!b.exists(), "b.txt must have moved out of workspace");
    assert!(keep.exists(), "unselected file must remain untouched");

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drag_two_selected_files_with_copy_modifier_copies_on_same_volume(cx: &mut TestAppContext) {
    let dir = temp_dir("multi_copy_modifier");
    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, "alpha").unwrap();
    std::fs::write(&b, "beta").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dir.clone()));
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.selection.clear();
            state.toggle_select_location(Location::Local(a.clone()));
            state.toggle_select_location(Location::Local(b.clone()));
            cx.notify();
        });
    });
    cx.run_until_parked();

    let from = list_row_center(&main, cx, "a.txt");
    let to = list_row_center(&main, cx, "dest");
    pointer_drag_with_modifiers(cx, from, to, copy_modifier());

    assert_eq!(
        std::fs::read_to_string(dest.join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(std::fs::read_to_string(dest.join("b.txt")).unwrap(), "beta");
    assert!(
        a.exists(),
        "original a.txt must stay when copy modifier is held"
    );
    assert!(
        b.exists(),
        "original b.txt must stay when copy modifier is held"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drag_two_selected_files_onto_other_volume_sidebar_drive_copies(cx: &mut TestAppContext) {
    let dir = temp_dir("cross_volume_drive");
    let disk_a = dir.join("disk_a");
    let disk_b = dir.join("disk_b");
    std::fs::create_dir_all(&disk_a).unwrap();
    std::fs::create_dir_all(&disk_b).unwrap();
    let a = disk_a.join("a.txt");
    let b = disk_a.join("b.txt");
    std::fs::write(&a, "one").unwrap();
    std::fs::write(&b, "two").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![PlaceEntry::Trash]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(disk_a.clone()));
            s.settings = settings;
            s.set_volume_mounts_for_test(vec![disk_a.clone(), disk_b.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    main.update(cx, |main, cx| {
        main.sidebar.update(cx, |sidebar, cx| {
            sidebar.volumes = vec![SidebarVolume::new(
                "usb-1",
                "USB Drive",
                disk_b.clone(),
                10_000_000,
                5_000_000,
                true,
            )];
            cx.notify();
        });
        main.app_state.update(cx, |state, cx| {
            state.selection.clear();
            state.toggle_select_location(Location::Local(a.clone()));
            state.toggle_select_location(Location::Local(b.clone()));
            cx.notify();
        });
    });
    cx.run_until_parked();

    let from = list_row_center(&main, cx, "a.txt");
    let drive_pos = cx
        .debug_bounds("drive-row-0")
        .expect("drive-row-0 bounds")
        .center();
    pointer_drag_with_modifiers(cx, from, drive_pos, Modifiers::none());

    assert_eq!(
        std::fs::read_to_string(disk_b.join("a.txt")).unwrap(),
        "one"
    );
    assert_eq!(
        std::fs::read_to_string(disk_b.join("b.txt")).unwrap(),
        "two"
    );
    assert!(
        a.exists(),
        "cross-volume drag must copy, leaving source a.txt intact"
    );
    assert!(
        b.exists(),
        "cross-volume drag must copy, leaving source b.txt intact"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drag_onto_place_on_other_volume_copies_and_same_volume_moves(cx: &mut TestAppContext) {
    let dir = temp_dir("cross_volume_place");
    let disk_a = dir.join("disk_a");
    let same_place = disk_a.join("same_place");
    let disk_b = dir.join("disk_b");
    std::fs::create_dir_all(&same_place).unwrap();
    std::fs::create_dir_all(&disk_b).unwrap();
    let f_move = disk_a.join("move_me.txt");
    let f_copy = disk_a.join("copy_me.txt");
    std::fs::write(&f_move, "mv").unwrap();
    std::fs::write(&f_copy, "cp").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![
            PlaceEntry::folder("SameDisk", same_place.clone()),
            PlaceEntry::folder("OtherDisk", disk_b.clone()),
            PlaceEntry::Trash,
        ]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(disk_a.clone()));
            s.settings = settings;
            s.set_volume_mounts_for_test(vec![disk_a.clone(), disk_b.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let same_pos = cx
        .debug_bounds("place-row-0")
        .expect("place-row-0 bounds")
        .center();
    let other_pos = cx
        .debug_bounds("place-row-1")
        .expect("place-row-1 bounds")
        .center();

    let move_from = list_row_center(&main, cx, "move_me.txt");
    pointer_drag_with_modifiers(cx, move_from, same_pos, Modifiers::none());
    assert!(same_place.join("move_me.txt").exists());
    assert!(!f_move.exists(), "same-volume drop on Place must move");

    let copy_from = list_row_center(&main, cx, "copy_me.txt");
    pointer_drag_with_modifiers(cx, copy_from, other_pos, Modifiers::none());
    assert!(disk_b.join("copy_me.txt").exists());
    assert!(f_copy.exists(), "cross-volume drop on Place must copy");

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drop_folder_onto_itself_or_subfolder_is_refused_and_nothing_moves(cx: &mut TestAppContext) {
    let dir = temp_dir("refuse_self_and_sub");
    let workspace = dir.join("workspace");
    let parent = workspace.join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let note = child.join("note.txt");
    std::fs::write(&note, "keep me").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![
            PlaceEntry::folder("ChildSubfolder", child.clone()),
            PlaceEntry::Trash,
        ]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(workspace.clone()));
            s.settings = settings;
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // 1. Dragging `parent` onto itself in ListView is refused and does not bubble to list-drop-zone.
    let parent_pos = list_row_center(&main, cx, "parent");
    cx.simulate_mouse_move(parent_pos, None, Modifiers::none());
    cx.simulate_mouse_down(parent_pos, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(parent_pos.x + px(12.), parent_pos.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(parent_pos.x + px(12.), parent_pos.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();

    assert!(parent.exists());
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "keep me");
    assert!(!parent.join("parent").exists());
    let status = main.read_with(cx, |main, cx| {
        main.app_state.read(cx).status_message.clone()
    });
    assert_eq!(
        status.as_deref(),
        Some("Cannot drop a folder into itself or one of its subfolders")
    );

    // 2. Dragging `parent` onto its own subfolder pinned in Places is refused (even with copy modifier).
    let place_pos = cx
        .debug_bounds("place-row-0")
        .expect("place-row-0 bounds")
        .center();
    pointer_drag_with_modifiers(cx, parent_pos, place_pos, Modifiers::none());
    assert!(parent.exists());
    assert!(!child.join("parent").exists());

    pointer_drag_with_modifiers(cx, parent_pos, place_pos, copy_modifier());
    assert!(parent.exists());
    assert!(!child.join("parent").exists());
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "keep me");

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drop_onto_empty_space_in_same_folder_is_noop_without_modifier(cx: &mut TestAppContext) {
    let dir = temp_dir("empty_space_noop");
    let doc = dir.join("doc.txt");
    std::fs::write(&doc, "hello").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dir.clone()));
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let from = list_row_center(&main, cx, "doc.txt");
    let empty_pos = {
        let bounds = cx
            .debug_bounds("list-drop-zone")
            .expect("list-drop-zone bounds");
        point(bounds.center().x, bounds.bottom() - px(40.))
    };
    pointer_drag_with_modifiers(cx, from, empty_pos, Modifiers::none());

    assert!(doc.exists());
    let dialog = main.read_with(cx, |main, cx| main.dialogs.read(cx).current_dialog.clone());
    assert_eq!(dialog, DialogKind::None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn external_file_drop_copies_into_current_folder_and_refuses_self(cx: &mut TestAppContext) {
    let dir = temp_dir("external_drop");
    let workspace = dir.join("workspace");
    let sub = workspace.join("sub");
    let place_dir = dir.join("place_dir");
    let external_src = dir.join("outside");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(&place_dir).unwrap();
    std::fs::create_dir_all(&external_src).unwrap();
    let ext_file = external_src.join("from_other_app.txt");
    let ext_sub_file = external_src.join("for_sub.txt");
    let ext_place_file = external_src.join("for_place.txt");
    std::fs::write(&ext_file, "external content").unwrap();
    std::fs::write(&ext_sub_file, "into sub").unwrap();
    std::fs::write(&ext_place_file, "into place").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![
            PlaceEntry::folder("DestPlace", place_dir.clone()),
            PlaceEntry::Trash,
        ]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(workspace.clone()));
            s.settings = settings;
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // 1. Exercise GPUI FileDropEvent -> ExternalPaths dispatch over list-drop-zone.
    let drop_pt = cx
        .debug_bounds("list-drop-zone")
        .expect("list-drop-zone bounds")
        .center();
    cx.simulate_event(FileDropEvent::Entered {
        position: drop_pt,
        paths: ExternalPaths::default(),
    });
    cx.simulate_event(FileDropEvent::Pending { position: drop_pt });
    cx.simulate_event(FileDropEvent::Submit { position: drop_pt });
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| cx.has_active_drag()));

    // 2. External file dropped into current folder copies it and leaves original intact.
    main.update(cx, |main, cx| {
        main.list.update(cx, |list, cx| {
            list.handle_external_paths_drop(
                std::slice::from_ref(&ext_file),
                Location::Local(workspace.clone()),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(workspace.join("from_other_app.txt")).unwrap(),
        "external content"
    );
    assert!(ext_file.exists(), "external file must be copied, not moved");

    // 3. External file dropped onto a subfolder copies it into that subfolder.
    main.update(cx, |main, cx| {
        main.list.update(cx, |list, cx| {
            list.handle_external_paths_drop(
                std::slice::from_ref(&ext_sub_file),
                Location::Local(sub.clone()),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(sub.join("for_sub.txt")).unwrap(),
        "into sub"
    );
    assert!(ext_sub_file.exists());

    // 4. External file dropped onto a sidebar target copies it there.
    main.update(cx, |main, cx| {
        main.sidebar.update(cx, |sidebar, cx| {
            sidebar.handle_external_paths_drop(
                std::slice::from_ref(&ext_place_file),
                &place_dir,
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(place_dir.join("for_place.txt")).unwrap(),
        "into place"
    );
    assert!(ext_place_file.exists());

    // 5. External drop of a folder onto itself or its subfolder is refused.
    main.update(cx, |main, cx| {
        main.list.update(cx, |list, cx| {
            list.handle_external_paths_drop(
                std::slice::from_ref(&workspace),
                Location::Local(sub.clone()),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(!sub.join("workspace").exists());
    let status = main.read_with(cx, |main, cx| {
        main.app_state.read(cx).status_message.clone()
    });
    assert_eq!(
        status.as_deref(),
        Some("Cannot drop a folder into itself or one of its subfolders")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn drag_onto_folder_works_in_compact_and_grid_modes(cx: &mut TestAppContext) {
    let dir = temp_dir("view_modes_dnd");
    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let compact_file = dir.join("compact.txt");
    let grid_file = dir.join("grid.txt");
    std::fs::write(&compact_file, "c").unwrap();
    std::fs::write(&grid_file, "g").unwrap();

    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dir.clone()));
            s.set_volume_mounts_for_test(vec![dir.clone()]);
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    // CompactList mode
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.set_view_mode(ViewMode::CompactList);
            cx.notify();
        });
    });
    cx.run_until_parked();
    let from = list_row_center(&main, cx, "compact.txt");
    let to = list_row_center(&main, cx, "dest");
    pointer_drag_with_modifiers(cx, from, to, Modifiers::none());
    assert!(dest.join("compact.txt").exists());
    assert!(!compact_file.exists());

    // Grid mode
    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state.set_view_mode(ViewMode::Grid);
            cx.notify();
        });
    });
    cx.run_until_parked();
    let from = list_row_center(&main, cx, "grid.txt");
    let to = list_row_center(&main, cx, "dest");
    pointer_drag_with_modifiers(cx, from, to, Modifiers::none());
    assert!(dest.join("grid.txt").exists());
    assert!(!grid_file.exists());

    let _ = std::fs::remove_dir_all(&dir);
}
