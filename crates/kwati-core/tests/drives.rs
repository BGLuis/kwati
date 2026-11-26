use kwati_core::drives::detail::{
    classify_diskutil_error, decode_cstr_bytes, diskutil_eject_args, diskutil_mount_args,
    diskutil_unmount_args, map_udisks_error_name, parse_diskutil_info_json,
    parse_diskutil_list_json, udisks2_unmount_options, DiskutilInfo,
};
use kwati_core::{DriveError, UnmountedDrive};

#[test]
fn udisks2_error_mapping() {
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.DeviceBusy",
            "target is busy".into()
        ),
        DriveError::Busy("target is busy".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.NotAuthorized",
            "Not authorized".into()
        ),
        DriveError::PermissionDenied("Not authorized".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain",
            "polkit denied".into()
        ),
        DriveError::PermissionDenied("polkit denied".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed",
            "dialog dismissed".into()
        ),
        DriveError::PermissionDenied("dialog dismissed".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.NotSupported",
            "operation not supported".into()
        ),
        DriveError::Unsupported("operation not supported".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.DoesNotExist",
            "missing object".into()
        ),
        DriveError::NotFound("missing object".into())
    );
    assert_eq!(
        map_udisks_error_name(
            "org.freedesktop.UDisks2.Error.AlreadyMounted",
            "Device /dev/sdb1 is already mounted".into()
        ),
        DriveError::Backend(
            "org.freedesktop.UDisks2.Error.AlreadyMounted: Device /dev/sdb1 is already mounted"
                .into()
        )
    );
}

#[test]
fn nul_terminated_udisks_byte_arrays() {
    assert_eq!(decode_cstr_bytes(b"/dev/sdb1\0"), "/dev/sdb1");
    assert_eq!(
        decode_cstr_bytes(b"/run/media/luis/USB\0extra"),
        "/run/media/luis/USB"
    );
    assert_eq!(decode_cstr_bytes(b"/dev/nvme0n1p1"), "/dev/nvme0n1p1");
    assert_eq!(decode_cstr_bytes(b""), "");
}

#[test]
fn diskutil_plist_json_parsing_filters_mounted_and_system_partitions() {
    let json = r#"{
        "AllDisksAndPartitions": [
            {
                "DeviceIdentifier": "disk2",
                "Content": "GUID_partition_scheme",
                "Size": 64000000000,
                "Partitions": [
                    {
                        "DeviceIdentifier": "disk2s1",
                        "Content": "EFI",
                        "Size": 209715200,
                        "VolumeName": "EFI"
                    },
                    {
                        "DeviceIdentifier": "disk2s2",
                        "Content": "ExFAT",
                        "Size": 32000000000,
                        "VolumeName": "SANDISK"
                    },
                    {
                        "DeviceIdentifier": "disk2s3",
                        "Content": "DOS_FAT_32",
                        "Size": 16000000000,
                        "VolumeName": "WORK",
                        "MountPoint": "/Volumes/WORK"
                    }
                ],
                "APFSVolumes": [
                    {
                        "DeviceIdentifier": "disk2s4",
                        "VolumeName": "Backup",
                        "Size": 15000000000
                    },
                    {
                        "DeviceIdentifier": "disk2s5",
                        "VolumeName": "Preboot",
                        "Size": 500000000,
                        "OSInternal": true
                    }
                ]
            },
            {
                "DeviceIdentifier": "disk4",
                "Content": "DOS_FAT_16",
                "Size": 1000000000
            }
        ]
    }"#;

    let drives = parse_diskutil_list_json(json).expect("valid diskutil json");
    assert_eq!(
        drives,
        vec![
            UnmountedDrive {
                id: "disk2s4".into(),
                drive_id: Some("disk2".into()),
                name: "Backup".into(),
                device: "/dev/disk2s4".into(),
                file_system: "apfs".into(),
                total_space: 15_000_000_000,
            },
            UnmountedDrive {
                id: "disk2s2".into(),
                drive_id: Some("disk2".into()),
                name: "SANDISK".into(),
                device: "/dev/disk2s2".into(),
                file_system: "ExFAT".into(),
                total_space: 32_000_000_000,
            },
            UnmountedDrive {
                id: "disk4".into(),
                drive_id: Some("disk4".into()),
                name: "disk4".into(),
                device: "/dev/disk4".into(),
                file_system: "DOS_FAT_16".into(),
                total_space: 1_000_000_000,
            },
        ]
    );
}

