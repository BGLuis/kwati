use std::path::{Path, PathBuf};

/// Whether a path lives on a local disk or on a network mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    Local,
    Network,
    /// The mount could not be determined; callers must not assume it is local.
    Unknown,
}

/// Classifies the volume that holds `path`.
///
/// Returns [`VolumeKind::Unknown`] whenever the mount cannot be read (missing path,
/// unreadable mount table, unsupported platform), never [`VolumeKind::Local`].
pub fn volume_kind(path: &Path) -> VolumeKind {
    platform_kind(path)
}

/// The mount point that holds `path`, used to group paths that share one filesystem.
///
/// Falls back to the filesystem root of `path` when the mount table cannot be read, so every
/// path still gets a key.
pub(crate) fn mount_key(path: &Path) -> PathBuf {
    platform_mount(path).unwrap_or_else(|| path.ancestors().last().unwrap_or(path).to_path_buf())
}

#[cfg(target_os = "linux")]
fn platform_mount(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    linux_mount(&mountinfo, &path).map(|(mount_point, _)| PathBuf::from(mount_point))
}

#[cfg(target_os = "macos")]
fn platform_mount(path: &Path) -> Option<PathBuf> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let mount_point = unsafe { CStr::from_ptr(stat.f_mntonname.as_ptr()) };
    Some(PathBuf::from(mount_point.to_string_lossy().into_owned()))
}

#[cfg(windows)]
fn platform_mount(path: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let absolute = std::path::absolute(path).ok()?;
    match absolute.components().next()? {
        Component::Prefix(prefix) => Some(PathBuf::from(prefix.as_os_str())),
        _ => None,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_mount(_path: &Path) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "linux")]
fn platform_kind(path: &Path) -> VolumeKind {
    let Ok(path) = path.canonicalize() else {
        return VolumeKind::Unknown;
    };
    match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(mountinfo) => linux_kind(&mountinfo, &path),
        Err(_) => VolumeKind::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn platform_kind(path: &Path) -> VolumeKind {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return VolumeKind::Unknown;
    };
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return VolumeKind::Unknown;
    }
    let fstype = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) };
    macos_kind(&fstype.to_string_lossy())
}

#[cfg(windows)]
fn platform_kind(path: &Path) -> VolumeKind {
    use std::path::{Component, Prefix};
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_UNKNOWN,
    };

    if is_unc(&path.to_string_lossy()) {
        return VolumeKind::Network;
    }
    let Ok(absolute) = std::path::absolute(path) else {
        return VolumeKind::Unknown;
    };
    let Some(Component::Prefix(prefix)) = absolute.components().next() else {
        return VolumeKind::Unknown;
    };
    let letter = match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
        _ => return VolumeKind::Unknown,
    };
    let root: Vec<u16> = format!("{}:\\", letter as char)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    match unsafe { GetDriveTypeW(root.as_ptr()) } {
        DRIVE_REMOTE => VolumeKind::Network,
        DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR => VolumeKind::Unknown,
        _ => VolumeKind::Local,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_kind(_path: &Path) -> VolumeKind {
    VolumeKind::Unknown
}

const LINUX_NETWORK_FS: &[&str] = &[
    "cifs",
    "smb3",
    "nfs",
    "nfs4",
    "fuse.sshfs",
    "fuse.gvfsd-fuse",
];

const MACOS_NETWORK_FS: &[&str] = &["smbfs", "nfs", "afpfs", "webdav"];

/// True for the filesystem types Linux and macOS use for network mounts.
pub(crate) fn is_network_fs(fstype: &str) -> bool {
    LINUX_NETWORK_FS.contains(&fstype) || MACOS_NETWORK_FS.contains(&fstype)
}

/// Finds the mount point and filesystem type of `path` (already absolute and symlink-free) in
/// `/proc/self/mountinfo` content; the longest matching mount point wins, and later entries
/// shadow earlier ones.
#[cfg(any(target_os = "linux", test))]
fn linux_mount<'a>(mountinfo: &'a str, path: &Path) -> Option<(String, &'a str)> {
    let mut best: Option<(String, &str)> = None;
    for line in mountinfo.lines() {
        let mut fields = line.split(' ');
        let Some(mount_point) = fields.nth(4) else {
            continue;
        };
        let Some(fstype) = fields.skip_while(|field| *field != "-").nth(1) else {
            continue;
        };
        let mount_point = unescape_octal(mount_point);
        if path.starts_with(&mount_point)
            && best
                .as_ref()
                .is_none_or(|(best_point, _)| mount_point.len() >= best_point.len())
        {
            best = Some((mount_point, fstype));
        }
    }
    best
}

