use crate::copy::{copy_entry, ConflictPolicy, CopyOptions, CopyOutcome, Progress};
use crate::delete::{delete_permanently, Confirmed};
use crate::move_entry::move_entry;
use crate::rename_entry;
use crate::trash::{
    is_trash_browsing_supported, is_trash_unavailable, list_trash, restore_trash_items, trash,
    trash_with, TrashItem,
};
use crate::volume::{volume_kind, VolumeKind};
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Default maximum number of operations retained by [`UndoStack`].
pub const DEFAULT_MAX_UNDO: usize = 100;

/// Maximum regular-file size hashed when capturing a staleness fingerprint.
const MAX_FINGERPRINT_BYTES: u64 = 1024 * 1024;

/// An undo step was refused because preconditions no longer hold or the platform cannot revert it.
///
/// Carried inside an [`io::Error`]; callers can detect it with [`is_undo_refused`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRefused {
    pub reason: String,
}

impl UndoRefused {
    fn error(kind: io::ErrorKind, reason: impl Into<String>) -> io::Error {
        io::Error::new(
            kind,
            Self {
                reason: reason.into(),
            },
        )
    }
}

impl fmt::Display for UndoRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for UndoRefused {}

/// Whether `err` indicates that an undo operation was refused safely without modifying disk state.
pub fn is_undo_refused(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<UndoRefused>())
}

/// Lightweight filesystem fingerprint used to detect external changes before undoing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSnapshot {
    is_dir: bool,
    is_symlink: bool,
    len: u64,
    modified: Option<SystemTime>,
    content_hash: Option<u64>,
}

impl PathSnapshot {
    /// Captures the current metadata (and content hash for small regular files) of `path`.
    pub fn capture(path: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        let is_dir = meta.is_dir();
        let is_symlink = meta.file_type().is_symlink();
        let len = if meta.is_file() { meta.len() } else { 0 };
        let modified = meta.modified().ok();
        let content_hash = if meta.is_file() && len <= MAX_FINGERPRINT_BYTES {
            Some(fnv1a_file(path)?)
        } else {
            None
        };
        Ok(Self {
            is_dir,
            is_symlink,
            len,
            modified,
            content_hash,
        })
    }

    fn matches(&self, path: &Path) -> bool {
        let Ok(current) = Self::capture(path) else {
            return false;
        };
        current == *self
    }
}

