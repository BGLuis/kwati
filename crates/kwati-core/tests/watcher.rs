use kwati_core::DirWatcher;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("kwati_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const WINDOW: Duration = Duration::from_millis(300);

fn watch(dir: &TestDir) -> (DirWatcher, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let watcher = DirWatcher::new(&dir.path, WINDOW, move || {
        let _ = tx.send(());
    })
    .expect("failed to start watcher");
    (watcher, rx)
}

#[test]
fn watcher_coalesces() {
    let dir = TestDir::new("watcher_coalesces");
    let (_watcher, rx) = watch(&dir);

    for i in 0..100 {
        std::fs::write(dir.path.join(format!("f{i}")), b"").unwrap();
    }

    rx.recv_timeout(Duration::from_secs(10))
        .expect("no signal after creating files");
    let mut signals = 1;
    while rx.recv_timeout(WINDOW * 4).is_ok() {
        signals += 1;
    }

    // Creating 100 files takes far less than a few windows, so a handful of signals is the
    // most scheduler jitter can explain; without coalescing there would be ~100.
    assert!(signals <= 5, "expected coalesced signals, got {signals}");
}

#[test]
fn watcher_stops() {
    let dir = TestDir::new("watcher_stops");
    let (watcher, rx) = watch(&dir);

    std::fs::write(dir.path.join("before"), b"").unwrap();
    rx.recv_timeout(Duration::from_secs(10))
        .expect("watcher never signalled");

    drop(watcher);
    while rx.try_recv().is_ok() {}

    for i in 0..20 {
        std::fs::write(dir.path.join(format!("after{i}")), b"").unwrap();
    }
    assert!(rx.recv_timeout(WINDOW * 4).is_err(), "signal after drop");
}
