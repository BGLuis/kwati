mod bounded;
mod connection;
mod copy;
mod delete;
mod move_entry;
mod profiles;
mod search;
mod secrets;
mod settings;
mod transfer;
mod trash;
<<<<<<< HEAD
mod volume;
=======
pub mod volume;
>>>>>>> a76791f (feat(core): poll network mounts instead of waiting for change events (#32))
mod watcher;

pub use bounded::{list_dir_bounded, list_dir_bounded_with, DEFAULT_MAX_ABANDONED};
pub use connection::{is_dropped_connection, ConnectionState, ReconnectingBackend, RetryPolicy};
pub use copy::{
    copy_entry, ConflictPolicy, CopyOptions, CopyOutcome, Progress, DEFAULT_CHUNK_SIZE,
};
pub use delete::{delete_permanently, Confirmed};
pub use move_entry::{move_entry, move_entry_with};
pub use profiles::{
    default_path as profiles_path, AuthMethod, ConnectionProfile, ConnectionProfiles,
    PendingSecret, Protocol,
};
pub use search::{SearchHandle, SearchHit, SearchOptions, Searcher};
pub use secrets::{KeyringSecrets, Secrets, SecretsError};
pub use settings::{default_path as settings_path, Settings, SettingsWarning};
<<<<<<< HEAD
pub use transfer::{copy_between, move_between};
pub use trash::{is_trash_unavailable, trash, trash_with, TrashUnavailable};
pub use volume::{volume_kind, VolumeKind};
pub use watcher::DirWatcher;
=======
pub use trash::trash;
pub use volume::{
    classify_linux_fstype, classify_macos_fstype, volume_kind, volume_kind_from_mountinfo,
    VolumeKind,
};
pub use watcher::{DirWatcher, WatcherBackend};
>>>>>>> a76791f (feat(core): poll network mounts instead of waiting for change events (#32))

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

pub mod selection;
pub use selection::Selection;

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

    /// Returns the [`Location`] corresponding to this entry.
    pub fn location(&self) -> Location {
        Location::Local(self.path.clone())
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

fn contains_path_separator(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str()
            .as_bytes()
            .iter()
            .any(|&b| b == b'/' || b == b'\\')
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .any(|c| c == b'/' as u16 || c == b'\\' as u16)
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.to_string_lossy()
            .chars()
            .any(|c| c == '/' || c == '\\' || std::path::is_separator(c))
    }
}

/// Renames a directory entry to a new name.
///
/// Refuses to rename if the target already exists or if the new name contains a path separator.
pub fn rename_entry(from: impl AsRef<Path>, new_name: impl AsRef<Path>) -> io::Result<()> {
    let from = from.as_ref();
    let new_name = new_name.as_ref();

    if from.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source path cannot be empty",
        ));
    }

    if new_name.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "new name cannot be empty",
        ));
    }

    if from.symlink_metadata().is_err() {
        return Err(from.symlink_metadata().unwrap_err());
    }

    let parent = match from.parent() {
        Some(p) => p,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot rename filesystem root",
            ));
        }
    };

    let (target, candidate_name) = if from.parent().is_some() && new_name.parent() == from.parent()
    {
        let file_name = match new_name.file_name() {
            Some(name) => Path::new(name),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid destination path",
                ));
            }
        };
        (new_name.to_path_buf(), file_name)
    } else {
        (parent.join(new_name), new_name)
    };

    if candidate_name == Path::new(".") || candidate_name == Path::new("..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid entry name",
        ));
    }

    if contains_path_separator(candidate_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name cannot contain path separators",
        ));
    }

    if target.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target already exists",
        ));
    }

    fs::rename(from, &target)
}

/// Creates a new directory at the given path.
///
/// Refuses to create the directory if a file or directory already exists at the target path.
pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();

    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path cannot be empty",
        ));
    }

    if path.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target already exists",
        ));
    }

    fs::create_dir(path)
}

/// Identifies a target location to be browsed or operated on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Location {
    Local(PathBuf),
    Remote { profile_id: String, path: PathBuf },
}

impl Location {
    /// Returns the filesystem path for this location.
    pub fn path(&self) -> &Path {
        match self {
            Location::Local(path) => path,
            Location::Remote { path, .. } => path,
        }
    }

    /// The last component of the path, if any.
    pub fn file_name(&self) -> Option<&OsStr> {
        self.path().file_name()
    }

    /// The same location kind (and profile) with `name` appended to the path.
    pub fn join(&self, name: &OsStr) -> Location {
        match self {
            Location::Local(path) => Location::Local(path.join(name)),
            Location::Remote { profile_id, path } => Location::Remote {
                profile_id: profile_id.clone(),
                path: path.join(name),
            },
        }
    }
}

