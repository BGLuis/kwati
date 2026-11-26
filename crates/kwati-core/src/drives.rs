use std::fmt;
use std::path::{Path, PathBuf};

/// An unmounted removable filesystem or partition that can be mounted from the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmountedDrive {
    /// Backend-specific identifier passed back to [`mount`].
    pub id: String,
    /// Identifier of the parent physical drive, when known.
    pub drive_id: Option<String>,
    /// User-facing label or fallback device name.
    pub name: String,
    /// Device node or OS volume path (for example `/dev/sdb1` or `/dev/disk2s1`).
    pub device: String,
    /// Filesystem format reported by the OS (for example `exfat`, `vfat`, `ntfs`, `apfs`).
    pub file_system: String,
    /// Total partition or volume capacity in bytes, or `0` when unavailable.
    pub total_space: u64,
}

/// Errors returned when listing, mounting, or safely ejecting removable drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveError {
    /// A process or open handle is using the volume; never forced.
    Busy(String),
    /// The caller lacks permission or desktop authorization was denied/dismissed.
    PermissionDenied(String),
    /// The target drive or mount point could not be found.
    NotFound(String),
    /// The operation is not supported on this platform or device.
    Unsupported(String),
    /// Underlying OS/daemon failure that does not fit a more specific category.
    Backend(String),
}

