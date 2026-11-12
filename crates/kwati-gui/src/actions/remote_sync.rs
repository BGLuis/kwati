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
