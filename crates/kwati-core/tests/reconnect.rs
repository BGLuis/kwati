use kwati_core::{
    is_dropped_connection, Backend, BackendError, Confirmed, ConnectionState, EntryMeta, Listing,
    Location, ReconnectingBackend, RetryPolicy,
};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct Counters {
    reads: AtomicUsize,
    writes: AtomicUsize,
    connects: AtomicUsize,
    failures_left: AtomicUsize,
}

enum Failure {
    Dropped,
    NotFound,
}

/// Fails the first `failures_left` calls (reads and writes alike), then succeeds.
struct FlakyBackend {
    counters: Arc<Counters>,
    failure: fn() -> io::Error,
}

impl FlakyBackend {
    fn fail(&self) -> Result<(), BackendError> {
        let left = self.counters.failures_left.load(Ordering::SeqCst);
        if left > 0 {
            self.counters
                .failures_left
                .store(left - 1, Ordering::SeqCst);
            return Err((self.failure)().into());
        }
        Ok(())
    }
}

impl Backend for FlakyBackend {
    fn list(&self, _: &Location) -> Result<Listing, BackendError> {
        self.counters.reads.fetch_add(1, Ordering::SeqCst);
        self.fail()?;
        Ok(Listing::new(Vec::new()))
    }

    fn stat(&self, _: &Location) -> Result<EntryMeta, BackendError> {
        self.counters.reads.fetch_add(1, Ordering::SeqCst);
        self.fail()?;
        Err(BackendError::Unsupported)
    }

    fn create_dir(&self, _: &Location) -> Result<(), BackendError> {
        self.counters.writes.fetch_add(1, Ordering::SeqCst);
        self.fail()
    }

    fn delete_permanently(&self, _: &Location, _: Confirmed) -> Result<(), BackendError> {
        self.counters.writes.fetch_add(1, Ordering::SeqCst);
        self.fail()
    }
}

fn dropped() -> io::Error {
    io::Error::from(io::ErrorKind::ConnectionReset)
}

fn missing() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}

fn setup(
    failures: usize,
    failure: Failure,
    max_attempts: u32,
) -> (ReconnectingBackend, Arc<Counters>) {
    let counters = Arc::new(Counters::default());
    counters.failures_left.store(failures, Ordering::SeqCst);
    let make: fn() -> io::Error = match failure {
        Failure::Dropped => dropped,
        Failure::NotFound => missing,
    };
    let shared = counters.clone();
    let backend = ReconnectingBackend::new(
        move || {
            shared.connects.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FlakyBackend {
                counters: shared.clone(),
                failure: make,
            }) as Box<dyn Backend>)
        },
        RetryPolicy::new(Duration::ZERO, max_attempts),
    )
    .unwrap();
    (backend, counters)
}

fn root() -> Location {
    Location::Remote {
        profile_id: "p".into(),
        path: "/".into(),
    }
}

#[test]
fn reconnect_retries_reads() {
    let (backend, counters) = setup(2, Failure::Dropped, 5);
    assert_eq!(backend.state(), ConnectionState::Connected);

    backend.list(&root()).expect("read recovers");

    assert_eq!(counters.reads.load(Ordering::SeqCst), 3);
    assert_eq!(backend.state(), ConnectionState::Connected);
    assert_eq!(counters.connects.load(Ordering::SeqCst), 3);
}

#[test]
fn reconnect_never_retries_writes() {
    let (backend, counters) = setup(1, Failure::Dropped, 5);

    let err = backend.create_dir(&root()).unwrap_err();
    assert!(is_dropped_connection(&err));
    assert_eq!(counters.writes.load(Ordering::SeqCst), 1);

    let (backend, counters) = setup(1, Failure::Dropped, 5);
    assert!(backend
        .delete_permanently(&root(), Confirmed::user_confirmed())
        .is_err());
    assert_eq!(counters.writes.load(Ordering::SeqCst), 1);
}

#[test]
fn reconnect_gives_up() {
    let (backend, counters) = setup(usize::MAX, Failure::Dropped, 3);

    let err = backend.list(&root()).unwrap_err();

    assert!(is_dropped_connection(&err));
    assert_eq!(backend.state(), ConnectionState::Disconnected);
    assert_eq!(counters.reads.load(Ordering::SeqCst), 3);
}

#[test]
fn reconnect_recovers_after_disconnected() {
    let (backend, counters) = setup(usize::MAX, Failure::Dropped, 2);
    assert!(backend.list(&root()).is_err());
    assert_eq!(backend.state(), ConnectionState::Disconnected);

    counters.failures_left.store(0, Ordering::SeqCst);
    backend.list(&root()).expect("server is back");

    assert_eq!(backend.state(), ConnectionState::Connected);
}

#[test]
fn reconnect_write_while_disconnected_reconnects_first() {
    let (backend, counters) = setup(usize::MAX, Failure::Dropped, 2);
    assert!(backend.list(&root()).is_err());
    counters.failures_left.store(0, Ordering::SeqCst);
    let connects = counters.connects.load(Ordering::SeqCst);

    backend.create_dir(&root()).expect("write after recovery");

    assert_eq!(counters.connects.load(Ordering::SeqCst), connects + 1);
    assert_eq!(backend.state(), ConnectionState::Connected);
}

#[test]
fn reconnect_does_not_retry_ordinary_errors() {
    let (backend, counters) = setup(5, Failure::NotFound, 5);

    let err = backend.list(&root()).unwrap_err();

    assert_eq!(err, BackendError::Io(missing()));
    assert_eq!(counters.reads.load(Ordering::SeqCst), 1);
    assert_eq!(backend.state(), ConnectionState::Connected);
}

#[test]
fn reconnect_classifies_dropped_connections() {
    use io::ErrorKind::*;
    for kind in [
        ConnectionReset,
        ConnectionAborted,
        BrokenPipe,
        NotConnected,
        TimedOut,
        UnexpectedEof,
    ] {
        assert!(
            is_dropped_connection(&BackendError::Io(kind.into())),
            "{kind:?}"
        );
    }
    for kind in [NotFound, PermissionDenied, AlreadyExists, InvalidInput] {
        assert!(
            !is_dropped_connection(&BackendError::Io(kind.into())),
            "{kind:?}"
        );
    }
    assert!(!is_dropped_connection(&BackendError::Unsupported));
}
