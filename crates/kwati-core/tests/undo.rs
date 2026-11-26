use kwati_core::{
    is_trash_browsing_supported, is_undo_refused, list_trash, purge_trash_items, Confirmed,
    ConflictPolicy, CopyOptions, CopyOutcome, TrashItem, UndoAction, UndoStack,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn normalize_test_dir(raw: PathBuf) -> PathBuf {
    let canon = raw.canonicalize().unwrap_or(raw);
    #[cfg(windows)]
    {
        let s = canon.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    canon
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let raw = std::env::temp_dir().join(format!("kwati_undo_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&raw);
        std::fs::create_dir_all(&raw).expect("failed to create test dir");
        // trash::delete canonicalizes parent directories (and strips \\?\ on Windows), so match
        // the exact form stored in the system trash.
        let path = normalize_test_dir(raw);
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if is_trash_browsing_supported() {
            if let Ok(items) = list_trash() {
                let leftovers: Vec<TrashItem> = items
                    .into_iter()
                    .filter(|item| item.original_path().starts_with(&self.path))
                    .collect();
                if !leftovers.is_empty() {
                    let _ = purge_trash_items(&leftovers, Confirmed::user_confirmed());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn default_copy_options() -> CopyOptions {
    CopyOptions::new(ConflictPolicy::Skip, Arc::new(AtomicBool::new(false)))
}

#[test]
fn undo_rename() {
    let dir = TestDir::new("undo_rename");
    let original = dir.path.join("before.txt");
    std::fs::write(&original, b"rename payload").unwrap();

    let mut stack = UndoStack::new();
    assert_eq!(stack.undo_label(), None);

    let renamed = stack
        .rename_and_record(&original, Path::new("after.txt"))
        .unwrap();

    assert!(!original.exists());
    assert!(renamed.exists());
    assert_eq!(stack.len(), 1);
    assert_eq!(stack.undo_label(), Some("Undo rename"));

    let action = stack.undo().unwrap();
    assert!(matches!(action, UndoAction::Rename { .. }));
    assert!(!renamed.exists());
    assert_eq!(std::fs::read(&original).unwrap(), b"rename payload");
    assert!(stack.is_empty());
    assert_eq!(stack.undo_label(), None);
}

#[test]
fn undo_move() {
    let dir = TestDir::new("undo_move");
    let src_dir = dir.path.join("src");
    let dest_dir = dir.path.join("dest");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dest_dir).unwrap();

    let first = src_dir.join("alpha.txt");
    let second = src_dir.join("beta.txt");
    std::fs::write(&first, b"alpha").unwrap();
    std::fs::write(&second, b"beta").unwrap();

    let mut stack = UndoStack::new();
    let outcome = stack
        .move_and_record(
            &[first.clone(), second.clone()],
            &dest_dir,
            &default_copy_options(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(stack.undo_label(), Some("Undo move"));

    assert!(!first.exists());
    assert!(!second.exists());
    assert!(dest_dir.join("alpha.txt").exists());
    assert!(dest_dir.join("beta.txt").exists());

    let action = stack.undo().unwrap();
    assert!(matches!(action, UndoAction::Move { .. }));
    assert!(!dest_dir.join("alpha.txt").exists());
    assert!(!dest_dir.join("beta.txt").exists());
    assert_eq!(std::fs::read(&first).unwrap(), b"alpha");
    assert_eq!(std::fs::read(&second).unwrap(), b"beta");
}

#[test]
fn undo_copy() {
    let dir = TestDir::new("undo_copy");
    let src_dir = dir.path.join("src");
    let dest_dir = dir.path.join("dest");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&dest_dir).unwrap();

    let src1 = src_dir.join("doc1.txt");
    let src2 = src_dir.join("doc2.txt");
    std::fs::write(&src1, b"first source").unwrap();
    std::fs::write(&src2, b"second source").unwrap();

    let mut stack = UndoStack::new();
    let outcome = stack
        .copy_and_record(
            &[src1.clone(), src2.clone()],
            &dest_dir,
            &default_copy_options(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(stack.undo_label(), Some("Undo copy"));

    let copy1 = dest_dir.join("doc1.txt");
    let copy2 = dest_dir.join("doc2.txt");
    assert!(copy1.exists());
    assert!(copy2.exists());

    let action = stack.undo().unwrap();
    assert!(matches!(action, UndoAction::Copy { .. }));
    assert!(!copy1.exists(), "copied doc1 should be trashed");
    assert!(!copy2.exists(), "copied doc2 should be trashed");
    assert_eq!(std::fs::read(&src1).unwrap(), b"first source");
    assert_eq!(std::fs::read(&src2).unwrap(), b"second source");
}

#[test]
fn undo_trash() {
    let dir = TestDir::new("undo_trash");
    let doomed = dir.path.join("trashed.txt");
    std::fs::write(&doomed, b"recycle bin contents").unwrap();

    let mut stack = UndoStack::new();
    stack
        .trash_and_record(std::slice::from_ref(&doomed))
        .unwrap();
    assert!(!doomed.exists());
    assert_eq!(stack.len(), 1);
    assert_eq!(stack.undo_label(), Some("Undo trash"));

    if is_trash_browsing_supported() {
        let action = stack.undo().unwrap();
        assert!(matches!(action, UndoAction::Trash { .. }));
        assert_eq!(std::fs::read(&doomed).unwrap(), b"recycle bin contents");
        assert!(stack.is_empty());
    } else {
        let err = stack.undo().unwrap_err();
        assert!(is_undo_refused(&err), "got: {err}");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(!doomed.exists());
    }
}

#[test]
fn undo_create_dir() {
    let dir = TestDir::new("undo_create_dir");
    let created = dir.path.join("new-folder");

    let mut stack = UndoStack::new();
    stack.create_dir_and_record(&created).unwrap();
    assert!(created.is_dir());
    assert_eq!(stack.undo_label(), Some("Undo new folder"));

    let child = created.join("inside.txt");
    std::fs::write(&child, b"keep folder").unwrap();

    let err = stack.undo().unwrap_err();
    assert!(is_undo_refused(&err), "got: {err}");
    assert!(created.is_dir(), "non-empty folder must remain on disk");
    assert_eq!(std::fs::read(&child).unwrap(), b"keep folder");
    assert_eq!(stack.len(), 1);

    std::fs::remove_file(&child).unwrap();
    stack.undo().unwrap();
    assert!(!created.exists(), "empty folder should now be removed");
    assert!(stack.is_empty());
}

#[test]
fn undo_refuses_stale() {
    let dir = TestDir::new("undo_refuses_stale");

    // Modified content (same byte length to prove snapshot detects in-place edits).
    let orig_a = dir.path.join("a-old.txt");
    std::fs::write(&orig_a, b"aaaa").unwrap();
    let mut stack_mod = UndoStack::new();
    let renamed_a = stack_mod
        .rename_and_record(&orig_a, Path::new("a-new.txt"))
        .unwrap();
    std::fs::write(&renamed_a, b"bbbb").unwrap();

    let err_mod = stack_mod.undo().unwrap_err();
    assert!(is_undo_refused(&err_mod), "got: {err_mod}");
    assert!(!orig_a.exists());
    assert_eq!(std::fs::read(&renamed_a).unwrap(), b"bbbb");
    assert_eq!(stack_mod.len(), 1);

    // Moved away since the rename.
    let orig_b = dir.path.join("b-old.txt");
    std::fs::write(&orig_b, b"moving").unwrap();
    let mut stack_moved = UndoStack::new();
    let renamed_b = stack_moved
        .rename_and_record(&orig_b, Path::new("b-new.txt"))
        .unwrap();
    let elsewhere = dir.path.join("b-elsewhere.txt");
    std::fs::rename(&renamed_b, &elsewhere).unwrap();

    let err_moved = stack_moved.undo().unwrap_err();
    assert!(is_undo_refused(&err_moved), "got: {err_moved}");
    assert!(!orig_b.exists());
    assert!(!renamed_b.exists());
    assert_eq!(std::fs::read(&elsewhere).unwrap(), b"moving");
    assert_eq!(stack_moved.len(), 1);

    // Collision on trash restore refuses safely without overwriting.
    if is_trash_browsing_supported() {
        let trashed_path = dir.path.join("collision.txt");
        std::fs::write(&trashed_path, b"trashed").unwrap();
        let mut stack_trash = UndoStack::new();
        stack_trash
            .trash_and_record(std::slice::from_ref(&trashed_path))
            .unwrap();
        std::fs::write(&trashed_path, b"replacement").unwrap();

        let err_trash = stack_trash.undo().unwrap_err();
        assert!(is_undo_refused(&err_trash), "got: {err_trash}");
        assert_eq!(std::fs::read(&trashed_path).unwrap(), b"replacement");
        assert_eq!(stack_trash.len(), 1);
    }
}

#[test]
fn undo_skips_permanent() {
    let dir = TestDir::new("undo_skips_permanent");
    let folder = dir.path.join("folder");
    let doomed = dir.path.join("permanent.txt");
    std::fs::write(&doomed, b"bye").unwrap();

    let mut stack = UndoStack::new();
    stack.create_dir_and_record(&folder).unwrap();
    assert_eq!(stack.len(), 1);

    stack
        .delete_permanently_and_record(std::slice::from_ref(&doomed), Confirmed::user_confirmed())
        .unwrap();
    stack.record_permanent_delete();

    assert!(!doomed.exists());
    assert_eq!(stack.len(), 1);
    assert_eq!(
        stack.peek(),
        Some(&UndoAction::CreateDir {
            path: folder.clone()
        })
    );

    stack.undo().unwrap();
    assert!(!folder.exists());
    assert!(stack.is_empty());
}
