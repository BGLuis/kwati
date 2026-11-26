use kwati_core::{all_local_volumes, volume_kind, VolumeKind};
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
struct TestDir {
    path: PathBuf,
}

#[cfg(unix)]
impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("kwati_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

#[cfg(unix)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
#[cfg(unix)]
fn volume_kind_unknown() {
    let dir = TestDir::new("volume_kind_unknown");
    let missing = dir.path.join("does-not-exist");

    assert_eq!(volume_kind(&missing), VolumeKind::Unknown);
}

#[test]
#[cfg(unix)]
fn volume_kind_existing_temp_dir_is_not_unknown() {
    let dir = TestDir::new("volume_kind_existing");

    assert_ne!(volume_kind(&dir.path), VolumeKind::Unknown);
}

#[test]
#[cfg(unix)]
fn all_local_volumes_checks_slice_together() {
    let dir = TestDir::new("all_local_volumes");
    let file = dir.path.join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let missing = dir.path.join("does-not-exist");

    assert!(all_local_volumes(&[]));
    assert!(all_local_volumes(&[dir.path.as_path(), file.as_path()]));
    assert!(!all_local_volumes(&[dir.path.as_path(), missing.as_path()]));
}

#[test]
#[cfg(windows)]
fn volume_kind_unc() {
    assert_eq!(
        volume_kind(std::path::Path::new(r"\\server\share")),
        VolumeKind::Network
    );
}
