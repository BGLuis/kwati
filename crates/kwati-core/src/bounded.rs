use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::volume::mount_key;
use crate::{list_dir_with_options, ListOptions, Listing};

/// Timed-out listings that may still be running on one mount before new reads there are refused.
pub const DEFAULT_MAX_ABANDONED: usize = 4;

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

static ABANDONED_BY_MOUNT: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

fn adjust_abandoned(mount: &Path, up: bool) -> usize {
    let mut guard = ABANDONED_BY_MOUNT.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let count = map.entry(mount.to_path_buf()).or_insert(0);
    if up {
        *count += 1;
    } else {
        *count = count.saturating_sub(1);
    }
    let value = *count;
    if value == 0 {
        map.remove(mount);
    }
    value
}

fn abandoned_on(mount: &Path) -> usize {
    let guard = ABANDONED_BY_MOUNT.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(|map| map.get(mount))
        .copied()
        .unwrap_or(0)
}

/// [`list_dir`] that gives up after `timeout`, so a share that stops answering cannot hang
/// the caller.
///
/// The listing runs on a dedicated thread that cannot be interrupted; when the deadline
/// passes the thread is abandoned and keeps running until the filesystem answers. The call
/// fails with [`io::ErrorKind::TimedOut`] when the deadline passes, and with
/// [`io::ErrorKind::ResourceBusy`] at once while [`DEFAULT_MAX_ABANDONED`] abandoned listings
/// are still running on the same mount.
pub fn list_dir_bounded(path: &Path, timeout: Duration) -> io::Result<Listing> {
    list_dir_bounded_with_options(path, timeout, ListOptions::default())
}

/// [`list_dir_bounded`] with the hidden-file and sort choices of `options`.
pub fn list_dir_bounded_with_options(
    path: &Path,
    timeout: Duration,
    options: ListOptions,
) -> io::Result<Listing> {
    list_dir_bounded_with(
        path,
        mount_key(path),
        timeout,
        DEFAULT_MAX_ABANDONED,
        move |path| list_dir_with_options(path, &options),
    )
}

/// [`list_dir_bounded`] with the mount key, the abandoned-thread cap and the listing function
/// supplied, so tests can block the listing without a real hung mount.
#[doc(hidden)]
pub fn list_dir_bounded_with(
    path: &Path,
    mount: PathBuf,
    timeout: Duration,
    max_abandoned: usize,
    list: impl Fn(&Path) -> io::Result<Listing> + Send + 'static,
) -> io::Result<Listing> {
    if abandoned_on(&mount) >= max_abandoned {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            format!("too many unresponsive reads on {}", mount.display()),
        ));
    }

    let (sender, receiver) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(RUNNING));
    let worker_state = Arc::clone(&state);
    let worker_mount = mount.clone();
    let worker_path = path.to_path_buf();
    thread::Builder::new()
        .name("kwati-list".into())
        .spawn(move || {
            let _ = sender.send(list(&worker_path));
            let outcome =
                worker_state.compare_exchange(RUNNING, DONE, Ordering::AcqRel, Ordering::Acquire);
            if outcome.is_err() {
                adjust_abandoned(&worker_mount, false);
            }
        })?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            // Count before publishing ABANDONED so the worker never decrements first.
            adjust_abandoned(&mount, true);
            let outcome =
                state.compare_exchange(RUNNING, ABANDONED, Ordering::AcqRel, Ordering::Acquire);
            if outcome.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("listing {} timed out after {timeout:?}", path.display()),
                ));
            }
            // The listing finished between the timeout and the check; its result is waiting.
            adjust_abandoned(&mount, false);
            receiver
                .try_recv()
                .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
        }
    }
}
