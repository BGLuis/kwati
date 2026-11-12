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

/// Returns the currently mounted volumes, excluding pseudo filesystems.
pub fn list_volumes() -> Vec<Volume> {
    VolumeManager::new().volumes()
}

/// Alias for [`list_volumes`].
pub fn mounted_volumes() -> Vec<Volume> {
    list_volumes()
}
