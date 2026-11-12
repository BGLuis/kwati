use kwati_core::{
    is_hidden_entry, list_dir, list_dir_with_options, sort_entries, sort_entries_by, Backend,
    BackendError, Entry, ListOptions, Listing, LocalBackend, Location, SortColumn, SortDirection,
};
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

#[test]
fn sort_by_size() {
    let test_dir = TestDir::new("sort_by_size");
    let base = test_dir.path();

    std::fs::create_dir(base.join("dir_b")).unwrap();
    std::fs::create_dir(base.join("dir_a")).unwrap();
    std::fs::write(base.join("small.txt"), b"123").unwrap(); // 3 bytes
    std::fs::write(base.join("medium.txt"), b"123456").unwrap(); // 6 bytes
    std::fs::write(base.join("large.txt"), b"1234567890").unwrap(); // 10 bytes

    // 1. Ascending sort by size: directories first, then files smallest to largest
    let asc_options = ListOptions::default().sort(SortColumn::Size, SortDirection::Ascending);
    let listing_asc = list_dir_with_options(base, &asc_options).unwrap();
    assert_eq!(listing_asc.len(), 5);

    // Verify directories come first
    assert!(listing_asc[0].is_dir);
    assert!(listing_asc[1].is_dir);
    assert!(!listing_asc[2].is_dir);
    assert!(!listing_asc[3].is_dir);
    assert!(!listing_asc[4].is_dir);

    let names_asc: Vec<&str> = listing_asc
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert_eq!(
        names_asc,
        vec!["dir_a", "dir_b", "small.txt", "medium.txt", "large.txt"]
    );
    assert_eq!(listing_asc[2].size, Some(3));
    assert_eq!(listing_asc[3].size, Some(6));
    assert_eq!(listing_asc[4].size, Some(10));

    // 2. Descending sort by size: directories STILL first, then files largest to smallest
    let desc_options = ListOptions::default().sort(SortColumn::Size, SortDirection::Descending);
    let listing_desc = list_dir_with_options(base, &desc_options).unwrap();
    assert_eq!(listing_desc.len(), 5);

    // Directories must remain first even when descending
    assert!(listing_desc[0].is_dir);
    assert!(listing_desc[1].is_dir);
    assert!(!listing_desc[2].is_dir);
    assert!(!listing_desc[3].is_dir);
    assert!(!listing_desc[4].is_dir);

    let names_desc: Vec<&str> = listing_desc
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert_eq!(
        names_desc,
        vec!["dir_a", "dir_b", "large.txt", "medium.txt", "small.txt"]
    );
    assert_eq!(listing_desc[2].size, Some(10));
    assert_eq!(listing_desc[3].size, Some(6));
    assert_eq!(listing_desc[4].size, Some(3));

    // 3. In-memory sorting test with explicit entries
    let mut entries = vec![
        Entry::new(OsString::from("f_small"), PathBuf::from("f_small"), false).with_size(10),
        Entry::new(OsString::from("d_two"), PathBuf::from("d_two"), true),
        Entry::new(OsString::from("f_large"), PathBuf::from("f_large"), false).with_size(100),
        Entry::new(OsString::from("d_one"), PathBuf::from("d_one"), true),
    ];

    sort_entries_by(&mut entries, SortColumn::Size, SortDirection::Ascending);
    let names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["d_one", "d_two", "f_small", "f_large"]);

    sort_entries_by(&mut entries, SortColumn::Size, SortDirection::Descending);
    let names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["d_one", "d_two", "f_large", "f_small"]);
}

#[cfg(unix)]
#[test]
fn hidden_entries() {
    let test_dir = TestDir::new("hidden_entries");
    let base = test_dir.path();

    std::fs::write(base.join(".hidden_file"), b"secret").unwrap();
    std::fs::create_dir(base.join(".hidden_dir")).unwrap();
    std::fs::write(base.join("visible_file.txt"), b"visible").unwrap();
    std::fs::create_dir(base.join("visible_dir")).unwrap();

    // Excluded by default:
    let listing = list_dir(base).unwrap();
    let names: Vec<&str> = listing.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["visible_dir", "visible_file.txt"]);
    assert_eq!(listing.len(), 2);
    for entry in listing.iter() {
        assert!(!entry.is_hidden);
    }

    // Included when requested:
    let options = ListOptions::default().show_hidden(true);
    let listing_with_hidden = list_dir_with_options(base, &options).unwrap();
    assert_eq!(listing_with_hidden.len(), 4);

    let all_names: Vec<&str> = listing_with_hidden
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert!(all_names.contains(&".hidden_file"));
    assert!(all_names.contains(&".hidden_dir"));
    assert!(all_names.contains(&"visible_file.txt"));
    assert!(all_names.contains(&"visible_dir"));

    let hidden_file_entry = listing_with_hidden
        .iter()
        .find(|e| e.name == ".hidden_file")
        .unwrap();
    assert!(hidden_file_entry.is_hidden);
    assert!(!hidden_file_entry.is_dir);

    let hidden_dir_entry = listing_with_hidden
        .iter()
        .find(|e| e.name == ".hidden_dir")
        .unwrap();
    assert!(hidden_dir_entry.is_hidden);
    assert!(hidden_dir_entry.is_dir);

    let visible_file_entry = listing_with_hidden
        .iter()
        .find(|e| e.name == "visible_file.txt")
        .unwrap();
    assert!(!visible_file_entry.is_hidden);

    let visible_dir_entry = listing_with_hidden
        .iter()
        .find(|e| e.name == "visible_dir")
        .unwrap();
    assert!(!visible_dir_entry.is_hidden);
}

