use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use smb::{
    Client, ClientConfig, ConnectionConfig, CreateOptions, Dialect, DirAccessMask, Directory,
    FileAccessMask, FileAttributes, FileBasicInformation, FileCreateArgs,
    FileDispositionInformation, FileIdBothDirectoryInformation, FileRenameInformation,
    FileStandardInformation, GetLen, Resource, ResourceHandle, Status, UncPath,
};

use crate::chunking::split_range;
use crate::{
    AuthenticationFailed, Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryKind,
    EntryMeta, Listing, Location, Protocol, RangeSource, ReadSeek, Secrets,
};

const TIMEOUT: Duration = Duration::from_secs(15);
const SMB_CONCURRENT_CHUNK_SIZE: u32 = 1024 * 1024;
const SMB_MAX_CONCURRENT_CHUNKS: usize = 4;
const SMB_CHUNK_RETRY_ATTEMPTS: u32 = 2;

/// A Windows or Samba server, browsed and changed through the [`Backend`] trait, with the pure
/// Rust `smb` client in its blocking mode (no runtime of its own).
///
/// The first component of a remote path names the share (`/photos/2026/a.jpg` is `a.jpg` in
/// folder `2026` of share `photos`); listing `/` is unsupported, because enumerating shares
/// needs RPC over `IPC$`, which not every server allows. SMB 2.0.2 is the lowest dialect offered:
/// a server that only speaks SMB1 is refused.
///
/// Calls block the calling thread; call them from a worker thread. There is no trash on a share
/// reached this way: [`Backend::trash`] keeps its unavailable default.
pub struct SmbBackend {
    profile_id: String,
    server: String,
    user: String,
    password: String,
    client: Client,
    connected: Mutex<HashSet<String>>,
}

impl SmbBackend {
    /// Connects to the share named by the profile's start path and authenticates with the
    /// password stored in `secrets` under the profile id.
    ///
    /// A refused password fails with [`BackendError::authentication_failed`] and is not retried.
    /// A server that negotiates nothing newer than SMB1 fails with
    /// [`io::ErrorKind::Unsupported`].
    pub fn connect(
        profile: &ConnectionProfile,
        secrets: &dyn Secrets,
    ) -> Result<Self, BackendError> {
        if profile.protocol != Protocol::Smb {
            return Err(invalid("the profile is not an SMB profile").into());
        }
        let share = share_of(Path::new(&profile.start_path))
            .ok_or_else(|| invalid("the start path must begin with a share name"))?
            .to_owned();
        let password = secrets
            .get(&profile.id)
            .map_err(io::Error::from)?
            .ok_or_else(|| invalid("no password is saved for the profile"))?;
        let client = Client::new(ClientConfig {
            connection: ConnectionConfig {
                port: Some(profile.port),
                timeout: Some(TIMEOUT),
                min_dialect: Some(Dialect::Smb0202),
                ..Default::default()
            },
            ..Default::default()
        });
        let backend = Self {
            profile_id: profile.id.clone(),
            server: profile.host.clone(),
            user: profile.user.clone(),
            password,
            client,
            connected: Mutex::new(HashSet::new()),
        };
        backend
            .ensure_share(&share)
            .map_err(|err| connect_error(profile, err))?;
        Ok(backend)
    }

    /// Opens a session on `share` the first time it is used.
    fn ensure_share(&self, share: &str) -> Result<(), smb::Error> {
        let mut connected = self.connected.lock().unwrap_or_else(|p| p.into_inner());
        if connected.contains(share) {
            return Ok(());
        }
        let root = UncPath::new(&self.server)?.with_share(share)?;
        self.client
            .share_connect(&root, &self.user, self.password.clone())?;
        connected.insert(share.to_owned());
        Ok(())
    }

    /// The UNC path of `location`, after connecting to its share. `None` for the root, which
    /// names no share.
    fn unc(&self, location: &Location) -> Result<(UncPath, String), BackendError> {
        let path = match location {
            Location::Remote { profile_id, path } if *profile_id == self.profile_id => path,
            _ => return Err(BackendError::Unsupported),
        };
        let mut parts = components(path)?;
        if parts.is_empty() {
            return Err(BackendError::Unsupported);
        }
        let share = parts.remove(0);
        self.ensure_share(&share).map_err(smb_error)?;
        let unc = UncPath::new(&self.server)
            .and_then(|unc| unc.with_share(&share))
            .map_err(smb_error)?;
        let inner = parts.join("\\");
        Ok(if inner.is_empty() {
            (unc, inner)
        } else {
            (unc.with_path(&inner), inner)
        })
    }

