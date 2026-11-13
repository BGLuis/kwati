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

fn volume(device: &str, mount: &str, fs: &str) -> kwati_core::Volume {
    kwati_core::Volume {
        name: device.into(),
        mount_point: mount.into(),
        total_space: 100,
        available_space: 50,
        free_space: 50,
        file_system: fs.into(),
        is_removable: false,
        is_read_only: false,
    }
}

#[test]
fn user_facing_volumes_keep_one_entry_per_device_at_the_shortest_mount() {
    use kwati_core::user_facing_volumes;

    // A btrfs root with its subvolumes, as Arch and Fedora install it.
    let shown = user_facing_volumes([
        volume("/dev/mapper/root", "/home", "btrfs"),
        volume("/dev/mapper/root", "/", "btrfs"),
        volume("/dev/sdc1", "/mnt/ssd", "ext4"),
    ]);

    let mounts: Vec<_> = shown.iter().map(|v| v.mount_point.as_path()).collect();
    assert_eq!(mounts, [Path::new("/"), Path::new("/mnt/ssd")]);
}

#[test]
fn user_facing_volumes_hide_system_container_and_network_mounts() {
    use kwati_core::user_facing_volumes;

    let shown = user_facing_volumes([
        volume("/dev/nvme0n1p1", "/boot", "vfat"),
        volume("/dev/mapper/root", "/var/log", "btrfs"),
        volume("overlay", "/var/lib/docker/rootfs/overlayfs/46", "overlay"),
        volume("tmpfs", "/tmp", "tmpfs"),
        volume("me@host:/", "/mnt/sshfs", "fuse.sshfs"),
        volume("/dev/sda1", "/mnt/hdd", "fuseblk"),
    ]);

    let mounts: Vec<_> = shown.iter().map(|v| v.mount_point.as_path()).collect();
    assert_eq!(mounts, [Path::new("/mnt/hdd")]);
}

#[test]
fn user_facing_volumes_keep_volumes_named_by_label() {
    use kwati_core::user_facing_volumes;

    // macOS and Windows name volumes by label, and two disks can share one.
    let shown = user_facing_volumes([
        volume("Untitled", "/Volumes/Untitled", "msdos"),
        volume("Untitled", "/Volumes/Untitled 1", "msdos"),
    ]);
    assert_eq!(shown.len(), 2);
}

#[test]
fn parse_network_mounts_lists_shares_by_source() {
    use kwati_core::parse_network_mounts;

    let mountinfo = "\
24 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
30 24 0:27 / /mnt/nas rw,relatime shared:2 - cifs //nas/media rw,vers=3.1.1
31 24 0:28 / /mnt/my\\040docs rw shared:3 - nfs4 server:/export rw
32 24 0:29 / /mnt/sshfs rw - fuse.sshfs me@host:/ rw
33 24 0:30 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw";

    let mounts = parse_network_mounts(mountinfo);
    let found: Vec<_> = mounts
        .iter()
        .map(|m| (m.name.as_str(), m.mount_point.to_str().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            ("//nas/media", "/mnt/nas"),
            ("server:/export", "/mnt/my docs"),
            ("me@host:/", "/mnt/sshfs"),
            ("gvfsd-fuse", "/run/user/1000/gvfs"),
        ]
    );
}

#[test]
fn gvfs_share_name_reads_the_server_and_share() {
    use kwati_core::gvfs_share_name;

    assert_eq!(
        gvfs_share_name("smb-share:server=nas,share=media"),
        "media on nas"
    );
    assert_eq!(
        gvfs_share_name("sftp:host=example.com,user=me"),
        "example.com"
    );
    assert_eq!(gvfs_share_name("unknown"), "unknown");
}
