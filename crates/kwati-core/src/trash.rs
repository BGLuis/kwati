use crate::delete::Confirmed;
use crate::volume::{volume_kind, VolumeKind};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The trash was not used because the path is not known to be on a local disk.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::Unsupported`]; detect it with
/// [`is_trash_unavailable`]. The caller can then offer [`crate::delete_permanently`].
#[derive(Debug)]
pub struct TrashUnavailable {
    pub path: PathBuf,
}

impl fmt::Display for TrashUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot trash {}: no trash on a network or unidentified volume",
            self.path.display()
        )
    }
}

impl std::error::Error for TrashUnavailable {}

/// Whether `err` was produced because the trash is unavailable for the path.
pub fn is_trash_unavailable(err: &io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<TrashUnavailable>())
}

/// Restoring a trashed item would overwrite an existing entry at its original location.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::AlreadyExists`]; detect it with
/// [`restore_conflict`] or [`is_restore_conflict`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreConflict {
    pub item_name: OsString,
    pub destination: PathBuf,
}

impl fmt::Display for RestoreConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot restore '{}': '{}' already exists",
            self.item_name.to_string_lossy(),
            self.destination.display()
        )
    }
}

impl std::error::Error for RestoreConflict {}

/// Extracts the [`RestoreConflict`] details from `err`, if restoring failed due to an existing
/// entry at the original destination.
pub fn restore_conflict(err: &io::Error) -> Option<&RestoreConflict> {
    err.get_ref()?.downcast_ref()
}

/// Whether `err` was produced because restoring would collide with an existing destination entry.
pub fn is_restore_conflict(err: &io::Error) -> bool {
    restore_conflict(err).is_some()
}

/// Metadata for an item currently inside the system trash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrashItem {
    pub id: OsString,
    pub name: OsString,
    pub original_parent: PathBuf,
    /// Seconds elapsed since the Unix epoch when the item was trashed, or negative if unknown.
    pub time_deleted: i64,
}

impl TrashItem {
    /// Full path where the item lived before being moved to the trash.
    pub fn original_path(&self) -> PathBuf {
        self.original_parent.join(&self.name)
    }

    /// Deletion timestamp as a [`SystemTime`], if available.
    pub fn deleted_at(&self) -> Option<SystemTime> {
        u64::try_from(self.time_deleted)
            .ok()
            .and_then(|secs| UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
    }

    /// Returns a lossy string representation of the item name with control characters replaced
    /// by `\u{FFFD}`.
    pub fn display_name(&self) -> String {
        self.name
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
            .collect()
    }
}

/// Whether listing, restoring, and purging trash items in-process is supported on this OS.
pub fn is_trash_browsing_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
mod platform {
    use super::*;

    fn to_core_item(item: ::trash::TrashItem) -> TrashItem {
        TrashItem {
            id: item.id,
            name: item.name,
            original_parent: item.original_parent,
            time_deleted: item.time_deleted,
        }
    }

    fn to_crate_item(item: &TrashItem) -> ::trash::TrashItem {
        ::trash::TrashItem {
            id: item.id.clone(),
            name: item.name.clone(),
            original_parent: item.original_parent.clone(),
            time_deleted: item.time_deleted,
        }
    }

    fn conflict_error(item_name: OsString, destination: PathBuf) -> io::Error {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            RestoreConflict {
                item_name,
                destination,
            },
        )
    }

    pub fn list_trash() -> io::Result<Vec<TrashItem>> {
        ::trash::os_limited::list()
            .map(|items| items.into_iter().map(to_core_item).collect())
            .map_err(|err| io::Error::other(format!("cannot list trash: {err}")))
    }

    pub fn restore_trash_items(items: &[TrashItem]) -> io::Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        // Pre-check every item before restoring any so a conflict in a multi-item batch leaves
        // both the trash and the filesystem untouched.
        let mut seen_destinations = HashSet::with_capacity(items.len());
        for item in items {
            let dest = item.original_path();
            if !seen_destinations.insert(dest.clone()) || dest.symlink_metadata().is_ok() {
                return Err(conflict_error(item.name.clone(), dest));
            }
        }

        let converted: Vec<::trash::TrashItem> = items.iter().map(to_crate_item).collect();
        ::trash::os_limited::restore_all(converted).map_err(|err| match err {
            ::trash::Error::RestoreCollision {
                path,
                remaining_items,
            } => {
                let item_name = remaining_items
                    .first()
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| path.file_name().unwrap_or_default().to_os_string());
                conflict_error(item_name, path)
            }
            ::trash::Error::RestoreTwins { path, items } => {
                let item_name = items
                    .first()
                    .map(|i| i.name.clone())
                    .unwrap_or_else(|| path.file_name().unwrap_or_default().to_os_string());
                conflict_error(item_name, path)
            }
            other => io::Error::other(format!("cannot restore trash items: {other}")),
        })
    }

    pub fn purge_trash_items(items: &[TrashItem]) -> io::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let converted: Vec<::trash::TrashItem> = items.iter().map(to_crate_item).collect();
        ::trash::os_limited::purge_all(converted)
            .map_err(|err| io::Error::other(format!("cannot purge trash items: {err}")))
    }
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
mod platform {
    use super::*;

    pub fn list_trash() -> io::Result<Vec<TrashItem>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "listing trash items is not supported on this platform",
        ))
    }

    pub fn restore_trash_items(_items: &[TrashItem]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "restoring trash items is not supported on this platform",
        ))
    }

    pub fn purge_trash_items(_items: &[TrashItem]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "purging trash items is not supported on this platform",
        ))
    }
}

/// Lists all items currently in the system trash.
///
/// Returns [`io::ErrorKind::Unsupported`] on macOS, where the OS does not expose trash listing;
/// callers should offer to open the system trash instead.
pub fn list_trash() -> io::Result<Vec<TrashItem>> {
    platform::list_trash()
}

/// Restores `items` from the system trash back to their original paths.
///
/// Never overwrites existing entries: if any item's original destination already exists (or two
/// selected items share the same original path), returns an [`io::Error`] of kind
/// [`io::ErrorKind::AlreadyExists`] wrapping [`RestoreConflict`] without modifying anything.
pub fn restore_trash_items(items: &[TrashItem]) -> io::Result<()> {
    platform::restore_trash_items(items)
}

/// Permanently deletes `items` from the system trash.
pub fn purge_trash_items(items: &[TrashItem], _confirmed: Confirmed) -> io::Result<()> {
    platform::purge_trash_items(items)
}

/// Permanently deletes every item currently in the system trash.
pub fn empty_trash(confirmed: Confirmed) -> io::Result<()> {
    let items = list_trash()?;
    purge_trash_items(&items, confirmed)
}

/// Moves the file or directory at `path` to the system trash.
///
/// Never deletes permanently. Paths on a network or unidentified volume are refused with
/// [`TrashUnavailable`] without touching them, because it is not established that the
/// platform trash recycles there instead of destroying. The error message always includes
/// `path`, because the `trash` crate's errors do not consistently name the offending path.
pub fn trash(path: &Path) -> io::Result<()> {
    trash_with(path, volume_kind)
}

#[doc(hidden)]
pub fn trash_with(path: &Path, kind: impl Fn(&Path) -> VolumeKind) -> io::Result<()> {
    std::fs::symlink_metadata(path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("cannot trash {}: {err}", path.display()),
        )
    })?;
    if kind(path) != VolumeKind::Local {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            TrashUnavailable {
                path: path.to_path_buf(),
            },
        ));
    }
    ::trash::delete(path)
        .map_err(|err| io::Error::other(format!("cannot trash {}: {err}", path.display())))
}