    fn open(&self, location: &Location, args: &FileCreateArgs) -> Result<Handle, BackendError> {
        let (unc, _) = self.unc(location)?;
        let resource = self.client.create_file(&unc, args).map_err(smb_error)?;
        Ok(Handle(Some(resource)))
    }

    fn list_dir(&self, location: &Location) -> Result<Vec<Entry>, BackendError> {
        let access = DirAccessMask::new()
            .with_list_directory(true)
            .with_synchronize(true);
        let handle = self.open(location, &FileCreateArgs::make_open_existing(access.into()))?;
        let dir = Arc::new(handle.into_dir()?);
        let mut entries = Vec::new();
        let result = (|| {
            for item in
                Directory::query::<FileIdBothDirectoryInformation>(&dir, "*").map_err(smb_error)?
            {
                let item = item.map_err(smb_error)?;
                let name = item.file_name.to_string();
                if name == "." || name == ".." {
                    continue;
                }
                let attributes = item.file_attributes;
                let mut entry = Entry::new(
                    name.clone().into(),
                    location.path().join(&name),
                    attributes.directory(),
                );
                if !attributes.directory() {
                    entry = entry.with_size(item.end_of_file);
                }
                if !item.last_write_time.is_zero() {
                    entry = entry.with_modified(SystemTime::from(item.last_write_time));
                }
                if attributes.hidden() {
                    entry = entry.with_hidden(true);
                }
                entries.push(entry);
            }
            Ok::<_, BackendError>(())
        })();
        let closed = dir.close().map_err(smb_error);
        result?;
        closed?;
        Ok(entries)
    }

    fn delete_tree(&self, location: &Location) -> Result<(), BackendError> {
        for entry in self.list_dir(location)? {
            let child = location.join(entry.name());
            if entry.is_dir {
                self.delete_tree(&child)?;
            } else {
                self.delete_one(&child)?;
            }
        }
        self.delete_one(location)
    }

    fn delete_one(&self, location: &Location) -> Result<(), BackendError> {
        let access = FileAccessMask::new()
            .with_delete(true)
            .with_file_read_attributes(true);
        let handle = self.open(location, &FileCreateArgs::make_open_existing(access))?;
        handle.with(|resource| handle_of(resource).set_info(FileDispositionInformation::default()))
    }
}

/// An open file or folder that is closed when dropped: the `smb` client only closes handles on
/// request in its blocking mode, and a handle left open keeps the file locked on the server.
struct Handle(Option<Resource>);

impl Handle {
    fn with<T>(mut self, op: impl FnOnce(&Resource) -> smb::Result<T>) -> Result<T, BackendError> {
        let resource = self.0.take().expect("open until consumed");
        let result = op(&resource).map_err(smb_error);
        let closed = handle_of(&resource).close().map_err(smb_error);
        let value = result?;
        closed?;
        Ok(value)
    }

    fn into_dir(mut self) -> Result<Directory, BackendError> {
        match self.0.take().expect("open until consumed") {
            Resource::Directory(dir) => Ok(dir),
            other => {
                let _ = handle_of(&other).close();
                Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder").into())
            }
        }
    }

    fn into_file(mut self) -> Result<smb::File, BackendError> {
        match self.0.take().expect("open until consumed") {
            Resource::File(file) => Ok(file),
            other => {
                let _ = handle_of(&other).close();
                Err(io::Error::new(io::ErrorKind::IsADirectory, "not a file").into())
            }
        }
    }
}

fn handle_of(resource: &Resource) -> &ResourceHandle {
    match resource {
        Resource::File(file) => file,
        Resource::Directory(dir) => dir,
        Resource::Pipe(pipe) => pipe,
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(resource) = self.0.take() {
            let _ = handle_of(&resource).close();
        }
    }
}

/// An open remote file as a blocking `Read` or `Write`, closed when dropped.
struct SmbFile(Option<smb::File>);

impl SmbFile {
    fn file(&mut self) -> &mut smb::File {
        self.0.as_mut().expect("open until dropped")
    }
}

impl Read for SmbFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file().read(buf)
    }
}

impl Write for SmbFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file().flush()
    }
}

