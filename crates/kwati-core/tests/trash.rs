use kwati_core::trash;
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

#[test]
fn trash_removes_entry() {
    let dir = TestDir::new("trash_removes_entry");
    let file = dir.path.join("doomed.txt");
    std::fs::write(&file, b"x").unwrap();

    trash(&file).unwrap();

    assert!(!file.exists());
    assert!(std::fs::read_dir(&dir.path).unwrap().next().is_none());
}

#[test]
fn trash_missing_path() {
    let dir = TestDir::new("trash_missing_path");
    let missing = dir.path.join("does-not-exist.txt");

    let err = trash(&missing).unwrap_err();

    assert!(
        err.to_string().contains(&missing.display().to_string()),
        "error should name the path, got: {err}"
    );
}
