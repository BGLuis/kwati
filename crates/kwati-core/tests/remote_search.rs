use kwati_core::{Backend, BackendError, Entry, Listing, Location, RemoteSearchOptions, Searcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A remote tree where every folder holds `width` sub-folders and one `match.txt`, down to
/// `height` levels, recording each listing request.
struct TreeBackend {
    width: usize,
    height: usize,
    delay: Duration,
    listed: Mutex<Vec<PathBuf>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl TreeBackend {
    fn new(width: usize, height: usize, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            width,
            height,
            delay,
            listed: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        })
    }

    fn listed(&self) -> Vec<PathBuf> {
        self.listed.lock().unwrap().clone()
    }
}

fn level(path: &Path) -> usize {
    path.strip_prefix("/root").unwrap().components().count()
}

impl Backend for TreeBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let dir = location.path().to_path_buf();
        self.listed.lock().unwrap().push(dir.clone());
        std::thread::sleep(self.delay);

        let mut entries = vec![Entry::new("match.txt".into(), dir.join("match.txt"), false)];
        if level(&dir) < self.height {
            for i in 0..self.width {
                let name = format!("d{i}");
                entries.push(Entry::new(name.clone().into(), dir.join(name), true));
            }
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(Listing::new(entries))
    }
}

fn root() -> Location {
    Location::Remote {
        profile_id: "p".into(),
        path: "/root".into(),
    }
}

fn options(max_depth: u32, parallel_requests: usize) -> RemoteSearchOptions {
    RemoteSearchOptions {
        max_depth,
        parallel_requests,
    }
}

#[test]
fn remote_search_depth() {
    let backend = TreeBackend::new(2, 10, Duration::ZERO);
    let hits = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&hits);
    Searcher::new()
        .search_remote(
            backend.clone(),
            root(),
            "match",
            options(3, 2),
            move |hit| sink.lock().unwrap().push(hit.path),
        )
        .join();

    let listed = backend.listed();
    assert!(
        listed.iter().all(|dir| level(dir) <= 2),
        "depth 3 lists the root and two levels below it: {listed:?}"
    );
    assert_eq!(
        listed.len(),
        1 + 2 + 4,
        "every folder within the depth is listed"
    );
    assert_eq!(hits.lock().unwrap().len(), 7, "one match per listed folder");
}

#[test]
fn remote_search_parallel_limit() {
    let backend = TreeBackend::new(4, 3, Duration::from_millis(20));
    Searcher::new()
        .search_remote(backend.clone(), root(), "match", options(3, 2), |_| {})
        .join();
    assert_eq!(backend.max_in_flight.load(Ordering::SeqCst), 2);
}

#[test]
fn remote_search_cancel() {
    let backend = TreeBackend::new(4, 10, Duration::from_millis(30));
    let first_hit = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&first_hit);
    let handle = Searcher::new().search_remote(
        backend.clone(),
        root(),
        "match",
        options(10, 2),
        move |_| seen.store(true, Ordering::SeqCst),
    );
    while !first_hit.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));
    }
    handle.stop();
    let at_stop = backend.listed().len();
    handle.join();
    assert!(
        backend.listed().len() <= at_stop + 2,
        "only requests already in flight may finish after the stop"
    );
    assert!(
        backend.listed().len() < 1 + 4 + 16,
        "the walk did not run to the end"
    );
}

#[test]
fn starting_a_new_remote_search_stops_the_previous_one() {
    let backend = TreeBackend::new(4, 10, Duration::from_millis(30));
    let mut searcher = Searcher::new();
    let first = searcher.search_remote(backend.clone(), root(), "match", options(10, 1), |_| {});
    std::thread::sleep(Duration::from_millis(50));
    let second = searcher.search_remote(
        TreeBackend::new(0, 0, Duration::ZERO),
        root(),
        "match",
        options(1, 1),
        |_| {},
    );
    first.join();
    second.join();
    assert!(backend.listed().len() < 10);
}
