use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{AsyncApp, Context, Task, WeakEntity};
use kwati_core::{
    settings_path, sort_entries_by, volume_kind, Backend, BackendError, Bookmark,
    ConnectionProfile, ConnectionState, DirWatcher, Entry, History, HostKeyUnknown, ListOptions,
    LocalBackend, Location, ReconnectingBackend, Selection, Settings, SettingsWarning, SortColumn,
    SortDirection, SortOrder, VolumeKind,
};

use crate::layout::PanelLayout;
use crate::remote::{start_location, RemoteSessions};

/// A question the user must answer before a connection can go on.
#[derive(Debug, Clone)]
pub enum RemotePrompt {
    /// No password is saved, or the server refused the one sent.
    Password {
        profile_id: String,
        error: Option<String>,
    },
    /// First contact with this server: trusting its key lets the connection go on.
    TrustHostKey {
        profile_id: String,
        unknown: HostKeyUnknown,
        password: Option<String>,
        remember: bool,
    },
    /// The server's key differs from the stored one; the connection was refused.
    HostKeyChanged {
        host: String,
        port: u16,
        presented: String,
        stored: Vec<String>,
    },
    /// Plain FTP is disabled and the server does not offer TLS; retrying sets `allow_insecure`.
    TlsUnavailable {
        profile_id: String,
        host: String,
        port: u16,
        reply: String,
        password: Option<String>,
        remember: bool,
    },
    /// The server's TLS certificate was rejected; connection is refused.
    CertificateInvalid {
        host: String,
        port: u16,
        reason: String,
    },
}

/// Classification of background file operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Delete,
    Trash,
}

/// Status of an active background file operation (owned by guiops-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    pub id: u64,
    pub title: String,
    pub kind: JobKind,
    pub bytes_processed: u64,
    pub bytes_total: u64,
    pub progress_pct: u8,
    pub is_cancelled: bool,
}

impl JobStatus {
    pub fn new(id: u64, title: impl Into<String>, kind: JobKind, bytes_total: u64) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            bytes_processed: 0,
            bytes_total,
            progress_pct: 0,
            is_cancelled: false,
        }
    }

    pub fn update_progress(&mut self, processed: u64) {
        self.bytes_processed = processed;
        if self.bytes_total > 0 {
            self.progress_pct =
                ((processed as f64 / self.bytes_total as f64) * 100.0).min(100.0) as u8;
        }
    }
}

/// Operations recorded in the undo stack (owned by guiops-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoOperation {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    CreateDir {
        path: PathBuf,
    },
    Trash {
        original_paths: Vec<PathBuf>,
    },
    Copy {
        destinations: Vec<PathBuf>,
    },
    Move {
        from: Vec<PathBuf>,
        to: Vec<PathBuf>,
    },
}

/// Undo history stack (owned by guiops-worker).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoStack {
    pub operations: Vec<UndoOperation>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    pub fn push(&mut self, op: UndoOperation) {
        self.operations.push(op);
    }

    pub fn pop(&mut self) -> Option<UndoOperation> {
        self.operations.pop()
    }

    pub fn peek(&self) -> Option<&UndoOperation> {
        self.operations.last()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }
}

/// Kind of preview content (owned by inspect-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewKind {
    Text(String),
    Image(PathBuf),
    Binary {
        name: String,
        size: u64,
    },
    Loading,
    /// The file cannot be read here, with the reason to show.
    Unavailable(String),
}

/// Preview data for the selected file (owned by inspect-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewData {
    pub path: PathBuf,
    pub kind: PreviewKind,
}

/// Connection and health status for network mounts (owned by guinet-worker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkMountStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    TimedOut,
    MountBusy,
}

const JOB_TICK: Duration = Duration::from_millis(100);

/// How a finished directory load updates the browsing position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrival {
    Visit,
    Back,
    Forward,
    Reload,
}

struct Foreground {
    cx: AsyncApp,
    this: WeakEntity<AppState>,
}

/// Root application state shared across views and background tasks.
pub struct AppState {
    pub current_location: Location,
    pub entries: Arc<Vec<Entry>>,
    pub selection: Selection,
    pub history: History,
    pub settings: Settings,
    pub settings_open: bool,
    pub settings_warnings: Vec<SettingsWarning>,
    pub active_jobs: Vec<JobStatus>,        // Owned by guiops-worker
    pub undo_stack: UndoStack,              // Owned by guiops-worker
    pub preview_state: Option<PreviewData>, // Owned by inspect-worker
    pub search_query: Option<String>,       // Owned by inspect-worker
    pub network_status: NetworkMountStatus, // Owned by guinet-worker
    pub remote: RemoteSessions,
    /// Set when a connection needs an answer; the remote dialogs take it.
    pub remote_prompt: Option<RemotePrompt>,
    /// The profile being connected, while the attempt runs.
    pub connecting: Option<String>,
    pub status_message: Option<String>,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub show_hidden: bool,
    pub panels: PanelLayout,
    pub dir_watcher: Option<DirWatcher>,
    settings_path: Option<PathBuf>,
    foreground: Option<Foreground>,
    pending_navigation: Option<(Location, Arrival)>,
    reconnect_target: Option<Location>,
    connect_generation: u64,
    // Set while the status bar shows a failed read, so the next successful read clears that
    // message and no other.
    read_failure_shown: bool,
    // Replacing either task drops, and so cancels, the one it supersedes.
    load_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
}