fn fnv1a_file(path: &Path) -> io::Result<u64> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buf = [0u8; 8192];
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        for &byte in &buf[..read] {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Ok(hash)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedEntry {
    pub original_path: PathBuf,
    pub current_path: PathBuf,
    snapshot: PathSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedEntry {
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    snapshot: PathSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedTarget {
    pub original_path: PathBuf,
    /// Specific trash item ID if captured at delete time (unused on platforms without `list_trash`).
    pub trash_id: Option<OsString>,
}

/// A reversible file operation recorded by [`UndoStack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoAction {
    Rename {
        from: PathBuf,
        to: PathBuf,
        snapshot: PathSnapshot,
    },
    CreateDir {
        path: PathBuf,
    },
    Copy {
        entries: Vec<CopiedEntry>,
    },
    Move {
        entries: Vec<MovedEntry>,
    },
    Trash {
        targets: Vec<TrashedTarget>,
    },
}

impl UndoAction {
    /// Human-readable label for this operation, suitable for menu items (e.g., `"Undo rename"`).
    pub fn label(&self) -> &'static str {
        match self {
            UndoAction::Rename { .. } => "Undo rename",
            UndoAction::CreateDir { .. } => "Undo new folder",
            UndoAction::Copy { .. } => "Undo copy",
            UndoAction::Move { .. } => "Undo move",
            UndoAction::Trash { .. } => "Undo trash",
        }
    }
}

/// Bounded FIFO-evicting stack of reversible filesystem operations.
///
/// Permanent deletions never enter the stack. Before reverting an operation, [`UndoStack::undo`]
/// verifies every affected path so stale, missing, non-empty, colliding, or platform-unsupported
/// reversions are refused cleanly without touching disk state or popping the action.
#[derive(Debug, Clone)]
pub struct UndoStack {
    entries: VecDeque<UndoAction>,
    max_entries: usize,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_UNDO)
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries.min(DEFAULT_MAX_UNDO)),
            max_entries,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn peek(&self) -> Option<&UndoAction> {
        self.entries.back()
    }

    /// Human-readable label for the top operation (e.g., `"Undo rename"`), or `None` when empty.
    pub fn undo_label(&self) -> Option<&'static str> {
        self.peek().map(UndoAction::label)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn push(&mut self, action: UndoAction) {
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(action);
    }

    /// Renames `from` to `new_name` (name or sibling path) and records the operation on success.
    pub fn rename_and_record(&mut self, from: &Path, new_name: &Path) -> io::Result<PathBuf> {
        let target = resolve_rename_target(from, new_name)?;
        rename_entry(from, new_name)?;
        self.record_rename(from, &target)?;
        Ok(target)
    }

    /// Records an already-completed rename from `from` to `to`.
    pub fn record_rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let snapshot = PathSnapshot::capture(to)?;
        self.push(UndoAction::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            snapshot,
        });
        Ok(())
    }

    /// Creates a new directory at `path` and records the operation on success.
    pub fn create_dir_and_record(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        crate::create_dir(path)?;
        self.record_create_dir(path);
        Ok(())
    }

    /// Records an already-created empty directory at `path`.
    pub fn record_create_dir(&mut self, path: impl AsRef<Path>) {
        self.push(UndoAction::CreateDir {
            path: path.as_ref().to_path_buf(),
        });
    }

    /// Copies each path in `sources` into `dest_dir` and records newly created destinations.
    pub fn copy_and_record(
        &mut self,
        sources: &[PathBuf],
        dest_dir: &Path,
        options: &CopyOptions,
        on_progress: &mut dyn FnMut(Progress),
    ) -> io::Result<CopyOutcome> {
        let mut created = Vec::with_capacity(sources.len());
        for src in sources {
            let name = src.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
            })?;
            let before_names = if options.policy == ConflictPolicy::KeepBoth {
                Some(dir_entry_names(dest_dir)?)
            } else {
                None
            };
            let candidate = dest_dir.join(name);
            let existed_before = candidate.symlink_metadata().is_ok();

            let outcome = copy_entry(src, dest_dir, options, on_progress)?;
            if outcome == CopyOutcome::Cancelled {
                return Ok(CopyOutcome::Cancelled);
            }

            if let Some(dest_path) =
                resolve_created_dest(dest_dir, &candidate, existed_before, before_names, options)
            {
                created.push(CopiedEntry {
                    source_path: src.clone(),
                    snapshot: PathSnapshot::capture(&dest_path)?,
                    destination_path: dest_path,
                });
            }
        }
        if !created.is_empty() {
            self.push(UndoAction::Copy { entries: created });
        }
        Ok(CopyOutcome::Completed)
    }

    /// Records already-completed copy pairs `(source, destination)`.
    pub fn record_copy(&mut self, pairs: &[(PathBuf, PathBuf)]) -> io::Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let mut entries = Vec::with_capacity(pairs.len());
        for (source_path, destination_path) in pairs {
            entries.push(CopiedEntry {
                source_path: source_path.clone(),
                destination_path: destination_path.clone(),
                snapshot: PathSnapshot::capture(destination_path)?,
            });
        }
        self.push(UndoAction::Copy { entries });
        Ok(())
    }

    /// Moves each path in `sources` into `dest_dir` and records the transfers on completion.
    pub fn move_and_record(
        &mut self,
        sources: &[PathBuf],
        dest_dir: &Path,
        options: &CopyOptions,
        on_progress: &mut dyn FnMut(Progress),
    ) -> io::Result<CopyOutcome> {
        let mut moved = Vec::with_capacity(sources.len());
        for src in sources {
            let name = src.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "source has no file name")
            })?;
            let before_names = if options.policy == ConflictPolicy::KeepBoth {
                Some(dir_entry_names(dest_dir)?)
            } else {
                None
            };
            let candidate = dest_dir.join(name);
            let existed_before = candidate.symlink_metadata().is_ok();

            let outcome = move_entry(src, dest_dir, options, on_progress)?;
            if outcome == CopyOutcome::Cancelled {
                return Ok(CopyOutcome::Cancelled);
            }

            if let Some(dest_path) =
                resolve_created_dest(dest_dir, &candidate, existed_before, before_names, options)
            {
                moved.push(MovedEntry {
                    original_path: src.clone(),
                    snapshot: PathSnapshot::capture(&dest_path)?,
                    current_path: dest_path,
                });
            }
        }
        if !moved.is_empty() {
            self.push(UndoAction::Move { entries: moved });
        }
        Ok(CopyOutcome::Completed)
    }

    /// Records already-completed move pairs `(original_path, current_path)`.
    pub fn record_move(&mut self, pairs: &[(PathBuf, PathBuf)]) -> io::Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let mut entries = Vec::with_capacity(pairs.len());
        for (original_path, current_path) in pairs {
            entries.push(MovedEntry {
                original_path: original_path.clone(),
                current_path: current_path.clone(),
                snapshot: PathSnapshot::capture(current_path)?,
            });
        }
        self.push(UndoAction::Move { entries });
        Ok(())
    }

    /// Moves each path in `paths` to the system trash and records the operation for undo.
    ///
    /// Note that trashing succeeds on macOS even though [`UndoStack::undo`] refuses to restore
    /// trashed items where trash browsing is unsupported.
    pub fn trash_and_record(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        let mut trashed = Vec::with_capacity(paths.len());
        for path in paths {
            if let Err(err) = trash(path) {
                if !trashed.is_empty() {
                    self.record_trash(&trashed);
                }
                return Err(err);
            }
            trashed.push(path.clone());
        }
        self.record_trash(&trashed);
        Ok(())
    }

    /// Records paths that were sent to the system trash.
    pub fn record_trash(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let trash_snapshot = if is_trash_browsing_supported() {
            list_trash().ok()
        } else {
            None
        };
        let mut assigned_ids = HashSet::with_capacity(paths.len());
        let targets = paths
            .iter()
            .map(|path| {
                let trash_id = trash_snapshot.as_deref().and_then(|items| {
                    let entry = newest_matching_trash_entry(items, path, &assigned_ids)?;
                    assigned_ids.insert(entry.id.clone());
                    Some(entry.id)
                });
                TrashedTarget {
                    original_path: path.clone(),
                    trash_id,
                }
            })
            .collect();
        self.push(UndoAction::Trash { targets });
    }

    /// Deletes `paths` permanently without adding anything to the undo stack.
    pub fn delete_permanently_and_record(
        &mut self,
        paths: &[PathBuf],
        confirmed: Confirmed,
    ) -> io::Result<()> {
        delete_permanently(paths, confirmed)
    }

    /// No-op hook for explicit callers: permanent deletions are never undoable.
    pub fn record_permanent_delete(&mut self) {}

    /// Undoes the most recent recorded operation.
    ///
    /// All preconditions are verified before mutating disk state; on refusal an error is returned
    /// and neither the filesystem nor the undo stack is modified. Returns the undone action.
    pub fn undo(&mut self) -> io::Result<UndoAction> {
        let Some(action) = self.entries.back() else {
            return Err(UndoRefused::error(
                io::ErrorKind::NotFound,
                "nothing to undo",
            ));
        };

        execute_undo(action)?;
        Ok(self.entries.pop_back().expect("peeked entry must exist"))
    }
}

