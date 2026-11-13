//! File operations and undo actions (#17, #19, #21, #23, #35, #48, #65).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::actions;
use kwati_core::{
    copy_between, move_between, move_entry, Backend, BackendError, Confirmed, ConflictPolicy,
    CopyOptions, CopyOutcome, EntryKind, LocalBackend, Location, Progress,
};

use crate::state::{AppState, JobKind, JobStatus, UndoOperation};

actions!(
    operations,
    [Copy, Cut, Paste, Delete, Trash, Rename, NewFolder, Undo,]
);

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_JOB_CANCELS: Mutex<Option<HashMap<u64, Arc<AtomicBool>>>> = Mutex::new(None);
static CLIPBOARD: Mutex<Option<LocationClipboard>> = Mutex::new(None);

/// Location-aware clipboard contents for Copy and Cut/Paste across local and remote backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationClipboard {
    pub sources: Vec<Location>,
    pub is_cut: bool,
}

/// Legacy local-only clipboard view kept for callers that have not yet migrated to [`LocationClipboard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipboard {
    pub sources: Vec<PathBuf>,
    pub is_cut: bool,
}

pub fn copy_locations_to_clipboard(sources: Vec<Location>) {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = Some(LocationClipboard {
        sources,
        is_cut: false,
    });
}

pub fn cut_locations_to_clipboard(sources: Vec<Location>) {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = Some(LocationClipboard {
        sources,
        is_cut: true,
    });
}

pub fn get_location_clipboard() -> Option<LocationClipboard> {
    CLIPBOARD.lock().unwrap().clone()
}

pub fn copy_to_clipboard(sources: Vec<PathBuf>) {
    copy_locations_to_clipboard(sources.into_iter().map(Location::Local).collect());
}

pub fn cut_to_clipboard(sources: Vec<PathBuf>) {
    cut_locations_to_clipboard(sources.into_iter().map(Location::Local).collect());
}

pub fn get_clipboard() -> Option<Clipboard> {
    let cb = get_location_clipboard()?;
    let paths: Option<Vec<PathBuf>> = cb
        .sources
        .into_iter()
        .map(|loc| match loc {
            Location::Local(p) => Some(p),
            Location::Remote { .. } => None,
        })
        .collect();
    paths.map(|sources| Clipboard {
        sources,
        is_cut: cb.is_cut,
    })
}

pub fn clear_clipboard() {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = None;
}

pub fn register_job_cancel(job_id: u64, cancel: Arc<AtomicBool>) {
    let mut map = ACTIVE_JOB_CANCELS.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    map.insert(job_id, cancel);
}

pub fn unregister_job_cancel(job_id: u64) {
    let mut map = ACTIVE_JOB_CANCELS.lock().unwrap();
    if let Some(m) = map.as_mut() {
        m.remove(&job_id);
    }
}

