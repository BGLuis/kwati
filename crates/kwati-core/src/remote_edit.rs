use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::{
    copy_between, Backend, BackendError, ConflictPolicy, CopyOptions, CopyOutcome, LocalBackend,
    Location, Progress,
};

/// Result of preparing a local cached copy of a remote file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// Downloaded fresh bytes from the remote backend into the local cache path.
    Downloaded(PathBuf),
    /// Kept an existing cached copy because it already had unsaved local modifications.
    ReusedModified(PathBuf),
    /// Download was cancelled before completion.
    Cancelled,
}

/// Result of checking a cached remote file and uploading local edits if changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// No local edits compared to the last synced digest.
    Unchanged,
    /// Cached file was temporarily absent (e.g. mid-atomic-save rename).
    SkippedMissing,
    /// Local edits were uploaded and the synced digest updated.
    Uploaded,
    /// Local edits were uploaded, and another local edit happened while uploading.
    UploadedNeedResync,
    /// The remote file changed on the server since it was downloaded; upload paused to avoid
    /// silently overwriting remote work.
    Conflict {
        remote_len: u64,
        remote_modified: Option<SystemTime>,
    },
}

#[derive(Debug, Clone)]
struct CachedEntryState {
    local_path: PathBuf,
    last_synced_digest: (u64, u128),
    last_local_meta: Option<(u64, Option<SystemTime>)>,
    baseline_remote_len: Option<u64>,
    baseline_remote_modified: Option<SystemTime>,
}

