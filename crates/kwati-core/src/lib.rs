mod bounded;
mod connection;
mod copy;
mod delete;
mod ftp;
mod move_entry;
pub mod places;
mod profiles;
mod remote;
pub mod remote_poll;
mod search;
mod secrets;
mod settings;
mod sftp;
mod smb;
mod transfer;
mod trash;
pub mod volume;
pub mod volumes;
mod watcher;

pub use bounded::{
    list_dir_bounded, list_dir_bounded_with, list_dir_bounded_with_options, DEFAULT_MAX_ABANDONED,
};
pub use connection::{is_dropped_connection, ConnectionState, ReconnectingBackend, RetryPolicy};
pub use copy::{
    copy_entry, ConflictPolicy, CopyOptions, CopyOutcome, Progress, DEFAULT_CHUNK_SIZE,
};
pub use delete::{delete_permanently, Confirmed};
pub use ftp::{CertificateInvalid, FtpBackend, TlsUnavailable};
pub use move_entry::{move_entry, move_entry_with};
pub use places::{
    list_places, resolve_bookmarks, standard_places, standard_places_from, Bookmark, Place,
    PlaceKind, ResolvedBookmark, SidebarPlaces,
};
pub use profiles::{
    default_path as profiles_path, AuthMethod, ConnectionProfile, ConnectionProfiles,
    PendingSecret, Protocol,
};
pub use remote::connect_profile;
pub use remote_poll::{RemotePoller, MIN_POLL_INTERVAL};
pub use search::{RemoteSearchOptions, SearchHandle, SearchHit, SearchOptions, Searcher};
pub use secrets::{KeyringSecrets, Secrets, SecretsError};
pub use settings::{
    default_path as settings_path, Settings, SettingsWarning, DEFAULT_REFRESH_INTERVAL_SECS,
    DEFAULT_REMOTE_POLL_INTERVAL_SECS, DEFAULT_REMOTE_SEARCH_DEPTH, DEFAULT_REMOTE_SEARCH_PARALLEL,
    MIN_REMOTE_POLL_INTERVAL_SECS,
};
pub use sftp::{known_hosts_path, HostKeyChanged, HostKeyUnknown, KnownHosts, SftpBackend};
pub use smb::SmbBackend;
pub use transfer::{copy_between, move_between};
pub use trash::{is_trash_unavailable, trash, trash_with, TrashUnavailable};
pub use volume::{volume_kind, VolumeKind};
pub use volumes::{
    device_volumes, filter_volumes, gvfs_share_name, is_pseudo_filesystem, list_volumes,
    mounted_volumes, network_mounts, parse_network_mounts, user_facing_volumes, NetworkMount,
    Volume, VolumeManager,
};
pub use watcher::{DirWatcher, WatcherBackend};

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
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
    pub is_hidden: bool,
}

