use kwati_core::{
    is_restore_conflict, is_trash_browsing_supported, is_trash_unavailable, list_trash,
    purge_trash_items, restore_conflict, restore_trash_items, trash, trash_with, Confirmed,
    TrashItem, VolumeKind,
};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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
        let raw = std::env::temp_dir().join(format!("kwati_test_{}_{}", name, std::process::id()));
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
                    .filter(|item| item.original_parent == self.path)
                    .collect();
                if !leftovers.is_empty() {
                    let _ = purge_trash_items(&leftovers, Confirmed::user_confirmed());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
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
fn find_trashed(path: &Path) -> Option<TrashItem> {
    list_trash()
        .expect("list_trash should succeed")
        .into_iter()
        .find(|item| item.original_path() == path)
}

#[test]
fn trash_removes_entry() {
    let dir = TestDir::new("trash_removes_entry");
    let file = dir.path.join("doomed.txt");
    std::fs::write(&file, b"x").unwrap();

    trash(&file).unwrap();

    assert!(!file.exists());
    assert!(std::fs::read_dir(&dir.path).unwrap().next().is_none());
}

#[test]
fn trash_missing_path() {
    let dir = TestDir::new("trash_missing_path");
    let missing = dir.path.join("does-not-exist.txt");

    let err = trash(&missing).unwrap_err();

    assert_eq!(err.kind(), ErrorKind::NotFound);
    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "error should name the path, got: {err}"
    );
}

#[test]
fn trash_refuses_network() {
    let dir = TestDir::new("trash_refuses_network");
    let file = dir.path.join("on-share.txt");
    std::fs::write(&file, b"x").unwrap();

    let err = trash_with(&file, |_| VolumeKind::Network).unwrap_err();

    assert!(is_trash_unavailable(&err), "got: {err}");
    assert!(file.exists());
}

#[test]
fn trash_refuses_unknown() {
    let dir = TestDir::new("trash_refuses_unknown");
    let file = dir.path.join("somewhere.txt");
    std::fs::write(&file, b"x").unwrap();

    let err = trash_with(&file, |_| VolumeKind::Unknown).unwrap_err();

    assert!(is_trash_unavailable(&err), "got: {err}");
    assert!(file.exists());
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
#[test]
fn trash_restore() {
    let dir = TestDir::new("trash_restore");
    let file = dir.path.join("restore_me.txt");
    std::fs::write(&file, b"preserved payload").unwrap();

    trash(&file).unwrap();
    assert!(!file.exists());

    let item = find_trashed(&file).expect("trashed item should appear in list_trash");
    assert_eq!(item.name, "restore_me.txt");
    assert_eq!(item.original_parent, dir.path);
    assert!(item.deleted_at().is_some());

    restore_trash_items(std::slice::from_ref(&item)).unwrap();

    assert_eq!(std::fs::read(&file).unwrap(), b"preserved payload");
    assert!(
        !list_trash()
            .unwrap()
            .into_iter()
            .any(|listed| listed.id == item.id),
        "restored item should leave the trash"
    );
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
#[test]
fn trash_restore_conflict() {
    let dir = TestDir::new("trash_restore_conflict");
    let file = dir.path.join("conflict.txt");
    std::fs::write(&file, b"trashed version").unwrap();

    trash(&file).unwrap();
    let item = find_trashed(&file).expect("trashed item should appear in list_trash");

    std::fs::write(&file, b"existing version").unwrap();

    let err = restore_trash_items(std::slice::from_ref(&item)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AlreadyExists);
    assert!(is_restore_conflict(&err), "got: {err}");
    let conflict = restore_conflict(&err).expect("RestoreConflict should be attached");
    assert_eq!(conflict.item_name, "conflict.txt");
    assert_eq!(conflict.destination, file);

    // Existing file and trashed item must both stay untouched.
    assert_eq!(std::fs::read(&file).unwrap(), b"existing version");
    assert!(
        list_trash()
            .unwrap()
            .into_iter()
            .any(|listed| listed.id == item.id),
        "conflicting item should stay in the trash"
    );
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
#[test]
fn trash_purge() {
    let dir = TestDir::new("trash_purge");
    let doomed = dir.path.join("purge_me.txt");
    let spared = dir.path.join("keep_me.txt");
    std::fs::write(&doomed, b"bye").unwrap();
    std::fs::write(&spared, b"stay").unwrap();

    trash(&doomed).unwrap();
    trash(&spared).unwrap();

    let doomed_item = find_trashed(&doomed).expect("doomed item should be in trash");
    let spared_item = find_trashed(&spared).expect("spared item should be in trash");

    purge_trash_items(
        std::slice::from_ref(&doomed_item),
        Confirmed::user_confirmed(),
    )
    .unwrap();

    let remaining = list_trash().unwrap();
    assert!(!remaining.iter().any(|item| item.id == doomed_item.id));
    assert!(remaining.iter().any(|item| item.id == spared_item.id));
}

#[cfg(target_os = "macos")]
#[test]
fn trash_list_macos() {
    assert!(!is_trash_browsing_supported());
    let err = list_trash().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);

    let dummy = TrashItem {
        id: "dummy".into(),
        name: "dummy.txt".into(),
        original_parent: PathBuf::from("/tmp"),
        time_deleted: 0,
    };
    assert_eq!(
        restore_trash_items(std::slice::from_ref(&dummy))
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
    assert_eq!(
        purge_trash_items(&[dummy], Confirmed::user_confirmed())
            .unwrap_err()
            .kind(),
        ErrorKind::Unsupported
    );
}
