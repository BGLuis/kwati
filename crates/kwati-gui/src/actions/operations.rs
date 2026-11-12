//! File operations and undo actions (owned by guiops-worker for #17, #19, #21, #23, #35, #65).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::actions;
use kwati_core::{
    copy_entry, create_dir, move_entry, move_entry_with, rename_entry, trash, ConflictPolicy,
    CopyOptions, CopyOutcome, Location,
};

use crate::state::{AppState, JobKind, JobStatus, UndoOperation};

actions!(
    operations,
    [Copy, Cut, Paste, Delete, Trash, Rename, NewFolder, Undo,]
);

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_JOB_CANCELS: Mutex<Option<HashMap<u64, Arc<AtomicBool>>>> = Mutex::new(None);
static CLIPBOARD: Mutex<Option<Clipboard>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipboard {
    pub sources: Vec<PathBuf>,
    pub is_cut: bool,
}

pub fn copy_to_clipboard(sources: Vec<PathBuf>) {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = Some(Clipboard {
        sources,
        is_cut: false,
    });
}

pub fn cut_to_clipboard(sources: Vec<PathBuf>) {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = Some(Clipboard {
        sources,
        is_cut: true,
    });
}

pub fn get_clipboard() -> Option<Clipboard> {
    CLIPBOARD.lock().unwrap().clone()
}

pub fn clear_clipboard() {
    let mut cb = CLIPBOARD.lock().unwrap();
    *cb = None;
}