/// Computes `(byte_len, fnv1a_128)` for `path` in bounded memory.
pub fn content_digest(path: &Path) -> io::Result<(u64, u128)> {
    const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    let mut file = File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    let mut hash = OFFSET_BASIS;
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        len = len.saturating_add(n as u64);
        for &byte in &buf[..n] {
            hash ^= u128::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    Ok((len, hash))
}

/// Manages temporary local copies of opened remote files and tracks their synced digests so edits
/// made in external applications can be uploaded back to the remote server.
pub struct RemoteFileCache {
    root: PathBuf,
    entries: Mutex<HashMap<Location, CachedEntryState>>,
}

impl Default for RemoteFileCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteFileCache {
    /// Creates a cache under the system temporary directory scoped to this instance.
    pub fn new() -> Self {
        static CACHE_SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = CACHE_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("kwati-remote-cache-{}-{seq}", std::process::id()));
        Self::with_root(root)
    }

    /// Creates a cache rooted at `root`.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns `(entry_dir, local_file_path)` for `location`, preserving its filename and extension.
    pub fn cache_paths_for(&self, location: &Location) -> (PathBuf, PathBuf) {
        let (profile_id, remote_path) = match location {
            Location::Remote { profile_id, path } => (profile_id.as_str(), path.as_path()),
            Location::Local(path) => ("local", path.as_path()),
        };
        let safe_profile: String = profile_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let hash = fnv1a_64_pair(
            profile_id.as_bytes(),
            remote_path.to_string_lossy().as_bytes(),
        );
        let file_name = remote_path
            .file_name()
            .filter(|n| !n.is_empty())
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "remote-file".into());
        let dir = self.root.join(safe_profile).join(format!("{hash:016x}"));
        let file_path = dir.join(file_name);
        (dir, file_path)
    }

    /// Returns a dedicated cache path for non-watched preview assets (e.g. remote images).
    pub fn preview_path_for(&self, location: &Location) -> PathBuf {
        let (_, regular) = self.cache_paths_for(location);
        let name = regular
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "preview".into());
        regular
            .parent()
            .unwrap_or(&self.root)
            .join("preview")
            .join(name)
    }

    /// Downloads `location` into the local cache unless a locally modified cached copy already
    /// exists (so unsaved user edits are never overwritten).
    pub fn prepare_local_copy(
        &self,
        backend: &dyn Backend,
        location: &Location,
        cancel: Arc<AtomicBool>,
        on_progress: &mut dyn FnMut(Progress),
    ) -> Result<PrepareOutcome, BackendError> {
        let (entry_dir, local_path) = self.cache_paths_for(location);
        if let Some(existing) = self.entries.lock().unwrap().get(location).cloned() {
            if existing.local_path.is_file() {
                if let Ok(digest) = content_digest(&existing.local_path) {
                    if digest != existing.last_synced_digest {
                        return Ok(PrepareOutcome::ReusedModified(existing.local_path));
                    }
                }
            }
        }

        fs::create_dir_all(&entry_dir)?;
        let baseline_remote_len = backend.stat(location).ok().map(|m| m.len);
        let baseline_remote_modified = backend.stat_modified(location).ok().flatten();
        let options = CopyOptions::new(ConflictPolicy::Replace, cancel);
        let local_backend = LocalBackend::new();
        let dest_dir = Location::Local(entry_dir);
        let outcome = copy_between(
            backend,
            location,
            &local_backend,
            &dest_dir,
            &options,
            on_progress,
        )?;
        if outcome == CopyOutcome::Cancelled {
            return Ok(PrepareOutcome::Cancelled);
        }

        let digest = content_digest(&local_path)?;
        let local_meta = local_file_meta(&local_path);
        self.entries.lock().unwrap().insert(
            location.clone(),
            CachedEntryState {
                local_path: local_path.clone(),
                last_synced_digest: digest,
                last_local_meta: local_meta,
                baseline_remote_len,
                baseline_remote_modified,
            },
        );
        Ok(PrepareOutcome::Downloaded(local_path))
    }

    /// Checks whether the cached copy of `location` changed since the last sync and, if so,
    /// uploads it atomically back to the remote `backend` unless the remote file changed
    /// concurrently.
    pub fn sync_if_modified(
        &self,
        backend: &dyn Backend,
        location: &Location,
    ) -> Result<SyncOutcome, BackendError> {
        self.sync_with_policy(backend, location, false)
    }

    /// Uploads local modifications to `backend`, overwriting even if the remote file changed
    /// since the baseline was recorded.
    pub fn force_sync_modified(
        &self,
        backend: &dyn Backend,
        location: &Location,
    ) -> Result<SyncOutcome, BackendError> {
        self.sync_with_policy(backend, location, true)
    }

    fn sync_with_policy(
        &self,
        backend: &dyn Backend,
        location: &Location,
        force_overwrite: bool,
    ) -> Result<SyncOutcome, BackendError> {
        let Some(state) = self.entries.lock().unwrap().get(location).cloned() else {
            return Ok(SyncOutcome::Unchanged);
        };
        if !state.local_path.is_file() {
            return Ok(SyncOutcome::SkippedMissing);
        }

        let current_local_meta = local_file_meta(&state.local_path);
        if let (Some(prev), Some(curr)) = (&state.last_local_meta, &current_local_meta) {
            if prev == curr && prev.1.is_some() {
                return Ok(SyncOutcome::Unchanged);
            }
        }

        let upload_digest = match content_digest(&state.local_path) {
            Ok(d) => d,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(SyncOutcome::SkippedMissing);
            }
            Err(err) => return Err(err.into()),
        };

        if upload_digest == state.last_synced_digest {
            if let Some(entry) = self.entries.lock().unwrap().get_mut(location) {
                entry.last_local_meta = current_local_meta;
            }
            return Ok(SyncOutcome::Unchanged);
        }

        if !force_overwrite {
            if let Ok(remote_meta) = backend.stat(location) {
                let remote_modified = backend.stat_modified(location).ok().flatten();
                let len_changed = state
                    .baseline_remote_len
                    .is_some_and(|base| base != remote_meta.len);
                let mtime_changed = match (state.baseline_remote_modified, remote_modified) {
                    (Some(base), Some(curr)) => base != curr,
                    _ => false,
                };
                if len_changed || mtime_changed {
                    return Ok(SyncOutcome::Conflict {
                        remote_len: remote_meta.len,
                        remote_modified,
                    });
                }
            }
        }

        let parent = location.parent().ok_or_else(|| {
            BackendError::from(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote location has no parent directory",
            ))
        })?;

        let cancel = Arc::new(AtomicBool::new(false));
        let options = CopyOptions::new(ConflictPolicy::Replace, cancel);
        let local_backend = LocalBackend::new();
        copy_between(
            &local_backend,
            &Location::Local(state.local_path.clone()),
            backend,
            &parent,
            &options,
            &mut |_| {},
        )?;

        let new_remote_len = backend.stat(location).ok().map(|m| m.len);
        let new_remote_modified = backend.stat_modified(location).ok().flatten();
        let after_local_meta = local_file_meta(&state.local_path);
        {
            let mut map = self.entries.lock().unwrap();
            if let Some(entry) = map.get_mut(location) {
                entry.last_synced_digest = upload_digest;
                entry.last_local_meta = after_local_meta;
                entry.baseline_remote_len = new_remote_len;
                entry.baseline_remote_modified = new_remote_modified;
            }
        }

        if let Ok(after_digest) = content_digest(&state.local_path) {
            if after_digest != upload_digest {
                return Ok(SyncOutcome::UploadedNeedResync);
            }
        }
        Ok(SyncOutcome::Uploaded)
    }

    /// Returns the cached local path if `location` has been prepared.
    pub fn cached_path(&self, location: &Location) -> Option<PathBuf> {
        self.entries
            .lock()
            .unwrap()
            .get(location)
            .map(|e| e.local_path.clone())
    }
}

