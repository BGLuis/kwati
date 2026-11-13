use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{AsyncApp, Context, Task, WeakEntity};
use kwati_core::{
    list_dir_with_options, sort_entries_by, BackendError, ConnectionState, DirWatcher, Entry,
    History, HostKeyUnknown, ListOptions, Location, ReconnectingBackend, Selection, Settings,
    SortColumn, SortDirection,
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
    foreground: Option<Foreground>,
    pending_navigation: Option<(Location, Arrival)>,
    // Set while the status bar shows a failed read, so the next successful read clears that
    // message and no other.
    read_failure_shown: bool,
    // Replacing either task drops, and so cancels, the one it supersedes.
    load_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
}

impl AppState {
    pub fn new(initial_location: Location) -> Self {
        Self {
            history: History::with_initial(initial_location.clone()),
            current_location: initial_location,
            entries: Arc::new(Vec::new()),
            selection: Selection::new(),
            settings: Settings::default(),
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
            foreground: None,
            pending_navigation: None,
            read_failure_shown: false,
            load_task: None,
            watch_task: None,
        }
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

    /// Reloads the entries of the current local directory using current sort and hidden options.
    pub fn reload_entries(&mut self) {
        self.begin_load(self.current_location.clone(), Arrival::Reload);
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
        let read: Box<dyn FnOnce() -> io::Result<Vec<Entry>> + Send> = match &target {
            Location::Local(path) => {
                let path = path.clone();
                Box::new(move || list_dir_with_options(&path, &options).map(|l| l.into_entries()))
            }
            Location::Remote { profile_id, .. } => {
                let Some(backend) = self.remote.backend(profile_id) else {
                    let name = self.remote.profile(profile_id).map(|p| p.name.clone());
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
                if std::mem::take(&mut self.read_failure_shown) {
                    self.clear_status();
                }
            }
            Err(err) => {
                self.read_failure_shown = true;
                self.set_status(format!("Cannot open {}: {err}", target.path().display()));
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

    pub fn is_remote(&self) -> bool {
        matches!(self.current_location, Location::Remote { .. })
    }

    /// The connection state of the current remote location, if it is one.
    pub fn remote_state(&self) -> Option<ConnectionState> {
        match &self.current_location {
            Location::Remote { profile_id, .. } => self.remote.state(profile_id),
            Location::Local(_) => None,
        }
    }

    /// Opens saved server `profile_id` and browses its start path. Asks for a password first
    /// when none is saved; `password` is one the user just typed, saved only if `remember`.
    pub fn connect(&mut self, profile_id: &str, password: Option<String>, remember: bool) {
        let Some(profile) = self.remote.profile(profile_id).cloned() else {
            return;
        };
        if self.remote.is_connected(profile_id) {
            self.navigate_to(start_location(&profile));
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
        self.connecting = Some(profile_id.to_owned());
        self.set_status(format!("Connecting to {}…", profile.name));
        let id = profile_id.to_owned();
        self.spawn_job(
            work,
            |_| {},
            move |state, result| state.finish_connect(&id, password, remember, result),
        );
    }

    fn finish_connect(
        &mut self,
        profile_id: &str,
        password: Option<String>,
        remember: bool,
        result: Result<ReconnectingBackend, BackendError>,
    ) {
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
                self.navigate_to(start_location(&profile));
                return;
            }
            Err(err) => err,
        };
        if let Some(unknown) = err.host_key_unknown() {
            self.clear_status();
            self.remote_prompt = Some(RemotePrompt::TrustHostKey {
                profile_id: profile_id.to_owned(),
                unknown: unknown.clone(),
                password,
                remember,
            });
        } else if let Some(changed) = err.host_key_changed() {
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
        } else {
            self.set_status(format!(
                "Cannot connect to {}: {}",
                profile.name,
                describe(&err)
            ));
        }
    }

    /// Stores the key the user agreed to trust and connects again.
    pub fn trust_and_connect(
        &mut self,
        profile_id: &str,
        unknown: &HostKeyUnknown,
        password: Option<String>,
        remember: bool,
    ) {
        match self.remote.trust(unknown) {
            Ok(()) => self.connect(profile_id, password, remember),
            Err(message) => self.set_status(message),
        }
    }

    /// Replaces the listed entries.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = Arc::new(entries);
        let listed: Vec<Location> = self.entries.iter().map(|e| self.location_of(e)).collect();
        self.selection.reload_locations(&listed);
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