#[test]
fn diskutil_info_json_extracts_mount_point_and_parent_disk() {
    let json = r#"{
        "DeviceIdentifier": "disk2s2",
        "ParentWholeDisk": "disk2",
        "MountPoint": "/Volumes/SANDISK",
        "FilesystemType": "exfat",
        "VolumeName": "SANDISK",
        "TotalSize": 32000000000
    }"#;

    let info = parse_diskutil_info_json(json).expect("valid diskutil info json");
    assert_eq!(
        info,
        DiskutilInfo {
            device_identifier: "disk2s2".into(),
            parent_whole_disk: Some("disk2".into()),
            mount_point: Some("/Volumes/SANDISK".into()),
            file_system: "exfat".into(),
            volume_name: "SANDISK".into(),
            total_size: 32_000_000_000,
        }
    );
}

#[test]
fn diskutil_busy_classification() {
    assert!(matches!(
        classify_diskutil_error(
            "Volume SANDISK on disk2s2 failed to unmount: dissented by PID 4321 (bash)",
            ""
        ),
        DriveError::Busy(_)
    ));
    assert!(matches!(
        classify_diskutil_error("Unmount failed: Resource busy", ""),
        DriveError::Busy(_)
    ));
    assert!(matches!(
        classify_diskutil_error("", "disk2s2 is currently in use"),
        DriveError::Busy(_)
    ));
    assert!(matches!(
        classify_diskutil_error("Unable to find disk for /Volumes/MISSING", ""),
        DriveError::NotFound(_)
    ));
    assert!(matches!(
        classify_diskutil_error("Operation not permitted", ""),
        DriveError::PermissionDenied(_)
    ));
}

#[test]
fn never_force_arguments() {
    assert_eq!(udisks2_unmount_options(), vec![("force", false)]);

    let mount_args = diskutil_mount_args("disk2s2");
    assert_eq!(mount_args, ["mount", "disk2s2"]);
    assert!(!mount_args
        .iter()
        .any(|a| a.eq_ignore_ascii_case("force") || *a == "-f"));

    let unmount_args = diskutil_unmount_args("/Volumes/SANDISK");
    assert_eq!(unmount_args, ["unmount", "/Volumes/SANDISK"]);
    assert!(!unmount_args
        .iter()
        .any(|a| a.eq_ignore_ascii_case("force") || *a == "-f"));

    let eject_args = diskutil_eject_args("disk2");
    assert_eq!(eject_args, ["eject", "disk2"]);
    assert!(!eject_args
        .iter()
        .any(|a| a.eq_ignore_ascii_case("force") || *a == "-f"));
}

#[cfg(feature = "device-tests")]
mod device_tests {
    use kwati_core::{eject, mount, unmounted_drives, DriveError};
    use std::fs::File;
    use std::path::PathBuf;

    fn test_device() -> String {
        std::env::var("KWATI_TEST_DRIVE_DEVICE")
            .unwrap_or_else(|_| panic!("set KWATI_TEST_DRIVE_DEVICE to run device-tests"))
    }

    fn test_mount() -> PathBuf {
        PathBuf::from(
            std::env::var("KWATI_TEST_DRIVE_MOUNT")
                .unwrap_or_else(|_| panic!("set KWATI_TEST_DRIVE_MOUNT to run device-tests")),
        )
    }

    #[test]
    fn mount_idle() {
        let device = test_device();
        let drives = unmounted_drives().expect("unmounted_drives should succeed");
        let drive = drives
            .into_iter()
            .find(|d| d.device == device || d.id == device)
            .unwrap_or_else(|| panic!("expected unmounted test drive matching {device:?}"));

        let mount_point = mount(&drive).expect("mount_idle should mount the test drive");
        assert!(
            mount_point.exists(),
            "mounted path {} should exist",
            mount_point.display()
        );
    }

    #[test]
    fn eject_busy() {
        let mount_point = test_mount();
        let probe_file = mount_point.join(".kwati-busy-test-lock");
        let _open_handle = File::create(&probe_file)
            .unwrap_or_else(|e| panic!("failed creating busy probe {}: {e}", probe_file.display()));

        let err = eject(&mount_point).expect_err("eject_busy must refuse to eject a busy mount");
        drop(_open_handle);
        let _ = std::fs::remove_file(&probe_file);

        assert!(
            matches!(err, DriveError::Busy(_)),
            "expected DriveError::Busy, got {err:?}"
        );
    }

    #[test]
    fn eject_idle() {
        let mount_point = test_mount();
        eject(&mount_point).expect("eject_idle should cleanly unmount and eject idle drive");
    }
}