impl Drop for SmbFile {
    fn drop(&mut self) {
        if let Some(file) = self.0.take() {
            let _ = file.flush();
            let _ = file.close();
        }
    }
}

/// Random-access reader over an SMB file (`smb::File::seek` in 0.12.1 does not update its
/// internal cursor, so this wrapper tracks `pos` explicitly and calls `read_block`).
struct SmbSeekFile {
    file: Option<smb::File>,
    pos: u64,
    len: u64,
}

impl Read for SmbSeekFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let file = self.file.as_ref().expect("open until dropped");
        let n = file.read_block(buf, self.pos, None, false)?;
        self.pos = self.pos.saturating_add(n as u64);
        Ok(n)
    }
}

impl Seek for SmbSeekFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target: i128 = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.pos) + i128::from(offset),
        };
        let next = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek position"))?;
        self.pos = next;
        Ok(self.pos)
    }
}

impl Drop for SmbSeekFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.close();
        }
    }
}

/// Concurrent positional reader over an SMB file (`smb::File::read_block` takes `&self` and
/// multiplexes requests by `MessageId` over the single connection).
struct SmbRangeSource {
    file: Option<Arc<smb::File>>,
    len: u64,
}

impl SmbRangeSource {
    fn read_chunk_filling(file: &smb::File, offset: u64, dst: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0usize;
        while filled < dst.len() {
            let n = file.read_block(
                &mut dst[filled..],
                offset.saturating_add(filled as u64),
                None,
                false,
            )?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        Ok(filled)
    }

    fn read_chunk_with_retry(file: &smb::File, offset: u64, dst: &mut [u8]) -> io::Result<usize> {
        let mut last_err = match Self::read_chunk_filling(file, offset, dst) {
            Ok(n) => return Ok(n),
            Err(err) => err,
        };
        for attempt in 1..=SMB_CHUNK_RETRY_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(100 * u64::from(attempt)));
            match Self::read_chunk_filling(file, offset, dst) {
                Ok(n) => return Ok(n),
                Err(err) => last_err = err,
            }
        }
        Err(last_err)
    }
}

impl RangeSource for SmbRangeSource {
    fn read_range(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Some(file) = self.file.as_ref() else {
            return Err(io::Error::other("SmbRangeSource already closed"));
        };
        let want = if offset >= self.len {
            0
        } else {
            (buf.len() as u64).min(self.len - offset) as u32
        };
        if want == 0 {
            return Ok(0);
        }

        let chunks = split_range(offset, want, SMB_CONCURRENT_CHUNK_SIZE);
        let mut total = 0usize;

        for batch in chunks.chunks(SMB_MAX_CONCURRENT_CHUNKS) {
            if batch.len() == 1 {
                let (chunk_offset, chunk_len) = batch[0];
                let start_in_buf = (chunk_offset - offset) as usize;
                let end_in_buf = start_in_buf + chunk_len as usize;
                let n = Self::read_chunk_with_retry(
                    file,
                    chunk_offset,
                    &mut buf[start_in_buf..end_in_buf],
                )?;
                total += n;
                if n < chunk_len as usize {
                    break;
                }
                continue;
            }

            let mut slot_bufs: Vec<Vec<u8>> = batch
                .iter()
                .map(|&(_, len)| vec![0u8; len as usize])
                .collect();
            let results: Vec<io::Result<usize>> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .zip(slot_bufs.iter_mut())
                    .map(|(&(chunk_offset, _), slot)| {
                        scope.spawn(move || Self::read_chunk_with_retry(file, chunk_offset, slot))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join().unwrap_or_else(|_| {
                            Err(io::Error::other("SMB concurrent read worker panicked"))
                        })
                    })
                    .collect()
            });

            for ((chunk_offset, chunk_len), (slot, res)) in
                batch.iter().zip(slot_bufs.into_iter().zip(results))
            {
                let n = res?;
                let start_in_buf = (chunk_offset - offset) as usize;
                buf[start_in_buf..start_in_buf + n].copy_from_slice(&slot[..n]);
                total += n;
                if n < *chunk_len as usize {
                    return Ok(total);
                }
            }
        }

        Ok(total)
    }

    fn len(&self) -> Option<u64> {
        Some(self.len)
    }
}

impl Drop for SmbRangeSource {
    fn drop(&mut self) {
        if let Some(arc) = self.file.take() {
            if let Ok(file) = Arc::try_unwrap(arc) {
                let _ = file.close();
            }
        }
    }
}