impl Entry {
    pub fn new(name: OsString, path: PathBuf, is_dir: bool) -> Self {
        let is_hidden = is_hidden_entry(&name, None) || name.to_string_lossy().starts_with('.');
        Self {
            name,
            path,
            is_dir,
            size: None,
            modified: None,
            is_hidden,
        }
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_modified(mut self, modified: std::time::SystemTime) -> Self {
        self.modified = Some(modified);
        self
    }

    pub fn with_hidden(mut self, is_hidden: bool) -> Self {
        self.is_hidden = is_hidden;
        self
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

    pub fn size(&self) -> Option<u64> {
        self.size
    }

    pub fn modified(&self) -> Option<std::time::SystemTime> {
        self.modified
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
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

/// Determines whether an entry is hidden based on platform conventions.
#[cfg_attr(windows, allow(unused_variables))]
pub fn is_hidden_entry(name: &OsStr, _metadata: Option<&fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x00000002;
        if let Some(meta) = _metadata {
            if (meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0 {
                return true;
            }
        }
        false
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().starts_with(b".")
    }
    #[cfg(not(any(unix, windows)))]
    {
        name.to_string_lossy().starts_with('.')
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortOrder {
    pub column: SortColumn,
    pub direction: SortDirection,
}

impl SortOrder {
    pub fn new(column: SortColumn, direction: SortDirection) -> Self {
        Self { column, direction }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
        }
    }
}

impl ListOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn show_hidden(mut self, show: bool) -> Self {
        self.show_hidden = show;
        self
    }

    pub fn sort(mut self, column: SortColumn, direction: SortDirection) -> Self {
        self.sort_column = column;
        self.sort_direction = direction;
        self
    }
}

pub fn sort_entries(entries: &mut [Entry]) {
    sort_entries_by(entries, SortColumn::Name, SortDirection::Ascending);
}

pub fn sort_entries_by(entries: &mut [Entry], column: SortColumn, direction: SortDirection) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| {
                let ord = match column {
                    SortColumn::Name => {
                        let a_lower = a.name.to_string_lossy().to_lowercase();
                        let b_lower = b.name.to_string_lossy().to_lowercase();
                        a_lower.cmp(&b_lower)
                    }
                    SortColumn::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                    SortColumn::Date => a.modified.cmp(&b.modified),
                };
                match direction {
                    SortDirection::Ascending => ord,
                    SortDirection::Descending => ord.reverse(),
                }
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

    pub fn sort_by(&mut self, column: SortColumn, direction: SortDirection) {
        sort_entries_by(&mut self.entries, column, direction);
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
    list_dir_with_options(path, &ListOptions::default())
}

pub fn list_dir_with_options(path: &Path, options: &ListOptions) -> io::Result<Listing> {
    let read_dir = fs::read_dir(path)?;
    let mut entries = Vec::new();

    for item in read_dir {
        let entry = match item {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let entry_path = path.join(&name);
        let metadata = fs::metadata(&entry_path).ok();

        let is_hidden = is_hidden_entry(&name, metadata.as_ref());
        if !options.show_hidden && is_hidden {
            continue;
        }

        let is_dir = metadata.as_ref().map(|meta| meta.is_dir()).unwrap_or(false);
        let size = if is_dir {
            None
        } else {
            metadata.as_ref().map(|meta| meta.len())
        };
        let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());

        entries.push(Entry {
            name,
            path: entry_path,
            is_dir,
            size,
            modified,
            is_hidden,
        });
    }

    let mut listing = Listing { entries };
    listing.sort_by(options.sort_column, options.sort_direction);
    Ok(listing)
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

    /// The folder holding this location, on the same side (and profile), or `None` at a root.
    pub fn parent(&self) -> Option<Location> {
        let parent = self.path().parent()?;
        Some(match self {
            Location::Local(_) => Location::Local(parent.to_path_buf()),
            Location::Remote { profile_id, .. } => Location::Remote {
                profile_id: profile_id.clone(),
                path: parent.to_path_buf(),
            },
        })
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

    /// The rejection behind this error, if the server refused the user name and credential.
    pub fn authentication_failed(&self) -> Option<&AuthenticationFailed> {
        match self {
            BackendError::Io(err) => err.get_ref()?.downcast_ref(),
            BackendError::Unsupported => None,
        }
    }
}

/// The server refused the user name and the password, key or passphrase sent with it.
///
/// Carried inside an [`io::Error`] of kind [`io::ErrorKind::PermissionDenied`]; detect it with
/// [`BackendError::authentication_failed`]. Backends never retry it on their own, so a wrong
/// password does not lock the account; the interface asks the user again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationFailed {
    pub host: String,
    pub port: u16,
    pub user: String,
}

impl AuthenticationFailed {
    pub(crate) fn error(profile: &ConnectionProfile) -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            Self {
                host: profile.host.clone(),
                port: profile.port,
                user: profile.user.clone(),
            },
        )
    }
}

impl std::fmt::Display for AuthenticationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{} rejected the credentials of {}",
            self.host, self.port, self.user
        )
    }
}

impl std::error::Error for AuthenticationFailed {}

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

    fn list_with_options(
        &self,
        location: &Location,
        options: &ListOptions,
    ) -> Result<Listing, BackendError> {
        match location {
            Location::Local(path) => Ok(list_dir_with_options(path, options)?),
            Location::Remote { .. } => {
                let mut entries = self.list(location)?.into_entries();
                entries.retain(|entry| options.show_hidden || !entry.is_hidden);
                sort_entries_by(&mut entries, options.sort_column, options.sort_direction);
                Ok(Listing { entries })
            }
        }
    }

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalBackend {
    list_timeout: std::time::Duration,
    options: ListOptions,
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self {
            list_timeout: std::time::Duration::from_secs(u64::from(
                settings::DEFAULT_LIST_TIMEOUT_SECS,
            )),
            options: ListOptions::default(),
        }
    }
}

impl LocalBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_list_timeout(list_timeout: std::time::Duration) -> Self {
        Self {
            list_timeout,
            options: ListOptions::default(),
        }
    }

    pub fn with_options(options: ListOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
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
        self.list_with_options(location, &self.options)
    }

    fn list_with_options(
        &self,
        location: &Location,
        options: &ListOptions,
    ) -> Result<Listing, BackendError> {
        let path = local_path(location)?;
        let listing = match volume_kind(path) {
            VolumeKind::Local => list_dir_with_options(path, options)?,
            VolumeKind::Network | VolumeKind::Unknown => {
                list_dir_bounded_with_options(path, self.list_timeout, *options)?
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_new_marks_dotfiles_hidden_across_platforms() {
        let secret = Entry::new(".secret".into(), PathBuf::from("/.secret"), false);
        assert!(secret.is_hidden);

        let visible = Entry::new("visible.txt".into(), PathBuf::from("/visible.txt"), false);
        assert!(!visible.is_hidden);
    }
}