pub fn register_job_cancel(job_id: u64, cancel: Arc<AtomicBool>) {
    let mut map = ACTIVE_JOB_CANCELS.lock().unwrap();
    if map.is_none() {
        *map = Some(HashMap::new());
    }
    if let Some(m) = map.as_mut() {
        m.insert(job_id, cancel);
    }
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

/// Checks if pasting the given sources into dest_dir would conflict with existing entries.
/// Returns the name of the first conflicting entry, if any.
pub fn check_paste_conflict(sources: &[PathBuf], dest_dir: &Path) -> Option<String> {
    for src in sources {
        if let Some(name) = src.file_name() {
            let target = dest_dir.join(name);
            if target.exists() || target.symlink_metadata().is_ok() {
                return Some(name.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// A copy or a move of `sources` into `dest_dir`.
#[derive(Debug, Clone)]
pub struct Transfer {
    pub kind: JobKind,
    pub sources: Vec<PathBuf>,
    pub dest_dir: PathBuf,
    pub policy: ConflictPolicy,
    pub cancel: Arc<AtomicBool>,
    simulate_cross_device: bool,
}

impl Transfer {
    pub fn copy(sources: Vec<PathBuf>, dest_dir: PathBuf, policy: ConflictPolicy) -> Self {
        Self {
            kind: JobKind::Copy,
            sources,
            dest_dir,
            policy,
            cancel: Arc::new(AtomicBool::new(false)),
            simulate_cross_device: false,
        }
    }

    pub fn moving(sources: Vec<PathBuf>, dest_dir: PathBuf, policy: ConflictPolicy) -> Self {
        Self {
            kind: JobKind::Move,
            ..Self::copy(sources, dest_dir, policy)
        }
    }

    fn is_move(&self) -> bool {
        self.kind == JobKind::Move
    }
}

/// What a transfer did on disk, gathered without touching [`AppState`] so it can be produced
/// off the main thread.
struct TransferReport {
    outcome: CopyOutcome,
    transferred: usize,
    // Only entries whose target did not exist before: undoing anything else would delete or
    // move a file the user already had under that name.
    undo_from: Vec<PathBuf>,
    undo_to: Vec<PathBuf>,
    error: Option<String>,
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
        [single] => format!(
            "{verb} '{}'",
            single.file_name().unwrap_or_default().to_string_lossy()
        ),
        sources => format!("{verb} {} items", sources.len()),
    };
    let total_bytes = transfer
        .sources
        .iter()
        .filter_map(|src| std::fs::symlink_metadata(src).ok())
        .map(|meta| meta.len())
        .sum();

    state.add_job(JobStatus::new(
        job_id,
        title,
        transfer.kind.clone(),
        total_bytes,
    ));
    job_id
}

fn run_transfer(transfer: &Transfer, bytes_done: &AtomicU64) -> TransferReport {
    let mut report = TransferReport {
        outcome: CopyOutcome::Completed,
        transferred: 0,
        undo_from: Vec::new(),
        undo_to: Vec::new(),
        error: None,
    };
    let mut finished_bytes = 0u64;

    for src in &transfer.sources {
        if transfer.cancel.load(Ordering::Relaxed) {
            report.outcome = CopyOutcome::Cancelled;
            break;
        }

        let target = src.file_name().map(|name| transfer.dest_dir.join(name));
        let target_existed = target
            .as_ref()
            .is_some_and(|target| target.symlink_metadata().is_ok());
        let options = CopyOptions::new(transfer.policy, transfer.cancel.clone());
        let mut on_progress = |p: kwati_core::Progress| {
            bytes_done.store(finished_bytes + p.bytes_done, Ordering::Relaxed);
        };

        let result = if !transfer.is_move() {
            copy_entry(src, &transfer.dest_dir, &options, &mut on_progress)
        } else if transfer.simulate_cross_device {
            move_entry_with(
                src,
                &transfer.dest_dir,
                &options,
                &mut on_progress,
                &|_, _| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::CrossesDevices,
                        "cross device",
                    ))
                },
            )
        } else {
            move_entry(src, &transfer.dest_dir, &options, &mut on_progress)
        };

        match result {
            Ok(CopyOutcome::Completed) => {
                finished_bytes = bytes_done.load(Ordering::Relaxed);
                if let Some(target) = target {
                    report.transferred += 1;
                    if !target_existed {
                        report.undo_from.push(src.clone());
                        report.undo_to.push(target);
                    }
                }
            }
            Ok(CopyOutcome::Cancelled) => {
                report.outcome = CopyOutcome::Cancelled;
                break;
            }
            Err(err) => {
                report.error = Some(err.to_string());
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
        let viewing_destination = state.current_location.path() == transfer.dest_dir;

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
                state.selection.select_only(Location::Local(first));
            }
        }
        if transfer.is_move() {
            clear_clipboard();
        }
    }
    state.set_status(format!("{past} {} item(s)", report.transferred));
    Ok(report.outcome)
}

/// Starts `transfer` as a job in the jobs panel. With an attached [`AppState`] the files are
/// copied on the background executor and the panel is refreshed while they are.
pub fn start_transfer(state: &mut AppState, transfer: Transfer) {
    if transfer.sources.is_empty() {
        return;
    }
    let job_id = begin_transfer(state, &transfer);
    let bytes_done = Arc::new(AtomicU64::new(0));

    let work = {
        let transfer = transfer.clone();
        let bytes_done = bytes_done.clone();
        move || run_transfer(&transfer, &bytes_done)
    };
    let tick = move |state: &mut AppState| {
        if let Some(job) = state.active_jobs.iter_mut().find(|j| j.id == job_id) {
            job.update_progress(bytes_done.load(Ordering::Relaxed));
        }
    };
    let done = move |state: &mut AppState, report| {
        let _ = finish_transfer(state, job_id, &transfer, report);
    };
    state.spawn_job(work, tick, done);
}

fn execute_transfer(state: &mut AppState, transfer: Transfer) -> Result<CopyOutcome, String> {
    if transfer.sources.is_empty() {
        return Ok(CopyOutcome::Completed);
    }
    let job_id = begin_transfer(state, &transfer);
    let report = run_transfer(&transfer, &AtomicU64::new(0));
    finish_transfer(state, job_id, &transfer, report)
}

/// Copies files or folders into `dest_dir` on the calling thread and waits for the result.
/// The interface uses [`start_transfer`] instead, which does not block.
pub fn execute_copy(
    state: &mut AppState,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
) -> Result<CopyOutcome, String> {
    let mut transfer = Transfer::copy(sources.to_vec(), dest_dir.to_path_buf(), policy);
    transfer.cancel = cancel_flag;
    execute_transfer(state, transfer)
}

/// Moves files or folders into `dest_dir` on the calling thread and waits for the result.
/// The interface uses [`start_transfer`] instead, which does not block.
pub fn execute_move(
    state: &mut AppState,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
    simulate_cross_device: bool,
) -> Result<CopyOutcome, String> {
    let mut transfer = Transfer::moving(sources.to_vec(), dest_dir.to_path_buf(), policy);
    transfer.cancel = cancel_flag;
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

/// Renames a directory entry, updates the active app state, selection, and undo stack.
pub fn execute_rename(
    state: &mut AppState,
    source: &Path,
    new_name: &str,
) -> Result<PathBuf, String> {
    let valid_name = validate_entry_name(new_name)?;
    let parent = source
        .parent()
        .ok_or_else(|| "Cannot rename root directory".to_string())?;
    let target = parent.join(valid_name);

    if target == source {
        return Ok(source.to_path_buf());
    }

    if target.exists() || target.symlink_metadata().is_ok() {
        return Err(format!("Target already exists: {}", valid_name));
    }

    rename_entry(source, valid_name).map_err(|err| format!("Failed to rename: {}", err))?;

    // Record in undo stack for #65
    state.undo_stack.push(UndoOperation::Rename {
        from: source.to_path_buf(),
        to: target.clone(),
    });

    // Refresh listing if currently browsing the parent
    if state.current_location.path() == parent {
        state.reload_entries();
    }

    // Select the renamed entry
    state.selection.select_only(Location::Local(target.clone()));

    let old_name = source
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    state.set_status(format!("Renamed '{}' to '{}'", old_name, valid_name));

    Ok(target)
}

/// Creates a new directory, updates the active app state, selection, and undo stack.
pub fn execute_create_folder(
    state: &mut AppState,
    parent: &Path,
    name: &str,
) -> Result<PathBuf, String> {
    let valid_name = validate_entry_name(name)?;
    let target = parent.join(valid_name);

    if target.exists() || target.symlink_metadata().is_ok() {
        return Err(format!("Target already exists: {}", valid_name));
    }

    create_dir(&target).map_err(|err| format!("Failed to create folder: {}", err))?;

    // Record in undo stack for #65
    state.undo_stack.push(UndoOperation::CreateDir {
        path: target.clone(),
    });

    // Refresh listing if currently browsing the parent
    if state.current_location.path() == parent {
        state.reload_entries();
    }

    // Select the newly created folder
    state.selection.select_only(Location::Local(target.clone()));

    state.set_status(format!("Created folder '{}'", valid_name));

    Ok(target)
}

/// Moves entries to the system trash, updates state listing, and records into undo stack.
/// Refuses to offer permanent delete on failure.
pub fn execute_trash(state: &mut AppState, paths: &[PathBuf]) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }

    let mut trashed = Vec::new();
    let mut last_err = None;

    for path in paths {
        match trash(path) {
            Ok(()) => trashed.push(path.clone()),
            Err(err) => {
                last_err = Some(format!("{}", err));
                break;
            }
        }
    }

    if !trashed.is_empty() {
        // Record trashed paths in undo stack
        state.undo_stack.push(UndoOperation::Trash {
            original_paths: trashed.clone(),
        });

        // Deselect trashed locations
        for p in &trashed {
            state.selection.deselect(&Location::Local(p.clone()));
        }

        // Refresh listing
        state.reload_entries();
    }

    if let Some(err) = last_err {
        let msg = format!("Failed to move to trash: {}", err);
        state.set_status(msg.clone());
        Err(msg)
    } else {
        state.set_status(format!("Moved {} item(s) to trash", trashed.len()));
        Ok(trashed.len())
    }
}

/// Permanently deletes files or folders from disk. Irreversible; cannot be undone.
/// Updates state listing and removes deleted items from selection.
pub fn execute_permanent_delete(state: &mut AppState, paths: &[PathBuf]) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }

    let mut deleted = Vec::new();
    let mut last_err = None;

    for path in paths {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                last_err = Some(format!("{}", e));
                break;
            }
        };

        let res = if meta.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match res {
            Ok(()) => deleted.push(path.clone()),
            Err(err) => {
                last_err = Some(format!("{}", err));
                break;
            }
        }
    }

    if !deleted.is_empty() {
        for p in &deleted {
            state.selection.deselect(&Location::Local(p.clone()));
        }
        state.reload_entries();
    }

    if let Some(err) = last_err {
        let msg = format!("Failed to delete permanently: {}", err);
        state.set_status(msg.clone());
        Err(msg)
    } else {
        state.set_status(format!("Permanently deleted {} item(s)", deleted.len()));
        Ok(deleted.len())
    }
}

