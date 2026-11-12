use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::sync::atomic::Ordering;

use crate::{
    Backend, BackendError, Confirmed, ConflictPolicy, CopyOptions, CopyOutcome, EntryKind,
    EntryMeta, Location, Progress,
};

/// Copies `from` (file or folder) on `src` into `dest_dir` on `dst` under its own name,
/// streaming through [`Backend::open_read`] and [`Backend::open_write`].
///
/// `options` and `on_progress` behave as in [`crate::copy_entry`]: files go to a hidden
/// `.name.kwati-partial` sibling renamed into place when complete, and a cancel or error removes
/// what the job created. Symlinks are not copied between backends; they count as 0 bytes.
pub fn copy_between(
    src: &dyn Backend,
    from: &Location,
    dst: &dyn Backend,
    dest_dir: &Location,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<CopyOutcome, BackendError> {
    run(src, from, dst, dest_dir, options, on_progress).map(|(outcome, _)| outcome)
}

/// [`copy_between`], then removes the source once the copy completed. Entries skipped by
/// [`ConflictPolicy::Skip`] stay on `src`, and so do the folders that still hold them. A failed
/// or cancelled copy leaves the source intact.
pub fn move_between(
    src: &dyn Backend,
    from: &Location,
    dst: &dyn Backend,
    dest_dir: &Location,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<CopyOutcome, BackendError> {
    let (outcome, skipped) = run(src, from, dst, dest_dir, options, on_progress)?;
    if outcome == CopyOutcome::Completed {
        remove_moved(src, from, &skipped)?;
    }
    Ok(outcome)
}

fn run(
    src: &dyn Backend,
    from: &Location,
    dst: &dyn Backend,
    dest_dir: &Location,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(CopyOutcome, Vec<Location>), BackendError> {
    let name = from.file_name().ok_or_else(|| {
        BackendError::from(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source has no file name",
        ))
    })?;
    let mut job = Job {
        src,
        dst,
        options,
        on_progress,
        done: 0,
        total: size_of(src, from)?,
        created: Vec::new(),
        skipped: Vec::new(),
    };
    job.report();
    match job.entry(from, &dest_dir.join(name)) {
        Ok(true) => Ok((CopyOutcome::Completed, job.skipped)),
        Ok(false) => {
            job.rollback();
            Ok((CopyOutcome::Cancelled, Vec::new()))
        }
        Err(err) => {
            job.rollback();
            Err(err)
        }
    }
}

/// Returns whether `location` was removed entirely.
fn remove_moved(
    src: &dyn Backend,
    location: &Location,
    skipped: &[Location],
) -> Result<bool, BackendError> {
    if skipped.contains(location) {
        return Ok(false);
    }
    let holds_skipped = skipped
        .iter()
        .any(|s| s.path().starts_with(location.path()));
    if !holds_skipped {
        src.delete_permanently(location, Confirmed::user_confirmed())?;
        return Ok(true);
    }
    let mut emptied = true;
    for entry in src.list(location)?.iter() {
        emptied &= remove_moved(src, &location.join(entry.name()), skipped)?;
    }
    if emptied {
        src.delete_permanently(location, Confirmed::user_confirmed())?;
    }
    Ok(emptied)
}

fn is_not_found(err: &BackendError) -> bool {
    matches!(err, BackendError::Io(e) if e.kind() == io::ErrorKind::NotFound)
}

fn stat_if_exists(
    backend: &dyn Backend,
    location: &Location,
) -> Result<Option<EntryMeta>, BackendError> {
    match backend.stat(location) {
        Ok(meta) => Ok(Some(meta)),
        Err(err) if is_not_found(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn size_of(backend: &dyn Backend, location: &Location) -> Result<u64, BackendError> {
    let meta = backend.stat(location)?;
    match meta.kind {
        EntryKind::Dir => {
            let mut sum = 0;
            for entry in backend.list(location)?.iter() {
                sum += size_of(backend, &location.join(entry.name()))?;
            }
            Ok(sum)
        }
        EntryKind::File => Ok(meta.len),
        EntryKind::Symlink => Ok(0),
    }
}

fn with_file_name(location: &Location, name: OsString) -> Location {
    match location {
        Location::Local(path) => Location::Local(path.with_file_name(name)),
        Location::Remote { profile_id, path } => Location::Remote {
            profile_id: profile_id.clone(),
            path: path.with_file_name(name),
        },
    }
}

fn partial_location(dst: &Location) -> Location {
    let mut name = OsString::from(".");
    name.push(dst.file_name().unwrap_or_default());
    name.push(".kwati-partial");
    with_file_name(dst, name)
}

fn unique_location(backend: &dyn Backend, dst: &Location) -> Result<Location, BackendError> {
    let path = dst.path();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy());
    for n in 1.. {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = with_file_name(dst, name.into());
        if stat_if_exists(backend, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("unbounded range")
}

struct Job<'a> {
    src: &'a dyn Backend,
    dst: &'a dyn Backend,
    options: &'a CopyOptions,
    on_progress: &'a mut dyn FnMut(Progress),
    done: u64,
    total: u64,
    created: Vec<Location>,
    skipped: Vec<Location>,
}

impl Job<'_> {
    fn report(&mut self) {
        (self.on_progress)(Progress {
            bytes_done: self.done,
            bytes_total: self.total,
        });
    }

    fn cancelled(&self) -> bool {
        self.options.cancel.load(Ordering::Relaxed)
    }

    fn rollback(&self) {
        // Everything listed was created by this job, so no user confirmation is involved.
        for location in self.created.iter().rev() {
            let _ = self
                .dst
                .delete_permanently(location, Confirmed::user_confirmed());
        }
    }

    /// Returns `Ok(false)` when cancelled.
    fn entry(&mut self, from: &Location, to: &Location) -> Result<bool, BackendError> {
        if self.cancelled() {
            return Ok(false);
        }
        let meta = self.src.stat(from)?;
        if meta.kind == EntryKind::Symlink {
            return Ok(true);
        }
        let Some(to) = self.resolve_conflict(&meta, to)? else {
            self.done += size_of(self.src, from)?;
            self.skipped.push(from.clone());
            self.report();
            return Ok(true);
        };

        if meta.kind == EntryKind::Dir {
            if stat_if_exists(self.dst, &to)?.is_none() {
                self.dst.create_dir(&to)?;
                self.created.push(to.clone());
            }
            for entry in self.src.list(from)?.iter() {
                if !self.entry(&from.join(entry.name()), &to.join(entry.name()))? {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            self.file(from, &to)
        }
    }

    /// `None` means skip. Folders merge into an existing folder unless the policy is KeepBoth.
    fn resolve_conflict(
        &self,
        src: &EntryMeta,
        to: &Location,
    ) -> Result<Option<Location>, BackendError> {
        let Some(existing) = stat_if_exists(self.dst, to)? else {
            return Ok(Some(to.clone()));
        };
        let src_is_dir = src.kind == EntryKind::Dir;
        let existing_is_dir = existing.kind == EntryKind::Dir;
        let policy = self.options.policy;
        if src_is_dir && existing_is_dir && policy != ConflictPolicy::KeepBoth {
            return Ok(Some(to.clone()));
        }
        match policy {
            ConflictPolicy::Skip => Ok(None),
            ConflictPolicy::Replace => {
                // A finished file is renamed over the old one, which keeps the original on cancel.
                if !src_is_dir && !existing_is_dir {
                    return Ok(Some(to.clone()));
                }
                self.dst
                    .delete_permanently(to, Confirmed::user_confirmed())?;
                Ok(Some(to.clone()))
            }
            ConflictPolicy::KeepBoth => unique_location(self.dst, to).map(Some),
        }
    }

    fn file(&mut self, from: &Location, to: &Location) -> Result<bool, BackendError> {
        let existed = stat_if_exists(self.dst, to)?.is_some();
        let partial = partial_location(to);
        let mut reader = self.src.open_read(from)?;
        let mut writer = self.dst.open_write(&partial)?;
        self.created.push(partial.clone());
        let mut buf = vec![0; self.options.chunk_size.max(1)];
        loop {
            if self.cancelled() {
                return Ok(false);
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            self.done += n as u64;
            self.report();
        }
        writer.flush()?;
        drop(writer);
        self.dst.rename(&partial, to)?;
        self.created.pop();
        if !existed {
            self.created.push(to.clone());
        }
        Ok(true)
    }
}