fn execute_undo(action: &UndoAction) -> io::Result<()> {
    match action {
        UndoAction::Rename { from, to, snapshot } => undo_rename(from, to, snapshot),
        UndoAction::CreateDir { path } => undo_create_dir(path),
        UndoAction::Copy { entries } => undo_copy_with(entries, volume_kind),
        UndoAction::Move { entries } => undo_move(entries),
        UndoAction::Trash { targets } => undo_trash(targets),
    }
}

fn undo_rename(from: &Path, to: &Path, snapshot: &PathSnapshot) -> io::Result<()> {
    if !snapshot.matches(to) {
        return Err(UndoRefused::error(
            io::ErrorKind::InvalidData,
            format!(
                "cannot undo rename of {}: target changed or moved since rename",
                to.display()
            ),
        ));
    }
    if from.symlink_metadata().is_ok() {
        return Err(UndoRefused::error(
            io::ErrorKind::AlreadyExists,
            format!(
                "cannot undo rename: original path {} now exists",
                from.display()
            ),
        ));
    }
    fs::rename(to, from)
}

fn undo_create_dir(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|err| {
        UndoRefused::error(
            err.kind(),
            format!("cannot undo folder creation {}: {err}", path.display()),
        )
    })?;
    if !meta.is_dir() {
        return Err(UndoRefused::error(
            io::ErrorKind::InvalidData,
            format!(
                "cannot undo folder creation: {} is no longer a folder",
                path.display()
            ),
        ));
    }
    let mut entries = fs::read_dir(path)?;
    if entries.next().transpose()?.is_some() {
        return Err(UndoRefused::error(
            io::ErrorKind::DirectoryNotEmpty,
            format!(
                "cannot undo folder creation: {} is not empty",
                path.display()
            ),
        ));
    }
    fs::remove_dir(path)
}

