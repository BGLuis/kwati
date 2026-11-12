use kwati_core::{
    Backend, BackendError, ConnectionState, Entry, Listing, Location, RemotePoller, Settings,
    DEFAULT_REMOTE_POLL_INTERVAL_SECS,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

struct MockBackend {
    entries: Mutex<Vec<Entry>>,
    request_count: AtomicUsize,
}

impl MockBackend {
    fn new(initial_entries: Vec<Entry>) -> Self {
        Self {
            entries: Mutex::new(initial_entries),
            request_count: AtomicUsize::new(0),
        }
    }

    fn add_entry(&self, name: &str, is_dir: bool) {
        let mut entries = self.entries.lock().unwrap();
        entries.push(Entry::new(
            OsString::from(name),
            PathBuf::from("/remote").join(name),
            is_dir,
        ));
    }

    fn requests(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }
}

impl Backend for MockBackend {
    fn list(&self, _location: &Location) -> Result<Listing, BackendError> {
        self.request_count.fetch_add(1, Ordering::SeqCst);
        let entries = self.entries.lock().unwrap().clone();
        Ok(Listing::new(entries))
    }
}

#[test]
fn remote_poll_no_change() {
    let initial = vec![Entry::new(
        OsString::from("f1.txt"),
        PathBuf::from("/remote/f1.txt"),
        false,
    )];
    let backend = Arc::new(MockBackend::new(initial));
    let location = Location::Remote {
        profile_id: "test-profile".into(),
        path: PathBuf::from("/remote"),
    };

    let interval = Duration::from_millis(50);
    let (tx, rx) = mpsc::channel();

    let _poller = RemotePoller::new(backend, location, interval, move || {
        let _ = tx.send(());
    });

    // Across 3 intervals (150ms + margin), no change should produce no signal
    std::thread::sleep(interval * 4);
    assert!(
        rx.try_recv().is_err(),
        "expected no signals for unchanged directory"
    );
}

#[test]
fn remote_poll_detects_change() {
    let initial = vec![Entry::new(
        OsString::from("f1.txt"),
        PathBuf::from("/remote/f1.txt"),
        false,
    )];
    let backend = Arc::new(MockBackend::new(initial));
    let location = Location::Remote {
        profile_id: "test-profile".into(),
        path: PathBuf::from("/remote"),
    };

    let interval = Duration::from_millis(50);
    let (tx, rx) = mpsc::channel();

    let _poller = RemotePoller::new(backend.clone(), location, interval, move || {
        let _ = tx.send(());
    });

    // Verify initially no signals
    std::thread::sleep(Duration::from_millis(30));
    assert!(rx.try_recv().is_err());

    // Add a file to the backend
    backend.add_entry("f2.txt", false);

    // On the next interval, it should produce one signal
    let received = rx.recv_timeout(interval * 3);
    assert!(received.is_ok(), "expected signal after adding file");

    // And no extra duplicate signals on subsequent intervals
    std::thread::sleep(interval * 2);
    assert!(rx.try_recv().is_err(), "unexpected duplicate signal");
}

#[test]
fn remote_poll_paused() {
    let initial = vec![Entry::new(
        OsString::from("f1.txt"),
        PathBuf::from("/remote/f1.txt"),
        false,
    )];
    let backend = Arc::new(MockBackend::new(initial));
    let location = Location::Remote {
        profile_id: "test-profile".into(),
        path: PathBuf::from("/remote"),
    };

    let interval = Duration::from_millis(40);
    let poller = RemotePoller::new(backend.clone(), location, interval, || {});

    // Initial listing was fetched during setup
    assert_eq!(backend.requests(), 1);

    // Pause the poller
    poller.pause();
    assert!(poller.is_paused());

    // Wait across 3 intervals while paused
    std::thread::sleep(interval * 3);
    assert_eq!(
        backend.requests(),
        1,
        "poller must send no requests while paused"
    );

    // Resume the poller and observe requests resume
    poller.resume();
    assert!(!poller.is_paused());
    std::thread::sleep(interval * 2);
    assert!(
        backend.requests() > 1,
        "poller should resume sending requests after resume"
    );

    // Set connection state to Disconnected
    poller.set_connection_state(ConnectionState::Disconnected);
    assert_eq!(poller.connection_state(), ConnectionState::Disconnected);
    let count_before = backend.requests();

    // Wait across 3 intervals while disconnected
    std::thread::sleep(interval * 3);
    assert_eq!(
        backend.requests(),
        count_before,
        "poller must send no requests while disconnected"
    );
}

#[test]
fn remote_poll_minimum() {
    let mut settings = Settings::default();

    // Direct setter validation rejects intervals below 10 seconds
    assert!(settings.set_remote_poll_interval_secs(0).is_err());
    assert!(settings.set_remote_poll_interval_secs(5).is_err());
    assert!(settings.set_remote_poll_interval_secs(9).is_err());
    assert!(settings.set_remote_poll_interval_secs(10).is_ok());
    assert_eq!(settings.remote_poll_interval_secs, 10);
    assert!(settings.set_remote_poll_interval_secs(60).is_ok());
    assert_eq!(settings.remote_poll_interval_secs, 60);

    // File loading rejects intervals below minimum and records warning
    let temp_dir = std::env::temp_dir().join(format!("kwati_test_settings_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    let settings_file = temp_dir.join("settings.toml");

    std::fs::write(&settings_file, "remote_poll_interval_secs = 4\n").unwrap();
    let (loaded, warnings) = Settings::load_from(&settings_file).unwrap();
    assert_eq!(
        loaded.remote_poll_interval_secs,
        DEFAULT_REMOTE_POLL_INTERVAL_SECS
    );
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].key, "remote_poll_interval_secs");

    let _ = std::fs::remove_dir_all(&temp_dir);
}