pub fn cancel_active_job(job_id: u64) {
    let map = ACTIVE_JOB_CANCELS.lock().unwrap();
    if let Some(m) = map.as_ref() {
        if let Some(flag) = m.get(&job_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Formats a [`BackendError`] cleanly for status bar and dialog error messages.
pub fn describe_backend_error(err: &BackendError) -> String {
    match err {
        BackendError::Io(io_err) => io_err.to_string(),
        BackendError::Unsupported => "operation unsupported".to_string(),
    }
}

/// Returns a human-readable entry name for a [`Location`].
pub fn location_display_name(location: &Location) -> String {
    location
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| location.path().to_string_lossy().into_owned())
}

fn is_not_found(err: &BackendError) -> bool {
    matches!(err, BackendError::Io(e) if e.kind() == io::ErrorKind::NotFound)
}

fn backend_exists(backend: &dyn Backend, location: &Location) -> Result<bool, BackendError> {
    match backend.stat(location) {
        Ok(_) => Ok(true),
        Err(err) if is_not_found(&err) => Ok(false),
        Err(BackendError::Unsupported) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Checks if pasting `sources` into `dest_dir` on `dest_backend` would conflict with existing entries.
/// Returns the name of the first conflicting entry, if any.
pub fn check_paste_conflict_at(
    sources: &[Location],
    dest_dir: &Location,
    dest_backend: &dyn Backend,
) -> Result<Option<String>, String> {
    for src in sources {
        if let Some(name) = src.file_name() {
            let target = dest_dir.join(name);
            match dest_backend.stat(&target) {
                Ok(_) => return Ok(Some(name.to_string_lossy().into_owned())),
                Err(err) if is_not_found(&err) => {}
                Err(BackendError::Unsupported) => {}
                Err(err) => return Err(describe_backend_error(&err)),
            }
        }
    }
    Ok(None)
}

/// Convenience wrapper around [`check_paste_conflict_at`] resolving the destination backend from `state`.
pub fn check_paste_conflict_in_state(
    state: &AppState,
    sources: &[Location],
    dest_dir: &Location,
) -> Result<Option<String>, String> {
    let backend = state
        .backend_for(dest_dir)
        .ok_or_else(|| "Destination server is not connected".to_string())?;
    check_paste_conflict_at(sources, dest_dir, &*backend)
}

/// Checks if pasting the given local sources into `dest_dir` would conflict with existing entries.
pub fn check_paste_conflict(sources: &[PathBuf], dest_dir: &Path) -> Option<String> {
    let locs: Vec<Location> = sources.iter().cloned().map(Location::Local).collect();
    let dest = Location::Local(dest_dir.to_path_buf());
    check_paste_conflict_at(&locs, &dest, &LocalBackend::new())
        .ok()
        .flatten()
}

fn backend_key(loc: &Location) -> Option<String> {
    match loc {
        Location::Local(_) => None,
        Location::Remote { profile_id, .. } => Some(profile_id.clone()),
    }
}

/// A copy or a move of `sources` into `dest_dir`, across any combination of local and remote locations.
#[derive(Clone)]
pub struct Transfer {
    pub kind: JobKind,
    pub sources: Vec<Location>,
    pub dest_dir: Location,
    pub policy: ConflictPolicy,
    pub cancel: Arc<AtomicBool>,
    src_backends: HashMap<Option<String>, Arc<dyn Backend>>,
    dst_backend: Option<Arc<dyn Backend>>,
    simulate_cross_device: bool,
}

impl fmt::Debug for Transfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transfer")
            .field("kind", &self.kind)
            .field("sources", &self.sources)
            .field("dest_dir", &self.dest_dir)
            .field("policy", &self.policy)
            .field("cancel", &self.cancel)
            .field("simulate_cross_device", &self.simulate_cross_device)
            .finish()
    }
}

impl Transfer {
    pub fn copy(
        sources: impl IntoIterator<Item = impl Into<Location>>,
        dest_dir: impl Into<Location>,
        policy: ConflictPolicy,
    ) -> Self {
        Self {
            kind: JobKind::Copy,
            sources: sources.into_iter().map(Into::into).collect(),
            dest_dir: dest_dir.into(),
            policy,
            cancel: Arc::new(AtomicBool::new(false)),
            src_backends: HashMap::new(),
            dst_backend: None,
            simulate_cross_device: false,
        }
    }

    pub fn moving(
        sources: impl IntoIterator<Item = impl Into<Location>>,
        dest_dir: impl Into<Location>,
        policy: ConflictPolicy,
    ) -> Self {
        Self {
            kind: JobKind::Move,
            ..Self::copy(sources, dest_dir, policy)
        }
    }

    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_dest_backend(mut self, backend: Arc<dyn Backend>) -> Self {
        self.dst_backend = Some(backend);
        self
    }

    pub fn with_source_backend(
        mut self,
        profile_id: Option<String>,
        backend: Arc<dyn Backend>,
    ) -> Self {
        self.src_backends.insert(profile_id, backend);
        self
    }

    fn is_move(&self) -> bool {
        self.kind == JobKind::Move
    }

    fn resolve_backends(&mut self, state: &AppState) -> Result<(), String> {
        if self.dst_backend.is_none() {
            let backend = state
                .backend_for(&self.dest_dir)
                .ok_or_else(|| "Destination server is not connected".to_string())?;
            self.dst_backend = Some(backend);
        }
        for src in &self.sources {
            let key = backend_key(src);
            if !self.src_backends.contains_key(&key) {
                let backend = state
                    .backend_for(src)
                    .ok_or_else(|| "Source server is not connected".to_string())?;
                self.src_backends.insert(key, backend);
            }
        }
        Ok(())
    }
}

/// What a transfer did, gathered without touching [`AppState`] so it can be produced off the main thread.
struct TransferReport {
    outcome: CopyOutcome,
    transferred: usize,
    // Only local entries whose target did not exist before: undoing anything else would delete or
    // move a file the user already had under that name, and remote operations are not undoable.
    undo_from: Vec<PathBuf>,
    undo_to: Vec<PathBuf>,
    error: Option<String>,
}

fn size_of_location(
    backend: &dyn Backend,
    location: &Location,
    cancel: &AtomicBool,
) -> Option<u64> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let meta = backend.stat(location).ok()?;
    match meta.kind {
        EntryKind::File => Some(meta.len),
        EntryKind::Symlink => Some(0),
        EntryKind::Dir => {
            let listing = backend.list(location).ok()?;
            let mut sum = 0u64;
            for entry in listing.iter() {
                sum = sum.saturating_add(size_of_location(
                    backend,
                    &location.join(entry.name()),
                    cancel,
                )?);
            }
            Some(sum)
        }
    }
}

fn compute_total_size(transfer: &Transfer) -> u64 {
    let mut sum = 0u64;
    for src in &transfer.sources {
        let Some(src_backend) = transfer.src_backends.get(&backend_key(src)) else {
            continue;
        };
        if let Some(bytes) = size_of_location(&**src_backend, src, &transfer.cancel) {
            sum = sum.saturating_add(bytes);
        }
    }
    sum
}

fn begin_transfer(state: &mut AppState, transfer: &Transfer) -> u64 {
    let job_id = NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst);
    register_job_cancel(job_id, transfer.cancel.clone());

    let verb = if transfer.is_move() {
        "Moving"
    } else {
        "Copying"
    };
    let title = match transfer.sources.as_slice() {
        [single] => format!("{verb} '{}'", location_display_name(single)),
        sources => format!("{verb} {} items", sources.len()),
    };

    state.add_job(JobStatus::new(job_id, title, transfer.kind.clone(), 0));
    job_id
}

