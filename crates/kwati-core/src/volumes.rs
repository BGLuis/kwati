use std::path::{Path, PathBuf};

/// A mounted storage volume or drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    pub mount_point: PathBuf,
    pub total_space: u64,
    pub available_space: u64,
    pub free_space: u64,
    pub file_system: String,
    pub is_removable: bool,
    pub is_read_only: bool,
}

impl Volume {
    /// Creates a [`Volume`] from a [`sysinfo::Disk`].
    pub fn from_disk(disk: &sysinfo::Disk) -> Self {
        let name_lossy = disk.name().to_string_lossy();
        let name = if !name_lossy.is_empty() {
            name_lossy.to_string()
        } else {
            disk.mount_point()
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| disk.mount_point().to_string_lossy().to_string())
        };

        let file_system = disk.file_system().to_string_lossy().to_string();
        let total_space = disk.total_space();
        let available_space = disk.available_space();
        let is_removable = disk.is_removable();
        let is_read_only = disk.is_read_only();

        Self {
            name,
            mount_point: disk.mount_point().to_path_buf(),
            total_space,
            available_space,
            free_space: available_space,
            file_system,
            is_removable,
            is_read_only,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    pub fn total_space(&self) -> u64 {
        self.total_space
    }

    pub fn available_space(&self) -> u64 {
        self.available_space
    }

    pub fn free_space(&self) -> u64 {
        self.free_space
    }

    pub fn file_system(&self) -> &str {
        &self.file_system
    }

    pub fn is_removable(&self) -> bool {
        self.is_removable
    }

    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }
}

/// Checks if a filesystem type or mount point is a pseudo filesystem (e.g. proc, sysfs).
pub fn is_pseudo_filesystem(file_system: &str, mount_point: &Path) -> bool {
    let fs_lower = file_system.to_ascii_lowercase();
    const PSEUDO_FS: &[&str] = &[
        "proc",
        "procfs",
        "sysfs",
        "devtmpfs",
        "devpts",
        "cgroup",
        "cgroup2",
        "pstore",
        "bpf",
        "autofs",
        "securityfs",
        "configfs",
        "debugfs",
        "tracefs",
        "fusectl",
        "mqueue",
        "hugetlbfs",
        "ramfs",
        "binfmt_misc",
        "nsfs",
        "fuse.lxcfs",
    ];

    if PSEUDO_FS
        .iter()
        .any(|&p| fs_lower == p || fs_lower.starts_with(&format!("{}.", p)))
    {
        return true;
    }

    if mount_point == Path::new("/proc")
        || mount_point.starts_with("/proc/")
        || mount_point == Path::new("/sys")
        || mount_point.starts_with("/sys/")
        || mount_point == Path::new("/dev")
        || mount_point.starts_with("/dev/")
    {
        return true;
    }

    false
}

/// Filters a list of volumes, removing pseudo filesystems.
pub fn filter_volumes(volumes: impl IntoIterator<Item = Volume>) -> Vec<Volume> {
    volumes
        .into_iter()
        .filter(|v| !is_pseudo_filesystem(&v.file_system, &v.mount_point))
        .collect()
}

/// Manages querying and refreshing mounted volumes.
#[derive(Debug)]
pub struct VolumeManager {
    disks: sysinfo::Disks,
}

impl Default for VolumeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeManager {
    /// Creates a new [`VolumeManager`] with refreshed disks list.
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|err| {
            panic!("failed to initialize sysinfo disks: {}", err);
        })
    }

    /// Attempts to create a new [`VolumeManager`] with refreshed disks list.
    pub fn try_new() -> Result<Self, sysinfo::Error> {
        let disks = sysinfo::Disks::new_with_refreshed_list().or_else(|_| sysinfo::Disks::new())?;
        Ok(Self { disks })
    }

    /// Refreshes the mounted disks from the operating system.
    pub fn refresh(&mut self) {
        self.disks.refresh(true);
    }

    /// Returns the mounted volumes, excluding pseudo filesystems.
    pub fn volumes(&self) -> Vec<Volume> {
        self.disks
            .iter()
            .filter_map(|disk| {
                let fs = disk.file_system().to_string_lossy();
                let mount = disk.mount_point();
                if is_pseudo_filesystem(&fs, mount) {
                    None
                } else {
                    Some(Volume::from_disk(disk))
                }
            })
            .collect()
    }
}

/// Returns the currently mounted volumes, excluding pseudo filesystems. When the system's
/// disk list cannot be read the result is empty, so a sidebar still renders.
pub fn list_volumes() -> Vec<Volume> {
    VolumeManager::try_new()
        .map(|manager| manager.volumes())
        .unwrap_or_default()
}

/// Alias for [`list_volumes`].
pub fn mounted_volumes() -> Vec<Volume> {
    list_volumes()
}

/// Filesystems that hold no user data of their own: container layers, snap images, RAM disks
/// and the desktop's FUSE bridges.
const HIDDEN_FILE_SYSTEMS: &[&str] = &[
    "overlay",
    "squashfs",
    "tmpfs",
    "efivarfs",
    "fuse.portal",
    "fuse.kio-fuse",
];

/// Where the operating system mounts volumes for itself (boot partitions, btrfs subvolumes for
/// logs and caches, macOS system volumes).
const SYSTEM_MOUNT_PREFIXES: &[&str] = &[
    "/boot",
    "/efi",
    "/var",
    "/usr",
    "/snap",
    "/nix",
    "/System/Volumes",
    "/private/var",
];

