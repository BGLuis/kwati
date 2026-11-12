use crate::{Backend, BackendError, Confirmed, EntryMeta, Listing, Location};
use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// Whether a remote session is usable, as last observed by [`ReconnectingBackend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
    Disconnected,
}

/// How long and how often idempotent reads are retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    initial_delay: Duration,
    max_attempts: u32,
}

impl RetryPolicy {
    /// `max_attempts` is the total number of tries per read, the first one included. The delay
    /// before each retry starts at `initial_delay` and doubles.
    pub fn new(initial_delay: Duration, max_attempts: u32) -> Self {
        Self {
            initial_delay,
            max_attempts: max_attempts.max(1),
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), 5)
    }
}

/// Whether `err` means the connection itself failed, as opposed to the operation being refused.
pub fn is_dropped_connection(err: &BackendError) -> bool {
    use io::ErrorKind::*;
    matches!(
        err,
        BackendError::Io(e) if matches!(
            e.kind(),
            ConnectionReset | ConnectionAborted | BrokenPipe | NotConnected | TimedOut | UnexpectedEof
        )
    )
}

/// Wraps a remote backend, tracks whether its connection is alive and re-establishes it.
///
/// Reads (`list`, `stat`, `open_read`) that fail with a dropped connection are retried on a fresh
/// session with exponential backoff, up to the [`RetryPolicy`] limit; then the state becomes
/// [`ConnectionState::Disconnected`]. Writes are never retried: a repeated write could apply
/// twice, so the failure goes to the caller. Every call tries again regardless of the state, and
/// the state reflects the last outcome. A write issued while `Disconnected` first opens a fresh
/// session, once.
pub struct ReconnectingBackend {
    connect: Box<dyn Fn() -> Result<Box<dyn Backend>, BackendError> + Send + Sync>,
    policy: RetryPolicy,
    inner: RwLock<Arc<dyn Backend>>,
    state: Mutex<ConnectionState>,
}

impl ReconnectingBackend {
    /// Opens the first session with `connect`, which is called again for every reconnection.
    pub fn new(
        connect: impl Fn() -> Result<Box<dyn Backend>, BackendError> + Send + Sync + 'static,
        policy: RetryPolicy,
    ) -> Result<Self, BackendError> {
        let inner = Arc::from(connect()?);
        Ok(Self {
            connect: Box::new(connect),
            policy,
            inner: RwLock::new(inner),
            state: Mutex::new(ConnectionState::Connected),
        })
    }

    /// Safe to poll from another thread.
    pub fn state(&self) -> ConnectionState {
        *self.state.lock().unwrap()
    }

    fn set_state(&self, state: ConnectionState) {
        *self.state.lock().unwrap() = state;
    }

    fn current(&self) -> Arc<dyn Backend> {
        self.inner.read().unwrap().clone()
    }

    fn reconnect(&self) -> Result<(), BackendError> {
        *self.inner.write().unwrap() = Arc::from((self.connect)()?);
        Ok(())
    }

    fn read<T>(
        &self,
        op: impl Fn(&dyn Backend) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        let mut err = match op(&*self.current()) {
            Ok(value) => {
                self.set_state(ConnectionState::Connected);
                return Ok(value);
            }
            Err(err) => err,
        };
        let mut delay = self.policy.initial_delay;
        let mut attempt = 1;
        while is_dropped_connection(&err) {
            if attempt >= self.policy.max_attempts {
                self.set_state(ConnectionState::Disconnected);
                return Err(err);
            }
            self.set_state(ConnectionState::Reconnecting);
            std::thread::sleep(delay);
            delay = delay.saturating_mul(2);
            attempt += 1;
            match self.reconnect().and_then(|()| op(&*self.current())) {
                Ok(value) => {
                    self.set_state(ConnectionState::Connected);
                    return Ok(value);
                }
                Err(next) => err = next,
            }
        }
        self.set_state(ConnectionState::Connected);
        Err(err)
    }

    fn write<T>(
        &self,
        op: impl FnOnce(&dyn Backend) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        if self.state() == ConnectionState::Disconnected {
            self.reconnect()?;
        }
        match op(&*self.current()) {
            Ok(value) => {
                self.set_state(ConnectionState::Connected);
                Ok(value)
            }
            Err(err) => {
                self.set_state(if is_dropped_connection(&err) {
                    ConnectionState::Disconnected
                } else {
                    ConnectionState::Connected
                });
                Err(err)
            }
        }
    }
}

impl Backend for ReconnectingBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        self.read(|b| b.list(location))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        self.read(|b| b.stat(location))
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn io::Read + Send>, BackendError> {
        self.read(|b| b.open_read(location))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn io::Write + Send>, BackendError> {
        self.write(|b| b.open_write(location))
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        self.write(|b| b.rename(from, to))
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        self.write(|b| b.create_dir(location))
    }

    fn delete_permanently(
        &self,
        location: &Location,
        confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        self.write(|b| b.delete_permanently(location, confirmed))
    }

    fn trash(&self, location: &Location) -> Result<(), BackendError> {
        self.write(|b| b.trash(location))
    }
}
