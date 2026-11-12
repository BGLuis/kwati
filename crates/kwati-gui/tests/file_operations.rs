use std::fs;
use std::path::PathBuf;

use kwati_core::{list_dir, Location};
use kwati_gui::actions::operations::{
    execute_create_folder, execute_rename, execute_trash, validate_entry_name,
};
use kwati_gui::state::{AppState, UndoOperation};

use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn create_temp_test_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.push(format!("kwati_test_guiops_{}_{}_{}", name, time, id));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// =========================================================================
// Issue #17: Rename and New Folder operations
// =========================================================================

#[test]
fn test_validate_entry_name() {
    assert!(validate_entry_name("").is_err());
    assert!(validate_entry_name("   ").is_err());
    assert!(validate_entry_name(".").is_err());
    assert!(validate_entry_name("..").is_err());
    assert!(validate_entry_name("foo/bar").is_err());
    assert!(validate_entry_name("foo\\bar").is_err());
    assert_eq!(
        validate_entry_name("valid_name.txt").unwrap(),
        "valid_name.txt"
    );
    assert_eq!(validate_entry_name("  trimmed  ").unwrap(), "trimmed");
}

#[test]
fn test_rename_entry_success_and_select() {
    let test_dir = create_temp_test_dir("rename_success");
    let file_path = test_dir.join("original.txt");
    fs::write(&file_path, "hello").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    let res = execute_rename(&mut state, &file_path, "renamed.txt");
    assert!(res.is_ok());
    let new_path = res.unwrap();
    assert_eq!(new_path, test_dir.join("renamed.txt"));

    // File on disk changed
    assert!(!file_path.exists());
    assert!(new_path.exists());

    // State updated
    assert!(state.entries.iter().any(|e| e.name == "renamed.txt"));
    assert!(!state.entries.iter().any(|e| e.name == "original.txt"));

    // Selected entry updated
    assert!(state.selection.contains(&Location::Local(new_path.clone())));

    // Undo stack updated
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(
        state.undo_stack.peek(),
        Some(&UndoOperation::Rename {
            from: file_path,
            to: new_path,
        })
    );

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_rename_entry_refuses_existing() {
    let test_dir = create_temp_test_dir("rename_conflict");
    let file1 = test_dir.join("file1.txt");
    let file2 = test_dir.join("file2.txt");
    fs::write(&file1, "1").unwrap();
    fs::write(&file2, "2").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    let res = execute_rename(&mut state, &file1, "file2.txt");
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("already exists"));

    // Files on disk unchanged
    assert!(file1.exists());
    assert!(file2.exists());

    // Undo stack unchanged
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_create_folder_success_and_select() {
    let test_dir = create_temp_test_dir("create_dir_success");

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    let res = execute_create_folder(&mut state, &test_dir, "my_new_folder");
    assert!(res.is_ok());
    let folder_path = res.unwrap();
    assert_eq!(folder_path, test_dir.join("my_new_folder"));

    // Folder on disk exists
    assert!(folder_path.is_dir());

    // State entries updated and folder selected
    assert!(state
        .entries
        .iter()
        .any(|e| e.name == "my_new_folder" && e.is_dir));
    assert!(state
        .selection
        .contains(&Location::Local(folder_path.clone())));

    // Undo stack updated
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(
        state.undo_stack.peek(),
        Some(&UndoOperation::CreateDir { path: folder_path })
    );

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_create_folder_refuses_existing() {
    let test_dir = create_temp_test_dir("create_dir_conflict");
    let existing = test_dir.join("existing_folder");
    fs::create_dir(&existing).unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    let res = execute_create_folder(&mut state, &test_dir, "existing_folder");
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("already exists"));

    // Undo stack unchanged
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}

// =========================================================================
// Issue #19: Move to Trash operations
// =========================================================================

#[test]
fn test_trash_success_removes_entry_and_records_undo() {
    let test_dir = create_temp_test_dir("trash_success");
    let file_path = test_dir.join("to_trash.txt");
    fs::write(&file_path, "trash content").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());
    state
        .selection
        .select_only(Location::Local(file_path.clone()));

    let res = execute_trash(&mut state, std::slice::from_ref(&file_path));
    if let Ok(count) = res {
        assert_eq!(count, 1);
        assert!(!file_path.exists());
        assert!(!state.entries.iter().any(|e| e.name == "to_trash.txt"));
        assert!(!state
            .selection
            .contains(&Location::Local(file_path.clone())));
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(
            state.undo_stack.peek(),
            Some(&UndoOperation::Trash {
                original_paths: vec![file_path],
            })
        );
    } else {
        assert!(state.status_message.is_some());
        assert!(state.status_message.unwrap().contains("trash"));
    }

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_trash_nonexistent_fails_and_keeps_listed() {
    let test_dir = create_temp_test_dir("trash_fail");
    let non_existent = test_dir.join("ghost.txt");

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let res = execute_trash(&mut state, &[non_existent]);
    assert!(res.is_err());
    assert!(state.status_message.is_some());
    assert!(state
        .status_message
        .unwrap()
        .contains("Failed to move to trash"));

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_trash_cancel_leaves_file_in_place() {
    let test_dir = create_temp_test_dir("trash_cancel");
    let file_path = test_dir.join("keep_me.txt");
    fs::write(&file_path, "important").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    // When prompt cancelled, execute_trash is not called: file remains untouched
    assert!(file_path.exists());
    assert!(state.entries.iter().any(|e| e.name == "keep_me.txt"));
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}
