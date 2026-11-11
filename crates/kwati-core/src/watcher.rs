use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// Watches one directory (non-recursively) and calls back once per burst of changes.
///
/// After the first event, the watcher waits `window` collecting further events and then
/// invokes the callback once. Dropping the watcher stops the background thread; no callback
/// runs after `drop` returns.
pub struct DirWatcher {
    // Dropped first so the event channel disconnects and the thread exits.
    watcher: Option<RecommendedWatcher>,
    thread: Option<JoinHandle<()>>,
}

impl DirWatcher {
    pub fn new(
        dir: &Path,
        window: Duration,
        mut on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;

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
        })
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