impl Drop for RemoteFileCache {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn local_file_meta(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

fn fnv1a_64_pair(a: &[u8], b: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in a.iter().chain(&[0xff]).chain(b.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[allow(dead_code)]
enum EditWatcherImpl {
    Recommended(RecommendedWatcher),
    Poll(PollWatcher),
}

/// Watches the cache directory of an opened remote file and invokes `on_change` once per burst
/// of filesystem edits after `debounce`.
pub struct RemoteEditWatcher {
    watcher: Option<EditWatcherImpl>,
    thread: Option<JoinHandle<()>>,
}

impl RemoteEditWatcher {
    pub fn watch(
        dir: &Path,
        location: Location,
        debounce: Duration,
        mut on_change: impl FnMut(Location) + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let tx_native = tx.clone();
        let watcher =
            match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(ev) = event {
                    if !matches!(ev.kind, EventKind::Access(_)) {
                        let _ = tx_native.send(());
                    }
                }
            }) {
                Ok(mut native) => {
                    native.watch(dir, RecursiveMode::NonRecursive)?;
                    EditWatcherImpl::Recommended(native)
                }
                Err(_) => {
                    let config =
                        notify::Config::default().with_poll_interval(Duration::from_secs(1));
                    let mut poll = notify::PollWatcher::new(
                        move |event: notify::Result<notify::Event>| {
                            if let Ok(ev) = event {
                                if !matches!(ev.kind, EventKind::Access(_)) {
                                    let _ = tx.send(());
                                }
                            }
                        },
                        config,
                    )?;
                    poll.watch(dir, RecursiveMode::NonRecursive)?;
                    EditWatcherImpl::Poll(poll)
                }
            };

        let thread = thread::Builder::new()
            .name("kwati-remote-edit-watch".to_string())
            .spawn(move || {
                while rx.recv().is_ok() {
                    let deadline = Instant::now() + debounce;
                    loop {
                        let left = deadline.saturating_duration_since(Instant::now());
                        match rx.recv_timeout(left) {
                            Ok(()) => {}
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    on_change(location.clone());
                }
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;

        Ok(Self {
            watcher: Some(watcher),
            thread: Some(thread),
        })
    }
}

impl Drop for RemoteEditWatcher {
    fn drop(&mut self) {
        self.watcher.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Confirmed, Entry, EntryKind, EntryMeta, Listing};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MemBackend {
        files: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
    }

    impl MemBackend {
        fn new() -> Self {
            Self {
                files: Arc::new(Mutex::new(BTreeMap::new())),
            }
        }

        fn insert(&self, path: &str, content: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert(PathBuf::from(path), content.to_vec());
        }

        fn get(&self, path: &str) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(Path::new(path)).cloned()
        }
    }

    struct MemWriter {
        path: PathBuf,
        buf: Vec<u8>,
        files: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
    }

    impl Write for MemWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(self.path.clone(), self.buf.clone());
            Ok(())
        }
    }

    impl Drop for MemWriter {
        fn drop(&mut self) {
            let _ = self.flush();
        }
    }

    impl Backend for MemBackend {
        fn list(&self, location: &Location) -> Result<Listing, BackendError> {
            let dir = location.path();
            let entries = self
                .files
                .lock()
                .unwrap()
                .keys()
                .filter(|p| p.parent() == Some(dir))
                .map(|p| Entry::new(p.file_name().unwrap().to_os_string(), p.clone(), false))
                .collect();
            Ok(Listing::new(entries))
        }

        fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
            if let Some(bytes) = self.files.lock().unwrap().get(location.path()) {
                return Ok(EntryMeta {
                    kind: EntryKind::File,
                    len: bytes.len() as u64,
                });
            }
            if location.path() == Path::new("/") || location.path() == Path::new("/docs") {
                return Ok(EntryMeta {
                    kind: EntryKind::Dir,
                    len: 0,
                });
            }
            Err(io::Error::from(io::ErrorKind::NotFound).into())
        }

        fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
            let bytes = self
                .files
                .lock()
                .unwrap()
                .get(location.path())
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            Ok(Box::new(io::Cursor::new(bytes)))
        }

        fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
            Ok(Box::new(MemWriter {
                path: location.path().to_path_buf(),
                buf: Vec::new(),
                files: Arc::clone(&self.files),
            }))
        }

        fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
            let mut files = self.files.lock().unwrap();
            let bytes = files
                .remove(from.path())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            files.insert(to.path().to_path_buf(), bytes);
            Ok(())
        }

