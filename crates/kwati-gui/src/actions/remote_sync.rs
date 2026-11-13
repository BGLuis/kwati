//! Remote sync, auto-refresh polling, and network actions (#50, #52).
//!
//! Wires [`RemoteSyncController`] and [`RemotePollerHost`] to [`kwati_core::RemotePoller`]
//! for automatic remote directory refreshing, pauses polling immediately when the window
//! loses focus, and preserves selection by [`Location`].
//!
//! ## MainView integration (`views/mod.rs`)
//! - Instantiate [`RemotePollerHost`] in `MainView::new` as `Entity<RemotePollerHost>`:
//!   `let remote_sync = cx.new(|cx| RemotePollerHost::new(app_state.clone(), cx));`
//! - Forward window focus changes via `cx.observe_window_activation(window, ...)`:
//!   `this.remote_sync.update(cx, |host, cx| host.on_window_activation(window.is_window_active(), cx));`
//! - `RemotePollerHost` observes `AppState` automatically for location, settings, and
//!   connection changes; `MainView` may also call [`RemotePollerHost::on_location_changed`]
//!   or [`RemotePollerHost::on_settings_changed`] explicitly when needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{actions, Context, Entity, Subscription, Task};
use kwati_core::{Backend, ConnectionState, Entry, Location, RemotePoller, Selection, Settings};

pub use kwati_core::{
    DEFAULT_REMOTE_POLL_INTERVAL_SECS, DEFAULT_REMOTE_SEARCH_DEPTH, DEFAULT_REMOTE_SEARCH_PARALLEL,
    MIN_REMOTE_POLL_INTERVAL_SECS,
};

use crate::state::{entry_location, AppState};

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

/// Status of the remote folder poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePollingState {
    Disabled,
    Active,
    Polling,
    PausedUnfocused,
}

#[derive(Clone)]
struct PollerKey {
    location: Location,
    interval: Duration,
    backend: Weak<dyn Backend>,
}

impl PollerKey {
    fn matches(&self, location: &Location, interval: Duration, backend: &Arc<dyn Backend>) -> bool {
        self.location == *location
            && self.interval == interval
            && self
                .backend
                .upgrade()
                .is_some_and(|existing| Arc::ptr_eq(&existing, backend))
    }
}