impl From<PathBuf> for Location {
    fn from(path: PathBuf) -> Self {
        Location::Local(path)
    }
}

impl From<&PathBuf> for Location {
    fn from(path: &PathBuf) -> Self {
        Location::Local(path.clone())
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

impl From<&Entry> for Location {
    fn from(entry: &Entry) -> Self {
        Location::Local(entry.path.clone())
    }
}

impl From<Entry> for Location {
    fn from(entry: Entry) -> Self {
        Location::Local(entry.path)
    }
}

impl From<&Location> for Location {
    fn from(loc: &Location) -> Self {
        loc.clone()
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

    /// Whether the backend refused to trash because it has no trash for the location.
    pub fn is_trash_unavailable(&self) -> bool {
        matches!(self, BackendError::Io(err) if is_trash_unavailable(err))
    }
}

/// What a backend reports about one entry, without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryMeta {
    pub kind: EntryKind,
    /// Size in bytes for files; 0 for the other kinds.
    pub len: u64,
}

/// A storage backend. Only [`Backend::list`] is required; every other operation defaults to
/// [`BackendError::Unsupported`], and [`Backend::trash`] to a trash-unavailable error, so a
/// backend implements what its protocol supports.
pub trait Backend: Send + Sync {
    fn list(&self, location: &Location) -> Result<Listing, BackendError>;

    fn stat(&self, _location: &Location) -> Result<EntryMeta, BackendError> {
        Err(BackendError::Unsupported)
    }

    fn open_read(&self, _location: &Location) -> Result<Box<dyn io::Read + Send>, BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Creates the file, or truncates it if it exists.
    fn open_write(&self, _location: &Location) -> Result<Box<dyn io::Write + Send>, BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Renames within this backend, replacing `to` when it is an existing file.
    fn rename(&self, _from: &Location, _to: &Location) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    fn create_dir(&self, _location: &Location) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Deletes a file, a symlink (never followed) or a folder with its contents.
    fn delete_permanently(
        &self,
        _location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        Err(BackendError::Unsupported)
    }

    fn trash(&self, location: &Location) -> Result<(), BackendError> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            TrashUnavailable {
                path: location.path().to_path_buf(),
            },
        )
        .into())
    }
}

/// The local filesystem backend.
///
/// Folders on a network or unidentified volume are listed with [`list_dir_bounded`], so a
/// share that stops answering fails with a timeout instead of hanging the caller.
#[derive(Debug, Clone, Copy)]
pub struct LocalBackend {
    list_timeout: std::time::Duration,
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::with_list_timeout(std::time::Duration::from_secs(u64::from(
            settings::DEFAULT_LIST_TIMEOUT_SECS,
        )))
    }
}

impl LocalBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Uses `timeout` for listings on network and unidentified volumes, typically
    /// [`Settings::list_timeout_secs`].
    pub fn with_list_timeout(list_timeout: std::time::Duration) -> Self {
        Self { list_timeout }
    }
}

fn local_path(location: &Location) -> Result<&Path, BackendError> {
    match location {
        Location::Local(path) => Ok(path),
        Location::Remote { .. } => Err(BackendError::Unsupported),
    }
}

impl Backend for LocalBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let path = local_path(location)?;
        let listing = match volume_kind(path) {
            VolumeKind::Local => list_dir(path)?,
            VolumeKind::Network | VolumeKind::Unknown => list_dir_bounded(path, self.list_timeout)?,
        };
        Ok(listing)
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        let meta = fs::symlink_metadata(local_path(location)?)?;
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Symlink
        };
        let len = if kind == EntryKind::File {
            meta.len()
        } else {
            0
        };
        Ok(EntryMeta { kind, len })
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn io::Read + Send>, BackendError> {
        Ok(Box::new(fs::File::open(local_path(location)?)?))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn io::Write + Send>, BackendError> {
        Ok(Box::new(fs::File::create(local_path(location)?)?))
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        Ok(fs::rename(local_path(from)?, local_path(to)?)?)
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        Ok(create_dir(local_path(location)?)?)
    }

    fn delete_permanently(
        &self,
        location: &Location,
        confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        Ok(delete_permanently(
            &[local_path(location)?.to_path_buf()],
            confirmed,
        )?)
    }

    fn trash(&self, location: &Location) -> Result<(), BackendError> {
        Ok(trash(local_path(location)?)?)
    }
}

pub mod preview;
pub use preview::*;

pub mod history;
pub use history::History;
