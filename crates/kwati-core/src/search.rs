use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use ignore::{WalkBuilder, WalkState};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

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
