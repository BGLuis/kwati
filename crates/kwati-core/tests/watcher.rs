use kwati_core::{DirWatcher, VolumeKind, WatcherBackend};
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

#[test]
fn watcher_picks_backend() {
    let dir = TestDir::new("watcher_picks_backend");

    // Local directory keeps the native watcher
    let local_watcher = DirWatcher::new(&dir.path, WINDOW, || {}).unwrap();
    assert_eq!(local_watcher.backend(), WatcherBackend::Native);
    assert!(!local_watcher.is_polling());

    // Explicit network volume gets polling watcher
    let net_watcher = DirWatcher::new_with_volume_kind(
        &dir.path,
        VolumeKind::Network,
        WINDOW,
        Duration::from_millis(100),
        || {},
    )
    .unwrap();
    assert_eq!(net_watcher.backend(), WatcherBackend::Poll);
    assert!(net_watcher.is_polling());

    // Unknown volume gets polling watcher
    let unk_watcher = DirWatcher::new_with_volume_kind(
        &dir.path,
        VolumeKind::Unknown,
        WINDOW,
        Duration::from_millis(100),
        || {},
    )
    .unwrap();
    assert_eq!(unk_watcher.backend(), WatcherBackend::Poll);
    assert!(unk_watcher.is_polling());

    // backend_for_kind and backend_for_path
    assert_eq!(
        DirWatcher::backend_for_kind(VolumeKind::Local),
        WatcherBackend::Native
    );
    assert_eq!(
        DirWatcher::backend_for_kind(VolumeKind::Network),
        WatcherBackend::Poll
    );
    assert_eq!(
        DirWatcher::backend_for_kind(VolumeKind::Unknown),
        WatcherBackend::Poll
    );
    assert_eq!(
        DirWatcher::backend_for_path(std::path::Path::new("//server/share")),
        WatcherBackend::Poll
    );
}

#[test]
fn poll_watcher_signals() {
    let dir = TestDir::new("poll_watcher_signals");
    let poll_interval = Duration::from_millis(100);
    let window = Duration::from_millis(20);
    let (tx, rx) = mpsc::channel();

    let watcher = DirWatcher::new_with_backend_and_interval(
        &dir.path,
        window,
        WatcherBackend::Poll,
        poll_interval,
        move || {
            let _ = tx.send(());
        },
    )
    .expect("failed to start polling watcher");

    assert_eq!(watcher.backend(), WatcherBackend::Poll);

    // Initial state: no signals
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());

    // Create a file in the watched directory
    std::fs::write(dir.path.join("created.txt"), b"poll content").unwrap();

    // With polling, a file created in the directory produces one signal within two intervals
    let received = rx.recv_timeout(poll_interval * 2 + window + Duration::from_millis(100));
    assert!(
        received.is_ok(),
        "expected signal within two intervals, got: {:?}",
        received
    );

    // Verify coalescing: no extra burst of signals
    assert!(
        rx.recv_timeout(poll_interval * 2).is_err(),
        "unexpected extra signal"
    );
}
