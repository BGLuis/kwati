use kwati_core::{SearchHit, SearchOptions, Searcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "kwati_test_search_{}_{}_{}",
            name,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run_search(root: &Path, query: &str, options: SearchOptions) -> Vec<SearchHit> {
    let hits = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&hits);
    let handle = Searcher::new().search(root, query, options, move |hit| {
        sink.lock().unwrap().push(hit)
    });
    handle.join();
    let mut hits = hits.lock().unwrap().clone();
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits
}

fn names(hits: &[SearchHit]) -> Vec<String> {
    hits.iter()
        .map(|h| h.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn search_ranks() {
    let dir = TestDir::new("ranks");
    std::fs::write(dir.path().join("random.md"), b"").unwrap();
    std::fs::write(dir.path().join("README.md"), b"").unwrap();
    std::fs::write(dir.path().join("unrelated.txt"), b"").unwrap();

    let hits = run_search(dir.path(), "rdme", SearchOptions::default());
    // random.md has no "e", so the matcher drops it instead of ranking it lower.
    assert_eq!(names(&hits).first().map(String::as_str), Some("README.md"));
    assert!(!names(&hits).contains(&"unrelated.txt".to_string()));
}

#[cfg(unix)]
#[test]
fn search_symlink_cycle() {
    let dir = TestDir::new("cycle");
    let base = dir.path();
    std::fs::create_dir_all(base.join("a/b")).unwrap();
    std::fs::write(base.join("a/b/target.txt"), b"").unwrap();
    std::os::unix::fs::symlink(base, base.join("a/b/loop")).unwrap();

    let root = base.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let hits = run_search(&root, "target", SearchOptions::default());
        let _ = tx.send(hits);
    });
    let hits = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("walk did not finish on a symlink cycle");
    assert_eq!(names(&hits), vec!["target.txt"]);
}

#[test]
fn search_ignore_toggle() {
    let dir = TestDir::new("ignore");
    let base = dir.path();
    std::fs::write(base.join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(base.join("secret.txt"), b"").unwrap();
    std::fs::write(base.join("public.txt"), b"").unwrap();

    let respected = run_search(base, "txt", SearchOptions::default());
    assert_eq!(names(&respected), vec!["public.txt"]);

    let all = run_search(
        base,
        "txt",
        SearchOptions {
            respect_ignore: false,
        },
    );
    let mut all = names(&all);
    all.sort();
    assert_eq!(all, vec!["public.txt", "secret.txt"]);
}

#[test]
fn search_cancels_previous() {
    let dir = TestDir::new("cancels");
    for i in 0..50 {
        std::fs::write(dir.path().join(format!("file{i}.txt")), b"").unwrap();
    }

    let delivered = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let counter = Arc::clone(&delivered);
    let mut searcher = Searcher::new();
    let first = searcher.search(dir.path(), "file", SearchOptions::default(), move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        let _ = entered_tx.send(());
        let _ = gate_rx.recv();
    });

    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("first search delivered nothing");
    let second = searcher.search(dir.path(), "zzzz", SearchOptions::default(), |_| {});
    gate_tx.send(()).unwrap();
    drop(gate_tx);
    first.join();
    second.join();

    assert_eq!(delivered.load(Ordering::SeqCst), 1);
}
