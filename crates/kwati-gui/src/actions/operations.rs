//! File operations and undo actions (owned by guiops-worker for #17, #19, #21, #23, #35, #65).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::actions;
use kwati_core::{
    copy_entry, create_dir, list_dir, rename_entry, trash, ConflictPolicy, CopyOptions,
    CopyOutcome, Location,
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

/// Copies files or folders into `dest_dir` with progress reporting, cancellation,
/// and conflict resolution policy. Updates AppState jobs, undo stack, and selection.
pub fn execute_copy(
    state: &mut AppState,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
    cancel_flag: Arc<AtomicBool>,
) -> Result<CopyOutcome, String> {
    if sources.is_empty() {
        return Ok(CopyOutcome::Completed);
    }

    let job_id = NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst);
    register_job_cancel(job_id, cancel_flag.clone());

    let title = if sources.len() == 1 {
        format!(
            "Copying '{}'",
            sources[0].file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        format!("Copying {} items", sources.len())
    };

    // Calculate total size
    let mut total_bytes = 0u64;
    for src in sources {
        if let Ok(meta) = std::fs::symlink_metadata(src) {
            total_bytes += meta.len();
        }
    }

    let mut job_status = JobStatus::new(job_id, title, JobKind::Copy, total_bytes);
    state.add_job(job_status.clone());

    let mut destinations = Vec::new();
    let mut outcome = CopyOutcome::Completed;

    for src in sources {
        if cancel_flag.load(Ordering::Relaxed) {
            outcome = CopyOutcome::Cancelled;
            break;
        }

        let options = CopyOptions::new(policy, cancel_flag.clone());
        let mut progress_fn = |p: kwati_core::Progress| {
            job_status.update_progress(p.bytes_done);
            if let Some(j) = state.active_jobs.iter_mut().find(|j| j.id == job_id) {
                j.bytes_processed = p.bytes_done;
                j.progress_pct = job_status.progress_pct;
            }
        };

        match copy_entry(src, dest_dir, &options, &mut progress_fn) {
            Ok(CopyOutcome::Completed) => {
                if let Some(name) = src.file_name() {
                    destinations.push(dest_dir.join(name));
                }
            }
            Ok(CopyOutcome::Cancelled) => {
                outcome = CopyOutcome::Cancelled;
                break;
            }
            Err(err) => {
                state.remove_job(job_id);
                unregister_job_cancel(job_id);
                let msg = format!("Copy failed: {}", err);
                state.set_status(msg.clone());
                return Err(msg);
            }
        }
    }

    unregister_job_cancel(job_id);
    state.remove_job(job_id);

    if outcome == CopyOutcome::Completed {
        if !destinations.is_empty() {
            state.undo_stack.push(UndoOperation::Copy {
                destinations: destinations.clone(),
            });

            if state.current_location.path() == dest_dir {
                if let Ok(listing) = list_dir(dest_dir) {
                    state.set_entries(listing.into_entries());
                }
                if let Some(first) = destinations.first() {
                    state.selection.select_only(Location::Local(first.clone()));
                }
            }
        }
        state.set_status(format!("Copied {} item(s)", destinations.len()));
    } else {
        state.set_status("Copy operation cancelled");
    }

    Ok(outcome)
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
        if let Ok(listing) = list_dir(parent) {
            state.set_entries(listing.into_entries());
        }
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
        if let Ok(listing) = list_dir(parent) {
            state.set_entries(listing.into_entries());
        }
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
        if let Location::Local(current_dir) = &state.current_location {
            if let Ok(listing) = list_dir(current_dir) {
                state.set_entries(listing.into_entries());
            }
        }
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