impl Backend for SmbBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        Ok(Listing::new(self.list_dir(location)?))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        let access = FileAccessMask::new().with_file_read_attributes(true);
        let handle = self.open(location, &FileCreateArgs::make_open_existing(access))?;
        let info =
            handle.with(|resource| handle_of(resource).query_info::<FileStandardInformation>())?;
        Ok(if bool::from(info.directory) {
            EntryMeta {
                kind: EntryKind::Dir,
                len: 0,
            }
        } else {
            EntryMeta {
                kind: EntryKind::File,
                len: info.end_of_file,
            }
        })
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        let access = FileAccessMask::new().with_generic_read(true);
        let file = self
            .open(location, &FileCreateArgs::make_open_existing(access))?
            .into_file()?;
        Ok(Box::new(SmbFile(Some(file))))
    }

    fn open_read_seek(&self, location: &Location) -> Result<Box<dyn ReadSeek>, BackendError> {
        let access = FileAccessMask::new().with_generic_read(true);
        let file = self
            .open(location, &FileCreateArgs::make_open_existing(access))?
            .into_file()?;
        let len = file.get_len().map_err(smb_error)?;
        Ok(Box::new(SmbSeekFile {
            file: Some(file),
            pos: 0,
            len,
        }))
    }

    fn open_range_source(&self, location: &Location) -> Result<Box<dyn RangeSource>, BackendError> {
        let access = FileAccessMask::new().with_generic_read(true);
        let file = self
            .open(location, &FileCreateArgs::make_open_existing(access))?
            .into_file()?;
        let len = file.get_len().map_err(smb_error)?;
        Ok(Box::new(SmbRangeSource {
            file: Some(Arc::new(file)),
            len,
        }))
    }

    fn stat_modified(&self, location: &Location) -> Result<Option<SystemTime>, BackendError> {
        let access = FileAccessMask::new().with_file_read_attributes(true);
        let handle = self.open(location, &FileCreateArgs::make_open_existing(access))?;
        let info =
            handle.with(|resource| handle_of(resource).query_info::<FileBasicInformation>())?;
        Ok((!info.last_write_time.is_zero()).then(|| SystemTime::from(info.last_write_time)))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        let args = FileCreateArgs::make_overwrite(
            FileAttributes::new(),
            CreateOptions::new().with_non_directory_file(true),
        );
        let file = self.open(location, &args)?.into_file()?;
        Ok(Box::new(SmbFile(Some(file))))
    }

    /// Renames within one share, replacing `to` when it is an existing file.
    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let (from_unc, _) = self.unc(from)?;
        let (to_unc, to_inner) = self.unc(to)?;
        if from_unc.share() != to_unc.share() || to_inner.is_empty() {
            return Err(BackendError::Unsupported);
        }
        let access = FileAccessMask::new()
            .with_delete(true)
            .with_file_read_attributes(true)
            .with_synchronize(true);
        let handle = self.open(from, &FileCreateArgs::make_open_existing(access))?;
        handle.with(|resource| {
            handle_of(resource).set_info(FileRenameInformation {
                replace_if_exists: true.into(),
                root_directory: 0,
                file_name: to_inner.as_str().into(),
            })
        })
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        let args = FileCreateArgs::make_create_new(
            FileAttributes::new().with_directory(true),
            CreateOptions::new().with_directory_file(true),
        );
        self.open(location, &args)?.with(|_| Ok(()))
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        match self.stat(location)?.kind {
            EntryKind::Dir => self.delete_tree(location),
            _ => self.delete_one(location),
        }
    }
}

fn share_of(path: &Path) -> Option<&str> {
    path.components().find_map(|part| match part {
        Component::Normal(name) => name.to_str(),
        _ => None,
    })
}

/// The normal components of a remote path, as UTF-8 (SMB names are UTF-16 on the wire).
fn components(path: &Path) -> io::Result<Vec<String>> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(
                name.to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("remote paths must be valid UTF-8")),
            ),
            _ => None,
        })
        .collect()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

/// Errors while connecting: a refused logon becomes [`AuthenticationFailed`], and a reply the
/// client cannot parse at negotiation is an SMB1-only server (it answers with the `\xFFSMB`
/// header this client never asks for).
fn connect_error(profile: &ConnectionProfile, err: smb::Error) -> BackendError {
    if status_of(&err) == Some(Status::U32_LOGON_FAILURE) {
        return AuthenticationFailed::error(profile).into();
    }
    if matches!(
        err,
        smb::Error::BinRWError(_) | smb::Error::NegotiationError(_)
    ) {
        return io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{}:{} did not negotiate SMB 2 or newer; SMB1 is not supported",
                profile.host, profile.port
            ),
        )
        .into();
    }
    smb_error(err)
}

