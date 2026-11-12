use crate::connection::ConnectionState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::{Backend, Location, Settings, MIN_REMOTE_POLL_INTERVAL_SECS};

/// State of a remote connection as defined in issue #44.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(MIN_REMOTE_POLL_INTERVAL_SECS as u64);

/// Background poller for remote directories.
///
/// Relists a remote directory at a specified interval and invokes a callback
/// whenever the listing changes. Pauses on request and stops polling when
/// disconnected.
pub struct RemotePoller {
    thread: Option<JoinHandle<()>>,
    is_paused: Arc<AtomicBool>,
    is_stopped: Arc<Mutex<bool>>,
    cvar: Arc<Condvar>,
    state: Arc<RwLock<ConnectionState>>,
}

impl RemotePoller {
    /// Creates a poller using settings from [`Settings`].
    /// Returns `None` if `settings.remote_poll_enabled` is `false`.
    pub fn from_settings(
        settings: &Settings,
        backend: Arc<dyn Backend>,
        location: Location,
        on_change: impl FnMut() + Send + 'static,
    ) -> Option<Self> {
        if !settings.remote_poll_enabled {
            return None;
        }
        let interval = Duration::from_secs(settings.remote_poll_interval_secs as u64);
        Some(Self::new(backend, location, interval, on_change))
    }

    /// Creates a new remote poller with the specified interval.
    pub fn new(
        backend: Arc<dyn Backend>,
        location: Location,
        interval: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> Self {
        Self::with_connection_state(
            backend,
            location,
            interval,
            Arc::new(RwLock::new(ConnectionState::Connected)),
            on_change,
        )
    }

    /// Creates a remote poller with an explicitly managed [`ConnectionState`].
    pub fn with_connection_state(
        backend: Arc<dyn Backend>,
        location: Location,
        interval: Duration,
        connection_state: Arc<RwLock<ConnectionState>>,
        mut on_change: impl FnMut() + Send + 'static,
    ) -> Self {
        let is_paused = Arc::new(AtomicBool::new(false));
        let is_stopped = Arc::new(Mutex::new(false));
        let cvar = Arc::new(Condvar::new());

        let paused = Arc::clone(&is_paused);
        let stopped = Arc::clone(&is_stopped);
        let cv = Arc::clone(&cvar);
        let state = Arc::clone(&connection_state);

        // Fetch initial listing baseline if connected and not paused
        let mut last_listing = if *connection_state.read().unwrap() != ConnectionState::Disconnected
        {
            backend.list(&location).ok()
        } else {
            None
        };

        let thread = thread::spawn(move || {
            loop {
                // Wait for interval or stop notification
                {
                    let lock = stopped.lock().unwrap();
                    if *lock {
                        break;
                    }
                    let (new_lock, _) = cv.wait_timeout(lock, interval).unwrap();
                    if *new_lock {
                        break;
                    }
                }

                // While paused, send no request to backend
                if paused.load(Ordering::SeqCst) {
                    continue;
                }

                // While disconnected, send no request to backend
                let conn_state = *state.read().unwrap();
                if conn_state == ConnectionState::Disconnected {
                    continue;
                }

                // Query the backend
                if let Ok(listing) = backend.list(&location) {
                    let changed = match &last_listing {
                        Some(prev) => prev != &listing,
                        None => true,
                    };
                    if changed {
                        last_listing = Some(listing);
                        on_change();
                    }
                }
            }
        });

        Self {
            thread: Some(thread),
            is_paused,
            is_stopped,
            cvar,
            state: connection_state,
        }
    }

    /// Pauses polling. While paused, no requests are sent to the backend.
    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
    }

    /// Resumes polling.
    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
    }

    /// Returns whether the poller is currently paused.
    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    /// Updates the connection state.
    pub fn set_connection_state(&self, state: ConnectionState) {
        *self.state.write().unwrap() = state;
    }

    /// Returns the current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.read().unwrap()
    }

    /// Stops the background poller thread.
    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            *self.is_stopped.lock().unwrap() = true;
            self.cvar.notify_all();
            let _ = thread.join();
        }
    }
}

impl Drop for RemotePoller {
    fn drop(&mut self) {
        self.stop();
    }
}
