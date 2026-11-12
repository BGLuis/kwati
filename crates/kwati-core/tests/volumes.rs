use std::path::Path;

use kwati_core::{is_pseudo_filesystem, list_volumes, mounted_volumes, VolumeManager};

#[test]
fn volumes_contain_workspace() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let canonical_manifest = manifest_dir
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.to_path_buf());

    let volumes = list_volumes();
    assert!(!volumes.is_empty(), "volumes list should not be empty");

    let contains_workspace = volumes.iter().any(|v| {
        let v_mount = v
            .mount_point
            .canonicalize()
            .unwrap_or_else(|_| v.mount_point.clone());
        manifest_dir.starts_with(&v.mount_point)
            || manifest_dir.starts_with(&v_mount)
            || canonical_manifest.starts_with(&v.mount_point)
            || canonical_manifest.starts_with(&v_mount)
    });

    assert!(
        contains_workspace,
        "returned volumes {:?} do not contain workspace directory {:?}",
        volumes.iter().map(|v| &v.mount_point).collect::<Vec<_>>(),
        manifest_dir
    );

    // Also check through VolumeManager and mounted_volumes alias
    let mut manager = VolumeManager::new();
    manager.refresh();
    let mgr_volumes = manager.volumes();
    let mgr_contains = mgr_volumes.iter().any(|v| {
        let v_mount = v
            .mount_point
            .canonicalize()
            .unwrap_or_else(|_| v.mount_point.clone());
        canonical_manifest.starts_with(&v_mount) || manifest_dir.starts_with(&v.mount_point)
    });
    assert!(mgr_contains);

    assert_eq!(list_volumes().len(), mounted_volumes().len());
}

#[test]
fn volumes_hide_pseudo() {
    let volumes = list_volumes();

    for v in &volumes {
        let fs = v.file_system.to_lowercase();
        let mount = &v.mount_point;

        // Pseudo file systems such as proc and sysfs are absent from the list on Linux
        assert_ne!(fs, "proc", "proc filesystem should be hidden");
        assert_ne!(fs, "sysfs", "sysfs filesystem should be hidden");
        assert_ne!(fs, "devtmpfs", "devtmpfs filesystem should be hidden");
        assert_ne!(fs, "devpts", "devpts filesystem should be hidden");

        assert!(
            !mount.starts_with("/proc"),
            "mount point {:?} under /proc should be hidden",
            mount
        );
        assert!(
            !mount.starts_with("/sys"),
            "mount point {:?} under /sys should be hidden",
            mount
        );
    }

    // Direct unit assertions on pseudo filesystem detection
    assert!(is_pseudo_filesystem("proc", Path::new("/proc")));
    assert!(is_pseudo_filesystem("sysfs", Path::new("/sys")));
    assert!(is_pseudo_filesystem("cgroup2", Path::new("/sys/fs/cgroup")));
    assert!(is_pseudo_filesystem("devtmpfs", Path::new("/dev")));
    assert!(is_pseudo_filesystem("devpts", Path::new("/dev/pts")));
    assert!(is_pseudo_filesystem("autofs", Path::new("/misc")));

    assert!(!is_pseudo_filesystem("ext4", Path::new("/")));
    assert!(!is_pseudo_filesystem("btrfs", Path::new("/home")));
    assert!(!is_pseudo_filesystem("apfs", Path::new("/")));
    assert!(!is_pseudo_filesystem("ntfs", Path::new("C:\\")));
}
