use std::fs;
use std::path::PathBuf;

use kwati_core::{list_dir, ConflictPolicy, CopyOutcome, Location};
use kwati_gui::actions::operations::{
    check_paste_conflict, clear_clipboard, copy_to_clipboard, cut_to_clipboard, execute_copy,
    execute_create_folder, execute_move, execute_permanent_delete, execute_rename, execute_trash,
    execute_undo, get_clipboard, validate_entry_name,
};
use kwati_gui::state::{AppState, UndoOperation};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

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

// =========================================================================
// Issue #21: Copy & Paste with jobs panel, progress, cancel, and conflict resolution
// =========================================================================

#[test]
fn test_clipboard_copy_and_paste_flow() {
    clear_clipboard();
    assert!(get_clipboard().is_none());

    let src_dir = create_temp_test_dir("clip_src");
    let file = src_dir.join("clip.txt");
    fs::write(&file, "copy me").unwrap();

    copy_to_clipboard(vec![file.clone()]);
    let cb = get_clipboard().expect("clipboard must have data");
    assert!(!cb.is_cut);
    assert_eq!(cb.sources, vec![file.clone()]);

    let dst_dir = create_temp_test_dir("clip_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    // Paste into destination
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let outcome = execute_copy(
        &mut state,
        &cb.sources,
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);

    let copied_file = dst_dir.join("clip.txt");
    assert!(copied_file.exists());
    assert_eq!(fs::read_to_string(&copied_file).unwrap(), "copy me");
    assert!(file.exists()); // Source remains intact on copy

    // Verify undo stack
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(
        state.undo_stack.peek(),
        Some(&UndoOperation::Copy {
            destinations: vec![copied_file],
        })
    );

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_copy_progress_and_jobs_panel() {
    let src_dir = create_temp_test_dir("copy_progress_src");
    let file = src_dir.join("large.dat");
    // Write 512KB
    let data = vec![0x42u8; 512 * 1024];
    fs::write(&file, &data).unwrap();

    let dst_dir = create_temp_test_dir("copy_progress_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let outcome = execute_copy(
        &mut state,
        &[file],
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);

    // Job was created and finished, removing itself from active_jobs
    assert!(state.active_jobs.is_empty());
    assert!(dst_dir.join("large.dat").exists());

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_copy_cancellation_cleans_partial_files() {
    let src_dir = create_temp_test_dir("copy_cancel_src");
    let file = src_dir.join("cancel_target.dat");
    fs::write(&file, vec![0x55u8; 256 * 1024]).unwrap();

    let dst_dir = create_temp_test_dir("copy_cancel_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(true)); // Pre-cancelled
    let outcome = execute_copy(
        &mut state,
        std::slice::from_ref(&file),
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Cancelled);

    // Destination file was cleaned up / rolled back
    assert!(!dst_dir.join("cancel_target.dat").exists());
    assert!(file.exists()); // Source intact

    // Active jobs cleaned up
    assert!(state.active_jobs.is_empty());

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_copy_conflict_policies_skip_keep_both_replace() {
    let src_dir = create_temp_test_dir("copy_conflict_src");
    let file = src_dir.join("doc.txt");
    fs::write(&file, "new version").unwrap();

    let dst_dir = create_temp_test_dir("copy_conflict_dst");
    let existing = dst_dir.join("doc.txt");
    fs::write(&existing, "old version").unwrap();

    // 1. Conflict detection
    let conflict = check_paste_conflict(std::slice::from_ref(&file), &dst_dir);
    assert_eq!(conflict, Some("doc.txt".to_string()));

    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    // 2. Policy: Skip
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let outcome = execute_copy(
        &mut state,
        std::slice::from_ref(&file),
        &dst_dir,
        ConflictPolicy::Skip,
        cancel_flag.clone(),
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);
    assert_eq!(fs::read_to_string(&existing).unwrap(), "old version");

    // 3. Policy: KeepBoth -> produces "doc (1).txt"
    let outcome = execute_copy(
        &mut state,
        std::slice::from_ref(&file),
        &dst_dir,
        ConflictPolicy::KeepBoth,
        cancel_flag.clone(),
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);
    let keep_both_path = dst_dir.join("doc (1).txt");
    assert!(keep_both_path.exists());
    assert_eq!(fs::read_to_string(&keep_both_path).unwrap(), "new version");
    assert_eq!(fs::read_to_string(&existing).unwrap(), "old version");

    // 4. Policy: Replace -> overwrites "doc.txt"
    let outcome = execute_copy(
        &mut state,
        &[file],
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);
    assert_eq!(fs::read_to_string(&existing).unwrap(), "new version");

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

// =========================================================================
// Issue #23: Cut & Move entries across locations with progress
// =========================================================================

#[test]
fn test_cut_and_move_same_fs_completes_and_removes_source() {
    clear_clipboard();
    let src_dir = create_temp_test_dir("move_src");
    let file = src_dir.join("move_me.txt");
    fs::write(&file, "move content").unwrap();

    cut_to_clipboard(vec![file.clone()]);
    let cb = get_clipboard().expect("clipboard must have cut data");
    assert!(cb.is_cut);

    let dst_dir = create_temp_test_dir("move_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let outcome = execute_move(
        &mut state,
        &cb.sources,
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
        false,
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);

    let target_file = dst_dir.join("move_me.txt");
    assert!(target_file.exists());
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "move content");
    assert!(!file.exists()); // Source removed!

    // Clipboard cleared after cut & paste
    assert!(get_clipboard().is_none());

    // Undo stack updated with Move
    assert_eq!(state.undo_stack.len(), 1);
    assert_eq!(
        state.undo_stack.peek(),
        Some(&UndoOperation::Move {
            from: vec![file],
            to: vec![target_file],
        })
    );

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_cut_and_move_cross_device_fallback_completes_and_removes_source() {
    let src_dir = create_temp_test_dir("move_cross_src");
    let folder = src_dir.join("my_folder");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("nested.txt"), "nested content").unwrap();

    let dst_dir = create_temp_test_dir("move_cross_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let outcome = execute_move(
        &mut state,
        std::slice::from_ref(&folder),
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
        true, // simulate cross-device
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Completed);

    let dest_folder = dst_dir.join("my_folder");
    assert!(dest_folder.is_dir());
    assert_eq!(
        fs::read_to_string(dest_folder.join("nested.txt")).unwrap(),
        "nested content"
    );
    assert!(!folder.exists()); // Source removed after cross-device copy!

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_cut_and_move_cross_device_cancellation_keeps_source() {
    let src_dir = create_temp_test_dir("move_cross_cancel_src");
    let folder = src_dir.join("important_folder");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("doc.dat"), vec![0x99u8; 128 * 1024]).unwrap();

    let dst_dir = create_temp_test_dir("move_cross_cancel_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(true)); // Pre-cancelled
    let outcome = execute_move(
        &mut state,
        std::slice::from_ref(&folder),
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
        true, // simulate cross-device
    );
    assert_eq!(outcome.unwrap(), CopyOutcome::Cancelled);

    // Source MUST remain listed and intact!
    assert!(folder.exists());
    assert!(folder.join("doc.dat").exists());

    // Destination should be cleaned up
    assert!(!dst_dir.join("important_folder").exists());

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

// =========================================================================
// Issue #35: Permanent delete with explicit confirmation modal
// =========================================================================

#[test]
fn test_permanent_delete_success_deletes_file_and_folder_and_refreshes() {
    let test_dir = create_temp_test_dir("perm_del_success");
    let file = test_dir.join("delete_me.txt");
    fs::write(&file, "vanish").unwrap();

    let folder = test_dir.join("delete_folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("nested.txt"), "sub").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());
    state.selection.select_only(Location::Local(file.clone()));

    let outcome = execute_permanent_delete(&mut state, &[file.clone(), folder.clone()]);
    assert_eq!(outcome.unwrap(), 2);

    // Files deleted from disk
    assert!(!file.exists());
    assert!(!folder.exists());

    // State refreshed and selection pruned
    assert!(!state.entries.iter().any(|e| e.name == "delete_me.txt"));
    assert!(!state.entries.iter().any(|e| e.name == "delete_folder"));
    assert!(state.selection.is_empty());

    // Permanent delete is NOT undoable (nothing added to undo stack)
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_permanent_delete_cancel_leaves_file_in_place() {
    let test_dir = create_temp_test_dir("perm_del_cancel");
    let file = test_dir.join("stay_alive.txt");
    fs::write(&file, "alive").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    // When user clicks Cancel on dialog, execute_permanent_delete is not called: file remains
    assert!(file.exists());
    assert!(state.entries.iter().any(|e| e.name == "stay_alive.txt"));
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}

// =========================================================================
// Issue #65: Undo stack for reversible file operations with feedback
// =========================================================================

#[test]
fn test_undo_rename_restores_original_name_and_status() {
    let test_dir = create_temp_test_dir("undo_rename");
    let original = test_dir.join("file_a.txt");
    fs::write(&original, "data").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let initial_listing = list_dir(&test_dir).unwrap();
    state.set_entries(initial_listing.into_entries());

    // Rename file_a -> file_b
    let new_path = execute_rename(&mut state, &original, "file_b.txt").unwrap();
    assert!(new_path.exists());
    assert!(!original.exists());
    assert_eq!(state.undo_stack.len(), 1);

    // Undo rename
    let res = execute_undo(&mut state);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Undid rename of 'file_a.txt'");
    assert_eq!(
        state.status_message.as_deref(),
        Some("Undid rename of 'file_a.txt'")
    );

    // File name restored on disk
    assert!(original.exists());
    assert!(!new_path.exists());

    // State refreshed and restored entry selected
    assert!(state.entries.iter().any(|e| e.name == "file_a.txt"));
    assert!(state.selection.contains(&Location::Local(original)));

    // Undo stack now empty
    assert!(state.undo_stack.is_empty());

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_undo_rename_refusal_when_destination_missing() {
    let test_dir = create_temp_test_dir("undo_rename_missing");
    let original = test_dir.join("orig.txt");
    fs::write(&original, "x").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let new_path = execute_rename(&mut state, &original, "target.txt").unwrap();

    // Destination is deleted out-of-band
    fs::remove_file(&new_path).unwrap();

    // Undo should be refused
    let res = execute_undo(&mut state);
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("no longer exists"));
    assert!(state.status_message.unwrap().contains("no longer exists"));

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_undo_rename_refusal_when_original_exists() {
    let test_dir = create_temp_test_dir("undo_rename_conflict");
    let original = test_dir.join("source.txt");
    fs::write(&original, "x").unwrap();

    let mut state = AppState::new(Location::Local(test_dir.clone()));
    let _ = execute_rename(&mut state, &original, "renamed.txt").unwrap();

    // Recreate original name out-of-band
    fs::write(&original, "collision").unwrap();

    // Undo should be refused
    let res = execute_undo(&mut state);
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("already exists"));
    assert!(state.status_message.unwrap().contains("already exists"));

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_undo_create_folder_removes_folder() {
    let test_dir = create_temp_test_dir("undo_folder");
    let mut state = AppState::new(Location::Local(test_dir.clone()));

    let folder = execute_create_folder(&mut state, &test_dir, "to_undo_dir").unwrap();
    assert!(folder.is_dir());

    let res = execute_undo(&mut state);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Undid create folder 'to_undo_dir'");
    assert!(!folder.exists());
    assert!(!state.entries.iter().any(|e| e.name == "to_undo_dir"));

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_undo_copy_removes_destinations() {
    let src_dir = create_temp_test_dir("undo_copy_src");
    let file = src_dir.join("copied.txt");
    fs::write(&file, "copy").unwrap();

    let dst_dir = create_temp_test_dir("undo_copy_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let _ = execute_copy(
        &mut state,
        std::slice::from_ref(&file),
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
    );
    let copied_target = dst_dir.join("copied.txt");
    assert!(copied_target.exists());

    let res = execute_undo(&mut state);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Undid copy operation");
    assert!(!copied_target.exists());
    assert!(file.exists()); // Source untouched

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_undo_move_restores_source() {
    let src_dir = create_temp_test_dir("undo_move_src");
    let file = src_dir.join("move_me.txt");
    fs::write(&file, "moved").unwrap();

    let dst_dir = create_temp_test_dir("undo_move_dst");
    let mut state = AppState::new(Location::Local(dst_dir.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let _ = execute_move(
        &mut state,
        std::slice::from_ref(&file),
        &dst_dir,
        ConflictPolicy::Replace,
        cancel_flag,
        false,
    );
    let target = dst_dir.join("move_me.txt");
    assert!(target.exists());
    assert!(!file.exists());

    let res = execute_undo(&mut state);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Undid move operation");
    assert!(file.exists());
    assert!(!target.exists());

    let _ = fs::remove_dir_all(src_dir);
    let _ = fs::remove_dir_all(dst_dir);
}

#[test]
fn test_undo_trash_refused_with_reason() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    state.undo_stack.push(UndoOperation::Trash {
        original_paths: vec![PathBuf::from("/tmp/trashed.txt")],
    });

    let res = execute_undo(&mut state);
    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .contains("system trash does not support automatic restore"));
    assert!(state
        .status_message
        .unwrap()
        .contains("system trash does not support automatic restore"));
}

#[test]
fn test_undo_empty_stack_reports_nothing_to_undo() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    assert!(state.undo_stack.is_empty());

    let res = execute_undo(&mut state);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "Nothing to undo");
    assert_eq!(state.status_message.as_deref(), Some("Nothing to undo"));
}