fn status_of(err: &smb::Error) -> Option<u32> {
    match err {
        smb::Error::UnexpectedMessageStatus(code) | smb::Error::ReceivedErrorMessage(code, _) => {
            Some(*code)
        }
        _ => None,
    }
}

/// Maps `smb` failures to the [`io::ErrorKind`]s the rest of the crate reasons about, so a lost
/// connection is seen by [`is_dropped_connection`](crate::is_dropped_connection).
fn smb_error(err: smb::Error) -> BackendError {
    use io::ErrorKind::*;
    let kind = match &err {
        smb::Error::IoError(io) => io.kind(),
        smb::Error::ConnectionStopped => NotConnected,
        smb::Error::TransportError(smb::transport::TransportError::Timeout(_)) => TimedOut,
        smb::Error::TransportError(smb::transport::TransportError::NotConnected) => NotConnected,
        smb::Error::TransportError(smb::transport::TransportError::IoError(io)) => io.kind(),
        smb::Error::MissingPermissions(_) => PermissionDenied,
        smb::Error::UnsupportedOperation(_) => Unsupported,
        _ => match status_of(&err) {
            Some(Status::U32_OBJECT_NAME_NOT_FOUND)
            | Some(Status::U32_OBJECT_PATH_NOT_FOUND)
            | Some(Status::U32_BAD_NETWORK_NAME)
            | Some(Status::U32_NO_SUCH_DEVICE) => NotFound,
            Some(Status::U32_ACCESS_DENIED)
            | Some(Status::U32_LOGON_FAILURE)
            | Some(Status::U32_USER_ACCOUNT_LOCKED_OUT) => PermissionDenied,
            Some(Status::U32_OBJECT_NAME_COLLISION) => AlreadyExists,
            Some(Status::U32_DIRECTORY_NOT_EMPTY) => DirectoryNotEmpty,
            Some(Status::U32_FILE_IS_A_DIRECTORY) => IsADirectory,
            Some(Status::U32_SHARING_VIOLATION) => ResourceBusy,
            Some(Status::U32_IO_TIMEOUT) => TimedOut,
            Some(Status::U32_NETWORK_NAME_DELETED)
            | Some(Status::U32_USER_SESSION_DELETED)
            | Some(Status::U32_NETWORK_SESSION_EXPIRED) => ConnectionAborted,
            Some(Status::U32_NOT_SUPPORTED) | Some(Status::U32_NOT_IMPLEMENTED) => Unsupported,
            _ => Other,
        },
    };
    io::Error::new(kind, err.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_is_the_first_component_of_the_path() {
        assert_eq!(share_of(Path::new("/photos/2026")), Some("photos"));
        assert_eq!(share_of(Path::new("photos")), Some("photos"));
        assert_eq!(share_of(Path::new("/")), None);
    }

    #[test]
    fn statuses_map_to_the_error_kinds_the_crate_reasons_about() {
        let kind = |code| match smb_error(smb::Error::UnexpectedMessageStatus(code)) {
            BackendError::Io(err) => err.kind(),
            BackendError::Unsupported => unreachable!(),
        };
        assert_eq!(
            kind(Status::U32_OBJECT_NAME_NOT_FOUND),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            kind(Status::U32_OBJECT_NAME_COLLISION),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            kind(Status::U32_ACCESS_DENIED),
            io::ErrorKind::PermissionDenied
        );
        assert!(crate::is_dropped_connection(&BackendError::from(
            io::Error::from(kind(Status::U32_NETWORK_NAME_DELETED))
        )));
    }

    #[test]
    fn a_refused_logon_is_an_authentication_failure() {
        let profile = ConnectionProfile {
            id: "p".into(),
            name: "n".into(),
            protocol: Protocol::Smb,
            host: "h".into(),
            port: 445,
            user: "u".into(),
            start_path: "/share".into(),
            auth: crate::AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        };
        let err = connect_error(
            &profile,
            smb::Error::UnexpectedMessageStatus(Status::U32_LOGON_FAILURE),
        );
        assert!(err.authentication_failed().is_some());
    }
}