fn run_transfer(
    transfer: &Transfer,
    bytes_done: &AtomicU64,
    bytes_total: &AtomicU64,
) -> TransferReport {
    let mut report = TransferReport {
        outcome: CopyOutcome::Completed,
        transferred: 0,
        undo_from: Vec::new(),
        undo_to: Vec::new(),
        error: None,
    };

    if transfer.cancel.load(Ordering::Relaxed) {
        report.outcome = CopyOutcome::Cancelled;
        return report;
    }

    let Some(dst_backend) = transfer.dst_backend.as_deref() else {
        report.error = Some("Destination server is not connected".to_string());
        return report;
    };

    let grand_total = compute_total_size(transfer);
    if grand_total > 0 {
        bytes_total.store(grand_total, Ordering::Relaxed);
    }

    let mut finished_bytes = 0u64;

    for src in &transfer.sources {
        if transfer.cancel.load(Ordering::Relaxed) {
            report.outcome = CopyOutcome::Cancelled;
            break;
        }

        let Some(src_backend) = transfer.src_backends.get(&backend_key(src)).map(|b| &**b) else {
            report.error = Some("Source server is not connected".to_string());
            break;
        };

        let target = src.file_name().map(|name| transfer.dest_dir.join(name));
        let both_local = matches!(
            (src, &transfer.dest_dir),
            (Location::Local(_), Location::Local(_))
        );
        let target_existed = if both_local {
            target
                .as_ref()
                .is_some_and(|t| backend_exists(dst_backend, t).unwrap_or(true))
        } else {
            true
        };

        let options = CopyOptions::new(transfer.policy, transfer.cancel.clone());
        let mut last_src_done = 0u64;
        let mut on_progress = |p: Progress| {
            last_src_done = p.bytes_done;
            bytes_done.store(finished_bytes + p.bytes_done, Ordering::Relaxed);
            if grand_total == 0 && p.bytes_total > 0 {
                bytes_total.store(finished_bytes + p.bytes_total, Ordering::Relaxed);
            }
        };

        let result = if !transfer.is_move() {
            copy_between(
                src_backend,
                src,
                dst_backend,
                &transfer.dest_dir,
                &options,
                &mut on_progress,
            )
        } else if let (Location::Local(src_path), Location::Local(dst_path), false) =
            (src, &transfer.dest_dir, transfer.simulate_cross_device)
        {
            move_entry(src_path, dst_path, &options, &mut on_progress).map_err(BackendError::from)
        } else {
            move_between(
                src_backend,
                src,
                dst_backend,
                &transfer.dest_dir,
                &options,
                &mut on_progress,
            )
        };

        match result {
            Ok(CopyOutcome::Completed) => {
                finished_bytes = finished_bytes.saturating_add(last_src_done);
                bytes_done.store(finished_bytes, Ordering::Relaxed);
                if let Some(target) = target {
                    report.transferred += 1;
                    if both_local && !target_existed {
                        if let (Location::Local(from), Location::Local(to)) = (src, target) {
                            report.undo_from.push(from.clone());
                            report.undo_to.push(to);
                        }
                    }
                }
            }
            Ok(CopyOutcome::Cancelled) => {
                report.outcome = CopyOutcome::Cancelled;
                break;
            }
            Err(err) => {
                report.error = Some(describe_backend_error(&err));
                break;
            }
        }
    }
    report
}

