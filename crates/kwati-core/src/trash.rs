use std::io;
use std::path::Path;

/// Moves the file or directory at `path` to the system trash.
///
/// Never deletes permanently. The error message always includes `path`, because the
/// `trash` crate's errors do not consistently name the offending path.
pub fn trash(path: &Path) -> io::Result<()> {
    ::trash::delete(path)
        .map_err(|err| io::Error::other(format!("cannot trash {}: {err}", path.display())))
}
