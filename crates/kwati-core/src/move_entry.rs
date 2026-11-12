use crate::copy::{copy_entry, ConflictPolicy, CopyOptions, CopyOutcome, Progress};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Moves `src` into `dest_dir` under its own name.
///
/// Tries a plain rename first. If the target already exists, or the rename fails with
/// [`io::ErrorKind::CrossesDevices`], falls back to [`copy_entry`] (so `options` and
/// `on_progress` behave as there) and removes `src` only after the copy completed. A failed or
/// cancelled copy leaves `src` intact. Under `Skip`, entries whose target already existed are not
/// copied and therefore stay in `src`; only what was actually copied is removed.
pub fn move_entry(
    src: &Path,
    dest_dir: &Path,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<CopyOutcome> {
    move_entry_with(src, dest_dir, options, on_progress, &|from, to| {
        fs::rename(from, to)
    })
}

/// [`move_entry`] with the rename operation supplied, so callers can simulate a failure.
#[doc(hidden)]
pub fn move_entry_with(
    src: &Path,
    dest_dir: &Path,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
    rename: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<CopyOutcome> {
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no file name"))?;
    let target = dest_dir.join(name);
    if fs::symlink_metadata(&target).is_err() {
        match rename(src, &target) {
            Ok(()) => return Ok(CopyOutcome::Completed),
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {}
            Err(err) => return Err(err),
        }
    }
    let mut skipped = Vec::new();
    if options.policy == ConflictPolicy::Skip {
        collect_skipped(src, &target, &mut skipped)?;
    }
    let outcome = copy_entry(src, dest_dir, options, on_progress)?;
    if outcome == CopyOutcome::Completed {
        remove_moved(src, &skipped)?;
    }
    Ok(outcome)
}

/// Source entries that `copy_entry` skips under `Skip`: those whose target exists, except
/// folders merging into folders, which are decided per child.
fn collect_skipped(src: &Path, dst: &Path, skipped: &mut Vec<PathBuf>) -> io::Result<()> {
    let Ok(existing) = fs::symlink_metadata(dst) else {
        return Ok(());
    };
    if fs::symlink_metadata(src)?.is_dir() && existing.is_dir() {
        for child in fs::read_dir(src)? {
            let child = child?;
            collect_skipped(&child.path(), &dst.join(child.file_name()), skipped)?;
        }
    } else {
        skipped.push(src.to_path_buf());
    }
    Ok(())
}

/// Removes `path` except the skipped entries, then folders left empty, bottom-up.
fn remove_moved(path: &Path, skipped: &[PathBuf]) -> io::Result<()> {
    if skipped.iter().any(|s| s == path) {
        return Ok(());
    }
    if fs::symlink_metadata(path)?.is_dir() {
        for child in fs::read_dir(path)? {
            remove_moved(&child?.path(), skipped)?;
        }
        if fs::read_dir(path)?.next().is_none() {
            fs::remove_dir(path)?;
        }
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