impl AppState {
    /// Creates an isolated in-memory state suitable for headless tests (no disk config/keyring I/O).
    pub fn new(initial_location: Location) -> Self {
        Self {
            history: History::with_initial(initial_location.clone()),
            current_location: initial_location,
            entries: Arc::new(Vec::new()),
            selection: Selection::new(),
            settings: Settings::default(),
            settings_open: false,
            settings_warnings: Vec::new(),
            active_jobs: Vec::new(),
            undo_stack: UndoStack::new(),
            preview_state: None,
            search_query: None,
            network_status: NetworkMountStatus::Disconnected,
            remote: RemoteSessions::in_memory(),
            remote_prompt: None,
            connecting: None,
            status_message: None,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
            show_hidden: false,
            panels: PanelLayout::default(),
            dir_watcher: None,
            settings_path: None,
            foreground: None,
            pending_navigation: None,
            reconnect_target: None,
            connect_generation: 0,
            read_failure_shown: false,
            load_task: None,
            watch_task: None,
        }
    }

    /// Creates state backed by the user's native settings, profiles, known hosts, and keyring.
    pub fn load_native(initial_location: Location) -> Self {
        let mut state = Self::new(initial_location);
        if let Some(path) = settings_path() {
            state.settings_path = Some(path.clone());
            match Settings::load_from(&path) {
                Ok((settings, warnings)) => {
                    state.apply_loaded_settings(settings, warnings);
                }
                Err(err) => {
                    state.status_message = Some(format!("Cannot load settings: {err}"));
                }
            }
        }
        let (remote, problem) = RemoteSessions::load_native();
        state.remote = remote;
        if state.status_message.is_none() {
            state.status_message = problem;
        }
        state
    }

    fn apply_loaded_settings(&mut self, settings: Settings, warnings: Vec<SettingsWarning>) {
        self.show_hidden = settings.show_hidden;
        self.sort_column = settings.default_sort.column;
        self.sort_direction = settings.default_sort.direction;
        self.settings = settings;
        self.settings_warnings = warnings;
    }

    /// Associates a settings file path without touching disk (for tests or deferred saves).
    pub fn with_settings_path(mut self, path: Option<PathBuf>) -> Self {
        self.settings_path = path;
        self
    }

    /// Returns the configured settings persistence path, if any (`None` in isolated test mode).
    pub fn settings_path(&self) -> Option<&Path> {
        self.settings_path.as_deref()
    }

    /// Updates the settings persistence path (`None` disables writing to disk).
    pub fn set_settings_path(&mut self, path: Option<PathBuf>) {
        self.settings_path = path;
    }

    /// Loads settings from `path` and stores `path` as the persistence target.
    pub fn load_settings_from(&mut self, path: &Path) -> Result<Vec<SettingsWarning>, String> {
        let (settings, warnings) =
            Settings::load_from(path).map_err(|err| format!("Cannot load settings: {err}"))?;
        self.apply_loaded_settings(settings, warnings.clone());
        self.settings_path = Some(path.to_path_buf());
        Ok(warnings)
    }

    /// Saves the current `settings` to `settings_path` if one is configured.
    pub fn save_settings(&self) -> Result<(), String> {
        let Some(path) = &self.settings_path else {
            return Ok(());
        };
        self.settings
            .save_to(path)
            .map_err(|err| format!("Cannot save settings: {err}"))
    }

    /// Applies `f` to a copy of `settings`, persists if `settings_path` is set, and commits on success.
    pub fn update_settings(
        &mut self,
        f: impl FnOnce(&mut Settings) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut next = self.settings.clone();
        f(&mut next)?;
        if let Some(path) = &self.settings_path {
            next.save_to(path)
                .map_err(|err| format!("Cannot save settings: {err}"))?;
        }
        self.settings = next;
        Ok(())
    }

    /// Resets settings to defaults and persists them if `settings_path` is configured.
    pub fn reset_settings(&mut self) -> Result<(), String> {
        self.update_settings(|settings| {
            *settings = Settings::default();
            Ok(())
        })?;
        self.settings_warnings.clear();
        Ok(())
    }