fn undo_copy_with(entries: &[CopiedEntry], kind: impl Fn(&Path) -> VolumeKind) -> io::Result<()> {
    for entry in entries {
        if !entry.snapshot.matches(&entry.destination_path) {
            return Err(UndoRefused::error(
                io::ErrorKind::InvalidData,
                format!(
                    "cannot undo copy of {}: copied entry changed or moved since copy",
                    entry.destination_path.display()
                ),
            ));
        }
        if kind(&entry.destination_path) != VolumeKind::Local {
            return Err(UndoRefused::error(
                io::ErrorKind::Unsupported,
                format!(
                    "cannot undo copy of {}: trash is unavailable on non-local volume",
                    entry.destination_path.display()
                ),
            ));
        }
    }
    for entry in entries {
        trash_with(&entry.destination_path, &kind).map_err(|err| {
            if is_trash_unavailable(&err) {
                UndoRefused::error(io::ErrorKind::Unsupported, err.to_string())
            } else {
                err
            }
        })?;
    }
    Ok(())
}

fn undo_move(entries: &[MovedEntry]) -> io::Result<()> {
    let mut seen_originals = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !entry.snapshot.matches(&entry.current_path) {
            return Err(UndoRefused::error(
                io::ErrorKind::InvalidData,
                format!(
                    "cannot undo move of {}: entry changed or moved since transfer",
                    entry.current_path.display()
                ),
            ));
        }
        if !seen_originals.insert(&entry.original_path)
            || entry.original_path.symlink_metadata().is_ok()
        {
            return Err(UndoRefused::error(
                io::ErrorKind::AlreadyExists,
                format!(
                    "cannot undo move: original path {} now exists",
                    entry.original_path.display()
                ),
            ));
        }
        let Some(orig_parent) = entry.original_path.parent() else {
            return Err(UndoRefused::error(
                io::ErrorKind::InvalidInput,
                format!(
                    "cannot undo move: {} has no parent directory",
                    entry.original_path.display()
                ),
            ));
        };
        if !orig_parent.is_dir() {
            return Err(UndoRefused::error(
                io::ErrorKind::NotFound,
                format!(
                    "cannot undo move: original parent {} no longer exists",
                    orig_parent.display()
                ),
            ));
        }
    }

    for entry in entries {
        move_path_back(&entry.current_path, &entry.original_path)?;
    }
    Ok(())
}

fn move_path_back(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            let parent = to.parent().expect("validated in undo_move");
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let options = CopyOptions::new(ConflictPolicy::Skip, cancel);
            let _ = move_entry(from, parent, &options, &mut |_| {})?;
            // Preserve the exact original filename when moving across filesystems.
            let expected = parent.join(from.file_name().unwrap_or_default());
            if expected != to {
                fs::rename(expected, to)?;
            }
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn undo_trash(targets: &[TrashedTarget]) -> io::Result<()> {
    if !is_trash_browsing_supported() {
        return Err(UndoRefused::error(
            io::ErrorKind::Unsupported,
            "cannot undo trash: restoring from the trash is unsupported on this platform",
        ));
    }

    let items = list_trash().map_err(|err| {
        UndoRefused::error(
            err.kind(),
            format!("cannot undo trash: failed to inspect trash ({err})"),
        )
    })?;
    let mut to_restore = Vec::with_capacity(targets.len());
    let mut used_ids = HashSet::with_capacity(targets.len());

    for target in targets {
        let found = target
            .trash_id
            .as_ref()
            .and_then(|id| {
                items
                    .iter()
                    .find(|item| &item.id == id && !used_ids.contains(&item.id))
                    .cloned()
            })
            .or_else(|| newest_matching_trash_entry(&items, &target.original_path, &used_ids));

        let Some(entry) = found else {
            return Err(UndoRefused::error(
                io::ErrorKind::NotFound,
                format!(
                    "cannot undo trash: {} is no longer in the trash",
                    target.original_path.display()
                ),
            ));
        };
        used_ids.insert(entry.id.clone());
        to_restore.push(entry);
    }

    restore_trash_items(&to_restore)
        .map_err(|err| UndoRefused::error(err.kind(), format!("cannot undo trash: {err}")))
}

fn resolve_rename_target(from: &Path, new_name: &Path) -> io::Result<PathBuf> {
    let parent = from.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "cannot rename filesystem root")
    })?;
    if new_name.parent() == Some(parent) {
        Ok(new_name.to_path_buf())
    } else {
        Ok(parent.join(new_name))
    }
}

