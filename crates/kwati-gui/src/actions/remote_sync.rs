//! Remote sync and network actions (owned by guinet-worker for #50, #52).
//!
//! Provides remote directory auto-refresh poller controller, window focus pause/resume,
//! selection retention across refreshes, and remote search depth/cancellation management.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use gpui::actions;
use kwati_core::{Entry, Selection};

actions!(
    remote_sync,
    [
        TriggerRefresh,
        ToggleRemoteSearch,
        RetryConnection,
        ToggleAutoRefresh,
        PauseAutoRefresh,
        ResumeAutoRefresh,
    ]
);

pub const DEFAULT_REMOTE_POLL_INTERVAL_SECS: u32 = 30;
pub const MIN_REMOTE_POLL_INTERVAL_SECS: u32 = 10;

/// Status of the remote folder poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePollingState {
    Disabled,
    Active,
    Polling,
    PausedUnfocused,
}

/// Controller for remote folder synchronization, auto-refresh, and background polling.
pub struct RemoteSyncController {
    pub auto_refresh_enabled: bool,
    pub poll_interval_secs: u32,
    pub is_window_focused: bool,
    pub polling_state: RemotePollingState,
    pub last_poll_time: Option<Instant>,
    paused_flag: Arc<AtomicBool>,
}

impl Default for RemoteSyncController {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteSyncController {
    pub fn new() -> Self {
        Self {
            auto_refresh_enabled: false,
            poll_interval_secs: DEFAULT_REMOTE_POLL_INTERVAL_SECS,
            is_window_focused: true,
            polling_state: RemotePollingState::Disabled,
            last_poll_time: None,
            paused_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Enables automatic background refreshing of the open remote folder.
    pub fn enable_auto_refresh(&mut self, interval_secs: u32) {
        self.auto_refresh_enabled = true;
        self.poll_interval_secs = interval_secs.max(MIN_REMOTE_POLL_INTERVAL_SECS);
        if self.is_window_focused {
            self.polling_state = RemotePollingState::Active;
            self.paused_flag.store(false, Ordering::SeqCst);
        } else {
            self.polling_state = RemotePollingState::PausedUnfocused;
            self.paused_flag.store(true, Ordering::SeqCst);
        }
    }

    /// Disables automatic refreshing.
    pub fn disable_auto_refresh(&mut self) {
        self.auto_refresh_enabled = false;
        self.polling_state = RemotePollingState::Disabled;
        self.paused_flag.store(true, Ordering::SeqCst);
    }

    /// Toggles automatic refreshing on or off.
    pub fn toggle_auto_refresh(&mut self) {
        if self.auto_refresh_enabled {
            self.disable_auto_refresh();
        } else {
            self.enable_auto_refresh(self.poll_interval_secs);
        }
    }

    /// Updates the configured polling interval.
    pub fn set_poll_interval(&mut self, interval_secs: u32) {
        self.poll_interval_secs = interval_secs.max(MIN_REMOTE_POLL_INTERVAL_SECS);
    }

    /// Handles window focus changes. When unfocused, polling must pause immediately
    /// so that no listing requests are sent to the server.
    pub fn set_window_focused(&mut self, focused: bool) {
        self.is_window_focused = focused;
        if !self.auto_refresh_enabled {
            self.polling_state = RemotePollingState::Disabled;
            return;
        }

        if focused {
            self.polling_state = RemotePollingState::Active;
            self.paused_flag.store(false, Ordering::SeqCst);
        } else {
            self.polling_state = RemotePollingState::PausedUnfocused;
            self.paused_flag.store(true, Ordering::SeqCst);
        }
    }

    /// Returns whether background polling can run right now.
    pub fn can_poll(&self) -> bool {
        self.auto_refresh_enabled
            && self.is_window_focused
            && !self.paused_flag.load(Ordering::SeqCst)
    }

    /// Marks the start of a poll tick if allowed.
    pub fn start_poll(&mut self) -> bool {
        if self.can_poll() {
            self.polling_state = RemotePollingState::Polling;
            true
        } else {
            false
        }
    }

    /// Marks the completion of a poll tick.
    pub fn finish_poll(&mut self) {
        self.last_poll_time = Some(Instant::now());
        if self.auto_refresh_enabled {
            if self.is_window_focused {
                self.polling_state = RemotePollingState::Active;
            } else {
                self.polling_state = RemotePollingState::PausedUnfocused;
            }
        } else {
            self.polling_state = RemotePollingState::Disabled;
        }
    }

    /// Returns a shared flag indicating whether background threads should pause.
    pub fn paused_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.paused_flag)
    }

