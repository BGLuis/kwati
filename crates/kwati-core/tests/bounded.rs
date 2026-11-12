use kwati_core::{list_dir, list_dir_bounded, list_dir_bounded_with, Listing};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// A listing that blocks until the returned sender is used or dropped.
fn blocked_listing() -> (
    mpsc::Sender<()>,
    impl Fn(&Path) -> io::Result<Listing> + Send + 'static,
) {
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let list = move |_: &Path| {
        let _ = gate.lock().unwrap().recv();
        Ok(Listing::new(Vec::new()))
    };
    (release, list)
}

#[test]
fn bounded_list_times_out() {
    let (release, list) = blocked_listing();
    let deadline = Duration::from_millis(50);

    let start = Instant::now();
    let err = list_dir_bounded_with(
        Path::new("/unused"),
        PathBuf::from("mount_times_out"),
        deadline,
        1,
        list,
    )
    .unwrap_err();
    let elapsed = start.elapsed();

    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(elapsed >= deadline, "returned early: {elapsed:?}");
    assert!(
        elapsed < deadline + Duration::from_millis(200),
        "returned late: {elapsed:?}"
    );
    drop(release);
}

#[test]
fn bounded_list_caps_threads() {
    let mount = PathBuf::from("mount_caps_threads");
    let (release, list) = blocked_listing();
    let first = list_dir_bounded_with(
        Path::new("/unused"),
        mount.clone(),
        Duration::from_millis(20),
        1,
        list,
    )
    .unwrap_err();
    assert_eq!(first.kind(), io::ErrorKind::TimedOut);

    let called = Arc::new(Mutex::new(false));
    let probe = Arc::clone(&called);
    let start = Instant::now();
    let err = list_dir_bounded_with(
        Path::new("/unused"),
        mount.clone(),
        Duration::from_secs(5),
        1,
        move |_| {
            *probe.lock().unwrap() = true;
            Ok(Listing::new(Vec::new()))
        },
    )
    .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::ResourceBusy);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(!*called.lock().unwrap(), "a refused read must not list");

    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let retry = list_dir_bounded_with(
            Path::new("/unused"),
            mount.clone(),
            Duration::from_secs(5),
            1,
            |_| Ok(Listing::new(Vec::new())),
        );
        if retry.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "slot was never released");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn bounded_list_matches() {
    let dir = TestDir::new("bounded_list_matches");
    std::fs::create_dir(dir.path.join("sub")).unwrap();
    std::fs::write(dir.path.join("a.txt"), b"").unwrap();
    std::fs::write(dir.path.join("b.txt"), b"").unwrap();

    let bounded = list_dir_bounded(&dir.path, Duration::from_secs(5)).unwrap();
    assert_eq!(bounded, list_dir(&dir.path).unwrap());
    assert_eq!(bounded.len(), 3);
}
