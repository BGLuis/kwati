use kwati_core::{copy_entry, ConflictPolicy, CopyOptions, CopyOutcome, Progress};
use std::fs;
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

fn options(policy: ConflictPolicy) -> CopyOptions {
    CopyOptions::new(policy, Arc::new(AtomicBool::new(false)))
}

fn copy(src: &Path, dest: &Path, policy: ConflictPolicy) -> CopyOutcome {
    copy_entry(src, dest, &options(policy), &mut |_| {}).unwrap()
}

/// Copies with 4-byte chunks and raises the cancel flag after the first chunk.
fn copy_cancelled_after_first_chunk(
    src: &Path,
    dest: &Path,
    policy: ConflictPolicy,
) -> CopyOutcome {
    let mut opts = options(policy);
    opts.chunk_size = 4;
    let cancel = opts.cancel.clone();
    copy_entry(src, dest, &opts, &mut |p| {
        if p.bytes_done > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap()
}

#[test]
fn copy_cancel() {
    let t = TestDir::new("copy_cancel");
    let src = t.path().join("src");
    let dest = t.path().join("dest");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(src.join("sub/big.bin"), [7u8; 64]).unwrap();
    fs::write(src.join("small.txt"), b"hi").unwrap();

    let outcome = copy_cancelled_after_first_chunk(&src, &dest, ConflictPolicy::Skip);

    assert_eq!(outcome, CopyOutcome::Cancelled);
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    assert_eq!(fs::read(src.join("sub/big.bin")).unwrap(), [7u8; 64]);
    assert_eq!(fs::read(src.join("small.txt")).unwrap(), b"hi");
}

#[test]
fn copy_conflict_skip() {
    let t = TestDir::new("copy_conflict_skip");
    let dest = t.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let src = t.path().join("a.txt");
    fs::write(&src, b"new").unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();

    assert_eq!(
        copy(&src, &dest, ConflictPolicy::Skip),
        CopyOutcome::Completed
    );
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
}

#[test]
fn copy_conflict_keep_both() {
    let t = TestDir::new("copy_conflict_keep_both");
    let dest = t.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let src = t.path().join("a.txt");
    fs::write(&src, b"new").unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();

    copy(&src, &dest, ConflictPolicy::KeepBoth);
    copy(&src, &dest, ConflictPolicy::KeepBoth);

    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read(dest.join("a (1).txt")).unwrap(), b"new");
    assert_eq!(fs::read(dest.join("a (2).txt")).unwrap(), b"new");
}

#[test]
fn copy_conflict_replace() {
    let t = TestDir::new("copy_conflict_replace");
    let dest = t.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let src = t.path().join("a.txt");
    fs::write(&src, b"new content").unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();

    let outcome = copy_cancelled_after_first_chunk(&src, &dest, ConflictPolicy::Replace);
    assert_eq!(outcome, CopyOutcome::Cancelled);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);

    copy(&src, &dest, ConflictPolicy::Replace);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"new content");
    assert_eq!(fs::read(&src).unwrap(), b"new content");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn copy_symlink_cycle() {
    let t = TestDir::new("copy_symlink_cycle");
    let src = t.path().join("src");
    let dest = t.path().join("dest");
    fs::create_dir_all(src.join("inner")).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(src.join("inner/f.txt"), b"data").unwrap();
    std::os::unix::fs::symlink("..", src.join("inner/loop")).unwrap();

    assert_eq!(
        copy(&src, &dest, ConflictPolicy::Skip),
        CopyOutcome::Completed
    );

    let link = dest.join("src/inner/loop");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), Path::new(".."));
    assert_eq!(fs::read(dest.join("src/inner/f.txt")).unwrap(), b"data");
}

#[test]
fn copy_progress() {
    let t = TestDir::new("copy_progress");
    let src = t.path().join("src");
    let dest = t.path().join("dest");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(src.join("a.bin"), [1u8; 10]).unwrap();
    fs::write(src.join("sub/b.bin"), [2u8; 21]).unwrap();
    fs::write(src.join("empty"), b"").unwrap();

    let mut opts = options(ConflictPolicy::Skip);
    opts.chunk_size = 4;
    let mut reports: Vec<Progress> = Vec::new();
    let outcome = copy_entry(&src, &dest, &opts, &mut |p| reports.push(p)).unwrap();

    assert_eq!(outcome, CopyOutcome::Completed);
    let last = reports.last().unwrap();
    assert_eq!((last.bytes_done, last.bytes_total), (31, 31));
    assert!(reports
        .windows(2)
        .all(|w| w[0].bytes_done <= w[1].bytes_done));
    assert!(reports.iter().all(|p| p.bytes_total == 31));
}
