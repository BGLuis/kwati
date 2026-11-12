//! File operations and undo actions (owned by guiops-worker for #17, #19, #21, #23, #35, #65).

use std::path::{Path, PathBuf};

use gpui::actions;
use kwati_core::{create_dir, list_dir, rename_entry, trash, Location};

use crate::state::{AppState, UndoOperation};

actions!(
    operations,
    [Copy, Cut, Paste, Delete, Trash, Rename, NewFolder, Undo,]
);

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