impl fmt::Display for DriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(msg) => write!(f, "device is busy: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            Self::NotFound(msg) => write!(f, "drive not found: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Self::Backend(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DriveError {}

/// Lists unmounted removable filesystems and partitions eligible to be mounted.
pub fn unmounted_drives() -> Result<Vec<UnmountedDrive>, DriveError> {
    platform::unmounted_drives()
}

/// Mounts an unmounted removable drive and returns its resulting mount point.
pub fn mount(drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
    platform::mount(drive)
}

/// Cleanly unmounts and safely ejects/powers off the removable drive mounted at `mount_point`.
///
/// Never forces an unmount or eject when a file or working directory is still in use; returns
/// [`DriveError::Busy`] instead.
pub fn eject(mount_point: &Path) -> Result<(), DriveError> {
    platform::eject(mount_point)
}

#[doc(hidden)]
pub mod detail {
    use super::{DriveError, UnmountedDrive};
    use serde::Deserialize;

    /// Parsed metadata from `diskutil info -plist` converted to JSON.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DiskutilInfo {
        pub device_identifier: String,
        pub parent_whole_disk: Option<String>,
        pub mount_point: Option<String>,
        pub file_system: String,
        pub volume_name: String,
        pub total_size: u64,
    }

    /// Decodes a NUL-terminated byte array (as returned by UDisks2 `Block.Device` and
    /// `Filesystem.MountPoints`) into a Rust `String`.
    pub fn decode_cstr_bytes(bytes: &[u8]) -> String {
        let trimmed = bytes.split(|&b| b == 0).next().unwrap_or(bytes);
        String::from_utf8_lossy(trimmed).into_owned()
    }

    /// Maps a UDisks2 / D-Bus error name and detail message into [`DriveError`].
    pub fn map_udisks_error_name(err_name: &str, detail: String) -> DriveError {
        match err_name {
            "org.freedesktop.UDisks2.Error.DeviceBusy" => DriveError::Busy(detail),
            "org.freedesktop.UDisks2.Error.NotAuthorized"
            | "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain"
            | "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed" => {
                DriveError::PermissionDenied(detail)
            }
            "org.freedesktop.UDisks2.Error.NotSupported" => DriveError::Unsupported(detail),
            "org.freedesktop.UDisks2.Error.DoesNotExist"
            | "org.freedesktop.DBus.Error.UnknownObject" => DriveError::NotFound(detail),
            _ if detail.is_empty() => DriveError::Backend(err_name.to_string()),
            _ => DriveError::Backend(format!("{err_name}: {detail}")),
        }
    }

    /// Options passed to UDisks2 `Filesystem.Unmount` — always `force = false`.
    pub fn udisks2_unmount_options() -> Vec<(&'static str, bool)> {
        vec![("force", false)]
    }

    /// Arguments passed to `/usr/sbin/diskutil` to mount a volume without forcing.
    pub fn diskutil_mount_args(disk_id: &str) -> [&str; 2] {
        ["mount", disk_id]
    }

    /// Arguments passed to `/usr/sbin/diskutil` to unmount a volume without `force`.
    pub fn diskutil_unmount_args(target: &str) -> [&str; 2] {
        ["unmount", target]
    }

    /// Arguments passed to `/usr/sbin/diskutil` to eject a whole disk without `force`.
    pub fn diskutil_eject_args(whole_disk: &str) -> [&str; 2] {
        ["eject", whole_disk]
    }

    /// Classifies `diskutil` failure output into [`DriveError`].
    pub fn classify_diskutil_error(stderr: &str, stdout: &str) -> DriveError {
        let combined = match (stderr.trim(), stdout.trim()) {
            ("", "") => String::from("diskutil command failed"),
            (err, "") => err.to_string(),
            ("", out) => out.to_string(),
            (err, out) => format!("{err}; {out}"),
        };
        let lower = combined.to_ascii_lowercase();

        if lower.contains("dissented")
            || lower.contains("busy")
            || lower.contains("in use")
            || lower.contains("could not be unmounted")
            || lower.contains("failed to unmount")
        {
            DriveError::Busy(combined)
        } else if lower.contains("permission denied")
            || lower.contains("not permitted")
            || lower.contains("operation not permitted")
            || lower.contains("authorization")
        {
            DriveError::PermissionDenied(combined)
        } else if lower.contains("unable to find disk")
            || lower.contains("not found")
            || lower.contains("could not find disk")
            || lower.contains("no such file")
        {
            DriveError::NotFound(combined)
        } else {
            DriveError::Backend(combined)
        }
    }

    #[derive(Debug, Deserialize)]
    struct DiskutilListJson {
        #[serde(rename = "AllDisksAndPartitions", default)]
        all_disks_and_partitions: Vec<DiskutilDiskJson>,
    }

    #[derive(Debug, Deserialize)]
    struct DiskutilDiskJson {
        #[serde(rename = "DeviceIdentifier", default)]
        device_identifier: String,
        #[serde(rename = "Content", default)]
        content: Option<String>,
        #[serde(rename = "Size", default)]
        size: Option<u64>,
        #[serde(rename = "MountPoint", default)]
        mount_point: Option<String>,
        #[serde(rename = "VolumeName", default)]
        volume_name: Option<String>,
        #[serde(rename = "OSInternal", default)]
        os_internal: Option<bool>,
        #[serde(rename = "Partitions", default)]
        partitions: Vec<DiskutilPartitionJson>,
        #[serde(rename = "APFSVolumes", default)]
        apfs_volumes: Vec<DiskutilPartitionJson>,
    }

    #[derive(Debug, Deserialize)]
    struct DiskutilPartitionJson {
        #[serde(rename = "DeviceIdentifier", default)]
        device_identifier: String,
        #[serde(rename = "Content", default)]
        content: Option<String>,
        #[serde(rename = "Size", default)]
        size: Option<u64>,
        #[serde(rename = "CapacityInUse", default)]
        capacity_in_use: Option<u64>,
        #[serde(rename = "MountPoint", default)]
        mount_point: Option<String>,
        #[serde(rename = "VolumeName", default)]
        volume_name: Option<String>,
        #[serde(rename = "OSInternal", default)]
        os_internal: Option<bool>,
    }

    #[derive(Debug, Deserialize)]
    struct DiskutilInfoJson {
        #[serde(rename = "DeviceIdentifier", default)]
        device_identifier: String,
        #[serde(rename = "ParentWholeDisk", default)]
        parent_whole_disk: Option<String>,
        #[serde(rename = "MountPoint", default)]
        mount_point: Option<String>,
        #[serde(rename = "FilesystemType", default)]
        filesystem_type: Option<String>,
        #[serde(rename = "Content", default)]
        content: Option<String>,
        #[serde(rename = "VolumeName", default)]
        volume_name: Option<String>,
        #[serde(rename = "TotalSize", default)]
        total_size: Option<u64>,
        #[serde(rename = "Size", default)]
        size: Option<u64>,
    }

    fn is_non_mountable_content(content: &str) -> bool {
        matches!(
            content,
            "" | "EFI"
                | "GUID_partition_scheme"
                | "FDisk_partition_scheme"
                | "Apple_partition_scheme"
                | "Apple_partition_map"
                | "Apple_APFS"
                | "Apple_APFS_Recovery"
                | "Apple_APFS_ISC"
                | "Apple_Boot"
                | "Apple_KernelCoreDump"
                | "Microsoft Reserved"
        )
    }

    fn is_unmounted_mount_point(mount_point: Option<&str>) -> bool {
        mount_point.is_none_or(|m| m.trim().is_empty())
    }

    fn build_macos_drive(
        id: String,
        whole_disk: &str,
        volume_name: Option<String>,
        content: Option<String>,
        size: u64,
    ) -> Option<UnmountedDrive> {
        if id.is_empty() {
            return None;
        }
        let fs = content.unwrap_or_default();
        if is_non_mountable_content(fs.as_str()) {
            return None;
        }
        let device = format!("/dev/{id}");
        let name = volume_name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.clone());
        Some(UnmountedDrive {
            id,
            drive_id: (!whole_disk.is_empty()).then(|| whole_disk.to_string()),
            name,
            device,
            file_system: fs,
            total_space: size,
        })
    }

    /// Parses JSON produced by `/usr/sbin/diskutil list -plist external | /usr/bin/plutil -convert json -o - -`.
    pub fn parse_diskutil_list_json(json: &str) -> Result<Vec<UnmountedDrive>, DriveError> {
        let parsed: DiskutilListJson = serde_json::from_str(json)
            .map_err(|e| DriveError::Backend(format!("invalid diskutil list JSON: {e}")))?;

        let mut drives = Vec::new();
        for disk in parsed.all_disks_and_partitions {
            if disk.os_internal == Some(true) {
                continue;
            }
            let whole_disk = disk.device_identifier.clone();

            if disk.partitions.is_empty() && disk.apfs_volumes.is_empty() {
                if is_unmounted_mount_point(disk.mount_point.as_deref()) {
                    if let Some(entry) = build_macos_drive(
                        disk.device_identifier,
                        &whole_disk,
                        disk.volume_name,
                        disk.content,
                        disk.size.unwrap_or(0),
                    ) {
                        drives.push(entry);
                    }
                }
                continue;
            }

            for part in disk.partitions {
                if part.os_internal == Some(true)
                    || !is_unmounted_mount_point(part.mount_point.as_deref())
                {
                    continue;
                }
                if let Some(entry) = build_macos_drive(
                    part.device_identifier,
                    &whole_disk,
                    part.volume_name,
                    part.content,
                    part.size.or(part.capacity_in_use).unwrap_or(0),
                ) {
                    drives.push(entry);
                }
            }

            for vol in disk.apfs_volumes {
                if vol.os_internal == Some(true)
                    || !is_unmounted_mount_point(vol.mount_point.as_deref())
                {
                    continue;
                }
                let fs = vol
                    .content
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| "apfs".to_string());
                if let Some(entry) = build_macos_drive(
                    vol.device_identifier,
                    &whole_disk,
                    vol.volume_name,
                    Some(fs),
                    vol.size.or(vol.capacity_in_use).unwrap_or(0),
                ) {
                    drives.push(entry);
                }
            }
        }

        drives.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
        Ok(drives)
    }

    /// Parses JSON produced by `/usr/sbin/diskutil info -plist <target> | /usr/bin/plutil -convert json -o - -`.
    pub fn parse_diskutil_info_json(json: &str) -> Result<DiskutilInfo, DriveError> {
        let info: DiskutilInfoJson = serde_json::from_str(json)
            .map_err(|e| DriveError::Backend(format!("invalid diskutil info JSON: {e}")))?;

        Ok(DiskutilInfo {
            device_identifier: info.device_identifier,
            parent_whole_disk: info.parent_whole_disk.filter(|s| !s.is_empty()),
            mount_point: info.mount_point.filter(|s| !s.trim().is_empty()),
            file_system: info.filesystem_type.or(info.content).unwrap_or_default(),
            volume_name: info.volume_name.unwrap_or_default(),
            total_size: info.total_size.or(info.size).unwrap_or(0),
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::detail::{decode_cstr_bytes, map_udisks_error_name, udisks2_unmount_options};
    use super::{DriveError, UnmountedDrive};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use zbus::blocking::fdo::ObjectManagerProxy;
    use zbus::blocking::Connection;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

    const UDISKS2_SERVICE: &str = "org.freedesktop.UDisks2";
    const UDISKS2_PATH: &str = "/org/freedesktop/UDisks2";
    const IFACE_DRIVE: &str = "org.freedesktop.UDisks2.Drive";
    const IFACE_BLOCK: &str = "org.freedesktop.UDisks2.Block";
    const IFACE_FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";

    #[derive(Debug, Clone)]
    struct DriveInfo {
        object_path: String,
        model: String,
        vendor: String,
        connection_bus: String,
        removable: bool,
        media_removable: bool,
        ejectable: bool,
        can_power_off: bool,
    }

    impl DriveInfo {
        /// USB-attached SSDs/HDDs often report `Removable = false` and `MediaRemovable = false`
        /// while setting `ConnectionBus = "usb"` and `CanPowerOff = true`.
        fn is_removable_candidate(&self) -> bool {
            self.removable || self.media_removable || self.connection_bus == "usb"
        }

        fn display_label(&self) -> String {
            match (self.vendor.trim(), self.model.trim()) {
                ("", "") => String::new(),
                (v, "") => v.to_string(),
                ("", m) => m.to_string(),
                (v, m) => format!("{v} {m}"),
            }
        }
    }

    #[derive(Debug, Clone)]
    struct FilesystemPartition {
        object_path: String,
        device: String,
        label: String,
        id_usage: String,
        fs_type: String,
        size: u64,
        hint_system: bool,
        hint_ignore: bool,
        mount_points: Vec<String>,
        drive_path: Option<String>,
        drive: Option<DriveInfo>,
    }

    impl FilesystemPartition {
        fn is_mounted(&self) -> bool {
            !self.mount_points.is_empty()
        }

        fn is_user_removable(&self) -> bool {
            if self.hint_ignore {
                return false;
            }
            let drive_removable = self
                .drive
                .as_ref()
                .is_some_and(DriveInfo::is_removable_candidate);
            drive_removable || !self.hint_system
        }
    }

    fn map_udisks_error(err: zbus::Error) -> DriveError {
        match err {
            zbus::Error::MethodError(name, detail, _) => {
                map_udisks_error_name(name.as_str(), detail.unwrap_or_default())
            }
            other => DriveError::Backend(other.to_string()),
        }
    }

    fn map_fdo_error(err: zbus::fdo::Error) -> DriveError {
        match err {
            zbus::fdo::Error::ZBus(inner) => map_udisks_error(inner),
            other => DriveError::Backend(other.to_string()),
        }
    }

    fn prop_str(props: &HashMap<String, OwnedValue>, key: &str) -> String {
        props
            .get(key)
            .and_then(|v| <&str>::try_from(v).ok())
            .unwrap_or_default()
            .to_string()
    }

    fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
        props
            .get(key)
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false)
    }

    fn prop_u64(props: &HashMap<String, OwnedValue>, key: &str) -> u64 {
        props
            .get(key)
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    }

    fn prop_bytes_cstr(props: &HashMap<String, OwnedValue>, key: &str) -> String {
        props
            .get(key)
            .and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
            .map(|b| decode_cstr_bytes(&b))
            .unwrap_or_default()
    }

    fn prop_mount_points(props: &HashMap<String, OwnedValue>) -> Vec<String> {
        props
            .get("MountPoints")
            .and_then(|v| Vec::<Vec<u8>>::try_from(v.clone()).ok())
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|b| decode_cstr_bytes(&b))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn prop_object_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
        let path = props
            .get(key)
            .and_then(|v| OwnedObjectPath::try_from(v.clone()).ok())?;
        let s = path.as_str();
        if s == "/" {
            None
        } else {
            Some(s.to_string())
        }
    }

    fn list_udisks2_inventory(conn: &Connection) -> Result<Vec<FilesystemPartition>, DriveError> {
        let manager = ObjectManagerProxy::builder(conn)
            .destination(UDISKS2_SERVICE)
            .map_err(map_udisks_error)?
            .path(UDISKS2_PATH)
            .map_err(map_udisks_error)?
            .build()
            .map_err(map_udisks_error)?;

        let objects = manager.get_managed_objects().map_err(map_fdo_error)?;

        let mut drives: HashMap<String, DriveInfo> = HashMap::new();
        for (path, ifaces) in &objects {
            if let Some(drive_props) = ifaces.get(IFACE_DRIVE) {
                drives.insert(
                    path.to_string(),
                    DriveInfo {
                        object_path: path.to_string(),
                        model: prop_str(drive_props, "Model"),
                        vendor: prop_str(drive_props, "Vendor"),
                        connection_bus: prop_str(drive_props, "ConnectionBus"),
                        removable: prop_bool(drive_props, "Removable"),
                        media_removable: prop_bool(drive_props, "MediaRemovable"),
                        ejectable: prop_bool(drive_props, "Ejectable"),
                        can_power_off: prop_bool(drive_props, "CanPowerOff"),
                    },
                );
            }
        }

        let mut partitions = Vec::new();
        for (path, ifaces) in &objects {
            let Some(block_props) = ifaces.get(IFACE_BLOCK) else {
                continue;
            };
            let Some(fs_props) = ifaces.get(IFACE_FILESYSTEM) else {
                continue;
            };

            let drive_path = prop_object_path(block_props, "Drive");
            let drive = drive_path.as_ref().and_then(|p| drives.get(p).cloned());

            partitions.push(FilesystemPartition {
                object_path: path.to_string(),
                device: prop_bytes_cstr(block_props, "Device"),
                label: prop_str(block_props, "IdLabel"),
                id_usage: prop_str(block_props, "IdUsage"),
                fs_type: prop_str(block_props, "IdType"),
                size: prop_u64(block_props, "Size"),
                hint_system: prop_bool(block_props, "HintSystem"),
                hint_ignore: prop_bool(block_props, "HintIgnore"),
                mount_points: prop_mount_points(fs_props),
                drive_path,
                drive,
            });
        }

        partitions.sort_by(|a, b| a.device.cmp(&b.device));
        Ok(partitions)
    }

    pub fn unmounted_drives() -> Result<Vec<UnmountedDrive>, DriveError> {
        let conn = Connection::system().map_err(map_udisks_error)?;
        let partitions = list_udisks2_inventory(&conn)?;

        let mut unmounted: Vec<UnmountedDrive> = partitions
            .into_iter()
            .filter(|p| p.id_usage == "filesystem" && p.is_user_removable() && !p.is_mounted())
            .map(|p| {
                let name = if !p.label.trim().is_empty() {
                    p.label.clone()
                } else {
                    p.drive
                        .as_ref()
                        .map(DriveInfo::display_label)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| p.device.clone())
                };
                UnmountedDrive {
                    id: p.object_path,
                    drive_id: p.drive_path,
                    name,
                    device: p.device,
                    file_system: p.fs_type,
                    total_space: p.size,
                }
            })
            .collect();

        unmounted.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
        Ok(unmounted)
    }

    pub fn mount(drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        let path = ObjectPath::try_from(drive.id.as_str())
            .map_err(|e| DriveError::NotFound(format!("invalid object path {}: {e}", drive.id)))?;
        let conn = Connection::system().map_err(map_udisks_error)?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();

        let reply = conn
            .call_method(
                Some(UDISKS2_SERVICE),
                path,
                Some(IFACE_FILESYSTEM),
                "Mount",
                &(options,),
            )
            .map_err(map_udisks_error)?;

        let mount_point = reply
            .body()
            .deserialize::<String>()
            .map_err(map_udisks_error)?;
        Ok(PathBuf::from(mount_point))
    }

    fn unmount_filesystem(conn: &Connection, block_object_path: &str) -> Result<(), DriveError> {
        let path = ObjectPath::try_from(block_object_path)
            .map_err(|e| DriveError::Backend(format!("invalid object path: {e}")))?;

        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        for (key, value) in udisks2_unmount_options() {
            options.insert(key, Value::Bool(value));
        }

        conn.call_method(
            Some(UDISKS2_SERVICE),
            path,
            Some(IFACE_FILESYSTEM),
            "Unmount",
            &(options,),
        )
        .map_err(map_udisks_error)?;

        Ok(())
    }

    fn normalize_path(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub fn eject(mount_point: &Path) -> Result<(), DriveError> {
        let target_norm = normalize_path(mount_point);
        let conn = Connection::system().map_err(map_udisks_error)?;
        let partitions = list_udisks2_inventory(&conn)?;

        let matched = partitions.iter().find(|p| {
            p.mount_points.iter().any(|mp| {
                let mp_path = Path::new(mp);
                mp_path == mount_point || normalize_path(mp_path) == target_norm
            })
        });

        let Some(matched) = matched else {
            return Err(DriveError::NotFound(format!(
                "no mounted UDisks2 volume found at {}",
                mount_point.display()
            )));
        };

        let Some(drive_path) = matched.drive_path.as_deref() else {
            return unmount_filesystem(&conn, &matched.object_path);
        };

        for part in partitions
            .iter()
            .filter(|p| p.drive_path.as_deref() == Some(drive_path) && p.is_mounted())
        {
            unmount_filesystem(&conn, &part.object_path)?;
        }

        let Some(drive) = matched.drive.as_ref() else {
            return Ok(());
        };

        let obj_path = ObjectPath::try_from(drive.object_path.as_str())
            .map_err(|e| DriveError::Backend(format!("invalid drive object path: {e}")))?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();

        let mut ejected = false;
        if drive.ejectable {
            conn.call_method(
                Some(UDISKS2_SERVICE),
                obj_path.clone(),
                Some(IFACE_DRIVE),
                "Eject",
                &(options.clone(),),
            )
            .map_err(map_udisks_error)?;
            ejected = true;
        }

        if drive.can_power_off {
            match conn.call_method(
                Some(UDISKS2_SERVICE),
                obj_path,
                Some(IFACE_DRIVE),
                "PowerOff",
                &(options,),
            ) {
                Ok(_) => {}
                // Some drives disappear from UDisks2 immediately after Eject succeeds.
                Err(err) if ejected => match map_udisks_error(err) {
                    DriveError::NotFound(_) => {}
                    other => return Err(other),
                },
                Err(err) => return Err(map_udisks_error(err)),
            }
        }

        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::{DriveError, UnmountedDrive};
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::{addr_of_mut, null, null_mut};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Parent, CM_Request_Device_EjectW, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        CR_REMOVE_VETOED, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
        PNP_VETO_TYPE, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_BUSY, ERROR_BUSY_DRIVE,
        ERROR_DRIVE_LOCKED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_LOCKED,
        ERROR_LOCK_FAILED, ERROR_LOCK_VIOLATION, ERROR_MORE_DATA, ERROR_NOT_SUPPORTED,
        ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WIN32_ERROR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BusTypeUsb, CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        GetVolumePathNamesForVolumeNameW, SetVolumeMountPointW, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
        GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_GET_DEVICE_NUMBER,
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
        STORAGE_PROPERTY_QUERY,
    };
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOVABLE;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: `self.0` is an open Win32 handle owned by this guard.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct DevInfoGuard(HDEVINFO);

    impl Drop for DevInfoGuard {
        fn drop(&mut self) {
            if self.0 != INVALID_HANDLE_VALUE as isize {
                // SAFETY: `self.0` is a valid device information set returned by `SetupDiGetClassDevsW`.
                unsafe {
                    SetupDiDestroyDeviceInfoList(self.0);
                }
            }
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn from_wide_nul(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn map_win32_error(code: WIN32_ERROR, context: &str) -> DriveError {
        match code {
            ERROR_LOCK_VIOLATION
            | ERROR_SHARING_VIOLATION
            | ERROR_DRIVE_LOCKED
            | ERROR_BUSY
            | ERROR_BUSY_DRIVE
            | ERROR_LOCKED
            | ERROR_LOCK_FAILED => DriveError::Busy(format!("{context} (win32 error {code})")),
            ERROR_ACCESS_DENIED => {
                DriveError::PermissionDenied(format!("{context} (win32 error {code})"))
            }
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => {
                DriveError::NotFound(format!("{context} (win32 error {code})"))
            }
            _ => DriveError::Backend(format!("{context} (win32 error {code})")),
        }
    }

    fn map_lock_error(code: WIN32_ERROR, context: &str) -> DriveError {
        match code {
            ERROR_LOCK_VIOLATION
            | ERROR_SHARING_VIOLATION
            | ERROR_ACCESS_DENIED
            | ERROR_DRIVE_LOCKED
            | ERROR_BUSY
            | ERROR_BUSY_DRIVE
            | ERROR_LOCKED
            | ERROR_LOCK_FAILED => DriveError::Busy(format!("{context} (win32 error {code})")),
            _ => map_win32_error(code, context),
        }
    }

    fn volume_has_mount_points(volume_w: &[u16]) -> Result<bool, DriveError> {
        let mut buf = vec![0u16; 512];
        let mut required = 0u32;
        // SAFETY: `volume_w` is NUL-terminated and `buf` is a writable UTF-16 buffer of `buf.len()`.
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                volume_w.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut required,
            )
        };
        if ok == 0 {
            // SAFETY: `GetLastError` reads the calling thread's last-error value.
            let err = unsafe { GetLastError() };
            if err == ERROR_MORE_DATA && required > 0 {
                buf.resize(required as usize, 0);
                // SAFETY: `buf` was resized to `required` UTF-16 code units.
                let ok_retry = unsafe {
                    GetVolumePathNamesForVolumeNameW(
                        volume_w.as_ptr(),
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut required,
                    )
                };
                if ok_retry == 0 {
                    // SAFETY: reads thread-local last error.
                    return Err(map_win32_error(
                        unsafe { GetLastError() },
                        "GetVolumePathNamesForVolumeNameW",
                    ));
                }
            } else {
                return Err(map_win32_error(err, "GetVolumePathNamesForVolumeNameW"));
            }
        }
        Ok(buf.first().copied().unwrap_or(0) != 0)
    }

    fn query_storage_info(device_no_slash: &str) -> (bool, Option<u32>) {
        let wide = to_wide(device_no_slash);
        // SAFETY: `wide` is NUL-terminated; opening with desired access `0` queries device properties.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return (false, None);
        }
        let _guard = HandleGuard(handle);

        let mut query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut desc_buf = [0u8; 1024];
        let mut bytes_returned = 0u32;
        // SAFETY: `query` and `desc_buf` are valid buffers of their stated byte sizes.
        let prop_ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&mut query as *mut STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                desc_buf.as_mut_ptr().cast(),
                desc_buf.len() as u32,
                &mut bytes_returned,
                null_mut(),
            )
        };
        let is_removable = if prop_ok != 0
            && (bytes_returned as usize) >= std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>()
        {
            // SAFETY: `bytes_returned` covers `STORAGE_DEVICE_DESCRIPTOR`; read unaligned for safety.
            let desc = unsafe {
                desc_buf
                    .as_ptr()
                    .cast::<STORAGE_DEVICE_DESCRIPTOR>()
                    .read_unaligned()
            };
            desc.RemovableMedia || desc.BusType == BusTypeUsb
        } else {
            false
        };

        let dev_number = query_device_number(handle);
        (is_removable, dev_number)
    }

    fn query_device_number(handle: HANDLE) -> Option<u32> {
        let mut dev_num = STORAGE_DEVICE_NUMBER {
            DeviceType: 0,
            DeviceNumber: 0,
            PartitionNumber: 0,
        };
        let mut bytes_returned = 0u32;
        // SAFETY: `dev_num` is a valid mutable `STORAGE_DEVICE_NUMBER` buffer.
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                null(),
                0,
                (&mut dev_num as *mut STORAGE_DEVICE_NUMBER).cast(),
                std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                &mut bytes_returned,
                null_mut(),
            )
        };
        (ok != 0).then_some(dev_num.DeviceNumber)
    }

    fn query_volume_metadata(volume_w: &[u16]) -> Option<(String, String, u64)> {
        let mut label_buf = [0u16; 261];
        let mut fs_buf = [0u16; 64];
        // SAFETY: `volume_w` is NUL-terminated; output buffers are sized appropriately.
        let ok = unsafe {
            GetVolumeInformationW(
                volume_w.as_ptr(),
                label_buf.as_mut_ptr(),
                label_buf.len() as u32,
                null_mut(),
                null_mut(),
                null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let fs_name = from_wide_nul(&fs_buf);
        if fs_name.is_empty() {
            return None;
        }
        let label = from_wide_nul(&label_buf);

        let mut total_bytes = 0u64;
        // SAFETY: `volume_w` is NUL-terminated and `total_bytes` is a valid `u64` pointer.
        let _ = unsafe {
            GetDiskFreeSpaceExW(volume_w.as_ptr(), null_mut(), &mut total_bytes, null_mut())
        };

        Some((label, fs_name, total_bytes))
    }

    pub fn unmounted_drives() -> Result<Vec<UnmountedDrive>, DriveError> {
        let mut vol_buf = [0u16; 260];
        // SAFETY: `vol_buf` is a writable UTF-16 buffer of length `vol_buf.len()`.
        let find_handle = unsafe { FindFirstVolumeW(vol_buf.as_mut_ptr(), vol_buf.len() as u32) };
        if find_handle == INVALID_HANDLE_VALUE {
            // SAFETY: reads thread-local last error.
            return Err(map_win32_error(
                unsafe { GetLastError() },
                "FindFirstVolumeW",
            ));
        }

        struct FindGuard(HANDLE);
        impl Drop for FindGuard {
            fn drop(&mut self) {
                // SAFETY: `self.0` was opened by `FindFirstVolumeW`.
                unsafe {
                    FindVolumeClose(self.0);
                }
            }
        }
        let _find_guard = FindGuard(find_handle);

        let mut drives = Vec::new();
        loop {
            let vol_guid_slash = from_wide_nul(&vol_buf);
            let vol_w = to_wide(&vol_guid_slash);
            let vol_no_slash = vol_guid_slash.trim_end_matches('\\').to_string();

            if !volume_has_mount_points(&vol_w).unwrap_or(true) {
                // SAFETY: `vol_w` is NUL-terminated.
                let drive_type = unsafe { GetDriveTypeW(vol_w.as_ptr()) };
                let (storage_removable, dev_number) = query_storage_info(&vol_no_slash);

                if drive_type == DRIVE_REMOVABLE || storage_removable {
                    if let Some((label, file_system, total_space)) = query_volume_metadata(&vol_w) {
                        let name = if label.trim().is_empty() {
                            vol_no_slash.clone()
                        } else {
                            label
                        };
                        drives.push(UnmountedDrive {
                            id: vol_guid_slash,
                            drive_id: dev_number.map(|n| format!("disk:{n}")),
                            name,
                            device: vol_no_slash,
                            file_system,
                            total_space,
                        });
                    }
                }
            }

            vol_buf.fill(0);
            // SAFETY: `find_handle` is active and `vol_buf` is valid.
            let has_next =
                unsafe { FindNextVolumeW(find_handle, vol_buf.as_mut_ptr(), vol_buf.len() as u32) };
            if has_next == 0 {
                // SAFETY: reads thread-local last error.
                let err = unsafe { GetLastError() };
                if err == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(map_win32_error(err, "FindNextVolumeW"));
            }
        }

        drives.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
        Ok(drives)
    }

    pub fn mount(drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        // SAFETY: `GetLogicalDrives` takes no arguments and returns a drive bitmask.
        let mask = unsafe { GetLogicalDrives() };
        if mask == 0 {
            // SAFETY: reads thread-local last error.
            return Err(map_win32_error(
                unsafe { GetLastError() },
                "GetLogicalDrives",
            ));
        }

        // Prefer D:..=Z: so A:/B: floppy slots and C: system drive are avoided.
        let free_letter = (3u32..26)
            .find(|bit| (mask & (1u32 << bit)) == 0)
            .map(|bit| (b'A' + bit as u8) as char)
            .ok_or_else(|| DriveError::Backend("no free drive letters available".into()))?;

        let mount_str = format!("{free_letter}:\\");
        let volume_guid = if drive.id.ends_with('\\') {
            drive.id.clone()
        } else {
            format!("{}\\", drive.id)
        };

        let mount_w = to_wide(&mount_str);
        let volume_w = to_wide(&volume_guid);

        // SAFETY: both `mount_w` and `volume_w` are NUL-terminated UTF-16 strings.
        let ok = unsafe { SetVolumeMountPointW(mount_w.as_ptr(), volume_w.as_ptr()) };
        if ok == 0 {
            // SAFETY: reads thread-local last error.
            return Err(map_win32_error(
                unsafe { GetLastError() },
                "SetVolumeMountPointW",
            ));
        }

        Ok(PathBuf::from(mount_str))
    }

    fn volume_device_path(mount_point: &Path) -> Result<String, DriveError> {
        let raw = mount_point.to_string_lossy();
        let trimmed = raw.trim();
        if trimmed.starts_with(r"\\?\Volume{") || trimmed.starts_with(r"\\.\") {
            return Ok(trimmed.trim_end_matches('\\').to_string());
        }
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let rest = &trimmed[2..];
            if rest.is_empty() || rest == "\\" || rest == "/" {
                let letter = (bytes[0] as char).to_ascii_uppercase();
                return Ok(format!(r"\\.\{letter}:"));
            }
        }
        Err(DriveError::NotFound(format!(
            "cannot map mount point {} to a Windows volume",
            mount_point.display()
        )))
    }

    fn find_disk_parent_devinst(target_device_number: u32) -> Option<u32> {
        // SAFETY: `GUID_DEVINTERFACE_DISK` is a valid interface GUID.
        let dev_info = unsafe {
            SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_DISK,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if dev_info == INVALID_HANDLE_VALUE as isize {
            return None;
        }
        let _guard = DevInfoGuard(dev_info);

        let mut index = 0u32;
        loop {
            let mut iface_data = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: `dev_info` is valid and `iface_data.cbSize` is initialized.
            let enum_ok = unsafe {
                SetupDiEnumDeviceInterfaces(
                    dev_info,
                    null(),
                    &GUID_DEVINTERFACE_DISK,
                    index,
                    &mut iface_data,
                )
            };
            if enum_ok == 0 {
                break;
            }
            index += 1;

            let mut required_size = 0u32;
            // SAFETY: probing required buffer size with null detail pointer.
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &iface_data,
                    null_mut(),
                    0,
                    &mut required_size,
                    null_mut(),
                );
            }
            if required_size < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
                continue;
            }

            let mut aligned_buf = vec![0u32; (required_size as usize).div_ceil(4)];
            let detail_ptr = aligned_buf
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            // SAFETY: `aligned_buf` is 4-byte aligned and sized >= `required_size`.
            unsafe {
                addr_of_mut!((*detail_ptr).cbSize)
                    .write(std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32);
            }

            let mut devinfo_data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: `detail_ptr` and `devinfo_data` are properly sized and initialized.
            let detail_ok = unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &iface_data,
                    detail_ptr,
                    required_size,
                    null_mut(),
                    &mut devinfo_data,
                )
            };
            if detail_ok == 0 {
                continue;
            }

            // SAFETY: `DevicePath` is a NUL-terminated UTF-16 string inside `aligned_buf`.
            let path_ptr = unsafe { std::ptr::addr_of!((*detail_ptr).DevicePath).cast::<u16>() };
            // SAFETY: `CreateFileW` opens the disk interface path to query its device number.
            let disk_handle = unsafe {
                CreateFileW(
                    path_ptr,
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if disk_handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let disk_guard = HandleGuard(disk_handle);
            let dev_no = query_device_number(disk_guard.0);
            drop(disk_guard);

            if dev_no == Some(target_device_number) {
                let mut parent_dev_inst = 0u32;
                // SAFETY: `devinfo_data.DevInst` is a valid device instance handle.
                let cr = unsafe { CM_Get_Parent(&mut parent_dev_inst, devinfo_data.DevInst, 0) };
                if cr == CR_SUCCESS {
                    return Some(parent_dev_inst);
                }
                return Some(devinfo_data.DevInst);
            }
        }
        None
    }

    pub fn eject(mount_point: &Path) -> Result<(), DriveError> {
        let dev_path = volume_device_path(mount_point)?;
        let dev_w = to_wide(&dev_path);

        // SAFETY: `dev_w` is NUL-terminated; requesting read/write access required by `FSCTL_LOCK_VOLUME`.
        let handle = unsafe {
            CreateFileW(
                dev_w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: reads thread-local last error.
            return Err(map_lock_error(
                unsafe { GetLastError() },
                "CreateFileW for volume eject",
            ));
        }

        let dev_number = query_device_number(handle);
        let mut bytes_returned = 0u32;

        // SAFETY: `handle` is an open volume handle; no buffers required for `FSCTL_LOCK_VOLUME`.
        let lock_ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_LOCK_VOLUME,
                null(),
                0,
                null_mut(),
                0,
                &mut bytes_returned,
                null_mut(),
            )
        };
        if lock_ok == 0 {
            // SAFETY: reads thread-local last error before closing handle.
            let err = unsafe { GetLastError() };
            drop(HandleGuard(handle));
            return Err(map_lock_error(err, "FSCTL_LOCK_VOLUME"));
        }

        // SAFETY: volume is locked; `FSCTL_DISMOUNT_VOLUME` flushes and dismounts it.
        let dismount_ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_DISMOUNT_VOLUME,
                null(),
                0,
                null_mut(),
                0,
                &mut bytes_returned,
                null_mut(),
            )
        };
        if dismount_ok == 0 {
            // SAFETY: reads thread-local last error before closing handle.
            let err = unsafe { GetLastError() };
            drop(HandleGuard(handle));
            return Err(map_win32_error(err, "FSCTL_DISMOUNT_VOLUME"));
        }

        // SAFETY: dismounted volume handle; request media eject when supported.
        let eject_ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_EJECT_MEDIA,
                null(),
                0,
                null_mut(),
                0,
                &mut bytes_returned,
                null_mut(),
            )
        };
        // SAFETY: reads thread-local last error.
        let eject_err = if eject_ok == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };

        // Close the volume handle before PnP removal so our own handle cannot veto ejection.
        drop(HandleGuard(handle));

        if let Some(parent_inst) = dev_number.and_then(find_disk_parent_devinst) {
            let mut veto_type: PNP_VETO_TYPE = 0;
            let mut veto_name = [0u16; 260];
            // SAFETY: `parent_inst` is a valid device instance and `veto_name` has `len()` units.
            let cr = unsafe {
                CM_Request_Device_EjectW(
                    parent_inst,
                    &mut veto_type,
                    veto_name.as_mut_ptr(),
                    veto_name.len() as u32,
                    0,
                )
            };
            if cr == CR_REMOVE_VETOED {
                let veto = from_wide_nul(&veto_name);
                return Err(DriveError::Busy(format!(
                    "device eject vetoed (type={veto_type}): {veto}"
                )));
            }
            if cr != CR_SUCCESS {
                return Err(DriveError::Backend(format!(
                    "CM_Request_Device_EjectW failed with CONFIGRET {cr}"
                )));
            }
            return Ok(());
        }

        if let Some(code) = eject_err {
            if code != ERROR_INVALID_FUNCTION && code != ERROR_NOT_SUPPORTED {
                return Err(map_win32_error(code, "IOCTL_STORAGE_EJECT_MEDIA"));
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::detail::{
        classify_diskutil_error, diskutil_eject_args, diskutil_mount_args, diskutil_unmount_args,
        parse_diskutil_info_json, parse_diskutil_list_json, DiskutilInfo,
    };
    use super::{DriveError, UnmountedDrive};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    const DISKUTIL_BIN: &str = "/usr/sbin/diskutil";
    const PLUTIL_BIN: &str = "/usr/bin/plutil";

    fn run_diskutil(args: &[&str]) -> Result<Vec<u8>, DriveError> {
        let output = Command::new(DISKUTIL_BIN)
            .args(args)
            .output()
            .map_err(|e| DriveError::Backend(format!("failed to run {DISKUTIL_BIN}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(classify_diskutil_error(&stderr, &stdout));
        }

        Ok(output.stdout)
    }

    fn plist_to_json(plist_bytes: &[u8]) -> Result<String, DriveError> {
        let mut child = Command::new(PLUTIL_BIN)
            .args(["-convert", "json", "-o", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| DriveError::Backend(format!("failed to run {PLUTIL_BIN}: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(plist_bytes).map_err(|e| {
                DriveError::Backend(format!("failed to write plist to {PLUTIL_BIN}: {e}"))
            })?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| DriveError::Backend(format!("failed waiting for {PLUTIL_BIN}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DriveError::Backend(format!(
                "plutil failed converting plist to json: {stderr}"
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|e| DriveError::Backend(format!("plutil output was not UTF-8: {e}")))
    }

    fn diskutil_info(target: &str) -> Result<DiskutilInfo, DriveError> {
        let plist = run_diskutil(&["info", "-plist", target])?;
        let json = plist_to_json(&plist)?;
        parse_diskutil_info_json(&json)
    }

    pub fn unmounted_drives() -> Result<Vec<UnmountedDrive>, DriveError> {
        let plist = run_diskutil(&["list", "-plist", "external"])?;
        let json = plist_to_json(&plist)?;
        parse_diskutil_list_json(&json)
    }

    pub fn mount(drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        let args = diskutil_mount_args(&drive.id);
        run_diskutil(&args)?;
        let info = diskutil_info(&drive.id)?;
        let mount_point = info.mount_point.ok_or_else(|| {
            DriveError::Backend(format!(
                "diskutil mounted {} but reported no MountPoint",
                drive.id
            ))
        })?;
        Ok(PathBuf::from(mount_point))
    }

    pub fn eject(mount_point: &Path) -> Result<(), DriveError> {
        let target = mount_point.to_string_lossy();
        let info = diskutil_info(&target)?;

        let unmount_args = diskutil_unmount_args(&target);
        run_diskutil(&unmount_args)?;

        let whole_disk = info
            .parent_whole_disk
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(info.device_identifier.as_str());

        if !whole_disk.is_empty() {
            let eject_args = diskutil_eject_args(whole_disk);
            run_diskutil(&eject_args)?;
        }

        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod platform {
    use super::{DriveError, UnmountedDrive};
    use std::path::{Path, PathBuf};

    pub fn unmounted_drives() -> Result<Vec<UnmountedDrive>, DriveError> {
        Err(DriveError::Unsupported(
            "removable drive enumeration is not supported on this platform".into(),
        ))
    }

    pub fn mount(_drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        Err(DriveError::Unsupported(
            "mounting removable drives is not supported on this platform".into(),
        ))
    }

    pub fn eject(_mount_point: &Path) -> Result<(), DriveError> {
        Err(DriveError::Unsupported(
            "ejecting removable drives is not supported on this platform".into(),
        ))
    }
}
