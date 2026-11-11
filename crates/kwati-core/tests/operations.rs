use kwati_core::{create_dir, rename_entry};
use std::path::PathBuf;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("kwati_test_{}_{}_{}", name, std::process::id(), id));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn rename_refuses_existing() {
    let test_dir = TestDir::new("rename_refuses_existing");
    let base = test_dir.path();

    let source = base.join("source.txt");
    let target = base.join("target.txt");

    std::fs::write(&source, b"source content").unwrap();
    std::fs::write(&target, b"target content").unwrap();

    // Renaming onto existing target using the new name must return an error
    let result = rename_entry(&source, "target.txt");
    assert!(
        result.is_err(),
        "renaming onto existing target must return an error"
    );
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists,
        "error must be AlreadyExists"
    );

    // Leaves both files with their original content
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"source content",
        "source content must remain unchanged"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"target content",
        "target content must remain unchanged"
    );

    // Also verify when passing full target path
    let result_full = rename_entry(&source, &target);
    assert!(
        result_full.is_err(),
        "renaming onto existing target path must return an error"
    );
    assert_eq!(
        result_full.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"source content");
    assert_eq!(std::fs::read(&target).unwrap(), b"target content");
}

#[test]
fn rename_refuses_separator() {
    let test_dir = TestDir::new("rename_refuses_separator");
    let base = test_dir.path();

    let source = base.join("source.txt");
    std::fs::write(&source, b"source content").unwrap();

    // A name containing a path separator is refused
    let result = rename_entry(&source, "sub/name.txt");
    assert!(
        result.is_err(),
        "a name containing a path separator must return an error"
    );
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput,
        "error kind should be InvalidInput"
    );

    // Nothing moves and source is intact
    assert!(source.exists(), "source must remain at original location");
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"source content",
        "source content must remain intact"
    );
    assert!(
        !base.join("sub").exists(),
        "sub directory must not be created"
    );
    assert!(!base.join("sub/name.txt").exists(), "target must not exist");

    // Also test with leading / absolute separator
    let result_leading = rename_entry(&source, "/name.txt");
    assert!(
        result_leading.is_err(),
        "leading separator in name must be refused"
    );
    assert!(source.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"source content");
}

#[test]
fn create_dir_refuses_existing() {
    let test_dir = TestDir::new("create_dir_refuses_existing");
    let base = test_dir.path();

    let existing_folder = base.join("my_folder");
    std::fs::create_dir(&existing_folder).unwrap();
    let file_inside = existing_folder.join("file.txt");
    std::fs::write(&file_inside, b"keep this content").unwrap();

    let result = create_dir(&existing_folder);
    assert!(
        result.is_err(),
        "creating folder with existing name must return an error"
    );
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists,
        "error kind must be AlreadyExists"
    );

    // Keeps its content
    assert!(existing_folder.is_dir(), "folder must still exist");
    assert_eq!(
        std::fs::read(&file_inside).unwrap(),
        b"keep this content",
        "files inside existing folder must be preserved"
    );

    // Also test when an existing entry is a regular file
    let existing_file = base.join("a_file.txt");
    std::fs::write(&existing_file, b"existing file content").unwrap();

    let file_result = create_dir(&existing_file);
    assert!(
        file_result.is_err(),
        "creating directory over an existing file must return an error"
    );
    assert_eq!(
        file_result.unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        std::fs::read(&existing_file).unwrap(),
        b"existing file content",
        "existing file content must be preserved"
    );
}

#[test]
fn rename_success() {
    let test_dir = TestDir::new("rename_success");
    let base = test_dir.path();

    let source = base.join("old_name.txt");
    let target = base.join("new_name.txt");
    std::fs::write(&source, b"hello world").unwrap();

    rename_entry(&source, "new_name.txt").expect("valid rename must succeed");

    assert!(!source.exists(), "source file must no longer exist");
    assert!(target.exists(), "target file must exist");
    assert_eq!(std::fs::read(&target).unwrap(), b"hello world");
}

#[test]
fn rename_directory() {
    let test_dir = TestDir::new("rename_directory");
    let base = test_dir.path();

    let source_dir = base.join("old_folder");
    let target_dir = base.join("new_folder");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::write(source_dir.join("inner.txt"), b"inner content").unwrap();

    rename_entry(&source_dir, "new_folder").expect("renaming directory must succeed");

    assert!(!source_dir.exists(), "old folder must no longer exist");
    assert!(
        target_dir.is_dir(),
        "new folder must exist and be a directory"
    );
    assert_eq!(
        std::fs::read(target_dir.join("inner.txt")).unwrap(),
        b"inner content"
    );
}

#[test]
fn create_dir_success() {
    let test_dir = TestDir::new("create_dir_success");
    let base = test_dir.path();

    let new_folder = base.join("new_folder");
    create_dir(&new_folder).expect("create_dir on non-existing path must succeed");

    assert!(
        new_folder.is_dir(),
        "new folder must exist and be a directory"
    );
}

#[test]
fn rename_nonexistent_source() {
    let test_dir = TestDir::new("rename_nonexistent_source");
    let base = test_dir.path();

    let nonexistent = base.join("does_not_exist.txt");
    let result = rename_entry(&nonexistent, "new_name.txt");
    assert!(result.is_err(), "renaming nonexistent source must fail");
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}