/// Classifies `path` (already absolute and symlink-free) against `/proc/self/mountinfo` content.
#[cfg(any(target_os = "linux", test))]
fn linux_kind(mountinfo: &str, path: &Path) -> VolumeKind {
    match linux_mount(mountinfo, path) {
        Some((_, fstype)) if LINUX_NETWORK_FS.contains(&fstype) => VolumeKind::Network,
        Some(_) => VolumeKind::Local,
        None => VolumeKind::Unknown,
    }
}

/// Decodes the `\ooo` octal escapes that the kernel uses for spaces, tabs, newlines
/// and backslashes in mountinfo paths.
pub(crate) fn unescape_octal(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)));
        match octal {
            Some(digits) => {
                let value = digits
                    .iter()
                    .fold(0u32, |acc, d| acc * 8 + u32::from(d - b'0'));
                out.push(value as u8);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(any(target_os = "macos", test))]
fn macos_kind(fstypename: &str) -> VolumeKind {
    if MACOS_NETWORK_FS.contains(&fstypename) {
        VolumeKind::Network
    } else {
        VolumeKind::Local
    }
}

/// True for `\\server\share` and `\\?\UNC\server\share` paths.
#[cfg(any(windows, test))]
fn is_unc(path: &str) -> bool {
    path.starts_with(r"\\?\UNC\")
        || (path.starts_with(r"\\") && !path.starts_with(r"\\?\") && !path.starts_with(r"\\.\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
24 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
30 24 0:27 / /mnt/nas rw,relatime shared:2 - cifs //nas/share rw,vers=3.1.1
31 30 0:28 / /mnt/nas/local\\040disk rw shared:3 - ext4 /dev/sdb1 rw
32 24 0:29 / /mnt/sshfs rw - fuse.sshfs me@host:/ rw
";

    #[test]
    fn volume_kind_linux() {
        let nas = Path::new("/mnt/nas/docs/a.txt");
        assert_eq!(linux_kind(MOUNTINFO, nas), VolumeKind::Network);
        assert_eq!(linux_kind(MOUNTINFO, Path::new("/")), VolumeKind::Local);
        assert_eq!(
            linux_kind(MOUNTINFO, Path::new("/home/user")),
            VolumeKind::Local
        );
        assert_eq!(
            linux_kind(MOUNTINFO, Path::new("/mnt/sshfs/x")),
            VolumeKind::Network
        );
        assert_eq!(
            linux_kind(MOUNTINFO, Path::new("/mnt/nas2/x")),
            VolumeKind::Local,
            "a sibling that only shares a name prefix is not below the mount"
        );
    }

    #[test]
    fn volume_kind_linux_longest_mount_wins_and_unescapes() {
        let nested = Path::new("/mnt/nas/local disk/f");
        assert_eq!(linux_kind(MOUNTINFO, nested), VolumeKind::Local);
    }

    #[test]
    fn mount_point_is_the_longest_match() {
        let mount = |path: &str| linux_mount(MOUNTINFO, Path::new(path)).map(|(point, _)| point);
        assert_eq!(mount("/mnt/nas/docs/a.txt").as_deref(), Some("/mnt/nas"));
        assert_eq!(
            mount("/mnt/nas/local disk/f").as_deref(),
            Some("/mnt/nas/local disk")
        );
        assert_eq!(mount("/home/user").as_deref(), Some("/"));
        assert_eq!(linux_mount("", Path::new("/x")), None);
    }

    #[test]
    fn volume_kind_macos_table() {
        assert_eq!(macos_kind("smbfs"), VolumeKind::Network);
        assert_eq!(macos_kind("nfs"), VolumeKind::Network);
        assert_eq!(macos_kind("afpfs"), VolumeKind::Network);
        assert_eq!(macos_kind("webdav"), VolumeKind::Network);
        assert_eq!(macos_kind("apfs"), VolumeKind::Local);
    }

    #[test]
    fn volume_kind_unc_prefixes() {
        assert!(is_unc(r"\\server\share"));
        assert!(is_unc(r"\\?\UNC\server\share\dir"));
        assert!(!is_unc(r"C:\Users"));
        assert!(!is_unc(r"\\?\C:\Users"));
        assert!(!is_unc(r"\\.\PhysicalDrive0"));
    }

    #[test]
    fn volume_kind_unknown_without_matching_mount() {
        assert_eq!(linux_kind("", Path::new("/x")), VolumeKind::Unknown);
        assert_eq!(
            linux_kind("garbage line", Path::new("/x")),
            VolumeKind::Unknown
        );
    }
}