    /// Human-readable indicator string for status bar or remote banner.
    pub fn status_indicator(&self) -> String {
        match self.polling_state {
            RemotePollingState::Disabled => "Auto-refresh: Off".to_string(),
            RemotePollingState::Active => {
                format!("Auto-refresh: Active (every {}s)", self.poll_interval_secs)
            }
            RemotePollingState::Polling => "Auto-refresh: Polling...".to_string(),
            RemotePollingState::PausedUnfocused => {
                "Auto-refresh: Paused (window unfocused)".to_string()
            }
        }
    }

    /// Preserves user selection across directory listing refreshes.
    /// Only selected items that still exist in `new_entries` are retained.
    pub fn retain_selection(selection: &mut Selection, new_entries: &[Entry]) {
        selection.retain_existing(new_entries);
    }
}

pub const DEFAULT_REMOTE_SEARCH_DEPTH: usize = 3;
pub const DEFAULT_PARALLEL_REQUESTS: usize = 4;

/// A search hit found in a remote filesystem hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearchMatch {
    pub name: String,
    pub path: std::path::PathBuf,
    pub depth: usize,
    pub is_dir: bool,
}

/// Configuration options for remote searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearchOptions {
    pub depth_limit: usize,
    pub parallel_requests: usize,
}

impl Default for RemoteSearchOptions {
    fn default() -> Self {
        Self {
            depth_limit: DEFAULT_REMOTE_SEARCH_DEPTH,
            parallel_requests: DEFAULT_PARALLEL_REQUESTS,
        }
    }
}

/// Controller managing in-flight remote searches, depth limits, and streaming cancellation.
pub struct RemoteSearchController {
    pub options: RemoteSearchOptions,
    pub is_searching: bool,
    pub results: Vec<RemoteSearchMatch>,
    cancel_flag: Arc<AtomicBool>,
}

impl Default for RemoteSearchController {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteSearchController {
    pub fn new() -> Self {
        Self {
            options: RemoteSearchOptions::default(),
            is_searching: false,
            results: Vec::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_options(options: RemoteSearchOptions) -> Self {
        Self {
            options,
            is_searching: false,
            results: Vec::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Sets the maximum recursive depth limit for remote searching.
    pub fn set_depth_limit(&mut self, depth: usize) {
        self.options.depth_limit = depth;
    }

    /// Sets the number of parallel requests allowed for remote operations.
    pub fn set_parallel_requests(&mut self, parallel: usize) {
        self.options.parallel_requests = parallel.max(1);
    }

    /// Cancels any currently running remote search operation.
    pub fn cancel(&mut self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.is_searching = false;
    }

    /// Returns whether the search cancellation flag was raised.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    /// Returns a clone of the cancellation flag for background tasks.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel_flag)
    }

    /// Starts a new search session, resetting previous matches and clearing cancellation flag.
    pub fn start_search(&mut self) {
        self.cancel_flag.store(false, Ordering::SeqCst);
        self.results.clear();
        self.is_searching = true;
    }

    /// Adds a discovered match to the result list if its depth does not exceed `depth_limit`.
    /// Returns true if the match was accepted, or false if it was excluded by the depth limit.
    pub fn add_match(&mut self, item: RemoteSearchMatch) -> bool {
        if item.depth <= self.options.depth_limit {
            self.results.push(item);
            true
        } else {
            false
        }
    }

    /// Streams search matches through a callback while respecting depth limits and cancellation.
    /// Halts immediately when `cancel()` is triggered.
    pub fn stream_search_items<F>(
        &mut self,
        items: &[(std::path::PathBuf, String, usize, bool)],
        query: &str,
        mut on_stream_hit: F,
    ) where
        F: FnMut(RemoteSearchMatch),
    {
        self.start_search();
        let query_lower = query.to_lowercase();

        for (path, name, depth, is_dir) in items {
            // Check cancellation before processing next item
            if self.is_cancelled() {
                self.is_searching = false;
                break;
            }

            // Exclude items deeper than configured depth limit
            if *depth > self.options.depth_limit {
                continue;
            }

            // Match query
            if name.to_lowercase().contains(&query_lower) {
                let match_item = RemoteSearchMatch {
                    name: name.clone(),
                    path: path.clone(),
                    depth: *depth,
                    is_dir: *is_dir,
                };
                self.results.push(match_item.clone());
                on_stream_hit(match_item);
            }
        }

        self.is_searching = false;
    }

    /// Filters a collection of remote matches by depth limit.
    pub fn filter_by_depth(
        matches: &[RemoteSearchMatch],
        depth_limit: usize,
    ) -> Vec<RemoteSearchMatch> {
        matches
            .iter()
            .filter(|m| m.depth <= depth_limit)
            .cloned()
            .collect()
    }
}