fn finish_transfer(
    state: &mut AppState,
    job_id: u64,
    transfer: &Transfer,
    report: TransferReport,
) -> Result<CopyOutcome, String> {
    unregister_job_cancel(job_id);
    state.remove_job(job_id);

    let (noun, past) = if transfer.is_move() {
        ("Move", "Moved")
    } else {
        ("Copy", "Copied")
    };

    if let Some(err) = report.error {
        let msg = format!("{noun} failed: {err}");
        state.set_status(msg.clone());
        return Err(msg);
    }
    if report.outcome == CopyOutcome::Cancelled {
        state.set_status(format!("{noun} operation cancelled"));
        return Ok(report.outcome);
    }

    if report.transferred > 0 {
        let first_target = transfer
            .sources
            .first()
            .and_then(|src| src.file_name())
            .map(|name| transfer.dest_dir.join(name));
        let viewing_destination = state.current_location == transfer.dest_dir;

        if !report.undo_to.is_empty() {
            state.undo_stack.push(if transfer.is_move() {
                UndoOperation::Move {
                    from: report.undo_from,
                    to: report.undo_to,
                }
            } else {
                UndoOperation::Copy {
                    destinations: report.undo_to,
                }
            });
        }

        state.reload_entries();
        if let Some(first) = first_target {
            if viewing_destination || transfer.is_move() {
                state.select_location(first);
            }
        }
        if transfer.is_move() {
            clear_clipboard();
        }
    }
    state.set_status(format!("{past} {} item(s)", report.transferred));
    Ok(report.outcome)
}

