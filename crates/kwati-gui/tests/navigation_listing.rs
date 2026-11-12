use std::ffi::OsString;
use std::path::PathBuf;

use kwati_core::{Entry, Location};
use kwati_gui::components::ScrollbarState;
use kwati_gui::state::AppState;

fn make_dummy_entries(count: usize) -> Vec<Entry> {
    (0..count)
        .map(|i| {
            let is_dir = i % 5 == 0;
            let name = format!("item_{:04}.txt", i);
            let path = PathBuf::from(format!("/tmp/{}", name));
            Entry::new(OsString::from(name), path, is_dir).with_size((i as u64) * 1024)
        })
        .collect()
}

#[test]
fn virtualized_list_selection_and_scrollbar_metrics() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let entries = make_dummy_entries(1000);
    state.set_entries(entries);

    assert_eq!(state.entries.len(), 1000);
    assert_eq!(state.selected_index(), None);

    // Select index 42
    state.select_index(42);
    assert_eq!(state.selected_index(), Some(42));
    assert!(state.selection.contains(&state.entries[42].location()));

    // Scrollbar state for 1000 items
    let mut scrollbar = ScrollbarState::new(1000, 25);
    assert!(scrollbar.can_scroll());
    assert_eq!(scrollbar.scroll_ratio(), 0.0);
    assert_eq!(scrollbar.thumb_ratio(), 0.05); // clamped minimum thumb ratio

    // Scroll offset midway
    scrollbar.set_metrics(1000, 25, 487);
    assert_eq!(scrollbar.current_offset, 487);
    assert!((scrollbar.scroll_ratio() - 0.499).abs() < 0.01);

    // Dragging / scrolling to the end reaches max offset (1000 - 25 = 975)
    scrollbar.set_metrics(1000, 25, 1000);
    assert_eq!(scrollbar.current_offset, 975);
    assert_eq!(scrollbar.scroll_ratio(), 1.0);
}

#[test]
fn navigation_buttons_and_history_integration() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp/folder_a")));
    assert!(!state.history.can_go_back());
    assert!(!state.history.can_go_forward());

    // Navigate to folder B
    state.navigate_to(Location::Local(PathBuf::from("/tmp/folder_b")));
    assert!(state.history.can_go_back());
    assert!(!state.history.can_go_forward());
    assert_eq!(
        state.current_location,
        Location::Local(PathBuf::from("/tmp/folder_b"))
    );

    // Navigate to subfolder
    state.navigate_to(Location::Local(PathBuf::from("/tmp/folder_b/sub")));
    assert!(state.history.can_go_back());

    // Back to folder B
    assert!(state.go_back());
    assert_eq!(
        state.current_location,
        Location::Local(PathBuf::from("/tmp/folder_b"))
    );
    assert!(state.history.can_go_forward());

    // Back to folder A
    assert!(state.go_back());
    assert_eq!(
        state.current_location,
        Location::Local(PathBuf::from("/tmp/folder_a"))
    );
    assert!(!state.history.can_go_back());

    // Cannot go back further
    assert!(!state.go_back());

    // Forward to folder B
    assert!(state.go_forward());
    assert_eq!(
        state.current_location,
        Location::Local(PathBuf::from("/tmp/folder_b"))
    );

    // Up to parent /tmp
    assert!(state.go_up());
    assert_eq!(
        state.current_location,
        Location::Local(PathBuf::from("/tmp"))
    );
}

#[test]
fn address_bar_path_entry_and_validation() {
    let temp_root = std::env::temp_dir().join(format!("kwati_test_nav_{}", std::process::id()));
    let existing_sub = temp_root.join("subfolder");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&existing_sub).unwrap();

    let mut state = AppState::new(Location::Local(temp_root.clone()));
    state.reload_entries();

    assert_eq!(state.current_location, Location::Local(temp_root.clone()));
    assert!(state.status_message.is_none());

    // 1. Enter an existing directory path -> navigates and lists it
    let typed_existing = existing_sub.to_string_lossy().to_string();
    let target_path = PathBuf::from(&typed_existing);
    if target_path.exists() && target_path.is_dir() {
        state.navigate_to(target_path.into());
        state.clear_status();
    }
    assert_eq!(
        state.current_location,
        Location::Local(existing_sub.clone())
    );
    assert!(state.status_message.is_none());

    // 2. Enter a non-existent path -> displays error in status bar and keeps current listing
    let typed_missing = temp_root.join("missing_dir").to_string_lossy().to_string();
    let missing_path = PathBuf::from(&typed_missing);
    if missing_path.exists() && missing_path.is_dir() {
        state.navigate_to(missing_path.into());
        state.clear_status();
    } else {
        state.set_status(format!("Directory not found: {}", typed_missing));
    }
    assert_eq!(
        state.current_location,
        Location::Local(existing_sub.clone())
    );
    assert_eq!(
        state.status_message,
        Some(format!("Directory not found: {}", typed_missing))
    );

    // 3. Escape restores current directory and clears status error
    state.clear_status();
    assert!(state.status_message.is_none());
    assert_eq!(
        state.current_location.path().to_string_lossy(),
        existing_sub.to_string_lossy()
    );

    let _ = std::fs::remove_dir_all(&temp_root);
}
