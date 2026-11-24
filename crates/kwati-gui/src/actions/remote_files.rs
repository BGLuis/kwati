//! Opening and editing files on remote network locations (#remote-files).
//!
//! Streamable media (video and audio) is served directly over a loopback HTTP range server when
//! the backend supports random-access seeking and a URL-capable media player is configured.
//! Documents and all other files (or fallback media) are downloaded into a temporary session
//! cache, opened in the system application, and watched so saved edits are uploaded back to the
//! remote server automatically.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use kwati_core::{
    is_streamable_media, media_mime_type, Backend, BackendError, EntryKind, Location,
    PrepareOutcome, Progress, RemoteEditWatcher, SyncOutcome,
};

use crate::actions::open_with::{launch_application, stream_launcher_for, AppInfo};
use crate::actions::open_with_system;
use crate::actions::operations::{
    describe_backend_error, location_display_name, register_job_cancel, unregister_job_cancel,
};
use crate::state::{AppState, JobKind, JobStatus};

static NEXT_REMOTE_JOB_ID: AtomicU64 = AtomicU64::new(1_000_000);

/// Target passed to the OS application launcher when opening a remote file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOpenTarget {
    /// Local cached copy whose directory is watched for edits to sync back.
    File(PathBuf),
    /// Loopback HTTP stream URL for seekable media playback.
    StreamUrl { url: String, app: AppInfo },
}

/// Hook used to launch an opened remote file or stream URL (overridable in tests).
pub type RemoteExternalOpener = Arc<dyn Fn(&RemoteOpenTarget) -> io::Result<()> + Send + Sync>;

/// Hook used to resolve a URL-capable media player for a path (overridable in tests).
pub type StreamLauncherResolver = Arc<dyn Fn(&Path) -> Option<AppInfo> + Send + Sync>;

pub fn default_remote_opener(target: &RemoteOpenTarget) -> io::Result<()> {
    match target {
        RemoteOpenTarget::File(path) => open_with_system(path),
        RemoteOpenTarget::StreamUrl { url, app } => launch_application(app, Path::new(url)),
    }
}

enum OpenResolution {
    Streamed { url: String, app: AppInfo },
    Cached(PrepareOutcome),
}

/// Opens a file on a `Location::Remote` server: streams seekable media directly when possible,
/// otherwise downloads to the local edit-sync cache and opens it with the default application.
pub fn open_remote_file(state: &mut AppState, location: Location) {
    let Location::Remote { ref profile_id, .. } = location else {
        return;
    };
    let name = location_display_name(&location);
    let Some(backend) = state.backend_for(&location) else {
        state.set_status(format!("Cannot open '{name}': server is not connected"));
        return;
    };

    let job_id = NEXT_REMOTE_JOB_ID.fetch_add(1, Ordering::SeqCst);
    let cancel = Arc::new(AtomicBool::new(false));
    register_job_cancel(job_id, cancel.clone());
    state.add_job(JobStatus::new(
        job_id,
        format!("Opening '{name}'"),
        JobKind::Open,
        0,
    ));

    let bytes_done = Arc::new(AtomicU64::new(0));
    let bytes_total = Arc::new(AtomicU64::new(0));
    let cache = Arc::clone(&state.remote_cache);
    let stream_server = state.stream_server_cell();
    let launcher_resolver = state
        .stream_launcher_override()
        .unwrap_or_else(|| Arc::new(stream_launcher_for));
    let profile_id_owned = profile_id.clone();

    let work = {
        let location = location.clone();
        let cancel = cancel.clone();
        let bytes_done = bytes_done.clone();
        let bytes_total = bytes_total.clone();
        move || -> Result<OpenResolution, String> {
            if is_streamable_media(location.path()) {
                if let Some(app) = launcher_resolver(location.path()) {
                    if let Some(url) = try_prepare_stream(&backend, &location, &stream_server) {
                        return Ok(OpenResolution::Streamed { url, app });
                    }
                }
            }

            let outcome = cache
                .prepare_local_copy(&*backend, &location, cancel, &mut |p: Progress| {
                    bytes_done.store(p.bytes_done, Ordering::Relaxed);
                    if p.bytes_total > 0 {
                        bytes_total.store(p.bytes_total, Ordering::Relaxed);
                    }
                })
                .map_err(|err| describe_backend_error(&err))?;
            Ok(OpenResolution::Cached(outcome))
        }
    };

    let tick = {
        let bytes_done = bytes_done.clone();
        let bytes_total = bytes_total.clone();
        move |state: &mut AppState| {
            if let Some(job) = state.active_jobs.iter_mut().find(|j| j.id == job_id) {
                let total = bytes_total.load(Ordering::Relaxed);
                if total > 0 {
                    job.bytes_total = total;
                }
                job.update_progress(bytes_done.load(Ordering::Relaxed));
            }
        }
    };

    let done = move |state: &mut AppState, result: Result<OpenResolution, String>| {
        unregister_job_cancel(job_id);
        match result {
            Ok(OpenResolution::Streamed { url, app }) => {
                state.remove_job(job_id);
                let opener = state.remote_opener();
                let target = RemoteOpenTarget::StreamUrl { url, app };
                match opener(&target) {
                    Ok(()) => {
                        state.set_completion_status(format!("Streaming '{name}'"));
                    }
                    Err(err) => {
                        state.set_status(format!("Cannot stream '{name}': {err}"));
                    }
                }
            }
            Ok(OpenResolution::Cached(PrepareOutcome::Cancelled)) => {
                state.remove_job(job_id);
                state.set_status(format!("Cancelled opening '{name}'"));
            }
            Ok(OpenResolution::Cached(outcome)) => {
                let (local_path, reused_modified) = match outcome {
                    PrepareOutcome::Downloaded(path) => (path, false),
                    PrepareOutcome::ReusedModified(path) => (path, true),
                    PrepareOutcome::Cancelled => unreachable!(),
                };
                state.complete_job(job_id);
                ensure_edit_watcher(state, location.clone(), &local_path);
                let opener = state.remote_opener();
                let target = RemoteOpenTarget::File(local_path);
                match opener(&target) {
                    Ok(()) => {
                        if reused_modified {
                            state.set_completion_status(format!(
                                "Opened '{name}' (unsaved local edits preserved)"
                            ));
                        } else {
                            state.set_completion_status(format!(
                                "Opened '{name}' (edits sync automatically)"
                            ));
                        }
                    }
                    Err(err) => {
                        state.set_status(format!("Cannot open '{name}': {err}"));
                    }
                }
                let _ = profile_id_owned;
            }
            Err(err) => {
                state.remove_job(job_id);
                state.set_status(format!("Cannot open '{name}': {err}"));
            }
        }
    };

    state.spawn_job(work, tick, done);
}

