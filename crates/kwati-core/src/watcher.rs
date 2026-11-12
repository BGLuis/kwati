use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{PollWatcher, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::volume::{volume_kind, VolumeKind};

/// Backend watcher used to observe directory changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherBackend {
    Native,
    Poll,
}

#[allow(dead_code)]
enum WatcherImpl {
    Recommended(RecommendedWatcher),
    Poll(PollWatcher),
}

/// Watches one directory (non-recursively) and calls back once per burst of changes.
///
/// After the first event, the watcher waits `window` collecting further events and then
/// invokes the callback once. For network shares or unknown mounts, it uses [`PollWatcher`]
/// instead of the native watcher. Dropping the watcher stops the background thread; no
/// callback runs after `drop` returns.
pub struct DirWatcher {
    // Dropped first so the event channel disconnects and the thread exits.
    watcher: Option<WatcherImpl>,
    thread: Option<JoinHandle<()>>,
    backend: WatcherBackend,
}

impl DirWatcher {
    /// Creates a directory watcher with default poll interval (5s) for network mounts.
    pub fn new(
        dir: &Path,
        window: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        Self::new_with_interval(dir, window, Duration::from_secs(5), on_change)
    }

    /// Creates a directory watcher with a custom poll interval for network mounts.
    pub fn new_with_interval(
        dir: &Path,
        window: Duration,
        poll_interval: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        let kind = volume_kind(dir);
        let backend = Self::backend_for_kind(kind);
        Self::new_with_backend_and_interval(dir, window, backend, poll_interval, on_change)
    }

    /// Creates a directory watcher explicitly specifying the [`VolumeKind`].
    pub fn new_with_volume_kind(
        dir: &Path,
        kind: VolumeKind,
        window: Duration,
        poll_interval: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        let backend = Self::backend_for_kind(kind);
        Self::new_with_backend_and_interval(dir, window, backend, poll_interval, on_change)
    }

    /// Creates a directory watcher with a specific [`WatcherBackend`] and poll interval.
    pub fn new_with_backend_and_interval(
        dir: &Path,
        window: Duration,
        backend: WatcherBackend,
        poll_interval: Duration,
        mut on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (watcher, actual_backend) = match backend {
            WatcherBackend::Native => {
                let tx_native = tx.clone();
                match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if event.is_ok() {
                        let _ = tx_native.send(());
                    }
                }) {
                    Ok(mut native_watcher) => {
                        native_watcher.watch(dir, RecursiveMode::NonRecursive)?;
                        (
                            WatcherImpl::Recommended(native_watcher),
                            WatcherBackend::Native,
                        )
                    }
                    Err(_) => {
                        let config = notify::Config::default().with_poll_interval(poll_interval);
                        let mut poll_watcher = notify::PollWatcher::new(
                            move |event: notify::Result<notify::Event>| {
                                if event.is_ok() {
                                    let _ = tx.send(());
                                }
                            },
                            config,
                        )?;
                        poll_watcher.watch(dir, RecursiveMode::NonRecursive)?;
                        (WatcherImpl::Poll(poll_watcher), WatcherBackend::Poll)
                    }
                }
            }
            WatcherBackend::Poll => {
                let config = notify::Config::default().with_poll_interval(poll_interval);
                let mut poll_watcher = notify::PollWatcher::new(
                    move |event: notify::Result<notify::Event>| {
                        if event.is_ok() {
                            let _ = tx.send(());
                        }
                    },
                    config,
                )?;
                poll_watcher.watch(dir, RecursiveMode::NonRecursive)?;
                (WatcherImpl::Poll(poll_watcher), WatcherBackend::Poll)
            }
        };

        let thread = thread::spawn(move || {
            while rx.recv().is_ok() {
                let deadline = Instant::now() + window;
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        });

        Ok(Self {
            watcher: Some(watcher),
            thread: Some(thread),
            backend: actual_backend,
        })
    }

    /// Selects the appropriate [`WatcherBackend`] for a given [`VolumeKind`].
    pub fn backend_for_kind(kind: VolumeKind) -> WatcherBackend {
        match kind {
            VolumeKind::Local => WatcherBackend::Native,
            VolumeKind::Network | VolumeKind::Unknown => WatcherBackend::Poll,
        }
    }

    /// Selects the appropriate [`WatcherBackend`] for a given directory path.
    pub fn backend_for_path(path: &Path) -> WatcherBackend {
        Self::backend_for_kind(volume_kind(path))
    }

    /// Returns the backend being used by this watcher.
    pub fn backend(&self) -> WatcherBackend {
        self.backend
    }

    /// Returns whether this watcher is using polling.
    pub fn is_polling(&self) -> bool {
        self.backend == WatcherBackend::Poll
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.watcher.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