    /// Returns the configured listing timeout for local/mounted directories.
    pub fn list_timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.settings.list_timeout_secs.max(1)))
    }

    pub fn set_list_timeout_secs(&mut self, secs: u32) -> Result<(), String> {
        if secs == 0 {
            return Err("List timeout must be at least 1 second.".into());
        }
        self.update_settings(|s| {
            s.list_timeout_secs = secs;
            Ok(())
        })
    }

    pub fn set_remote_poll(&mut self, enabled: bool, interval_secs: u32) -> Result<(), String> {
        self.update_settings(|s| {
            s.set_remote_poll_interval_secs(interval_secs)
                .map_err(str::to_owned)?;
            s.remote_poll_enabled = enabled;
            Ok(())
        })
    }

    pub fn set_remote_search_limits(&mut self, depth: u32, parallel: u32) -> Result<(), String> {
        if depth == 0 || parallel == 0 {
            return Err("Remote search depth and parallelism must be at least 1.".into());
        }
        self.update_settings(|s| {
            s.remote_search_depth = depth;
            s.remote_search_parallel = parallel;
            Ok(())
        })
    }

    pub fn add_bookmark(&mut self, name: impl Into<String>, path: PathBuf) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("Bookmark name cannot be empty.".into());
        }
        self.update_settings(|s| {
            s.add_bookmark(Bookmark { name, path });
            Ok(())
        })
    }

    pub fn rename_bookmark(&mut self, index: usize, name: impl Into<String>) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("Bookmark name cannot be empty.".into());
        }
        self.update_settings(|s| {
            if s.rename_bookmark(index, name) {
                Ok(())
            } else {
                Err("Bookmark index out of range.".into())
            }
        })
    }

    pub fn remove_bookmark(&mut self, index: usize) -> Result<Bookmark, String> {
        let mut removed = None;
        self.update_settings(|s| {
            removed = s.remove_bookmark(index);
            if removed.is_some() {
                Ok(())
            } else {
                Err("Bookmark index out of range.".into())
            }
        })?;
        Ok(removed.expect("checked above"))
    }

    pub fn reorder_bookmarks(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.update_settings(|s| {
            if s.reorder_bookmarks(from, to) {
                Ok(())
            } else {
                Err("Bookmark index out of range.".into())
            }
        })
    }

    pub fn open_settings(&mut self) {
        self.settings_open = true;
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
    }

    pub fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
    }

    /// Moves directory reads to the background executor and refreshes the listing when the
    /// current directory changes on disk. Without this call every read runs inline, which is
    /// what headless tests rely on.
    pub fn attach(&mut self, cx: &mut Context<Self>) {
        self.foreground = Some(Foreground {
            cx: cx.to_async(),
            this: cx.weak_entity(),
        });
    }

    /// Returns a reference to the first currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&Entry> {
        let ix = self.selected_index()?;
        self.entries.get(ix)
    }

    /// Starts watching the current directory for filesystem changes.
    pub fn watch_current_directory(&mut self, on_change: impl FnMut() + Send + 'static) {
        if let Location::Local(path) = &self.current_location {
            if let Ok(watcher) =
                DirWatcher::new(path, std::time::Duration::from_millis(50), on_change)
            {
                self.dir_watcher = Some(watcher);
            }
        }
    }

    /// Reloads the entries of the current location using current sort and hidden options,
    /// preserving selection by `Location`.
    pub fn reload_entries(&mut self) {
        self.begin_load(self.current_location.clone(), Arrival::Reload);
    }

    /// Refreshes the current listing (alias for [`Self::reload_entries`]).
    pub fn refresh_current(&mut self) {
        self.reload_entries();
    }

    /// Cancels any in-flight directory load without altering the current listing.
    pub fn cancel_load(&mut self) {
        self.load_task = None;
        self.pending_navigation = None;
        if self.connecting.is_none() && self.network_status == NetworkMountStatus::Connecting {
            self.sync_network_status();
        }
    }

    /// Lists `location` and, once the read succeeds, makes it the current location and adds it
    /// to history. A failed read reports in the status bar and leaves the listing as it was.
    pub fn navigate_to(&mut self, location: Location) {
        self.begin_load(location, Arrival::Visit);
    }

    /// Navigates back in history, returning true if there was a previous location.
    pub fn go_back(&mut self) -> bool {
        let Some(index) = self.history.index().checked_sub(1) else {
            return false;
        };
        let Some(prev) = self.history.entries().get(index).cloned() else {
            return false;
        };
        self.begin_load(prev, Arrival::Back);
        true
    }

    /// Navigates forward in history, returning true if there was a next location.
    pub fn go_forward(&mut self) -> bool {
        let Some(next) = self
            .history
            .entries()
            .get(self.history.index() + 1)
            .cloned()
        else {
            return false;
        };
        self.begin_load(next, Arrival::Forward);
        true
    }

    fn begin_load(&mut self, target: Location, arrival: Arrival) {
        // A reload must not cancel a navigation still being read; restart that navigation
        // instead so it picks up the current listing options.
        let (target, arrival) = match (arrival, self.pending_navigation.take()) {
            (Arrival::Reload, Some(pending)) => pending,
            _ => (target, arrival),
        };
        let options = ListOptions {
            show_hidden: self.show_hidden,
            sort_column: self.sort_column,
            sort_direction: self.sort_direction,
        };
        let timeout = self.list_timeout();
        let is_network = is_network_location(&target);
        let read: Box<dyn FnOnce() -> io::Result<Vec<Entry>> + Send> = match &target {
            Location::Local(_) => {
                let location = target.clone();
                Box::new(move || {
                    LocalBackend::with_list_timeout(timeout)
                        .list_with_options(&location, &options)
                        .map(|l| l.into_entries())
                        .map_err(into_io)
                })
            }
            Location::Remote { profile_id, .. } => {
                let Some(backend) = self.remote.backend(profile_id) else {
                    let name = self.remote.profile(profile_id).map(|p| p.name.clone());
                    self.network_status = NetworkMountStatus::Disconnected;
                    self.set_status(format!(
                        "Not connected to {}",
                        name.as_deref().unwrap_or("this server")
                    ));
                    return;
                };
                let location = target.clone();
                Box::new(move || {
                    backend
                        .list_with_options(&location, &options)
                        .map(|listing| listing.into_entries())
                        .map_err(into_io)
                })
            }
        };

        if self.foreground.is_some() && is_network {
            self.network_status = NetworkMountStatus::Connecting;
        }

        let Some(foreground) = &self.foreground else {
            self.finish_load(target, arrival, read());
            return;
        };
        let this = foreground.this.clone();
        let background = foreground.cx.background_executor().clone();
        if arrival != Arrival::Reload {
            self.pending_navigation = Some((target.clone(), arrival));
        }
        self.load_task = Some(foreground.cx.spawn(async move |cx| {
            let result = background.spawn(async move { read() }).await;
            this.update(cx, |state, cx| {
                state.finish_load(target, arrival, result);
                cx.notify();
            })
            .ok();
        }));
    }

    fn finish_load(&mut self, target: Location, arrival: Arrival, result: io::Result<Vec<Entry>>) {
        self.pending_navigation = None;
        match result {
            Ok(entries) => {
                self.arrive(target, arrival);
                self.set_entries(entries);
                self.sync_network_status();
                if std::mem::take(&mut self.read_failure_shown) {
                    self.clear_status();
                }
            }
            Err(err) => {
                self.read_failure_shown = true;
                self.set_status(format!("Cannot open {}: {err}", target.path().display()));
                match err.kind() {
                    io::ErrorKind::TimedOut => {
                        self.network_status = NetworkMountStatus::TimedOut;
                    }
                    io::ErrorKind::ResourceBusy => {
                        self.network_status = NetworkMountStatus::MountBusy;
                    }
                    _ if is_network_location(&target) => {
                        self.network_status = match &target {
                            Location::Remote { profile_id, .. } => match self
                                .remote
                                .state(profile_id)
                            {
                                Some(ConnectionState::Connected) => NetworkMountStatus::Connected,
                                Some(ConnectionState::Reconnecting) => {
                                    NetworkMountStatus::Connecting
                                }
                                Some(ConnectionState::Disconnected) | None => {
                                    NetworkMountStatus::Disconnected
                                }
                            },
                            Location::Local(_) => NetworkMountStatus::Disconnected,
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    fn arrive(&mut self, target: Location, arrival: Arrival) {
        match arrival {
            Arrival::Reload => {
                if self.dir_watcher.is_none() {
                    self.watch_for_changes();
                }
                return;
            }
            Arrival::Visit => self.history.visit(target.clone()),
            Arrival::Back => {
                self.history.back();
            }
            Arrival::Forward => {
                self.history.forward();
            }
        }
        self.current_location = target;
        self.selection.clear();
        self.preview_state = None;
        self.watch_for_changes();
    }

    /// Runs blocking `work` and hands its result to `done`. Attached state runs it on the
    /// background executor and calls `tick` on the main thread while it runs, so progress can
    /// be shown; otherwise everything runs inline.
    pub(crate) fn spawn_job<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        tick: impl Fn(&mut AppState) + 'static,
        done: impl FnOnce(&mut AppState, T) + 'static,
    ) {
        let Some(foreground) = &self.foreground else {
            let result = work();
            done(self, result);
            return;
        };
        let this = foreground.this.clone();
        let background = foreground.cx.background_executor().clone();
        foreground
            .cx
            .spawn(async move |cx| {
                let ticker = {
                    let this = this.clone();
                    let timer = background.clone();
                    cx.spawn(async move |cx| loop {
                        timer.timer(JOB_TICK).await;
                        let ticked = this.update(cx, |state, cx| {
                            tick(state);
                            cx.notify();
                        });
                        if ticked.is_err() {
                            break;
                        }
                    })
                };
                let result = background.spawn(async move { work() }).await;
                drop(ticker);
                this.update(cx, |state, cx| {
                    done(state, result);
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    fn watch_for_changes(&mut self) {
        self.dir_watcher = None;
        self.watch_task = None;
        let Some(foreground) = &self.foreground else {
            return;
        };
        let this = foreground.this.clone();
        let cx = foreground.cx.clone();
        let (changed, mut changes) = mpsc::unbounded();
        self.watch_current_directory(move || {
            let _ = changed.unbounded_send(());
        });
        if self.dir_watcher.is_none() {
            return;
        }
        self.watch_task = Some(cx.spawn(async move |cx| {
            while changes.next().await.is_some() {
                if this.update(cx, |state, _| state.reload_entries()).is_err() {
                    break;
                }
            }
        }));
    }

    /// Toggles sorting on the given column.
    /// Clicking the active column inverts direction; clicking another resets to Ascending.
    pub fn toggle_sort(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_direction = match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.sort_column = column;
            self.sort_direction = SortDirection::Ascending;
        }

        let mut entries = (*self.entries).clone();
        sort_entries_by(&mut entries, self.sort_column, self.sort_direction);
        self.set_entries(entries);
    }

    /// Toggles the visibility of hidden files and reloads the directory listing.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reload_entries();
    }

    /// Sets hidden file visibility, persists when configured, and reloads the directory listing if changed.
    pub fn set_show_hidden(&mut self, show: bool) {
        if self.settings.show_hidden != show {
            if let Err(err) = self.update_settings(|s| {
                s.show_hidden = show;
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        if self.show_hidden != show {
            self.show_hidden = show;
            self.reload_entries();
        }
    }

    /// Sets the sort column and direction, persists when configured, and re-sorts the current entries.
    pub fn set_sort(&mut self, column: SortColumn, direction: SortDirection) {
        let order = SortOrder { column, direction };
        if self.settings.default_sort != order {
            if let Err(err) = self.update_settings(|s| {
                s.default_sort = order;
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        self.sort_column = column;
        self.sort_direction = direction;
        let mut entries = (*self.entries).clone();
        sort_entries_by(&mut entries, self.sort_column, self.sort_direction);
        self.set_entries(entries);
    }

    /// Navigates up to the parent directory, returning true if parent exists.
    pub fn go_up(&mut self) -> bool {
        match self.current_location.parent() {
            Some(parent) if !parent.path().as_os_str().is_empty() => {
                self.navigate_to(parent);
                true
            }
            _ => false,
        }
    }

    /// Where a listed entry lives: on the server of the current location when it is remote.
    /// `Entry::location` alone is always local, which on a remote folder would name a local
    /// file of the same path.
    pub fn location_of(&self, entry: &Entry) -> Location {
        entry_location(&self.current_location, entry)
    }

    pub fn current_location(&self) -> &Location {
        &self.current_location
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.current_location, Location::Remote { .. })
    }

    /// Whether the current location is a remote backend or a local OS network mount.
    pub fn is_current_network_location(&self) -> bool {
        is_network_location(&self.current_location)
    }

    /// Saved profile for the current remote location, if any.
    pub fn current_profile(&self) -> Option<&ConnectionProfile> {
        match &self.current_location {
            Location::Remote { profile_id, .. } => self.remote.profile(profile_id),
            Location::Local(_) => None,
        }
    }

    /// Backend handle for `location` (`LocalBackend` with `list_timeout` for local/mounts, or
    /// connected remote backend for remote locations).
    pub fn backend_for(&self, location: &Location) -> Option<Arc<dyn Backend>> {
        match location {
            Location::Local(_) => Some(Arc::new(LocalBackend::with_list_timeout(
                self.list_timeout(),
            ))),
            Location::Remote { profile_id, .. } => self
                .remote
                .backend(profile_id)
                .map(|b| b as Arc<dyn Backend>),
        }
    }

    /// Backend handle for the current location.
    pub fn current_backend(&self) -> Option<Arc<dyn Backend>> {
        self.backend_for(&self.current_location)
    }

    /// The connection state of the current remote location, if it is one.
    pub fn remote_state(&self) -> Option<ConnectionState> {
        match &self.current_location {
            Location::Remote { profile_id, .. } => self.remote.state(profile_id),
            Location::Local(_) => None,
        }
    }

    /// Recomputes `network_status` from `connecting`, `current_location`, and `remote` state.
    pub fn sync_network_status(&mut self) {
        if let Some(conn_id) = &self.connecting {
            if matches!(&self.current_location, Location::Remote { profile_id, .. } if profile_id == conn_id)
            {
                self.network_status = NetworkMountStatus::Connecting;
                return;
            }
        }
        self.network_status = match &self.current_location {
            Location::Remote { profile_id, .. } => match self.remote.state(profile_id) {
                Some(ConnectionState::Connected) => NetworkMountStatus::Connected,
                Some(ConnectionState::Reconnecting) => NetworkMountStatus::Connecting,
                Some(ConnectionState::Disconnected) | None => NetworkMountStatus::Disconnected,
            },
            Location::Local(_) => {
                if is_network_location(&self.current_location) {
                    NetworkMountStatus::Connected
                } else {
                    NetworkMountStatus::Disconnected
                }
            }
        };
    }

    /// Opens saved server `profile_id` and browses its start path (or pending reconnect target).
    /// Asks for a password first when none is saved; `password` is one the user just typed,
    /// saved only if `remember`.
    pub fn connect(&mut self, profile_id: &str, password: Option<String>, remember: bool) {
        let Some(profile) = self.remote.profile(profile_id).cloned() else {
            return;
        };
        if self.remote.is_connected(profile_id) {
            let dest = self
                .reconnect_target
                .take()
                .filter(|loc| matches!(loc, Location::Remote { profile_id: pid, .. } if pid == profile_id))
                .unwrap_or_else(|| start_location(&profile));
            self.navigate_to(dest);
            return;
        }
        if password.is_none() && self.remote.needs_password(profile_id) {
            self.remote_prompt = Some(RemotePrompt::Password {
                profile_id: profile_id.to_owned(),
                error: None,
            });
            return;
        }
        let Some(work) = self.remote.connector(profile_id, password.clone()) else {
            return;
        };
        self.connect_generation = self.connect_generation.wrapping_add(1);
        let generation = self.connect_generation;
        self.connecting = Some(profile_id.to_owned());
        self.network_status = NetworkMountStatus::Connecting;
        self.set_status(format!("Connecting to {}…", profile.name));
        let id = profile_id.to_owned();
        self.spawn_job(
            work,
            |_| {},
            move |state, result| state.finish_connect(&id, generation, password, remember, result),
        );
    }

    fn finish_connect(
        &mut self,
        profile_id: &str,
        generation: u64,
        password: Option<String>,
        remember: bool,
        result: Result<ReconnectingBackend, BackendError>,
    ) {
        if self.connect_generation != generation {
            return;
        }
        self.connecting = None;
        let Some(profile) = self.remote.profile(profile_id).cloned() else {
            return;
        };
        let err = match result {
            Ok(backend) => {
                self.remote.add_connection(profile_id, backend);
                self.clear_status();
                if let (true, Some(password)) = (remember, &password) {
                    if let Err(message) = self.remote.remember_password(profile_id, password) {
                        self.set_status(message);
                    }
                }
                let dest = self
                    .reconnect_target
                    .take()
                    .filter(|loc| matches!(loc, Location::Remote { profile_id: pid, .. } if pid == profile_id))
                    .unwrap_or_else(|| start_location(&profile));
                self.network_status = NetworkMountStatus::Connected;
                self.navigate_to(dest);
                return;
            }
            Err(err) => err,
        };
        self.sync_network_status();
        if let Some(unknown) = err.host_key_unknown() {
            self.clear_status();
            self.remote_prompt = Some(RemotePrompt::TrustHostKey {
                profile_id: profile_id.to_owned(),
                unknown: unknown.clone(),
                password,
                remember,
            });
        } else if let Some(changed) = err.host_key_changed() {
            self.reconnect_target = None;
            self.set_status(format!("Connection to {} refused", profile.name));
            self.remote_prompt = Some(RemotePrompt::HostKeyChanged {
                host: changed.host.clone(),
                port: changed.port,
                presented: changed.fingerprint.clone(),
                stored: changed.stored.clone(),
            });
        } else if err.authentication_failed().is_some() {
            self.clear_status();
            self.remote_prompt = Some(RemotePrompt::Password {
                profile_id: profile_id.to_owned(),
                error: Some("The server refused the user name or password.".into()),
            });
        } else if let Some(tls) = err.tls_unavailable() {
            self.clear_status();
            self.remote_prompt = Some(RemotePrompt::TlsUnavailable {
                profile_id: profile_id.to_owned(),
                host: tls.host.clone(),
                port: tls.port,
                reply: tls.reply.clone(),
                password,
                remember,
            });
        } else if let Some(cert) = err.certificate_invalid() {
            self.reconnect_target = None;
            self.set_status(format!("Connection to {} refused", profile.name));
            self.remote_prompt = Some(RemotePrompt::CertificateInvalid {
                host: cert.host.clone(),
                port: cert.port,
                reason: cert.reason.clone(),
            });
        } else {
            self.reconnect_target = None;
            self.set_status(format!(
                "Cannot connect to {}: {}",
                profile.name,
                describe(&err)
            ));
        }
    }

    /// Submits a prompted password for `profile_id` and retries connecting.
    pub fn submit_password(&mut self, profile_id: &str, password: String, remember: bool) {
        if matches!(
            &self.remote_prompt,
            Some(RemotePrompt::Password { profile_id: id, .. }) if id == profile_id
        ) {
            self.remote_prompt = None;
        }
        self.connect(profile_id, Some(password), remember);
    }

    /// Stores the key the user agreed to trust and connects again.
    pub fn trust_and_connect(
        &mut self,
        profile_id: &str,
        unknown: &HostKeyUnknown,
        password: Option<String>,
        remember: bool,
    ) {
        self.remote_prompt = None;
        match self.remote.trust(unknown) {
            Ok(()) => self.connect(profile_id, password, remember),
            Err(message) => {
                self.reconnect_target = None;
                self.set_status(message);
            }
        }
    }

    /// Marks `profile_id` as allowing plain FTP (`allow_insecure = true`) and retries connecting.
    pub fn allow_insecure_and_connect(
        &mut self,
        profile_id: &str,
        password: Option<String>,
        remember: bool,
    ) {
        self.remote_prompt = None;
        let Some(mut profile) = self.remote.profile(profile_id).cloned() else {
            return;
        };
        profile.allow_insecure = true;
        if let Err(message) = self.remote.save_profile(profile, None) {
            self.reconnect_target = None;
            self.set_status(message);
            return;
        }
        self.connect(profile_id, password, remember);
    }

    /// Dismisses the active remote prompt and cancels any pending reconnect target.
    pub fn dismiss_remote_prompt(&mut self) {
        self.remote_prompt = None;
        self.reconnect_target = None;
    }

    /// Cancels an in-flight connection attempt so its completion is ignored.
    pub fn cancel_connect(&mut self) {
        self.connect_generation = self.connect_generation.wrapping_add(1);
        self.connecting = None;
        self.reconnect_target = None;
        self.sync_network_status();
        self.clear_status();
    }

    /// Closes the active session for `profile_id`.
    pub fn disconnect_profile(&mut self, profile_id: &str) {
        self.remote.disconnect(profile_id);
        if self.connecting.as_deref() == Some(profile_id) {
            self.connect_generation = self.connect_generation.wrapping_add(1);
            self.connecting = None;
        }
        if matches!(&self.current_location, Location::Remote { profile_id: cur, .. } if cur == profile_id)
        {
            self.load_task = None;
            self.pending_navigation = None;
            self.network_status = NetworkMountStatus::Disconnected;
            let name = self
                .remote
                .profile(profile_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "server".to_owned());
            self.set_status(format!("Disconnected from {name}"));
        }
    }

    /// Disconnects the current remote location, returning `true` if the current location is remote.
    pub fn disconnect_current(&mut self) -> bool {
        let Location::Remote { profile_id, .. } = &self.current_location else {
            return false;
        };
        let id = profile_id.clone();
        self.disconnect_profile(&id);
        true
    }

    /// Reconnects the current remote server (returning to `current_location`) or reloads a network
    /// mount. Returns `false` if the current location is a normal local directory.
    pub fn reconnect_current(&mut self) -> bool {
        match &self.current_location {
            Location::Remote { profile_id, .. } => {
                let id = profile_id.clone();
                self.reconnect_target = Some(self.current_location.clone());
                self.remote.disconnect(&id);
                self.connect(&id, None, false);
                true
            }
            Location::Local(_) if is_network_location(&self.current_location) => {
                self.reload_entries();
                true
            }
            Location::Local(_) => false,
        }
    }

    /// Replaces the listed entries while preserving selection by `Location`.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = Arc::new(entries);
        let listed: Vec<Location> = self.entries.iter().map(|e| self.location_of(e)).collect();
        self.selection.reload_locations(&listed);
    }

    /// Returns selected entries in listing order, working correctly for both local and remote
    /// locations (`Selection::selected_in_order` constructs local locations and misses remote ones).
    pub fn selected_entries(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| self.selection.contains(&self.location_of(e)))
            .collect()
    }

    /// Returns the `Location` of each selected entry in listing order.
    pub fn selected_locations(&self) -> Vec<Location> {
        self.entries
            .iter()
            .filter(|e| self.selection.contains(&self.location_of(e)))
            .map(|e| self.location_of(e))
            .collect()
    }

    /// Number of selected entries.
    pub fn selected_count(&self) -> usize {
        self.selection.len()
    }

    /// Total byte size of selected files (directories contribute 0 bytes).
    pub fn selected_total_size(&self) -> u64 {
        self.selected_entries()
            .into_iter()
            .filter(|entry| !entry.is_dir())
            .filter_map(|entry| entry.size)
            .sum()
    }

    /// Selects only `location`.
    pub fn select_location(&mut self, location: Location) {
        self.selection.select_only(location);
    }

    /// Toggles selection of `location`.
    pub fn toggle_select_location(&mut self, location: Location) {
        self.selection.toggle(location);
    }

    /// Selects every listed entry using `location_of` so both local and remote entries work.
    pub fn select_all_entries(&mut self) {
        let first = self.entries.first().map(|e| self.location_of(e));
        for entry in self.entries.iter() {
            self.selection.select(self.location_of(entry));
        }
        if self.selection.anchor().is_none() {
            if let Some(first) = first {
                self.selection.set_anchor(Some(first));
            }
        }
    }

    /// Clears the selection and anchor.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Selects the range between the current anchor and `target_index`, preserving the anchor.
    /// When `keep_existing` is false, replaces the prior selection with that range.
    pub fn select_range_to(&mut self, target_index: usize, keep_existing: bool) {
        let Some(target_entry) = self.entries.get(target_index) else {
            return;
        };
        let target_loc = self.location_of(target_entry);
        let anchor_idx = self.selection.anchor().and_then(|anchor| {
            self.entries
                .iter()
                .position(|e| &self.location_of(e) == anchor)
        });
        let (start, end, preserved_anchor) = match anchor_idx {
            Some(a_idx) => {
                let anchor_loc = self.location_of(&self.entries[a_idx]);
                if a_idx <= target_index {
                    (a_idx, target_index, anchor_loc)
                } else {
                    (target_index, a_idx, anchor_loc)
                }
            }
            None => (target_index, target_index, target_loc),
        };

        if !keep_existing {
            self.selection.clear();
        }
        for entry in &self.entries[start..=end] {
            self.selection.select(self.location_of(entry));
        }
        self.selection.set_anchor(Some(preserved_anchor));
    }

    /// Replaces (or merges into `base_locations`) the selection with the listed entries at `indices`.
    pub fn select_indices(
        &mut self,
        indices: impl IntoIterator<Item = usize>,
        base_locations: Option<&std::collections::HashSet<Location>>,
    ) {
        self.selection.clear();
        if let Some(base) = base_locations {
            for loc in base {
                self.selection.select(loc.clone());
            }
        }
        let mut first_touched = None;
        for ix in indices {
            if let Some(entry) = self.entries.get(ix) {
                let loc = self.location_of(entry);
                if first_touched.is_none() {
                    first_touched = Some(loc.clone());
                }
                self.selection.select(loc);
            }
        }
        if let Some(first) = first_touched {
            self.selection.set_anchor(Some(first));
        }
    }

    /// Returns whether `entry` (under `current_location`) is selected.
    pub fn is_entry_selected(&self, entry: &Entry) -> bool {
        self.selection.contains(&self.location_of(entry))
    }

    /// Returns the index of the first selected entry in the listed entries, if any.
    pub fn selected_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| self.selection.contains(&self.location_of(e)))
    }

    /// Selects the entry at the given index.
    pub fn select_index(&mut self, index: usize) {
        if let Some(entry) = self.entries.get(index) {
            self.selection.select_only(self.location_of(entry));
        }
    }

    /// Moves the selection to the next entry in the list, returning the new index.
    pub fn select_next(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let next_ix = match self.selected_index() {
            Some(ix) => (ix + 1).min(self.entries.len() - 1),
            None => 0,
        };
        self.select_index(next_ix);
        Some(next_ix)
    }

    /// Moves the selection to the previous entry in the list, returning the new index.
    pub fn select_previous(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let prev_ix = match self.selected_index() {
            Some(ix) => ix.saturating_sub(1),
            None => self.entries.len() - 1,
        };
        self.select_index(prev_ix);
        Some(prev_ix)
    }

    /// Moves the selection to the first entry in the list.
    pub fn select_first(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        self.select_index(0);
        Some(0)
    }

    /// Moves the selection to the last entry in the list.
    pub fn select_last(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let last_ix = self.entries.len() - 1;
        self.select_index(last_ix);
        Some(last_ix)
    }

    /// Moves the selection up by `page_size` entries.
    pub fn select_page_up(&mut self, page_size: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let ix = self.selected_index().unwrap_or(0);
        let target = ix.saturating_sub(page_size);
        self.select_index(target);
        Some(target)
    }

    /// Moves the selection down by `page_size` entries.
    pub fn select_page_down(&mut self, page_size: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let ix = self.selected_index().unwrap_or(0);
        let target = (ix + page_size).min(self.entries.len() - 1);
        self.select_index(target);
        Some(target)
    }

    /// Opens the currently selected entry: if directory, enters it; returns true if navigated.
    pub fn open_selected_entry(&mut self) -> bool {
        if let Some(ix) = self.selected_index() {
            if let Some(entry) = self.entries.get(ix).cloned() {
                if entry.is_dir() {
                    self.navigate_to(self.location_of(&entry));
                    return true;
                }
            }
        }
        false
    }

    /// Sets or clears the status message shown in the footer.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Appends a new active job.
    pub fn add_job(&mut self, job: JobStatus) {
        self.active_jobs.push(job);
    }

    /// Removes an active job by ID.
    pub fn remove_job(&mut self, id: u64) {
        self.active_jobs.retain(|j| j.id != id);
    }

    /// Updates preview state.
    pub fn set_preview(&mut self, preview: Option<PreviewData>) {
        self.preview_state = preview;
    }

    /// Updates search query filter.
    pub fn set_search_query(&mut self, query: Option<String>) {
        self.search_query = query;
    }

    /// Updates network mount status.
    pub fn set_network_status(&mut self, status: NetworkMountStatus) {
        self.network_status = status;
    }
}

/// Whether `location` refers to a remote backend or an OS-mounted network volume / UNC path.
pub fn is_network_location(location: &Location) -> bool {
    match location {
        Location::Remote { .. } => true,
        Location::Local(path) => {
            if volume_kind(path) == VolumeKind::Network {
                return true;
            }
            let raw = path.to_string_lossy();
            raw.starts_with(r"\\") || raw.starts_with("//")
        }
    }
}

/// Where `entry`, listed in folder `current`, lives: see [`AppState::location_of`].
pub fn entry_location(current: &Location, entry: &Entry) -> Location {
    match current {
        Location::Remote { profile_id, .. } => Location::Remote {
            profile_id: profile_id.clone(),
            path: entry.path.clone(),
        },
        Location::Local(_) => entry.location(),
    }
}

fn into_io(err: BackendError) -> io::Error {
    match err {
        BackendError::Io(err) => err,
        BackendError::Unsupported => io::ErrorKind::Unsupported.into(),
    }
}

/// The reason without the `I/O error:` prefix `BackendError` adds.
fn describe(err: &BackendError) -> String {
    match err {
        BackendError::Io(err) => err.to_string(),
        BackendError::Unsupported => "the server does not support this".into(),
    }
}
