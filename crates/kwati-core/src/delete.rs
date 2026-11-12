use std::fs;
use std::io;
use std::path::PathBuf;

/// Proof that the user was asked and agreed to a permanent delete.
///
/// The field is private, so code outside this crate cannot build one by accident; only the
/// interface layer should call [`Confirmed::user_confirmed`], after showing the prompt.
#[derive(Debug, Clone, Copy)]
pub struct Confirmed(());

impl Confirmed {
    pub fn user_confirmed() -> Self {
        Self(())
    }
}

/// Deletes each path permanently, bypassing the trash.
///
/// A symlink is removed itself and never followed. Directories are removed with their
/// contents. Stops at the first failure; paths after it are left untouched.
///
/// ```
/// use kwati_core::{delete_permanently, Confirmed};
///
/// delete_permanently(&[], Confirmed::user_confirmed()).unwrap();
/// ```
pub fn delete_permanently(paths: &[PathBuf], _confirmed: Confirmed) -> io::Result<()> {
    for path in paths {
        let is_real_dir = fs::symlink_metadata(path)?.is_dir();
        if is_real_dir {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// ```compile_fail,E0061
/// use kwati_core::delete_permanently;
///
/// let _ = delete_permanently(&[]);
/// ```
#[cfg(doctest)]
mod delete_requires_confirmation {}
