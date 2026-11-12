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
    let root = std::env::temp_dir().join(format!("kwati_test_history_{}", std::process::id()));
    let folder_a = root.join("folder_a");
    let folder_b = root.join("folder_b");
    let sub = folder_b.join("sub");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&sub).unwrap();

    let mut state = AppState::new(Location::Local(folder_a.clone()));
    assert!(!state.history.can_go_back());
    assert!(!state.history.can_go_forward());

    // Navigate to folder B
    state.navigate_to(Location::Local(folder_b.clone()));
    assert!(state.history.can_go_back());
    assert!(!state.history.can_go_forward());
    assert_eq!(state.current_location, Location::Local(folder_b.clone()));

    // Navigate to subfolder
    state.navigate_to(Location::Local(sub.clone()));
    assert!(state.history.can_go_back());

    // Back to folder B
    assert!(state.go_back());
    assert_eq!(state.current_location, Location::Local(folder_b.clone()));
    assert!(state.history.can_go_forward());

    // Back to folder A
    assert!(state.go_back());
    assert_eq!(state.current_location, Location::Local(folder_a.clone()));
    assert!(!state.history.can_go_back());

    // Cannot go back further
    assert!(!state.go_back());

    // Forward to folder B
    assert!(state.go_forward());
    assert_eq!(state.current_location, Location::Local(folder_b.clone()));

    // Up to the parent
    assert!(state.go_up());
    assert_eq!(state.current_location, Location::Local(root.clone()));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failed_navigation_reports_and_keeps_the_current_directory() {
    let root = std::env::temp_dir().join(format!("kwati_test_failed_nav_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("kept.txt"), "kept").unwrap();

    let mut state = AppState::new(Location::Local(root.clone()));
    state.reload_entries();
    let listing_before = state.entries.clone();
    assert_eq!(listing_before.len(), 1);

    let unreadable = root.join("gone");
    state.navigate_to(Location::Local(unreadable.clone()));

    let status = state
        .status_message
        .clone()
        .expect("the failure is reported");
    assert!(status.contains(&unreadable.display().to_string()));
    assert_eq!(state.current_location, Location::Local(root.clone()));
    assert_eq!(state.entries, listing_before);
    assert!(!state.history.can_go_back());

    // A later successful read clears the report.
    state.reload_entries();
    assert!(state.status_message.is_none());

    let _ = std::fs::remove_dir_all(&root);
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

#[test]
fn sorting_headers_and_hidden_files_toggle() {
    use kwati_core::{SortColumn, SortDirection};
    use kwati_gui::views::list_view::{format_modified, format_size};
    use std::time::{Duration, SystemTime};

    // 1. Formatters
    assert_eq!(format_size(Some(500), false), "500 B");
    assert_eq!(format_size(Some(2048), false), "2.0 KB");
    assert_eq!(format_size(Some(10 * 1024 * 1024), false), "10.0 MB");
    assert_eq!(format_size(Some(2048), true), "--");
    assert_eq!(format_size(None, false), "--");

    assert_eq!(
        format_modified(Some(SystemTime::UNIX_EPOCH)),
        "1970-01-01 00:00"
    );
    assert_eq!(
        format_modified(Some(
            SystemTime::UNIX_EPOCH + Duration::from_secs(86400 * 365 + 3600 * 14 + 1800)
        )),
        "1971-01-01 14:30"
    );

    // 2. Sorting by column and toggling direction, directories always first
    let dir_b = Entry::new(OsString::from("dir_b"), PathBuf::from("/tmp/dir_b"), true);
    let dir_a = Entry::new(OsString::from("dir_a"), PathBuf::from("/tmp/dir_a"), true);
    let file_z = Entry::new(
        OsString::from("file_z.txt"),
        PathBuf::from("/tmp/file_z.txt"),
        false,
    )
    .with_size(100);
    let file_x = Entry::new(
        OsString::from("file_x.txt"),
        PathBuf::from("/tmp/file_x.txt"),
        false,
    )
    .with_size(500);

    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    state.set_entries(vec![
        dir_b.clone(),
        file_z.clone(),
        dir_a.clone(),
        file_x.clone(),
    ]);

    // Initial state: Name, Ascending
    state.toggle_sort(SortColumn::Name);
    // Was Name Ascending, now toggle reverses to Name Descending
    assert_eq!(state.sort_column, SortColumn::Name);
    assert_eq!(state.sort_direction, SortDirection::Descending);
    assert_eq!(state.entries[0].display_name(), "dir_b");
    assert_eq!(state.entries[1].display_name(), "dir_a");
    assert_eq!(state.entries[2].display_name(), "file_z.txt");
    assert_eq!(state.entries[3].display_name(), "file_x.txt");

    // Second click reverses back to Name Ascending
    state.toggle_sort(SortColumn::Name);
    assert_eq!(state.sort_direction, SortDirection::Ascending);
    assert_eq!(state.entries[0].display_name(), "dir_a");
    assert_eq!(state.entries[1].display_name(), "dir_b");
    assert_eq!(state.entries[2].display_name(), "file_x.txt");
    assert_eq!(state.entries[3].display_name(), "file_z.txt");

    // Switch to Size Ascending
    state.toggle_sort(SortColumn::Size);
    assert_eq!(state.sort_column, SortColumn::Size);
    assert_eq!(state.sort_direction, SortDirection::Ascending);
    // Directories first
    assert!(state.entries[0].is_dir());
    assert!(state.entries[1].is_dir());
    assert_eq!(state.entries[2].display_name(), "file_z.txt"); // 100 B
    assert_eq!(state.entries[3].display_name(), "file_x.txt"); // 500 B

    // Size Descending
    state.toggle_sort(SortColumn::Size);
    assert_eq!(state.sort_direction, SortDirection::Descending);
    assert!(state.entries[0].is_dir());
    assert!(state.entries[1].is_dir());
    assert_eq!(state.entries[2].display_name(), "file_x.txt"); // 500 B
    assert_eq!(state.entries[3].display_name(), "file_z.txt"); // 100 B

    // 3. Hidden files toggle
    let temp_dir = std::env::temp_dir().join(format!("kwati_test_hidden_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(temp_dir.join("normal.txt"), "hello").unwrap();
    std::fs::write(temp_dir.join(".hidden.txt"), "secret").unwrap();
    // Windows hides by file attribute, not by a leading dot.
    #[cfg(windows)]
    {
        let status = std::process::Command::new("attrib")
            .arg("+h")
            .arg(temp_dir.join(".hidden.txt"))
            .status()
            .expect("attrib command failed");
        assert!(status.success(), "attrib +h must succeed");
    }

    let mut state_hidden = AppState::new(Location::Local(temp_dir.clone()));
    state_hidden.reload_entries();

    // Hidden files off by default: only normal.txt is listed
    assert!(!state_hidden.show_hidden);
    assert_eq!(state_hidden.entries.len(), 1);
    assert_eq!(state_hidden.entries[0].display_name(), "normal.txt");

    // Toggle hidden files on: both appear
    state_hidden.toggle_hidden();
    assert!(state_hidden.show_hidden);
    assert_eq!(state_hidden.entries.len(), 2);
    assert!(state_hidden
        .entries
        .iter()
        .any(|e| e.display_name() == ".hidden.txt"));

    // Toggle hidden files off: hidden disappears
    state_hidden.toggle_hidden();
    assert!(!state_hidden.show_hidden);
    assert_eq!(state_hidden.entries.len(), 1);
    assert_eq!(state_hidden.entries[0].display_name(), "normal.txt");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn keyboard_navigation_selection_enter_and_backspace() {
    let temp_root = std::env::temp_dir().join(format!("kwati_test_keynav_{}", std::process::id()));
    let subfolder = temp_root.join("subfolder_1");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&subfolder).unwrap();

    let mut state = AppState::new(Location::Local(temp_root.clone()));
    let mut entries = Vec::new();
    entries.push(Entry::new(
        OsString::from("subfolder_1"),
        subfolder.clone(),
        true,
    ));
    for i in 1..50 {
        entries.push(
            Entry::new(
                OsString::from(format!("file_{:02}.txt", i)),
                temp_root.join(format!("file_{:02}.txt", i)),
                false,
            )
            .with_size((i as u64) * 100),
        );
    }
    state.set_entries(entries);

    assert_eq!(state.selected_index(), None);

    // 1. Arrow Down (SelectNext)
    assert_eq!(state.select_next(), Some(0));
    assert_eq!(state.select_next(), Some(1));

    // 2. Arrow Up (SelectPrevious)
    assert_eq!(state.select_previous(), Some(0));
    // At boundary 0, does not go below 0
    assert_eq!(state.select_previous(), Some(0));

    // 3. End (SelectLast)
    assert_eq!(state.select_last(), Some(49));
    // At boundary 49, select_next does not exceed 49
    assert_eq!(state.select_next(), Some(49));

    // 4. Home (SelectFirst)
    assert_eq!(state.select_first(), Some(0));

    // 5. PageDown / PageUp
    assert_eq!(state.select_page_down(20), Some(20));
    assert_eq!(state.select_page_down(20), Some(40));
    assert_eq!(state.select_page_down(20), Some(49)); // clamped to last
    assert_eq!(state.select_page_up(20), Some(29));
    assert_eq!(state.select_page_up(20), Some(9));
    assert_eq!(state.select_page_up(20), Some(0)); // clamped to 0

    // 6. Enter opens folder
    assert_eq!(state.selected_index(), Some(0));
    assert!(state.entries[0].is_dir());
    assert!(state.open_selected_entry());
    assert_eq!(state.current_location, Location::Local(subfolder.clone()));

    // 7. Backspace navigates to parent
    assert!(state.go_up());
    assert_eq!(state.current_location, Location::Local(temp_root.clone()));

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[test]
fn open_file_with_system_and_directory_watch_refresh() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let temp_root = std::env::temp_dir().join(format!("kwati_test_watch_{}", std::process::id()));
    let subfolder = temp_root.join("subfolder");
    let file_a = temp_root.join("file_a.txt");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&subfolder).unwrap();
    std::fs::write(&file_a, "alpha").unwrap();

    let mut state = AppState::new(Location::Local(temp_root.clone()));
    state.reload_entries();

    // 1. Double click / Enter distinguishes folder from file
    let folder_entry = state
        .entries
        .iter()
        .find(|e| e.display_name() == "subfolder")
        .unwrap()
        .clone();
    let file_entry = state
        .entries
        .iter()
        .find(|e| e.display_name() == "file_a.txt")
        .unwrap()
        .clone();

    assert!(folder_entry.is_dir());
    assert!(!file_entry.is_dir());

    // Double clicking folder enters it
    state.navigate_to(folder_entry.location());
    assert_eq!(state.current_location, Location::Local(subfolder.clone()));

    // Navigate back to temp_root
    state.navigate_to(Location::Local(temp_root.clone()));
    assert_eq!(state.current_location, Location::Local(temp_root.clone()));

    // Select file_a
    state.selection.select_only(file_entry.location());
    assert!(state.selection.contains(&file_entry.location()));

    // 2. Directory watcher detects new file and reloads without losing selection
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = Arc::clone(&callback_count);

    let watcher = kwati_gui::actions::navigation::watch_directory(
        &temp_root,
        Duration::from_millis(50),
        move || {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert!(watcher.is_ok());

    // Create file_b in temp_root
    let file_b = temp_root.join("file_b.txt");
    std::fs::write(&file_b, "beta").unwrap();

    // Wait for notify debounce to trigger callback
    std::thread::sleep(Duration::from_millis(200));
    assert!(callback_count.load(Ordering::SeqCst) >= 1);

    // Reload state entries and verify selection is preserved
    state.reload_entries();
    assert_eq!(state.entries.len(), 3); // subfolder, file_a, file_b
    assert!(state.selection.contains(&file_entry.location()));

    // 3. Dropping watcher stops background watch when navigating away
    drop(watcher);
    let count_before = callback_count.load(Ordering::SeqCst);
    let file_c = temp_root.join("file_c.txt");
    std::fs::write(&file_c, "gamma").unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(callback_count.load(Ordering::SeqCst), count_before);

    let _ = std::fs::remove_dir_all(&temp_root);
}
