use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use ignore::{WalkBuilder, WalkState};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::{Backend, Location, DEFAULT_REMOTE_SEARCH_DEPTH, DEFAULT_REMOTE_SEARCH_PARALLEL};

/// A file whose path below the search root matched the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    /// Higher is a better match. Hits arrive unordered; the consumer sorts by this.
    pub score: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Skip files excluded by ignore files (`.gitignore`, `.ignore`) and hidden files.
    pub respect_ignore: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            respect_ignore: true,
        }
    }
}

/// Handle to a running walk.
pub struct SearchHandle {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl SearchHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Blocks until the walk has finished or been stopped.
    pub fn join(self) {
        let _ = self.thread.join();
    }
}

/// Runs one search at a time: starting a new one stops the previous walk.
///
/// The stop signal is an atomic flag because async task cancellation only takes effect at
/// await points, and the walk is blocking.
#[derive(Default)]
pub struct Searcher {
    current: Option<Arc<AtomicBool>>,
}

impl Searcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walks below `root` on background threads, calling `on_hit` for each matching file as it
    /// is found. Symlinks are not followed.
    ///
    /// Calls to `on_hit` are serialized, and none starts after the stop signal is set; a call
    /// already running when the previous search is replaced still completes.
    pub fn search<F>(
        &mut self,
        root: &Path,
        query: &str,
        options: SearchOptions,
        on_hit: F,
    ) -> SearchHandle
    where
        F: FnMut(SearchHit) + Send + 'static,
    {
        if let Some(previous) = self.current.take() {
            previous.store(true, Ordering::Release);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.current = Some(Arc::clone(&stop));

        let root = root.to_path_buf();
        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let walk_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || walk(root, pattern, options, walk_stop, on_hit));
        SearchHandle { stop, thread }
    }
}

fn walk<F>(
    root: PathBuf,
    pattern: Pattern,
    options: SearchOptions,
    stop: Arc<AtomicBool>,
    on_hit: F,
) where
    F: FnMut(SearchHit) + Send + 'static,
{
    let on_hit = Mutex::new(on_hit);

    let mut builder = WalkBuilder::new(&root);
    // Without require_git(false), .gitignore is only honored inside a git repository.
    builder
        .standard_filters(options.respect_ignore)
        .require_git(false)
        .follow_links(false);

    builder.build_parallel().run(|| {
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();
        let (root, pattern, on_hit, stop) = (&root, &pattern, &on_hit, &stop);
        Box::new(move |entry| {
            if stop.load(Ordering::Acquire) {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if entry.file_type().is_none_or(|t| t.is_dir()) {
                return WalkState::Continue;
            }
            let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
            let haystack = relative.to_string_lossy();
            let Some(score) = pattern.score(Utf32Str::new(&haystack, &mut buf), &mut matcher)
            else {
                return WalkState::Continue;
            };
            let mut on_hit = on_hit.lock().unwrap_or_else(|e| e.into_inner());
            // Checked under the lock so no delivery begins once the stop signal is set.
            if stop.load(Ordering::Acquire) {
                return WalkState::Quit;
            }
            on_hit(SearchHit {
                path: entry.into_path(),
                score,
            });
            WalkState::Continue
        })
    });
}

/// Limits for [`Searcher::search_remote`], from the user's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteSearchOptions {
    /// How many folder levels are listed: 1 lists only the root, 2 also its sub-folders, and so
    /// on. 0 is treated as 1.
    pub max_depth: u32,
    /// How many listing requests may be in flight at once. 0 is treated as 1.
    pub parallel_requests: usize,
}

impl Default for RemoteSearchOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_REMOTE_SEARCH_DEPTH,
            parallel_requests: DEFAULT_REMOTE_SEARCH_PARALLEL as usize,
        }
    }
}

impl Searcher {
    /// Walks below `root` breadth-first through `backend`, calling `on_hit` for each matching
    /// file as its folder is listed. Hits carry the remote path.
    ///
    /// The walk stays within `options`, so the load on the server is bounded; a folder that
    /// cannot be listed is skipped. Calls to `on_hit` are serialized, and once the stop signal is
    /// set no new listing request is sent and no further hit is delivered.
    pub fn search_remote<F>(
        &mut self,
        backend: Arc<dyn Backend>,
        root: Location,
        query: &str,
        options: RemoteSearchOptions,
        on_hit: F,
    ) -> SearchHandle
    where
        F: FnMut(SearchHit) + Send + 'static,
    {
        if let Some(previous) = self.current.take() {
            previous.store(true, Ordering::Release);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.current = Some(Arc::clone(&stop));

        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let walk_stop = Arc::clone(&stop);
        let thread =
            thread::spawn(move || walk_remote(backend, root, pattern, options, walk_stop, on_hit));
        SearchHandle { stop, thread }
    }
}

struct Queue {
    pending: VecDeque<(Location, u32)>,
    busy: usize,
}

fn walk_remote<F>(
    backend: Arc<dyn Backend>,
    root: Location,
    pattern: Pattern,
    options: RemoteSearchOptions,
    stop: Arc<AtomicBool>,
    on_hit: F,
) where
    F: FnMut(SearchHit) + Send + 'static,
{
    let max_depth = options.max_depth.max(1);
    let queue = Mutex::new(Queue {
        pending: VecDeque::from([(root.clone(), 1)]),
        busy: 0,
    });
    let changed = Condvar::new();
    let on_hit = Mutex::new(on_hit);

    thread::scope(|scope| {
        for _ in 0..options.parallel_requests.max(1) {
            scope.spawn(|| {
                let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
                let mut buf = Vec::new();
                loop {
                    let (dir, depth) = {
                        let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if stop.load(Ordering::Acquire) {
                                return;
                            }
                            if let Some(next) = q.pending.pop_front() {
                                q.busy += 1;
                                break next;
                            }
                            if q.busy == 0 {
                                return;
                            }
                            q = changed.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let listing = backend.list(&dir);
                    let mut children = Vec::new();
                    for entry in listing.map(|l| l.into_entries()).unwrap_or_default() {
                        let child = dir.join(entry.name());
                        if entry.is_dir {
                            if depth < max_depth {
                                children.push((child, depth + 1));
                            }
                            continue;
                        }
                        let relative = child
                            .path()
                            .strip_prefix(root.path())
                            .unwrap_or(child.path());
                        let haystack = relative.to_string_lossy();
                        let Some(score) =
                            pattern.score(Utf32Str::new(&haystack, &mut buf), &mut matcher)
                        else {
                            continue;
                        };
                        let mut on_hit = on_hit.lock().unwrap_or_else(|e| e.into_inner());
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        on_hit(SearchHit {
                            path: child.path().to_path_buf(),
                            score,
                        });
                    }
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pending.extend(children);
                    q.busy -= 1;
                    changed.notify_all();
                }
            });
        }
    });
}
