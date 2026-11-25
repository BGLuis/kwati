//! Drag-and-drop policy, mount-point volume comparison, and target validation (#69).

use std::path::{Path, PathBuf};

use gpui::Modifiers;
use kwati_core::Location;

use crate::views::list_view::DragEntries;

/// Operation performed when entries are dropped onto a target folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropOperation {
    Move,
    Copy,
}

impl DropOperation {
    pub fn is_move(self) -> bool {
        self == Self::Move
    }
}

/// Reason a drop onto a destination folder is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropRejection {
    Empty,
    IntoSelfOrSubfolder,
    AlreadyInFolder,
}

impl DropRejection {
    pub fn status_message(self) -> Option<&'static str> {
        match self {
            Self::Empty => None,
            Self::IntoSelfOrSubfolder => {
                Some("Cannot drop a folder into itself or one of its subfolders")
            }
            Self::AlreadyInFolder => Some("The items are already in this folder"),
        }
    }
}

/// Returns `true` when the platform's force-copy modifier is held during a drag
/// (`Option`/`Alt` on macOS, `Ctrl` on Linux and Windows).
pub fn is_copy_modifier(modifiers: Modifiers) -> bool {
    is_copy_modifier_for_os(modifiers, cfg!(target_os = "macos"))
}

/// Pure OS-parameterized helper for [`is_copy_modifier`].
pub const fn is_copy_modifier_for_os(modifiers: Modifiers, is_macos: bool) -> bool {
    if is_macos {
        modifiers.alt
    } else {
        modifiers.control
    }
}

/// Collects all local mount points reported by `kwati_core` (`list_volumes` + `network_mounts`).
pub fn collect_volume_mounts() -> Vec<PathBuf> {
    let mut mounts: Vec<PathBuf> = kwati_core::list_volumes()
        .into_iter()
        .map(|v| v.mount_point)
        .chain(
            kwati_core::network_mounts()
                .into_iter()
                .map(|m| m.mount_point),
        )
        .collect();
    mounts.sort();
    mounts.dedup();
    mounts
}

/// Canonicalizes `path` when possible, or its nearest existing ancestor with the remaining
/// suffix appended, so symlinks (such as `/var` -> `/private/var`) match mount points reliably.
fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    let mut suffix = Vec::new();
    let mut cur = path;
    while let Some(name) = cur.file_name() {
        suffix.push(name.to_os_string());
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent;
        if let Ok(mut canon) = cur.canonicalize() {
            for part in suffix.into_iter().rev() {
                canon.push(part);
            }
            return canon;
        }
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Finds the longest matching mount point from `mounts` that contains `path`, falling back
/// to the filesystem root when no mount matches.
pub fn mount_point_for(path: &Path, mounts: &[PathBuf]) -> PathBuf {
    let normalized = normalize_path(path);
    let mut best: Option<(usize, PathBuf)> = None;

    for mount in mounts {
        let mount_norm = normalize_path(mount);
        let matched = if path.starts_with(mount) {
            Some((mount.components().count(), mount_norm.clone()))
        } else if normalized.starts_with(&mount_norm) {
            Some((mount_norm.components().count(), mount_norm))
        } else {
            None
        };
        if let Some((depth, key)) = matched {
            if best
                .as_ref()
                .is_none_or(|(best_depth, _)| depth > *best_depth)
            {
                best = Some((depth, key));
            }
        }
    }

    best.map(|(_, key)| key).unwrap_or_else(|| {
        normalized
            .ancestors()
            .last()
            .unwrap_or(&normalized)
            .to_path_buf()
    })
}

/// Returns `true` when `src` and `dst` reside on the same storage volume (or same remote profile).
pub fn same_volume(src: &Location, dst: &Location, mounts: &[PathBuf]) -> bool {
    match (src, dst) {
        (Location::Local(src_path), Location::Local(dst_path)) => {
            mount_point_for(src_path, mounts) == mount_point_for(dst_path, mounts)
        }
        (Location::Remote { profile_id: a, .. }, Location::Remote { profile_id: b, .. }) => a == b,
        _ => false,
    }
}

/// Decides whether dropping `sources` onto `dest_dir` moves or copies them.
///
/// Defaults to [`DropOperation::Move`] when every source shares the destination volume,
/// [`DropOperation::Copy`] across volumes, and always [`DropOperation::Copy`] when the
/// platform's force-copy modifier (`Ctrl`, or `Option` on macOS) is held.
pub fn resolve_drop_operation(
    sources: &[Location],
    dest_dir: &Location,
    modifiers: Modifiers,
    mounts: &[PathBuf],
) -> DropOperation {
    resolve_drop_operation_for_os(
        sources,
        dest_dir,
        modifiers,
        cfg!(target_os = "macos"),
        mounts,
    )
}

/// Pure OS-parameterized helper for [`resolve_drop_operation`].
pub fn resolve_drop_operation_for_os(
    sources: &[Location],
    dest_dir: &Location,
    modifiers: Modifiers,
    is_macos: bool,
    mounts: &[PathBuf],
) -> DropOperation {
    if is_copy_modifier_for_os(modifiers, is_macos) {
        return DropOperation::Copy;
    }
    if !sources.is_empty() && sources.iter().all(|src| same_volume(src, dest_dir, mounts)) {
        DropOperation::Move
    } else {
        DropOperation::Copy
    }
}

/// Returns `true` when `dest_dir` is `src` itself, or (when `src_is_dir`) a subfolder of `src`.
pub fn is_self_or_subfolder(src: &Location, src_is_dir: bool, dest_dir: &Location) -> bool {
    if src == dest_dir {
        return true;
    }
    match (src, dest_dir) {
        (Location::Local(src_path), Location::Local(dst_path)) => {
            if src_is_dir && dst_path.starts_with(src_path) {
                return true;
            }
            let src_norm = normalize_path(src_path);
            let dst_norm = normalize_path(dst_path);
            if src_norm == dst_norm || (src_is_dir && dst_norm.starts_with(&src_norm)) {
                return true;
            }
            false
        }
        (
            Location::Remote {
                profile_id: sp,
                path: src_path,
            },
            Location::Remote {
                profile_id: dp,
                path: dst_path,
            },
        ) => sp == dp && src_is_dir && dst_path.starts_with(src_path),
        _ => false,
    }
}

/// Validates dropping internal `dragged` entries onto `dest_dir` with `operation`.
pub fn validate_internal_drop(
    dragged: &DragEntries,
    dest_dir: &Location,
    operation: DropOperation,
) -> Result<(), DropRejection> {
    if dragged.items.is_empty() {
        return Err(DropRejection::Empty);
    }
    for item in &dragged.items {
        if is_self_or_subfolder(&item.location, item.is_dir, dest_dir) {
            return Err(DropRejection::IntoSelfOrSubfolder);
        }
    }
    if operation.is_move()
        && dragged
            .items
            .iter()
            .all(|item| item.location.parent().as_ref() == Some(dest_dir))
    {
        return Err(DropRejection::AlreadyInFolder);
    }
    Ok(())
}

/// Validates dropping external OS `paths` onto `dest_dir` (external drops always copy).
pub fn validate_external_drop(paths: &[PathBuf], dest_dir: &Location) -> Result<(), DropRejection> {
    if paths.is_empty() {
        return Err(DropRejection::Empty);
    }
    for path in paths {
        let src = Location::Local(path.clone());
        let is_dir = path.is_dir();
        if is_self_or_subfolder(&src, is_dir, dest_dir) {
            return Err(DropRejection::IntoSelfOrSubfolder);
        }
    }
    Ok(())
}