/// Starts `transfer` as a job in the jobs panel. With an attached [`AppState`] the transfer runs
/// on the background executor via `copy_between` / `move_between` with live progress updates.
pub fn start_transfer(state: &mut AppState, mut transfer: Transfer) {
    if transfer.sources.is_empty() {
        return;
    }
    if let Err(err) = transfer.resolve_backends(state) {
        state.set_status(err);
        return;
    }
    let job_id = begin_transfer(state, &transfer);
    let bytes_done = Arc::new(AtomicU64::new(0));
    let bytes_total = Arc::new(AtomicU64::new(0));

    let work = {
        let transfer = transfer.clone();
        let bytes_done = bytes_done.clone();
        let bytes_total = bytes_total.clone();
        move || run_transfer(&transfer, &bytes_done, &bytes_total)
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
    let done = move |state: &mut AppState, report| {
        let _ = finish_transfer(state, job_id, &transfer, report);
    };
    state.spawn_job(work, tick, done);
}

/// Runs `transfer` synchronously on the calling thread and updates `state`.
pub fn execute_transfer(
    state: &mut AppState,
    mut transfer: Transfer,
) -> Result<CopyOutcome, String> {
    if transfer.sources.is_empty() {
        return Ok(CopyOutcome::Completed);
    }
    if let Err(err) = transfer.resolve_backends(state) {
        state.set_status(err.clone());
        return Err(err);
    }
    let job_id = begin_transfer(state, &transfer);
    let report = run_transfer(&transfer, &AtomicU64::new(0), &AtomicU64::new(0));
    finish_transfer(state, job_id, &transfer, report)
}

/// Copies locations into `dest_dir` on the calling thread and waits for the result.
pub fn execute_copy_locations(
    state: &mut AppState,
    sources: &[Location],
    dest_dir: &Location,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
) -> Result<CopyOutcome, String> {
    let transfer =
        Transfer::copy(sources.iter().cloned(), dest_dir.clone(), policy).with_cancel(cancel_flag);
    execute_transfer(state, transfer)
}

/// Moves locations into `dest_dir` on the calling thread and waits for the result.
pub fn execute_move_locations(
    state: &mut AppState,
    sources: &[Location],
    dest_dir: &Location,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
) -> Result<CopyOutcome, String> {
    let transfer = Transfer::moving(sources.iter().cloned(), dest_dir.clone(), policy)
        .with_cancel(cancel_flag);
    execute_transfer(state, transfer)
}

/// Copies local files or folders into `dest_dir` on the calling thread and waits for the result.
/// The interface uses [`start_transfer`] instead, which does not block.
pub fn execute_copy(
    state: &mut AppState,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
) -> Result<CopyOutcome, String> {
    let transfer = Transfer::copy(sources.iter().cloned(), dest_dir.to_path_buf(), policy)
        .with_cancel(cancel_flag);
    execute_transfer(state, transfer)
}

/// Moves local files or folders into `dest_dir` on the calling thread and waits for the result.
/// The interface uses [`start_transfer`] instead, which does not block.
pub fn execute_move(
    state: &mut AppState,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
    simulate_cross_device: bool,
) -> Result<CopyOutcome, String> {
    let mut transfer = Transfer::moving(sources.iter().cloned(), dest_dir.to_path_buf(), policy)
        .with_cancel(cancel_flag);
    transfer.simulate_cross_device = simulate_cross_device;
    execute_transfer(state, transfer)
}

/// Validates that a filename/foldername is non-empty, does not contain path separators,
/// and is not "." or "..".
pub fn validate_entry_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("Invalid entry name".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Name cannot contain path separators".to_string());
    }
    Ok(trimmed)
}

/// Renames `source` to `new_name` via `backend`, refusing to overwrite an existing target.
pub fn rename_location_on_backend(
    backend: &dyn Backend,
    source: &Location,
    new_name: &str,
) -> Result<Location, String> {
    let valid_name = validate_entry_name(new_name)?;
    let parent = source
        .parent()
        .ok_or_else(|| "Cannot rename root directory".to_string())?;
    let target = parent.join(OsStr::new(valid_name));

    if &target == source {
        return Ok(source.clone());
    }

    if backend_exists(backend, &target)
        .map_err(|err| format!("Failed to rename: {}", describe_backend_error(&err)))?
    {
        return Err(format!("Target already exists: {}", valid_name));
    }

    backend
        .rename(source, &target)
        .map_err(|err| format!("Failed to rename: {}", describe_backend_error(&err)))?;
    Ok(target)
}

/// Applies the result of a completed rename to `state` (undo stack if local, reload, selection, status).
pub fn apply_rename_result(state: &mut AppState, source: &Location, target: &Location) {
    if let (Location::Local(from), Location::Local(to)) = (source, target) {
        state.undo_stack.push(UndoOperation::Rename {
            from: from.clone(),
            to: to.clone(),
        });
    }

    if source.parent().as_ref() == Some(&state.current_location) {
        state.reload_entries();
    }

    state.select_location(target.clone());

    let old_name = location_display_name(source);
    let new_name = location_display_name(target);
    state.set_status(format!("Renamed '{}' to '{}'", old_name, new_name));
}

/// Renames a local or remote entry via its [`Backend`], updating `state`.
pub fn execute_rename_location(
    state: &mut AppState,
    source: &Location,
    new_name: &str,
) -> Result<Location, String> {
    let backend = state
        .backend_for(source)
        .ok_or_else(|| "Server is not connected".to_string())?;
    let target = rename_location_on_backend(&*backend, source, new_name)?;
    apply_rename_result(state, source, &target);
    Ok(target)
}

/// Renames a local directory entry, updates the active app state, selection, and undo stack.
pub fn execute_rename(
    state: &mut AppState,
    source: &Path,
    new_name: &str,
) -> Result<PathBuf, String> {
    let target = execute_rename_location(state, &Location::Local(source.to_path_buf()), new_name)?;
    Ok(target.path().to_path_buf())
}

