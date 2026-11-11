mod search;
mod settings;
mod trash;
mod watcher;

pub use search::{SearchHandle, SearchHit, SearchOptions, Searcher};
pub use settings::{default_path as settings_path, Settings, SettingsWarning};
pub use trash::trash;
pub use watcher::DirWatcher;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A directory entry discovered during listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: OsString,
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Entry {
    pub fn new(name: OsString, path: PathBuf, is_dir: bool) -> Self {
        Self { name, path, is_dir }
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns a lossy string representation of the entry name with control characters replaced
    /// by the Unicode replacement character (`\u{FFFD}`).
    pub fn display_name(&self) -> String {
        self.name
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
            .collect()
    }
}

/// Sorts a slice of entries: directories first, then case-insensitive by name.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| {
                let a_lower = a.name.to_string_lossy().to_lowercase();
                let b_lower = b.name.to_string_lossy().to_lowercase();
                a_lower.cmp(&b_lower)
            })
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// A collection of sorted directory entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
}

impl Listing {
    pub fn new(mut entries: Vec<Entry>) -> Self {
        sort_entries(&mut entries);
        Self { entries }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<Entry> {
        self.entries
    }

    pub fn sort(&mut self) {
        sort_entries(&mut self.entries);
    }
}

impl Deref for Listing {
    type Target = [Entry];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl IntoIterator for Listing {
    type Item = Entry;
    type IntoIter = std::vec::IntoIter<Entry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a Listing {
    type Item = &'a Entry;
    type IntoIter = std::slice::Iter<'a, Entry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl From<Vec<Entry>> for Listing {
    fn from(entries: Vec<Entry>) -> Self {
        Self::new(entries)
    }
}

/// Lists a directory at the given path.
///
/// Follows symlinks to determine if an entry is a directory. Dangling symlinks are listed
/// as non-directories without failing the listing. Unreadable entries are skipped.
pub fn list_dir(path: &Path) -> io::Result<Listing> {
    let read_dir = fs::read_dir(path)?;
    let mut entries = Vec::new();

    for item in read_dir {
        let entry = match item {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let entry_path = path.join(&name);
        let is_dir = fs::metadata(&entry_path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);

        entries.push(Entry::new(name, entry_path, is_dir));
    }

    Ok(Listing::new(entries))
}

/// Identifies a target location to be browsed or operated on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    Remote { profile_id: String, path: PathBuf },
}

impl From<PathBuf> for Location {
    fn from(path: PathBuf) -> Self {
        Location::Local(path)
    }
}

impl From<&Path> for Location {
    fn from(path: &Path) -> Self {
        Location::Local(path.to_path_buf())
    }
}

impl From<&str> for Location {
    fn from(path: &str) -> Self {
        Location::Local(PathBuf::from(path))
    }
}

/// Errors that can occur during backend operations.
#[derive(Debug)]
pub enum BackendError {
    Io(io::Error),
    Unsupported,
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Io(err) => write!(f, "I/O error: {}", err),
            BackendError::Unsupported => write!(f, "operation unsupported"),
        }
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BackendError::Io(err) => Some(err),
            BackendError::Unsupported => None,
        }
    }
}

impl From<io::Error> for BackendError {
    fn from(err: io::Error) -> Self {
        BackendError::Io(err)
    }
}

impl PartialEq for BackendError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BackendError::Unsupported, BackendError::Unsupported) => true,
            (BackendError::Io(a), BackendError::Io(b)) => a.kind() == b.kind(),
            _ => false,
        }
    }
}

impl BackendError {
    pub fn is_unsupported(&self) -> bool {
        matches!(self, BackendError::Unsupported)
    }
}

/// Trait representing a storage backend capable of listing directory contents.
pub trait Backend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError>;
}

/// The local filesystem backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalBackend;

impl LocalBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for LocalBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        match location {
            Location::Local(path) => {
                let listing = list_dir(path)?;
                Ok(listing)
            }
            Location::Remote { .. } => Err(BackendError::Unsupported),
        }
    }
}
