use kwati_core::{list_dir, Backend, BackendError, Entry, Listing, LocalBackend, Location};
use std::ffi::OsString;
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
fn directories_come_first() {
    let test_dir = TestDir::new("directories_come_first");
    let base = test_dir.path();

    std::fs::create_dir(base.join("Z_dir")).unwrap();
    std::fs::create_dir(base.join("a_dir")).unwrap();
    std::fs::write(base.join("B_file.txt"), b"").unwrap();
    std::fs::write(base.join("a_file.txt"), b"").unwrap();
    std::fs::write(base.join("c_file.txt"), b"").unwrap();

    let listing = list_dir(base).unwrap();
    assert_eq!(listing.len(), 5);

    let first_file_idx = listing.iter().position(|e| !e.is_dir).unwrap();
    assert!(listing[..first_file_idx].iter().all(|e| e.is_dir));
    assert!(listing[first_file_idx..].iter().all(|e| !e.is_dir));

    let dir_names: Vec<&str> = listing[..first_file_idx]
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert_eq!(dir_names, vec!["a_dir", "Z_dir"]);

    let file_names: Vec<&str> = listing[first_file_idx..]
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert_eq!(file_names, vec!["a_file.txt", "B_file.txt", "c_file.txt"]);
}

#[test]
fn symlink() {
    let test_dir = TestDir::new("symlink");
    let base = test_dir.path();

    let target_dir = base.join("target_dir");
    std::fs::create_dir(&target_dir).unwrap();

    let link_to_dir = base.join("link_to_dir");
    let dangling_link = base.join("dangling_link");
    let nonexistent_target = base.join("does_not_exist");

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target_dir, &link_to_dir).unwrap();
        std::os::unix::fs::symlink(&nonexistent_target, &dangling_link).unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(&target_dir, &link_to_dir).unwrap();
        std::os::windows::fs::symlink_file(&nonexistent_target, &dangling_link).unwrap();
    }

    let listing = list_dir(base).unwrap();

    let link_entry = listing
        .iter()
        .find(|e| e.name == "link_to_dir")
        .expect("link_to_dir should be listed");
    assert!(link_entry.is_dir, "symlink to a directory reports is_dir");

    let dangling_entry = listing
        .iter()
        .find(|e| e.name == "dangling_link")
        .expect("dangling symlink is listed without counting as unreadable");
    assert!(
        !dangling_entry.is_dir,
        "dangling symlink is listed as a non-directory"
    );
}

#[test]
fn non_utf8() {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let test_dir = TestDir::new("non_utf8");
        let base = test_dir.path();

        let raw_name = b"invalid_\xff\xfe_name.txt";
        let os_name = std::ffi::OsStr::from_bytes(raw_name);
        let file_path = base.join(os_name);
        std::fs::write(&file_path, b"non utf8 file content").unwrap();

        let listing = list_dir(base).unwrap();
        let entry = listing
            .iter()
            .find(|e| e.name.as_bytes() == raw_name)
            .expect("non-utf8 entry must be listed");

        assert_eq!(entry.name.as_bytes(), raw_name);
        let content = std::fs::read(&entry.path).expect("reading path of non-utf8 file succeeds");
        assert_eq!(content, b"non utf8 file content");
    }
}

#[test]
fn display_name() {
    let name = OsString::from("file\nwith\x1bescape\tand\rcontrol.txt");
    let entry = Entry::new(name, PathBuf::from("dummy"), false);
    let display = entry.display_name();

    assert!(
        !display.chars().any(|c| c.is_control()),
        "display_name must have no control characters"
    );
    assert!(!display.contains('\n'));
    assert!(!display.contains('\x1b'));
    assert!(!display.contains('\r'));
    assert!(!display.contains('\t'));
}

#[test]
fn missing_directory() {
    let test_dir = TestDir::new("missing_directory");
    let missing_path = test_dir.path().join("does_not_exist");

    let err = list_dir(&missing_path).expect_err("listing missing directory must return Err");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    let backend = LocalBackend::new();
    let backend_err = backend
        .list(&Location::Local(missing_path))
        .expect_err("backend listing missing directory must return Err");
    assert!(matches!(backend_err, BackendError::Io(_)));
}

#[test]
fn backend_sorting() {
    struct MockBackend {
        entries: Vec<Entry>,
    }

    impl Backend for MockBackend {
        fn list(&self, _location: &Location) -> Result<Listing, BackendError> {
            Ok(Listing::new(self.entries.clone()))
        }
    }

    let test_dir = TestDir::new("backend_sorting");
    let base = test_dir.path();

    std::fs::create_dir(base.join("dir_b")).unwrap();
    std::fs::create_dir(base.join("DIR_A")).unwrap();
    std::fs::write(base.join("file_z.txt"), b"").unwrap();
    std::fs::write(base.join("File_A.txt"), b"").unwrap();

    let local_listing = LocalBackend::new()
        .list(&Location::Local(base.to_path_buf()))
        .unwrap();

    let mut out_of_order_entries = local_listing.entries.clone();
    out_of_order_entries.reverse();

    let mock_backend = MockBackend {
        entries: out_of_order_entries,
    };

    let test_listing = mock_backend
        .list(&Location::Local(base.to_path_buf()))
        .unwrap();

    assert_eq!(
        test_listing.entries, local_listing.entries,
        "A test backend that returns entries out of order gets the same sorting as LocalBackend"
    );
}

#[test]
fn remote_location_unsupported() {
    let backend = LocalBackend::new();
    let remote_loc = Location::Remote {
        profile_id: "my_profile".to_string(),
        path: PathBuf::from("/remote/path/to/nonexistent/directory"),
    };

    let result = backend.list(&remote_loc);
    match result {
        Err(BackendError::Unsupported) => {}
        other => panic!("expected Err(BackendError::Unsupported), got {:?}", other),
    }
}