/// Creates a new folder named `name` inside `parent` via `backend`, refusing existing targets.
pub fn create_folder_on_backend(
    backend: &dyn Backend,
    parent: &Location,
    name: &str,
) -> Result<Location, String> {
    let valid_name = validate_entry_name(name)?;
    let target = parent.join(OsStr::new(valid_name));

    if backend_exists(backend, &target)
        .map_err(|err| format!("Failed to create folder: {}", describe_backend_error(&err)))?
    {
        return Err(format!("Target already exists: {}", valid_name));
    }

    backend
        .create_dir(&target)
        .map_err(|err| format!("Failed to create folder: {}", describe_backend_error(&err)))?;
    Ok(target)
}

/// Applies the result of a completed folder creation to `state`.
pub fn apply_create_folder_result(state: &mut AppState, parent: &Location, target: &Location) {
    if let Location::Local(path) = target {
        state
            .undo_stack
            .push(UndoOperation::CreateDir { path: path.clone() });
    }

    if parent == &state.current_location {
        state.reload_entries();
    }

    state.select_location(target.clone());
    state.set_status(format!(
        "Created folder '{}'",
        location_display_name(target)
    ));
}

/// Creates a new local or remote directory via its [`Backend`], updating `state`.
pub fn execute_create_folder_location(
    state: &mut AppState,
    parent: &Location,
    name: &str,
) -> Result<Location, String> {
    let backend = state
        .backend_for(parent)
        .ok_or_else(|| "Server is not connected".to_string())?;
    let target = create_folder_on_backend(&*backend, parent, name)?;
    apply_create_folder_result(state, parent, &target);
    Ok(target)
}

/// Creates a new local directory, updates the active app state, selection, and undo stack.
pub fn execute_create_folder(
    state: &mut AppState,
    parent: &Path,
    name: &str,
) -> Result<PathBuf, String> {
    let target =
        execute_create_folder_location(state, &Location::Local(parent.to_path_buf()), name)?;
    Ok(target.path().to_path_buf())
}

/// Result of attempting to trash locations off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashReport {
    pub trashed: Vec<Location>,
    pub unavailable: Vec<Location>,
    pub error: Option<String>,
}

/// Outcome returned after applying a [`TrashReport`] to [`AppState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashOutcome {
    Trashed(usize),
    /// One or more locations do not have a trash (`TrashUnavailable`); caller may prompt for
    /// permanent deletion.
    Unavailable(Vec<Location>),
    /// A generic trash error occurred; permanent deletion must NOT be offered.
    Failed(String),
}

/// Moves locations to the system trash via their [`Backend`]s without touching [`AppState`].
pub fn trash_locations_on_backends(items: &[(Location, Arc<dyn Backend>)]) -> TrashReport {
    let mut report = TrashReport {
        trashed: Vec::new(),
        unavailable: Vec::new(),
        error: None,
    };

    for (loc, backend) in items {
        match backend.trash(loc) {
            Ok(()) => report.trashed.push(loc.clone()),
            Err(err) if err.is_trash_unavailable() => report.unavailable.push(loc.clone()),
            Err(err) => {
                report.error = Some(describe_backend_error(&err));
                break;
            }
        }
    }

    report
}

/// Applies a [`TrashReport`] to `state`, updating selection, listing, undo stack, and status.
pub fn apply_trash_report(state: &mut AppState, report: TrashReport) -> TrashOutcome {
    if !report.trashed.is_empty() {
        let local_paths: Vec<PathBuf> = report
            .trashed
            .iter()
            .filter_map(|loc| match loc {
                Location::Local(p) => Some(p.clone()),
                Location::Remote { .. } => None,
            })
            .collect();
        if !local_paths.is_empty() {
            state.undo_stack.push(UndoOperation::Trash {
                original_paths: local_paths,
            });
        }
        for loc in &report.trashed {
            state.selection.deselect(loc);
        }
        state.reload_entries();
    }

    if let Some(err) = report.error {
        let msg = format!("Failed to move to trash: {}", err);
        state.set_status(msg.clone());
        return TrashOutcome::Failed(msg);
    }

    if !report.unavailable.is_empty() {
        return TrashOutcome::Unavailable(report.unavailable);
    }

    state.set_status(format!("Moved {} item(s) to trash", report.trashed.len()));
    TrashOutcome::Trashed(report.trashed.len())
}

