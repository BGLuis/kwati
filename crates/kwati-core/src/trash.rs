use crate::volume::{volume_kind, VolumeKind};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

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