#[cfg(windows)]
#[test]
fn hidden_attribute() {
    let test_dir = TestDir::new("hidden_attribute");
    let base = test_dir.path();

    let secret_file = base.join("secret.txt");
    let normal_file = base.join("normal.txt");
    std::fs::write(&secret_file, b"secret").unwrap();
    std::fs::write(&normal_file, b"normal").unwrap();

    // Mark secret.txt as hidden using attrib.exe
    let status = std::process::Command::new("attrib")
        .args(["+h", secret_file.to_str().unwrap()])
        .status()
        .expect("attrib command failed");
    assert!(status.success(), "attrib +h must succeed");

    // Hidden by default:
    let listing = list_dir(base).unwrap();
    let names: Vec<&str> = listing.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["normal.txt"]);

    // Included when requested:
    let options = ListOptions::default().show_hidden(true);
    let listing_with_hidden = list_dir_with_options(base, &options).unwrap();
    let names_with_hidden: Vec<&str> = listing_with_hidden
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert!(names_with_hidden.contains(&"secret.txt"));
    assert!(names_with_hidden.contains(&"normal.txt"));

    let secret_entry = listing_with_hidden
        .iter()
        .find(|e| e.name == "secret.txt")
        .unwrap();
    assert!(secret_entry.is_hidden);
}

#[test]
fn sort_by_name_directions() {
    let test_dir = TestDir::new("sort_by_name_directions");
    let base = test_dir.path();

    std::fs::create_dir(base.join("dir_z")).unwrap();
    std::fs::create_dir(base.join("dir_a")).unwrap();
    std::fs::write(base.join("file_z.txt"), b"").unwrap();
    std::fs::write(base.join("file_a.txt"), b"").unwrap();

    let asc_options = ListOptions::default().sort(SortColumn::Name, SortDirection::Ascending);
    let listing_asc = list_dir_with_options(base, &asc_options).unwrap();
    let names_asc: Vec<&str> = listing_asc
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    assert_eq!(
        names_asc,
        vec!["dir_a", "dir_z", "file_a.txt", "file_z.txt"]
    );

    let desc_options = ListOptions::default().sort(SortColumn::Name, SortDirection::Descending);
    let listing_desc = list_dir_with_options(base, &desc_options).unwrap();
    let names_desc: Vec<&str> = listing_desc
        .iter()
        .map(|e| e.name.to_str().unwrap())
        .collect();
    // Directories must still come first!
    assert_eq!(
        names_desc,
        vec!["dir_z", "dir_a", "file_z.txt", "file_a.txt"]
    );
}

#[test]
fn sort_by_date() {
    let t_base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    let t_old = t_base + std::time::Duration::from_secs(10);
    let t_mid = t_base + std::time::Duration::from_secs(20);
    let t_new = t_base + std::time::Duration::from_secs(30);

    let mut entries = vec![
        Entry::new(OsString::from("f_new"), PathBuf::from("f_new"), false).with_modified(t_new),
        Entry::new(OsString::from("d_old"), PathBuf::from("d_old"), true).with_modified(t_old),
        Entry::new(OsString::from("f_old"), PathBuf::from("f_old"), false).with_modified(t_old),
        Entry::new(OsString::from("d_new"), PathBuf::from("d_new"), true).with_modified(t_new),
        Entry::new(OsString::from("f_mid"), PathBuf::from("f_mid"), false).with_modified(t_mid),
    ];

    // Ascending by date: directories first (old to new), then files (old to new)
    sort_entries_by(&mut entries, SortColumn::Date, SortDirection::Ascending);
    let names_asc: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names_asc, vec!["d_old", "d_new", "f_old", "f_mid", "f_new"]);

    // Descending by date: directories still first (new to old), then files (new to old)
    sort_entries_by(&mut entries, SortColumn::Date, SortDirection::Descending);
    let names_desc: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(
        names_desc,
        vec!["d_new", "d_old", "f_new", "f_mid", "f_old"]
    );
}

#[test]
fn entry_metadata_retrieval() {
    let test_dir = TestDir::new("entry_metadata_retrieval");
    let base = test_dir.path();

    let file_path = base.join("sample.txt");
    std::fs::write(&file_path, b"hello world").unwrap(); // 11 bytes
    let dir_path = base.join("subdir");
    std::fs::create_dir(&dir_path).unwrap();

    let listing = list_dir(base).unwrap();
    let file_entry = listing.iter().find(|e| e.name == "sample.txt").unwrap();
    assert_eq!(file_entry.size, Some(11));
    assert_eq!(file_entry.size(), Some(11));
    assert!(file_entry.modified.is_some());
    assert!(file_entry.modified().is_some());
    assert!(!file_entry.is_hidden);
    assert!(!file_entry.is_hidden());

    let dir_entry = listing.iter().find(|e| e.name == "subdir").unwrap();
    assert_eq!(dir_entry.size, None);
    assert_eq!(dir_entry.size(), None);
    assert!(dir_entry.modified.is_some());
    assert!(!dir_entry.is_hidden);
}

#[test]
fn default_sort_entries_helper() {
    let mut entries = vec![
        Entry::new(OsString::from("file_b"), PathBuf::from("file_b"), false),
        Entry::new(OsString::from("dir_b"), PathBuf::from("dir_b"), true),
        Entry::new(OsString::from("file_a"), PathBuf::from("file_a"), false),
        Entry::new(OsString::from("dir_a"), PathBuf::from("dir_a"), true),
    ];
    sort_entries(&mut entries);
    let names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
    assert_eq!(names, vec!["dir_a", "dir_b", "file_a", "file_b"]);
}

#[test]
fn is_hidden_entry_direct() {
    #[cfg(unix)]
    {
        assert!(is_hidden_entry(std::ffi::OsStr::new(".hidden"), None));
        assert!(!is_hidden_entry(std::ffi::OsStr::new("visible"), None));
    }
}