/// Controller for remote folder synchronization, auto-refresh, and background polling.
pub struct RemoteSyncController {
    pub auto_refresh_enabled: bool,
    pub poll_interval_secs: u32,
    pub is_window_focused: bool,
    pub polling_state: RemotePollingState,
    pub last_poll_time: Option<Instant>,
    paused_flag: Arc<AtomicBool>,
    poller: Option<RemotePoller>,
    poller_key: Option<PollerKey>,
    poll_duration_override: Option<Duration>,
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
            poller: None,
            poller_key: None,
            poll_duration_override: None,
        }
    }

    /// Synchronizes enabled flag and interval from `settings`.
    pub fn sync_from_settings(&mut self, settings: &Settings) {
        if settings.remote_poll_enabled {
            self.enable_auto_refresh(settings.remote_poll_interval_secs);
        } else {
            self.set_poll_interval(settings.remote_poll_interval_secs);
            self.disable_auto_refresh();
        }
    }

    /// Returns the effective poll interval duration (respecting any test duration override).
    pub fn effective_poll_duration(&self) -> Duration {
        self.poll_duration_override
            .unwrap_or_else(|| Duration::from_secs(u64::from(self.poll_interval_secs)))
    }

    #[doc(hidden)]
    pub fn set_poll_duration_override(&mut self, duration: Option<Duration>) {
        self.poll_duration_override = duration;
    }

    /// Enables automatic background refreshing of the open remote folder.
    pub fn enable_auto_refresh(&mut self, interval_secs: u32) {
        self.auto_refresh_enabled = true;
        self.poll_interval_secs = interval_secs.max(MIN_REMOTE_POLL_INTERVAL_SECS);
        if self.is_window_focused {
            self.polling_state = RemotePollingState::Active;
            self.paused_flag.store(false, Ordering::SeqCst);
            if let Some(poller) = &self.poller {
                poller.resume();
            }
        } else {
            self.polling_state = RemotePollingState::PausedUnfocused;
            self.paused_flag.store(true, Ordering::SeqCst);
            if let Some(poller) = &self.poller {
                poller.pause();
            }
        }
    }

    /// Disables automatic refreshing and stops any running [`RemotePoller`].
    pub fn disable_auto_refresh(&mut self) {
        self.auto_refresh_enabled = false;
        self.polling_state = RemotePollingState::Disabled;
        self.paused_flag.store(true, Ordering::SeqCst);
        self.stop_poller();
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
            if let Some(poller) = &self.poller {
                poller.pause();
            }
            return;
        }

        if focused {
            self.polling_state = RemotePollingState::Active;
            self.paused_flag.store(false, Ordering::SeqCst);
            if let Some(poller) = &self.poller {
                poller.resume();
            }
        } else {
            self.polling_state = RemotePollingState::PausedUnfocused;
            self.paused_flag.store(true, Ordering::SeqCst);
            if let Some(poller) = &self.poller {
                poller.pause();
            }
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

    /// Stops and drops the active [`RemotePoller`], if any.
    pub fn stop_poller(&mut self) {
        if let Some(mut poller) = self.poller.take() {
            poller.stop();
        }
        self.poller_key = None;
    }

    /// Returns whether a background [`RemotePoller`] instance is currently attached.
    pub fn has_poller(&self) -> bool {
        self.poller.is_some()
    }

    /// Returns whether an attached [`RemotePoller`] is currently paused.
    pub fn is_poller_paused(&self) -> bool {
        self.poller.as_ref().is_some_and(RemotePoller::is_paused)
    }

    /// Synchronizes the live [`RemotePoller`] with the given location, backend, and connection
    /// state (used directly in synchronous/headless setups; [`RemotePollerHost`] runs creation
    /// on the GPUI background executor).
    pub fn sync_poller<F, M>(
        &mut self,
        location: &Location,
        backend: Option<Arc<dyn Backend>>,
        connection_state: Option<ConnectionState>,
        make_on_change: M,
    ) where
        M: FnOnce() -> F,
        F: FnMut() + Send + 'static,
    {
        let Some(backend) = (match location {
            Location::Remote { .. } if self.auto_refresh_enabled => backend,
            _ => None,
        }) else {
            self.stop_poller();
            return;
        };

        let interval = self.effective_poll_duration();
        let conn_state = connection_state.unwrap_or(ConnectionState::Connected);

        if let (Some(poller), Some(key)) = (&self.poller, &self.poller_key) {
            if key.matches(location, interval, &backend) {
                poller.set_connection_state(conn_state);
                if self.is_window_focused {
                    poller.resume();
                } else {
                    poller.pause();
                }
                return;
            }
        }

        self.stop_poller();

        // Do not start a new poller (which fetches a baseline listing) while unfocused or
        // disconnected, so zero requests reach the server.
        if !self.is_window_focused || conn_state == ConnectionState::Disconnected {
            return;
        }

        let poller = RemotePoller::with_connection_state(
            backend.clone(),
            location.clone(),
            interval,
            Arc::new(RwLock::new(conn_state)),
            make_on_change(),
        );
        self.poller = Some(poller);
        self.poller_key = Some(PollerKey {
            location: location.clone(),
            interval,
            backend: Arc::downgrade(&backend),
        });
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

    /// Preserves user selection across local directory listing refreshes.
    pub fn retain_selection(selection: &mut Selection, new_entries: &[Entry]) {
        selection.retain_existing(new_entries);
    }

    /// Preserves user selection across local or remote directory refreshes by mapping each
    /// entry through `current_location` (`Location::Local` or `Location::Remote`).
    pub fn retain_selection_for_location(
        selection: &mut Selection,
        current_location: &Location,
        new_entries: &[Entry],
    ) {
        let locations: Vec<Location> = new_entries
            .iter()
            .map(|entry| entry_location(current_location, entry))
            .collect();
        selection.reload_locations(&locations);
    }

    /// Refreshes `state` while preserving existing selection by `Location`.
    pub fn refresh_preserving_selection(state: &mut AppState) {
        let previously_selected = state.selected_locations();
        state.reload_entries();
        if state.selected_locations().is_empty() && !previously_selected.is_empty() {
            for loc in previously_selected {
                if state
                    .entries
                    .iter()
                    .any(|entry| state.location_of(entry) == loc)
                {
                    state.toggle_select_location(loc);
                }
            }
        }
    }
}

/// GPUI host that wires [`RemoteSyncController`] and [`kwati_core::RemotePoller`] to
/// [`AppState`], reacting to location, settings, connection, and window-focus changes.
pub struct RemotePollerHost {
    pub app_state: Entity<AppState>,
    pub controller: RemoteSyncController,
    change_tx: mpsc::UnboundedSender<()>,
    target_key: Option<PollerKey>,
    poller_generation: u64,
    spawn_task: Option<Task<()>>,
    _state_sub: Subscription,
    _poll_task: Task<()>,
}

impl RemotePollerHost {
    /// Creates a host bound to `app_state` and starts observing settings, location, and
    /// remote connection changes.
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let (change_tx, mut change_rx) = mpsc::unbounded::<()>();

        let poll_task = cx.spawn(async move |this, cx| {
            while change_rx.next().await.is_some() {
                let updated = this.update(cx, |host, cx| {
                    if !host.controller.can_poll() {
                        return;
                    }
                    host.controller.start_poll();
                    host.app_state.update(cx, |state, cx| {
                        RemoteSyncController::refresh_preserving_selection(state);
                        cx.notify();
                    });
                    host.controller.finish_poll();
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        });

        let state_sub = cx.observe(&app_state, |this, _app_state, cx| {
            this.sync_with_state(cx);
        });

        let mut host = Self {
            app_state,
            controller: RemoteSyncController::new(),
            change_tx,
            target_key: None,
            poller_generation: 0,
            spawn_task: None,
            _state_sub: state_sub,
            _poll_task: poll_task,
        };
        host.sync_with_state(cx);
        host
    }

    /// Call from `MainView` (`cx.observe_window_activation`) when the window gains or loses
    /// focus. Immediately pauses polling while unfocused so zero listing requests are sent.
    pub fn on_window_activation(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.controller.set_window_focused(focused);
        self.sync_with_state(cx);
        cx.notify();
    }

    /// Call from `MainView` if location changed outside an observed `AppState` update.
    pub fn on_location_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_with_state(cx);
    }

    /// Call from `MainView` when polling settings change.
    pub fn on_settings_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_with_state(cx);
    }

    /// Overrides the poll interval duration for headless/integration tests so tests can run
    /// with sub-second intervals instead of waiting `MIN_REMOTE_POLL_INTERVAL_SECS` (10s).
    #[doc(hidden)]
    pub fn set_poll_duration_override(
        &mut self,
        duration: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        self.controller.set_poll_duration_override(duration);
        self.sync_with_state(cx);
    }

    /// Reconciles the active [`RemotePoller`] with `AppState` (`settings`, `current_location`,
    /// `current_backend`, `remote_state`) and current window focus.
    pub fn sync_with_state(&mut self, cx: &mut Context<Self>) {
        let (settings, location, backend, conn_state) = {
            let state = self.app_state.read(cx);
            (
                state.settings.clone(),
                state.current_location().clone(),
                state.current_backend(),
                state.remote_state(),
            )
        };

        self.controller.sync_from_settings(&settings);

        let Some(backend) = (match &location {
            Location::Remote { .. } if self.controller.auto_refresh_enabled => backend,
            _ => None,
        }) else {
            self.stop_active_poller();
            cx.notify();
            return;
        };

        let interval = self.controller.effective_poll_duration();
        let conn_state = conn_state.unwrap_or(ConnectionState::Connected);

        if let Some(key) = &self.target_key {
            if key.matches(&location, interval, &backend) {
                if let Some(poller) = &self.controller.poller {
                    poller.set_connection_state(conn_state);
                    if self.controller.is_window_focused {
                        poller.resume();
                    } else {
                        poller.pause();
                    }
                }
                cx.notify();
                return;
            }
        }

        self.stop_active_poller();

        if !self.controller.is_window_focused || conn_state == ConnectionState::Disconnected {
            cx.notify();
            return;
        }

        self.poller_generation = self.poller_generation.wrapping_add(1);
        let generation = self.poller_generation;
        self.target_key = Some(PollerKey {
            location: location.clone(),
            interval,
            backend: Arc::downgrade(&backend),
        });

        let tx = self.change_tx.clone();
        let bg = cx.background_executor().clone();
        self.spawn_task = Some(cx.spawn(async move |this, cx| {
            let poller = bg
                .spawn(async move {
                    RemotePoller::with_connection_state(
                        backend,
                        location,
                        interval,
                        Arc::new(RwLock::new(conn_state)),
                        move || {
                            let _ = tx.unbounded_send(());
                        },
                    )
                })
                .await;

            let _ = this.update(cx, |host, cx| {
                if host.poller_generation != generation || !host.controller.auto_refresh_enabled {
                    drop(poller);
                    return;
                }
                if !host.controller.is_window_focused {
                    poller.pause();
                }
                host.controller.poller = Some(poller);
                host.controller.poller_key = host.target_key.clone();
                host.spawn_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn stop_active_poller(&mut self) {
        self.poller_generation = self.poller_generation.wrapping_add(1);
        self.spawn_task = None;
        self.target_key = None;
        self.controller.stop_poller();
    }

    /// Human-readable indicator for the status bar or network banner.
    pub fn status_indicator(&self) -> String {
        self.controller.status_indicator()
    }

    /// Current polling state.
    pub fn polling_state(&self) -> RemotePollingState {
        self.controller.polling_state
    }

    /// Returns whether a [`RemotePoller`] is currently attached.
    pub fn has_poller(&self) -> bool {
        self.controller.has_poller()
    }

    /// Returns whether the attached [`RemotePoller`] is paused.
    pub fn is_poller_paused(&self) -> bool {
        self.controller.is_poller_paused()
    }
}

pub const DEFAULT_PARALLEL_REQUESTS: usize = DEFAULT_REMOTE_SEARCH_PARALLEL as usize;

/// A search hit found in a remote filesystem hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearchMatch {
    pub name: String,
    pub path: std::path::PathBuf,
    pub depth: u32,
    pub is_dir: bool,
}

/// Configuration options for remote searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearchOptions {
    pub depth_limit: u32,
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
    pub fn set_depth_limit(&mut self, depth: u32) {
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
        items: &[(std::path::PathBuf, String, u32, bool)],
        query: &str,
        mut on_stream_hit: F,
    ) where
        F: FnMut(RemoteSearchMatch),
    {
        self.start_search();
        let query_lower = query.to_lowercase();

        for (path, name, depth, is_dir) in items {
            if self.is_cancelled() {
                self.is_searching = false;
                break;
            }

            if *depth > self.options.depth_limit {
                continue;
            }

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
        depth_limit: u32,
    ) -> Vec<RemoteSearchMatch> {
        matches
            .iter()
            .filter(|m| m.depth <= depth_limit)
            .cloned()
            .collect()
    }
}
