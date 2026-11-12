use kwati_core::{move_entry, move_entry_with, ConflictPolicy, CopyOptions, CopyOutcome};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn options() -> CopyOptions {
    CopyOptions::new(ConflictPolicy::KeepBoth, Arc::new(AtomicBool::new(false)))
}

fn crosses_devices(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::CrossesDevices.into())
}

fn tree(root: &Path) -> (PathBuf, PathBuf) {
    let src = root.join("src");
    let dest = root.join("dest");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(src.join("sub/big.bin"), [7u8; 64]).unwrap();
    fs::write(src.join("small.txt"), b"hi").unwrap();
    (src, dest)
}

#[test]
fn move_same_fs() {
    let t = TestDir::new("move_same_fs");
    let (src, dest) = tree(t.path());

    let outcome = move_entry(&src, &dest, &options(), &mut |_| {}).unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    assert!(!src.exists());
    assert_eq!(fs::read(dest.join("src/sub/big.bin")).unwrap(), [7u8; 64]);
    assert_eq!(fs::read(dest.join("src/small.txt")).unwrap(), b"hi");
}

#[test]
fn move_cross_device_fallback() {
    let t = TestDir::new("move_cross_device_fallback");
    let (src, dest) = tree(t.path());
    let mut src_present_at_last_progress = false;

    let outcome = move_entry_with(
        &src,
        &dest,
        &options(),
        &mut |p| {
            if p.bytes_done == p.bytes_total {
                src_present_at_last_progress = src.exists();
            }
        },
        &crosses_devices,
    )
    .unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    assert!(src_present_at_last_progress);
    assert!(!src.exists());
    assert_eq!(fs::read(dest.join("src/sub/big.bin")).unwrap(), [7u8; 64]);
    assert_eq!(fs::read(dest.join("src/small.txt")).unwrap(), b"hi");
}

#[test]
fn move_failure_keeps_source() {
    let t = TestDir::new("move_failure_keeps_source");
    let (src, dest) = tree(t.path());

    let mut opts = options();
    opts.chunk_size = 4;
    let cancel = opts.cancel.clone();
    let outcome = move_entry_with(
        &src,
        &dest,
        &opts,
        &mut |p| {
            if p.bytes_done > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &crosses_devices,
    )
    .unwrap();
    assert_eq!(outcome, CopyOutcome::Cancelled);
    assert_eq!(fs::read(src.join("sub/big.bin")).unwrap(), [7u8; 64]);
    assert_eq!(fs::read(src.join("small.txt")).unwrap(), b"hi");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);

    let missing_dest = t.path().join("missing");
    let err = move_entry_with(
        &src,
        &missing_dest,
        &options(),
        &mut |_| {},
        &crosses_devices,
    );
    assert!(err.is_err());
    assert_eq!(fs::read(src.join("small.txt")).unwrap(), b"hi");
}

#[test]
fn move_skip_keeps_skipped_source() {
    let t = TestDir::new("move_skip_keeps_skipped_source");
    let (src, dest) = tree(t.path());
    fs::create_dir_all(dest.join("src/sub")).unwrap();
    fs::write(dest.join("src/small.txt"), b"existing").unwrap();
    let opts = CopyOptions::new(ConflictPolicy::Skip, Arc::new(AtomicBool::new(false)));

    let outcome = move_entry_with(&src, &dest, &opts, &mut |_| {}, &crosses_devices).unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    assert_eq!(fs::read(src.join("small.txt")).unwrap(), b"hi");
    assert!(!src.join("sub").exists());
    assert_eq!(fs::read(dest.join("src/sub/big.bin")).unwrap(), [7u8; 64]);
    assert_eq!(fs::read(dest.join("src/small.txt")).unwrap(), b"existing");
}
