use kwati_core::{
    copy_between, move_between, Backend, BackendError, Confirmed, ConflictPolicy, CopyOptions,
    CopyOutcome, Entry, EntryKind, EntryMeta, Listing, LocalBackend, Location, Progress,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::AtomicUsize;
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("kwati_test_{}_{}_{}", name, std::process::id(), id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// `None` is a folder, `Some(bytes)` a file. Paths are absolute under `/`.
type Tree = Arc<Mutex<BTreeMap<PathBuf, Option<Vec<u8>>>>>;

/// A remote-like backend with no trash, kept in memory.
#[derive(Default)]
struct MemBackend {
    tree: Tree,
}

impl MemBackend {
    fn with(nodes: &[(&str, Option<&str>)]) -> Self {
        let backend = Self::default();
        {
            let mut tree = backend.tree.lock().unwrap();
            tree.insert(PathBuf::from("/"), None);
            for (path, content) in nodes {
                tree.insert(PathBuf::from(path), content.map(|c| c.as_bytes().to_vec()));
            }
        }
        backend
    }

    fn get(&self, path: &str) -> Option<Option<Vec<u8>>> {
        self.tree.lock().unwrap().get(Path::new(path)).cloned()
    }
}

fn remote(path: &str) -> Location {
    Location::Remote {
        profile_id: "mem".into(),
        path: PathBuf::from(path),
    }
}

fn not_found() -> BackendError {
    io::Error::from(io::ErrorKind::NotFound).into()
}

struct MemWriter {
    tree: Tree,
    path: PathBuf,
}

impl Write for MemWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut tree = self.tree.lock().unwrap();
        if let Some(Some(content)) = tree.get_mut(&self.path) {
            content.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for MemBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let dir = location.path();
        let tree = self.tree.lock().unwrap();
        let entries = tree
            .iter()
            .filter(|(path, _)| path.parent() == Some(dir))
            .map(|(path, node)| {
                Entry::new(
                    path.file_name().unwrap().to_os_string(),
                    path.clone(),
                    node.is_none(),
                )
            })
            .collect::<Vec<_>>();
        Ok(Listing::new(entries))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        match self.tree.lock().unwrap().get(location.path()) {
            Some(None) => Ok(EntryMeta {
                kind: EntryKind::Dir,
                len: 0,
            }),
            Some(Some(content)) => Ok(EntryMeta {
                kind: EntryKind::File,
                len: content.len() as u64,
            }),
            None => Err(not_found()),
        }
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        match self.tree.lock().unwrap().get(location.path()) {
            Some(Some(content)) => Ok(Box::new(Cursor::new(content.clone()))),
            _ => Err(not_found()),
        }
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        let path = location.path().to_path_buf();
        self.tree
            .lock()
            .unwrap()
            .insert(path.clone(), Some(Vec::new()));
        Ok(Box::new(MemWriter {
            tree: Arc::clone(&self.tree),
            path,
        }))
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let mut tree = self.tree.lock().unwrap();
        let node = tree.remove(from.path()).ok_or_else(not_found)?;
        tree.insert(to.path().to_path_buf(), node);
        Ok(())
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        self.tree
            .lock()
            .unwrap()
            .insert(location.path().to_path_buf(), None);
        Ok(())
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        let root = location.path();
        let mut tree = self.tree.lock().unwrap();
        if tree.remove(root).is_none() {
            return Err(not_found());
        }
        tree.retain(|path, _| !path.starts_with(root));
        Ok(())
    }
}

fn options(policy: ConflictPolicy) -> CopyOptions {
    let mut options = CopyOptions::new(policy, Arc::new(AtomicBool::new(false)));
    options.chunk_size = 4;
    options
}

#[test]
fn copy_between_backends() {
    let dir = TestDir::new("copy_between_backends");
    let src = MemBackend::with(&[
        ("/docs", None),
        ("/docs/a.txt", Some("hello world")),
        ("/docs/sub", None),
        ("/docs/sub/b.txt", Some("nested")),
    ]);
    let mut reports = Vec::new();

    let outcome = copy_between(
        &src,
        &remote("/docs"),
        &LocalBackend::new(),
        &Location::Local(dir.path.clone()),
        &options(ConflictPolicy::Skip),
        &mut |p: Progress| reports.push(p),
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(
        fs::read_to_string(dir.path.join("docs/a.txt")).unwrap(),
        "hello world"
    );
    assert_eq!(
        fs::read_to_string(dir.path.join("docs/sub/b.txt")).unwrap(),
        "nested"
    );
    let total = ("hello world".len() + "nested".len()) as u64;
    assert_eq!(reports.first().unwrap().bytes_done, 0);
    assert_eq!(
        *reports.last().unwrap(),
        Progress {
            bytes_done: total,
            bytes_total: total
        }
    );
    assert!(
        reports.len() > 3,
        "a 4-byte chunk size must report per chunk"
    );
    assert!(reports
        .windows(2)
        .all(|w| w[0].bytes_done <= w[1].bytes_done));
    assert!(src.get("/docs/a.txt").is_some(), "copy keeps the source");
}

#[test]
fn copy_between_backends_cancel_removes_partial_target() {
    let dir = TestDir::new("copy_between_cancel");
    let src = MemBackend::with(&[("/big.bin", Some("0123456789abcdef"))]);
    let options = options(ConflictPolicy::Skip);
    let cancel = Arc::clone(&options.cancel);

    let outcome = copy_between(
        &src,
        &remote("/big.bin"),
        &LocalBackend::new(),
        &Location::Local(dir.path.clone()),
        &options,
        &mut |p: Progress| {
            if p.bytes_done >= 8 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::Cancelled);
    assert_eq!(fs::read_dir(&dir.path).unwrap().count(), 0);
}

#[test]
fn move_between_backends_removes_source_after_copy() {
    let src = MemBackend::with(&[("/a.txt", Some("payload"))]);
    let dst = MemBackend::with(&[("/inbox", None)]);

    let outcome = move_between(
        &src,
        &remote("/a.txt"),
        &dst,
        &remote("/inbox"),
        &options(ConflictPolicy::Skip),
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(dst.get("/inbox/a.txt"), Some(Some(b"payload".to_vec())));
    assert_eq!(src.get("/a.txt"), None);
}

#[test]
fn move_between_backends_keeps_skipped_source() {
    let src = MemBackend::with(&[
        ("/docs", None),
        ("/docs/same.txt", Some("new")),
        ("/docs/other.txt", Some("moved")),
    ]);
    let dst = MemBackend::with(&[("/docs", None), ("/docs/same.txt", Some("old"))]);

    move_between(
        &src,
        &remote("/docs"),
        &dst,
        &remote("/"),
        &options(ConflictPolicy::Skip),
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(dst.get("/docs/same.txt"), Some(Some(b"old".to_vec())));
    assert_eq!(dst.get("/docs/other.txt"), Some(Some(b"moved".to_vec())));
    assert_eq!(src.get("/docs/same.txt"), Some(Some(b"new".to_vec())));
    assert_eq!(src.get("/docs/other.txt"), None);
}

#[test]
fn trash_unsupported_backend() {
    let backend = MemBackend::with(&[("/a.txt", Some("x"))]);

    let err = backend.trash(&remote("/a.txt")).unwrap_err();

    assert!(err.is_trash_unavailable(), "got {err:?}");
    assert!(backend.get("/a.txt").is_some());
}

#[test]
fn local_backend_trashes_local_files() {
    let dir = TestDir::new("local_backend_trash");
    let file = dir.path.join("gone.txt");
    fs::write(&file, "x").unwrap();

    LocalBackend::new()
        .trash(&Location::Local(file.clone()))
        .unwrap();

    assert!(!file.exists());
}

#[test]
fn local_backend_lists_local_disk_without_the_bound() {
    let dir = TestDir::new("local_backend_unbounded");
    fs::write(dir.path.join("a.txt"), "x").unwrap();

    let listing = LocalBackend::with_list_timeout(Duration::ZERO)
        .list(&Location::Local(dir.path.clone()))
        .unwrap();

    assert_eq!(listing.len(), 1);
}

#[test]
fn local_backend_operations_round_trip() {
    let dir = TestDir::new("local_backend_ops");
    let backend = LocalBackend::new();
    let folder = Location::Local(dir.path.join("new"));
    let file = folder.join("f.txt".as_ref());

    backend.create_dir(&folder).unwrap();
    backend
        .open_write(&file)
        .unwrap()
        .write_all(b"data")
        .unwrap();
    let meta = backend.stat(&file).unwrap();
    let mut read = String::new();
    backend
        .open_read(&file)
        .unwrap()
        .read_to_string(&mut read)
        .unwrap();
    let renamed = folder.join("g.txt".as_ref());
    backend.rename(&file, &renamed).unwrap();
    backend
        .delete_permanently(&folder, Confirmed::user_confirmed())
        .unwrap();

    assert_eq!(
        meta,
        EntryMeta {
            kind: EntryKind::File,
            len: 4
        }
    );
    assert_eq!(read, "data");
    assert!(!dir.path.join("new").exists());
}

#[test]
fn a_remote_listing_honours_hidden_files_and_sort_order() {
    use kwati_core::{ListOptions, SortColumn, SortDirection};
    let backend = MemBackend::with(&[
        ("/b.txt", Some("")),
        ("/.secret", Some("")),
        ("/a.txt", Some("")),
        ("/dir", None),
    ]);
    let names = |options: ListOptions| -> Vec<String> {
        backend
            .list_with_options(&remote("/"), &options)
            .unwrap()
            .iter()
            .map(|e| e.display_name())
            .collect()
    };
    let descending = ListOptions {
        show_hidden: false,
        sort_column: SortColumn::Name,
        sort_direction: SortDirection::Descending,
    };
    assert_eq!(names(descending), ["dir", "b.txt", "a.txt"]);
    assert_eq!(
        names(ListOptions {
            show_hidden: true,
            ..descending
        }),
        ["dir", "b.txt", "a.txt", ".secret"]
    );
}