/// Moves `locations` to the system trash via `state.backend_for`, reporting unavailable vs generic
/// errors cleanly. Never offers permanent deletion on generic errors.
pub fn execute_trash_locations(state: &mut AppState, locations: &[Location]) -> TrashOutcome {
    if locations.is_empty() {
        return TrashOutcome::Trashed(0);
    }
    let mut items = Vec::with_capacity(locations.len());
    for loc in locations {
        let Some(backend) = state.backend_for(loc) else {
            let msg = "Failed to move to trash: server is not connected".to_string();
            state.set_status(msg.clone());
            return TrashOutcome::Failed(msg);
        };
        items.push((loc.clone(), backend));
    }
    let report = trash_locations_on_backends(&items);
    apply_trash_report(state, report)
}

/// Moves local entries to the system trash, updates state listing, and records into undo stack.
/// Refuses to offer permanent delete on failure.
pub fn execute_trash(state: &mut AppState, paths: &[PathBuf]) -> Result<usize, String> {
    let locs: Vec<Location> = paths.iter().cloned().map(Location::Local).collect();
    match execute_trash_locations(state, &locs) {
        TrashOutcome::Trashed(n) => Ok(n),
        TrashOutcome::Unavailable(unavailable) => {
            let msg = format!(
                "Failed to move to trash: no trash available for {}",
                unavailable
                    .first()
                    .map(location_display_name)
                    .unwrap_or_default()
            );
            state.set_status(msg.clone());
            Err(msg)
        }
        TrashOutcome::Failed(msg) => Err(msg),
    }
}

/// Result of permanently deleting locations off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteReport {
    pub deleted: Vec<Location>,
    pub error: Option<String>,
}

/// Permanently deletes each `(location, backend)` pair using explicit [`Confirmed`] proof.
pub fn delete_locations_permanently_with_backends(
    items: &[(Location, Arc<dyn Backend>)],
    confirmed: Confirmed,
) -> DeleteReport {
    let mut report = DeleteReport {
        deleted: Vec::new(),
        error: None,
    };

    for (loc, backend) in items {
        match backend.delete_permanently(loc, confirmed) {
            Ok(()) => report.deleted.push(loc.clone()),
            Err(err) => {
                report.error = Some(describe_backend_error(&err));
                break;
            }
        }
    }

    report
}

/// Applies a [`DeleteReport`] to `state`, updating selection, listing, and status.
pub fn apply_delete_report(state: &mut AppState, report: DeleteReport) -> Result<usize, String> {
    if !report.deleted.is_empty() {
        for loc in &report.deleted {
            state.selection.deselect(loc);
        }
        state.reload_entries();
    }

    if let Some(err) = report.error {
        let msg = format!("Failed to delete permanently: {}", err);
        state.set_status(msg.clone());
        Err(msg)
    } else {
        state.set_status(format!(
            "Permanently deleted {} item(s)",
            report.deleted.len()
        ));
        Ok(report.deleted.len())
    }
}

/// Permanently deletes local or remote locations using explicit [`Confirmed`] proof from the UI.
pub fn execute_permanent_delete_locations(
    state: &mut AppState,
    locations: &[Location],
    confirmed: Confirmed,
) -> Result<usize, String> {
    if locations.is_empty() {
        return Ok(0);
    }
    let mut items = Vec::with_capacity(locations.len());
    for loc in locations {
        let Some(backend) = state.backend_for(loc) else {
            let msg = "Failed to delete permanently: server is not connected".to_string();
            state.set_status(msg.clone());
            return Err(msg);
        };
        items.push((loc.clone(), backend));
    }
    let report = delete_locations_permanently_with_backends(&items, confirmed);
    apply_delete_report(state, report)
}

/// Legacy test-compatibility shim for deleting local paths permanently.
/// Interactive callers must go through [`crate::views::DialogsView`] so [`Confirmed::user_confirmed`]
/// is only constructed by the confirmation button.
pub fn execute_permanent_delete(state: &mut AppState, paths: &[PathBuf]) -> Result<usize, String> {
    let locs: Vec<Location> = paths.iter().cloned().map(Location::Local).collect();
    execute_permanent_delete_locations(state, &locs, Confirmed::user_confirmed())
}

