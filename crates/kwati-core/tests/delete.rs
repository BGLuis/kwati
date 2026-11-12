use kwati_core::{delete_permanently, Confirmed};
use std::path::PathBuf;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("kwati_test_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
#[test]
fn delete_permanently_scope() {
    let dir = TestDir::new("delete_permanently_scope");
    let doomed = dir.path.join("doomed");
    let sibling = dir.path.join("sibling.txt");
    let outside = dir.path.join("outside");
    std::fs::create_dir_all(doomed.join("nested")).unwrap();
    std::fs::write(doomed.join("nested/a.txt"), b"a").unwrap();
    std::fs::write(&sibling, b"s").unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), b"k").unwrap();
    std::os::unix::fs::symlink(&outside, doomed.join("link")).unwrap();

    delete_permanently(std::slice::from_ref(&doomed), Confirmed::user_confirmed()).unwrap();

    assert!(doomed.symlink_metadata().is_err());
    assert!(sibling.exists());
    assert!(outside.join("keep.txt").exists());
}

#[cfg(unix)]
#[test]
fn delete_permanently_removes_symlink_not_target() {
    let dir = TestDir::new("delete_permanently_symlink");
    let target = dir.path.join("target");
    let link = dir.path.join("link");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    delete_permanently(std::slice::from_ref(&link), Confirmed::user_confirmed()).unwrap();

    assert!(link.symlink_metadata().is_err());
    assert!(target.exists());
}

#[test]
fn delete_permanently_removes_file() {
    let dir = TestDir::new("delete_permanently_file");
    let file = dir.path.join("a.txt");
    std::fs::write(&file, b"x").unwrap();

    delete_permanently(std::slice::from_ref(&file), Confirmed::user_confirmed()).unwrap();

    assert!(!file.exists());
}
