use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// What to do when a target already exists. Applied per entry, so inside a merged folder each
/// file is decided on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Leave the existing target untouched; its bytes still count as done.
    Skip,
    /// Replace the target file once its new copy is complete, so cancelling keeps the original.
    /// Files already replaced before a cancel stay replaced. Replacing a folder with a file (or
    /// the reverse) deletes the old target up front.
    Replace,
    /// Copy to `name (N).ext` (`name (N)` without an extension), with the smallest free N >= 1.
    KeepBoth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    Completed,
    /// The cancel flag was set; everything this job newly created has been removed.
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub policy: ConflictPolicy,
    pub chunk_size: usize,
    pub cancel: Arc<AtomicBool>,
}

impl CopyOptions {
    pub fn new(policy: ConflictPolicy, cancel: Arc<AtomicBool>) -> Self {
        Self {
            policy,
            chunk_size: DEFAULT_CHUNK_SIZE,
            cancel,
        }
    }
}

/// Copies `src` (file, folder or symlink) into `dest_dir` under its own name.
///
/// Files are written to a hidden `.name.kwati-partial` sibling and renamed into place when
/// complete. Folders are walked without following symlinks. On Unix a symlink is recreated as a
/// symlink; on other platforms symlinks are skipped. `on_progress` is called once with 0 bytes
/// done, then after every chunk and every skipped entry, ending at exactly `bytes_total`.
/// Permissions of regular files are preserved.
pub fn copy_entry(
    src: &Path,
    dest_dir: &Path,
    options: &CopyOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<CopyOutcome> {
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no file name"))?;
    let mut job = Job {
        options,
        on_progress,
        done: 0,
        total: size_of(src)?,
        created: Vec::new(),
    };
    job.report();
    match job.entry(src, &dest_dir.join(name)) {
        Ok(true) => Ok(CopyOutcome::Completed),
        Ok(false) => {
            job.rollback();
            Ok(CopyOutcome::Cancelled)
        }
        Err(err) => {
            job.rollback();
            Err(err)
        }
    }
}

fn size_of(path: &Path) -> io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        let mut sum = 0;
        for child in fs::read_dir(path)? {
            sum += size_of(&child?.path())?;
        }
        Ok(sum)
    } else if meta.is_file() {
        Ok(meta.len())
    } else {
        Ok(0)
    }
}

fn unique_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy());
    (1..)
        .map(|n| match &ext {
            Some(ext) => path.with_file_name(format!("{stem} ({n}).{ext}")),
            None => path.with_file_name(format!("{stem} ({n})")),
        })
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .expect("unbounded range")
}

fn partial_path(dst: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(dst.file_name().unwrap_or_default());
    name.push(".kwati-partial");
    dst.with_file_name(name)
}

struct Job<'a> {
    options: &'a CopyOptions,
    on_progress: &'a mut dyn FnMut(Progress),
    done: u64,
    total: u64,
    created: Vec<PathBuf>,
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
        for path in self.created.iter().rev() {
            let _ = match fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => fs::remove_dir(path),
                _ => fs::remove_file(path),
            };
        }
    }

    /// Returns `Ok(false)` when cancelled.
    fn entry(&mut self, src: &Path, dst: &Path) -> io::Result<bool> {
        if self.cancelled() {
            return Ok(false);
        }
        let meta = fs::symlink_metadata(src)?;
        let Some(dst) = self.resolve_conflict(&meta, dst)? else {
            self.done += size_of(src)?;
            self.report();
            return Ok(true);
        };

        if meta.is_dir() {
            if fs::symlink_metadata(&dst).is_err() {
                fs::create_dir(&dst)?;
                self.created.push(dst.clone());
            }
            for child in fs::read_dir(src)? {
                let child = child?;
                if !self.entry(&child.path(), &dst.join(child.file_name()))? {
                    return Ok(false);
                }
            }
            Ok(true)
        } else if meta.is_file() {
            self.file(src, &dst, &meta)
        } else {
            self.symlink(src, &dst)?;
            Ok(true)
        }
    }

    /// `None` means skip. Folders merge into an existing folder unless the policy is KeepBoth.
    fn resolve_conflict(&self, src: &fs::Metadata, dst: &Path) -> io::Result<Option<PathBuf>> {
        let existing = match fs::symlink_metadata(dst) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Some(dst.to_path_buf()))
            }
            Err(err) => return Err(err),
        };
        let policy = self.options.policy;
        if src.is_dir() && existing.is_dir() && policy != ConflictPolicy::KeepBoth {
            return Ok(Some(dst.to_path_buf()));
        }
        match policy {
            ConflictPolicy::Skip => Ok(None),
            ConflictPolicy::Replace => {
                // A finished file is renamed over the old one, which keeps the original on cancel.
                if src.is_file() && !existing.is_dir() {
                    return Ok(Some(dst.to_path_buf()));
                }
                if existing.is_dir() {
                    fs::remove_dir_all(dst)?;
                } else {
                    fs::remove_file(dst)?;
                }
                Ok(Some(dst.to_path_buf()))
            }
            ConflictPolicy::KeepBoth => Ok(Some(unique_name(dst))),
        }
    }

    fn file(&mut self, src: &Path, dst: &Path, meta: &fs::Metadata) -> io::Result<bool> {
        let existed = fs::symlink_metadata(dst).is_ok();
        let partial = partial_path(dst);
        let mut reader = fs::File::open(src)?;
        let mut writer = fs::File::create(&partial)?;
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
        drop(writer);
        fs::set_permissions(&partial, meta.permissions())?;
        fs::rename(&partial, dst)?;
        self.created.pop();
        if !existed {
            self.created.push(dst.to_path_buf());
        }
        Ok(true)
    }

    #[cfg(unix)]
    fn symlink(&mut self, src: &Path, dst: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        self.created.push(dst.to_path_buf());
        Ok(())
    }

    // Not verified: symlink creation on Windows may need elevated rights, so they are skipped.
    #[cfg(not(unix))]
    fn symlink(&mut self, _src: &Path, _dst: &Path) -> io::Result<()> {
        Ok(())
    }
}