/// Undoes the last reversible local file operation from the undo stack.
/// Supports Rename, CreateDir, Copy, Move, and handles refusal when preconditions fail.
pub fn execute_undo(state: &mut AppState) -> Result<String, String> {
    let Some(op) = state.undo_stack.pop() else {
        let msg = "Nothing to undo".to_string();
        state.set_status(&msg);
        return Err(msg);
    };

    let local = LocalBackend::new();

    match op {
        UndoOperation::Rename { from, to } => {
            let from_name = from
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let from_loc = Location::Local(from.clone());
            let to_loc = Location::Local(to.clone());

            if !backend_exists(&local, &to_loc).unwrap_or(false) {
                let refusal = format!(
                    "Cannot undo rename: '{}' no longer exists",
                    to.file_name().unwrap_or_default().to_string_lossy()
                );
                state.set_status(&refusal);
                return Err(refusal);
            }

            if backend_exists(&local, &from_loc).unwrap_or(false) {
                let refusal = format!(
                    "Cannot undo rename: original name '{}' already exists",
                    from_name
                );
                state.set_status(&refusal);
                return Err(refusal);
            }

            if let Err(err) = local.rename(&to_loc, &from_loc) {
                let refusal = format!("Failed to undo rename: {}", describe_backend_error(&err));
                state.set_status(&refusal);
                return Err(refusal);
            }

            state.reload_entries();
            state.select_location(from_loc);

            let msg = format!("Undid rename of '{}'", from_name);
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::CreateDir { path } => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let loc = Location::Local(path.clone());

            if !backend_exists(&local, &loc).unwrap_or(false) {
                let refusal = format!("Cannot undo folder creation: '{}' no longer exists", name);
                state.set_status(&refusal);
                return Err(refusal);
            }

            // Use remove_dir so undo safely refuses if the user added files inside the folder.
            if let Err(err) = std::fs::remove_dir(&path) {
                let refusal = format!("Cannot undo folder creation: {}", err);
                state.set_status(&refusal);
                return Err(refusal);
            }

            state.selection.deselect(&loc);
            state.reload_entries();

            let msg = format!("Undid create folder '{}'", name);
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::Copy { destinations } => {
            for dest in &destinations {
                let loc = Location::Local(dest.clone());
                if backend_exists(&local, &loc).unwrap_or(false) {
                    // Removing copy destinations created by the reversible copy operation.
                    let _ = local.delete_permanently(&loc, Confirmed::user_confirmed());
                    state.selection.deselect(&loc);
                }
            }

            state.reload_entries();

            let msg = "Undid copy operation".to_string();
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::Move { from, to } => {
            for (f, t) in from.iter().zip(to.iter()) {
                let f_loc = Location::Local(f.clone());
                let t_loc = Location::Local(t.clone());
                if !backend_exists(&local, &t_loc).unwrap_or(false) {
                    let refusal = format!("Cannot undo move: '{}' no longer exists", t.display());
                    state.set_status(&refusal);
                    return Err(refusal);
                }
                if backend_exists(&local, &f_loc).unwrap_or(false) {
                    let refusal = format!(
                        "Cannot undo move: destination '{}' already exists",
                        f.display()
                    );
                    state.set_status(&refusal);
                    return Err(refusal);
                }
            }

            for (f, t) in from.iter().zip(to.iter()) {
                let f_loc = Location::Local(f.clone());
                let t_loc = Location::Local(t.clone());
                if f.parent() == t.parent() {
                    let _ = local.rename(&t_loc, &f_loc);
                } else if let Some(parent) = f.parent() {
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    let options = CopyOptions::new(ConflictPolicy::Replace, cancel_flag);
                    let mut dummy = |_: Progress| {};
                    let _ = move_entry(t, parent, &options, &mut dummy);
                }
            }

            state.reload_entries();

            if let Some(first) = from.first() {
                state.select_location(Location::Local(first.clone()));
            }

            let msg = "Undid move operation".to_string();
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::Trash { original_paths: _ } => {
            let refusal =
                "Cannot undo trash: system trash does not support automatic restore".to_string();
            state.set_status(&refusal);
            Err(refusal)
        }
    }
}