fn dir_entry_names(dir: &Path) -> io::Result<HashSet<OsString>> {
    let mut names = HashSet::new();
    for entry in fs::read_dir(dir)? {
        names.insert(entry?.file_name());
    }
    Ok(names)
}

fn resolve_created_dest(
    dest_dir: &Path,
    candidate: &Path,
    existed_before: bool,
    before_names: Option<HashSet<OsString>>,
    options: &CopyOptions,
) -> Option<PathBuf> {
    if !existed_before {
        return candidate
            .symlink_metadata()
            .is_ok()
            .then(|| candidate.to_path_buf());
    }
    match options.policy {
        ConflictPolicy::Skip | ConflictPolicy::Replace => None,
        ConflictPolicy::KeepBoth => {
            let before = before_names?;
            let entries = fs::read_dir(dest_dir).ok()?;
            for entry in entries.flatten() {
                if !before.contains(&entry.file_name()) {
                    return Some(entry.path());
                }
            }
            None
        }
    }
}

fn canonical_original_path(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    match parent.canonicalize() {
        Ok(canon) => {
            #[cfg(windows)]
            let canon = {
                let s = canon.to_string_lossy();
                if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                    PathBuf::from(format!(r"\\{rest}"))
                } else if let Some(rest) = s.strip_prefix(r"\\?\") {
                    PathBuf::from(rest)
                } else {
                    canon
                }
            };
            canon.join(name)
        }
        Err(_) => path.to_path_buf(),
    }
}

fn newest_matching_trash_entry(
    items: &[TrashItem],
    original_path: &Path,
    excluded_ids: &HashSet<OsString>,
) -> Option<TrashItem> {
    let canon_path = canonical_original_path(original_path);
    items
        .iter()
        .filter(|item| {
            if excluded_ids.contains(&item.id) {
                return false;
            }
            let item_orig = item.original_path();
            item_orig == original_path || item_orig == canon_path
        })
        .max_by_key(|item| item.time_deleted)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn bounded_stack_evicts_oldest_and_exposes_labels() {
        let dir = std::env::temp_dir().join(format!("kwati_undo_unit_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut stack = UndoStack::with_capacity(2);
        assert_eq!(stack.undo_label(), None);

        let d1 = dir.join("d1");
        let d2 = dir.join("d2");
        let d3 = dir.join("d3");
        stack.create_dir_and_record(&d1).unwrap();
        assert_eq!(stack.undo_label(), Some("Undo new folder"));
        stack.create_dir_and_record(&d2).unwrap();
        stack.create_dir_and_record(&d3).unwrap();

        assert_eq!(stack.len(), 2);
        stack.undo().unwrap();
        stack.undo().unwrap();
        assert!(stack.is_empty());
        assert!(d1.is_dir(), "oldest action should have been evicted");
        assert!(!d2.exists());
        assert!(!d3.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_copy_refuses_non_local_volume() {
        let dir = std::env::temp_dir().join(format!("kwati_undo_nonlocal_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src_dir = dir.join("src");
        let dst_dir = dir.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        let src = src_dir.join("file.txt");
        fs::write(&src, b"payload").unwrap();

        let mut stack = UndoStack::new();
        let options = CopyOptions::new(ConflictPolicy::Skip, Arc::new(AtomicBool::new(false)));
        stack
            .copy_and_record(std::slice::from_ref(&src), &dst_dir, &options, &mut |_| {})
            .unwrap();

        let UndoAction::Copy { entries } = stack.peek().unwrap() else {
            panic!("expected Copy action");
        };
        let err = undo_copy_with(entries, |_| VolumeKind::Network).unwrap_err();
        assert!(is_undo_refused(&err), "got: {err}");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(dst_dir.join("file.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