/// Undoes the last reversible file operation from the undo stack.
/// Supports Rename, CreateDir, Copy, Move, and handles refusal when preconditions fail.
pub fn execute_undo(state: &mut AppState) -> Result<String, String> {
    let Some(op) = state.undo_stack.pop() else {
        let msg = "Nothing to undo".to_string();
        state.set_status(&msg);
        return Err(msg);
    };

    match op {
        UndoOperation::Rename { from, to } => {
            let from_name = from
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if !to.exists() && to.symlink_metadata().is_err() {
                let refusal = format!(
                    "Cannot undo rename: '{}' no longer exists",
                    to.file_name().unwrap_or_default().to_string_lossy()
                );
                state.set_status(&refusal);
                return Err(refusal);
            }

            if from.exists() || from.symlink_metadata().is_ok() {
                let refusal = format!(
                    "Cannot undo rename: original name '{}' already exists",
                    from_name
                );
                state.set_status(&refusal);
                return Err(refusal);
            }

            if let Err(err) = rename_entry(&to, &from_name) {
                let refusal = format!("Failed to undo rename: {}", err);
                state.set_status(&refusal);
                return Err(refusal);
            }

            state.reload_entries();

            state.selection.select_only(Location::Local(from.clone()));

            let msg = format!("Undid rename of '{}'", from_name);
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::CreateDir { path } => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if !path.exists() {
                let refusal = format!("Cannot undo folder creation: '{}' no longer exists", name);
                state.set_status(&refusal);
                return Err(refusal);
            }

            if let Err(err) = std::fs::remove_dir(&path) {
                let refusal = format!("Cannot undo folder creation: {}", err);
                state.set_status(&refusal);
                return Err(refusal);
            }

            state.selection.deselect(&Location::Local(path.clone()));

            state.reload_entries();

            let msg = format!("Undid create folder '{}'", name);
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::Copy { destinations } => {
            for dest in &destinations {
                if let Ok(meta) = std::fs::symlink_metadata(dest) {
                    if meta.is_dir() {
                        let _ = std::fs::remove_dir_all(dest);
                    } else {
                        let _ = std::fs::remove_file(dest);
                    }
                    state.selection.deselect(&Location::Local(dest.clone()));
                }
            }

            state.reload_entries();

            let msg = "Undid copy operation".to_string();
            state.set_status(&msg);
            Ok(msg)
        }
        UndoOperation::Move { from, to } => {
            // Verify all targets exist and origins are free before moving back
            for (f, t) in from.iter().zip(to.iter()) {
                if !t.exists() && t.symlink_metadata().is_err() {
                    let refusal = format!("Cannot undo move: '{}' no longer exists", t.display());
                    state.set_status(&refusal);
                    return Err(refusal);
                }
                if f.exists() || f.symlink_metadata().is_ok() {
                    let refusal = format!(
                        "Cannot undo move: destination '{}' already exists",
                        f.display()
                    );
                    state.set_status(&refusal);
                    return Err(refusal);
                }
            }

            for (f, t) in from.iter().zip(to.iter()) {
                if f.parent() == t.parent() {
                    let _ = std::fs::rename(t, f);
                } else if let Some(parent) = f.parent() {
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    let options = CopyOptions::new(ConflictPolicy::Replace, cancel_flag);
                    let mut dummy = |_: kwati_core::Progress| {};
                    let _ = move_entry(t, parent, &options, &mut dummy);
                }
            }

            state.reload_entries();

            if let Some(first) = from.first() {
                state.selection.select_only(Location::Local(first.clone()));
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