/// The disks a user browses: local volumes without system mounts, and one entry per device,
/// at its shortest mount point.
pub fn device_volumes() -> Vec<Volume> {
    let labels = device_labels();
    user_facing_volumes(list_volumes())
        .into_iter()
        .map(|mut volume| {
            volume.name = display_name(&volume, &labels);
            volume
        })
        .collect()
}

/// Drops the volumes [`device_volumes`] hides. On Linux a volume's name is its device, so a
/// device mounted several times (btrfs subvolumes, bind mounts) is kept once, at the mount point
/// nearest the root.
pub fn user_facing_volumes(volumes: impl IntoIterator<Item = Volume>) -> Vec<Volume> {
    let mut shown: Vec<Volume> = Vec::new();
    for volume in volumes {
        let fs = volume.file_system.as_str();
        if HIDDEN_FILE_SYSTEMS.contains(&fs)
            || crate::volume::is_network_fs(fs)
            || SYSTEM_MOUNT_PREFIXES
                .iter()
                .any(|prefix| volume.mount_point.starts_with(prefix))
        {
            continue;
        }
        let same_device = volume
            .name
            .starts_with("/dev/")
            .then(|| shown.iter().position(|s| s.name == volume.name))
            .flatten();
        match same_device {
            Some(ix) => {
                let depth = |v: &Volume| v.mount_point.components().count();
                if depth(&volume) < depth(&shown[ix]) {
                    shown[ix] = volume;
                }
            }
            None => shown.push(volume),
        }
    }
    shown
}

/// The filesystem label of a Linux device, else the mount point's folder name. Other platforms
/// already name volumes by label.
fn display_name(volume: &Volume, labels: &[(PathBuf, String)]) -> String {
    if !volume.name.starts_with("/dev/") {
        return volume.name.clone();
    }
    let device = Path::new(&volume.name)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&volume.name));
    if let Some((_, label)) = labels.iter().find(|(target, _)| *target == device) {
        return label.clone();
    }
    match volume.mount_point.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "File System".to_string(),
    }
}

/// Devices and their filesystem labels, from the `/dev/disk/by-label` symlinks.
fn device_labels() -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir("/dev/disk/by-label") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let device = entry.path().canonicalize().ok()?;
            Some((device, unescape_hex(&entry.file_name().to_string_lossy())))
        })
        .collect()
}

/// udev writes characters that are unsafe in file names as `\xHH`, such as `\x20` for a space.
fn unescape_hex(name: &str) -> String {
    let mut out = Vec::with_capacity(name.len());
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let escaped = bytes
            .get(i..i + 4)
            .filter(|chunk| chunk.starts_with(b"\\x"))
            .and_then(|chunk| std::str::from_utf8(&chunk[2..]).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
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

/// A mounted network share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkMount {
    /// Where the share comes from, such as `//nas/media` or `media on nas`.
    pub name: String,
    pub mount_point: PathBuf,
    pub file_system: String,
}

/// The mounted network shares. On Linux they come from the mount table alone: asking a CIFS or
/// NFS mount for its capacity hangs while the server is unreachable.
pub fn network_mounts() -> Vec<NetworkMount> {
    #[cfg(target_os = "linux")]
    {
        let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return Vec::new();
        };
        let mut mounts = Vec::new();
        for mount in parse_network_mounts(&mountinfo) {
            if mount.file_system == GVFS_FS {
                mounts.extend(gvfs_shares(&mount.mount_point));
            } else {
                mounts.push(mount);
            }
        }
        mounts
    }
    #[cfg(not(target_os = "linux"))]
    {
        list_volumes()
            .into_iter()
            .filter(|v| crate::volume::is_network_fs(&v.file_system))
            .map(|v| NetworkMount {
                name: v.name,
                mount_point: v.mount_point,
                file_system: v.file_system,
            })
            .collect()
    }
}

/// GNOME mounts every share it opens below one FUSE mount, one folder per share.
const GVFS_FS: &str = "fuse.gvfsd-fuse";

/// The network mounts listed in `/proc/self/mountinfo` content, named by their source.
pub fn parse_network_mounts(mountinfo: &str) -> Vec<NetworkMount> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let mount_point = fields.nth(4)?;
            let mut after_separator = fields.skip_while(|field| *field != "-").skip(1);
            let fstype = after_separator.next()?;
            let source = after_separator.next().unwrap_or_default();
            crate::volume::is_network_fs(fstype).then(|| NetworkMount {
                name: crate::volume::unescape_octal(source),
                mount_point: PathBuf::from(crate::volume::unescape_octal(mount_point)),
                file_system: fstype.to_string(),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn gvfs_shares(root: &Path) -> Vec<NetworkMount> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            Some(NetworkMount {
                name: gvfs_share_name(&entry.file_name().to_string_lossy()),
                mount_point: entry.path(),
                file_system: GVFS_FS.to_string(),
            })
        })
        .collect()
}

/// Names a gvfs folder such as `smb-share:server=nas,share=media` as `media on nas`.
pub fn gvfs_share_name(folder: &str) -> String {
    let Some((_, params)) = folder.split_once(':') else {
        return folder.to_string();
    };
    let value = |key: &str| {
        params
            .split(',')
            .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
    };
    match (value("share"), value("server").or_else(|| value("host"))) {
        (Some(share), Some(server)) => format!("{share} on {server}"),
        (None, Some(server)) => server.to_string(),
        _ => folder.to_string(),
    }
}