        fn delete_permanently(
            &self,
            location: &Location,
            _confirmed: Confirmed,
        ) -> Result<(), BackendError> {
            self.files.lock().unwrap().remove(location.path());
            Ok(())
        }
    }

    fn temp_cache_root(name: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "kwati_cache_test_{name}_{}_{id}",
            std::process::id()
        ))
    }

    #[test]
    fn downloads_edits_and_uploads_changes_back_without_clobbering_dirty_local_copy() {
        let root = temp_cache_root("edit_sync");
        let cache = RemoteFileCache::with_root(root.clone());
        let backend = MemBackend::new();
        backend.insert("/docs/notes.txt", b"hello remote");

        let loc = Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/docs/notes.txt"),
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let local_path = match cache
            .prepare_local_copy(&backend, &loc, cancel.clone(), &mut |_| {})
            .unwrap()
        {
            PrepareOutcome::Downloaded(p) => p,
            other => panic!("expected Downloaded, got {other:?}"),
        };
        assert_eq!(local_path.file_name().unwrap(), "notes.txt");
        assert_eq!(fs::read(&local_path).unwrap(), b"hello remote");

        // No change yet -> Unchanged
        assert_eq!(
            cache.sync_if_modified(&backend, &loc).unwrap(),
            SyncOutcome::Unchanged
        );

        // External editor modifies the cached copy (simulate atomic save via rename)
        let tmp = local_path.with_file_name(".notes.txt.tmp");
        fs::write(&tmp, b"edited by user").unwrap();
        fs::rename(&tmp, &local_path).unwrap();

        // Re-opening before upload must preserve local unsaved edits
        let reused = cache
            .prepare_local_copy(&backend, &loc, cancel.clone(), &mut |_| {})
            .unwrap();
        assert_eq!(reused, PrepareOutcome::ReusedModified(local_path.clone()));
        assert_eq!(fs::read(&local_path).unwrap(), b"edited by user");

        // Sync uploads the modified file back to remote
        assert_eq!(
            cache.sync_if_modified(&backend, &loc).unwrap(),
            SyncOutcome::Uploaded
        );
        assert_eq!(backend.get("/docs/notes.txt").unwrap(), b"edited by user");

        // Subsequent sync is Unchanged
        assert_eq!(
            cache.sync_if_modified(&backend, &loc).unwrap(),
            SyncOutcome::Unchanged
        );

        // Drop cleans up the temporary root
        drop(cache);
        assert!(!root.exists());
    }

    #[test]
    fn watcher_notifies_on_atomic_rename_edit() {
        let root = temp_cache_root("watch");
        let cache = RemoteFileCache::with_root(root.clone());
        let backend = MemBackend::new();
        backend.insert("/docs/readme.md", b"v1");

        let loc = Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/docs/readme.md"),
        };
        let local_path = match cache
            .prepare_local_copy(
                &backend,
                &loc,
                Arc::new(AtomicBool::new(false)),
                &mut |_| {},
            )
            .unwrap()
        {
            PrepareOutcome::Downloaded(p) => p,
            other => panic!("expected Downloaded, got {other:?}"),
        };

        let (tx, rx) = mpsc::channel();
        let _watcher = RemoteEditWatcher::watch(
            local_path.parent().unwrap(),
            loc.clone(),
            Duration::from_millis(80),
            move |changed| {
                let _ = tx.send(changed);
            },
        )
        .unwrap();

        fs::write(&local_path, b"v2").unwrap();
        let notified = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(notified, loc);
    }

    #[test]
    fn detects_remote_conflict_before_overwriting_and_allows_force_sync() {
        let root = temp_cache_root("conflict");
        let cache = RemoteFileCache::with_root(root);
        let backend = MemBackend::new();
        backend.insert("/docs/shared.txt", b"base-v1");

        let loc = Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/docs/shared.txt"),
        };
        let local_path = match cache
            .prepare_local_copy(
                &backend,
                &loc,
                Arc::new(AtomicBool::new(false)),
                &mut |_| {},
            )
            .unwrap()
        {
            PrepareOutcome::Downloaded(p) => p,
            other => panic!("expected Downloaded, got {other:?}"),
        };

        // Another user modifies the file on the server concurrently (different size).
        backend.insert("/docs/shared.txt", b"remote-v2-concurrent");
        // Local user also edits the cached copy.
        fs::write(&local_path, b"local-v2").unwrap();

        let outcome = cache.sync_if_modified(&backend, &loc).unwrap();
        assert!(matches!(
            outcome,
            SyncOutcome::Conflict {
                remote_len: 20,
                remote_modified: None
            }
        ));
        // Server must NOT be overwritten on conflict.
        assert_eq!(
            backend.get("/docs/shared.txt").unwrap(),
            b"remote-v2-concurrent"
        );

        // Explicit force sync overwrites and updates the baseline.
        assert_eq!(
            cache.force_sync_modified(&backend, &loc).unwrap(),
            SyncOutcome::Uploaded
        );
        assert_eq!(backend.get("/docs/shared.txt").unwrap(), b"local-v2");
    }
}