fn try_prepare_stream(
    backend: &Arc<dyn Backend>,
    location: &Location,
    server_cell: &Arc<std::sync::OnceLock<Result<kwati_core::RemoteStreamServer, String>>>,
) -> Option<String> {
    let meta = backend.stat(location).ok()?;
    if meta.kind != EntryKind::File {
        return None;
    }
    // Verify the backend provides a RangeSource (SMB, SFTP, and FTP do; non-seekable mocks do not).
    let probe = backend.open_range_source(location).ok()?;
    drop(probe);

    let server = server_cell
        .get_or_init(|| kwati_core::RemoteStreamServer::start().map_err(|e| e.to_string()))
        .as_ref()
        .ok()?;
    let mime = media_mime_type(location.path());
    Some(server.register(Arc::clone(backend), location.clone(), meta.len, mime))
}

fn ensure_edit_watcher(state: &mut AppState, location: Location, local_path: &Path) {
    if state.has_remote_edit_watcher(&location) {
        return;
    }
    let Some(parent) = local_path.parent() else {
        return;
    };
    let tx = state.remote_edit_sender();
    let debounce = state.remote_edit_debounce();
    if let Ok(watcher) = RemoteEditWatcher::watch(parent, location.clone(), debounce, move |loc| {
        let _ = tx.unbounded_send(loc);
    }) {
        state.insert_remote_edit_watcher(location, watcher);
    }
}

/// Uploads any locally modified cached copy of `location` back to its remote server.
pub fn sync_remote_edit(state: &mut AppState, location: Location) {
    if !state.begin_remote_edit_upload(&location) {
        return;
    }

    let name = location_display_name(&location);
    let server_name = match &location {
        Location::Remote { profile_id, .. } => state
            .remote
            .profile(profile_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "server".to_string()),
        Location::Local(_) => {
            state.finish_remote_edit_upload(&location);
            return;
        }
    };

    let Some(backend) = state.backend_for(&location) else {
        state.finish_remote_edit_upload(&location);
        state.set_status(format!(
            "Cannot save '{name}': server is not connected (local edits kept)"
        ));
        return;
    };

    let cache = Arc::clone(&state.remote_cache);
    let loc_bg = location.clone();
    let work =
        move || -> Result<SyncOutcome, BackendError> { cache.sync_if_modified(&*backend, &loc_bg) };

    let done = move |state: &mut AppState, result: Result<SyncOutcome, BackendError>| {
        let need_queued_resync = state.finish_remote_edit_upload(&location);
        match result {
            Ok(SyncOutcome::Uploaded) => {
                state.set_completion_status(format!("Saved '{name}' to {server_name}"));
                if location.parent().as_ref() == Some(&state.current_location) {
                    state.reload_entries_from_background();
                }
                if need_queued_resync {
                    sync_remote_edit(state, location);
                }
            }
            Ok(SyncOutcome::UploadedNeedResync) => {
                state.set_completion_status(format!("Saved '{name}' to {server_name}"));
                if location.parent().as_ref() == Some(&state.current_location) {
                    state.reload_entries_from_background();
                }
                sync_remote_edit(state, location);
            }
            Ok(SyncOutcome::Unchanged | SyncOutcome::SkippedMissing) => {
                if need_queued_resync {
                    sync_remote_edit(state, location);
                }
            }
            Ok(SyncOutcome::Conflict { .. }) => {
                state.set_status(format!(
                    "Conflict saving '{name}': remote file changed on {server_name} (local edits kept)"
                ));
            }
            Err(err) => {
                state.set_status(format!(
                    "Failed to save '{name}': {}",
                    describe_backend_error(&err)
                ));
            }
        }
    };

    state.spawn_job(work, |_| {}, done);
}
