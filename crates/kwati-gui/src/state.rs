use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{AsyncApp, Context, Task, WeakEntity};
use kwati_core::{
    default_icons_path, list_trash, settings_path, sort_entries_with, volume_kind, Backend,
    BackendError, ConnectionProfile, ConnectionState, DirWatcher, Entry, FolderSettingsScope,
    History, HostKeyUnknown, IconColor, IconConfig, IconGroup, IconResolver, IconSource,
    ListOptions, LocalBackend, Location, PlaceEntry, ReconnectingBackend, RemoteEditWatcher,
    RemoteFileCache, RemoteStreamServer, ResolvedFolderSettings, Selection, Settings,
    SettingsWarning, SortColumn, SortDirection, SortOrder, TrashItem, UndoStack, ViewMode,
    VolumeKind,
};

use crate::actions::remote_files::{
    default_remote_opener, sync_remote_edit, RemoteExternalOpener, StreamLauncherResolver,
};
use crate::layout::PanelLayout;
use crate::motion::{set_active_motion_preference, JOB_COMPLETE_HOLD, QUICK};
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
    Open,
}

/// Completed background job kept briefly for the `secondary` completion pulse and 1s fade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedJob {
    pub id: u64,
    pub title: String,
    pub kind: JobKind,
    pub completed_at: Instant,
}

/// Direction of navigation for list entrance transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavigationDirection {
    #[default]
    Initial,
    Visit,
    Back,
    Forward,
    Reload,
}

/// Distinguishes direct user modifications from watcher or remote polling updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntryChangeOrigin {
    #[default]
    UserAction,
    WatcherOrPoll,
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

/// Sort columns supported by the trash browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrashSortColumn {
    Name,
    OriginalLocation,
    #[default]
    DeletedAt,
}

/// Root application state shared across views and background tasks.
pub struct AppState {
    pub current_location: Location,
    pub entries: Arc<Vec<Entry>>,
    pub selection: Selection,
    pub trash_active: bool,
    pub trash_items: Arc<Vec<TrashItem>>,
    pub trash_selection: HashSet<OsString>,
    pub trash_selection_anchor: Option<OsString>,
    pub trash_loading: bool,
    pub trash_revision: u64,
    pub trash_sort_column: TrashSortColumn,
    pub trash_sort_direction: SortDirection,
    pub history: History,
    pub settings: Settings,
    pub settings_open: bool,
    pub settings_warnings: Vec<SettingsWarning>,
    pub icon_config: IconConfig,
    pub icon_resolver: Arc<IconResolver>,
    pub active_jobs: Vec<JobStatus>, // Owned by guiops-worker
    pub completed_jobs: Vec<CompletedJob>,
    pub undo_stack: UndoStack,              // Owned by guiops-worker
    pub preview_state: Option<PreviewData>, // Owned by inspect-worker
    pub search_query: Option<String>,       // Owned by inspect-worker
    pub network_status: NetworkMountStatus, // Owned by guinet-worker
    pub connecting_since: Option<Instant>,
    pub remote: RemoteSessions,
    /// Set when a connection needs an answer; the remote dialogs take it.
    pub remote_prompt: Option<RemotePrompt>,
    /// The profile being connected, while the attempt runs.
    pub connecting: Option<String>,
    pub status_message: Option<String>,
    pub status_revision: u64,
    pub status_is_completion: bool,
    pub listing_revision: u64,
    pub last_arrival: NavigationDirection,
    pub last_change_origin: EntryChangeOrigin,
    pending_change_origin: EntryChangeOrigin,
    pub highlighted_inserts: Vec<(Location, Instant)>,
    pub recent_deletions: Vec<(Location, Instant)>,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub show_hidden: bool,
    pub folders_first: bool,
    pub hidden_last: bool,
    pub view_mode: ViewMode,
    pub panels: PanelLayout,
    pub dir_watcher: Option<DirWatcher>,
    pub volume_mounts: Arc<Vec<PathBuf>>,
    volume_mounts_locked: bool,
    pub remote_cache: Arc<RemoteFileCache>,
    remote_stream_server: Arc<OnceLock<Result<RemoteStreamServer, String>>>,
    remote_edit_watchers: HashMap<Location, RemoteEditWatcher>,
    remote_edit_tx: mpsc::UnboundedSender<Location>,
    remote_edit_rx: Option<mpsc::UnboundedReceiver<Location>>,
    remote_edit_debounce: Duration,
    uploading_remote_edits: HashSet<Location>,
    pending_remote_edit_resync: HashSet<Location>,
    remote_opener: Option<RemoteExternalOpener>,
    stream_launcher_override: Option<StreamLauncherResolver>,
    settings_path: Option<PathBuf>,
    icons_path: Option<PathBuf>,
    foreground: Option<Foreground>,
    pending_navigation: Option<(Location, Arrival)>,
    reconnect_target: Option<Location>,
    connect_generation: u64,
    // Set while the status bar shows a failed read, so the next successful read clears that
    // message and no other.
    read_failure_shown: bool,
    // Replacing either task drops, and so cancels, the one it supersedes.
    load_task: Option<Task<()>>,
    trash_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
    remote_edit_task: Option<Task<()>>,
}

impl AppState {
    /// Creates an isolated in-memory state suitable for headless tests (no disk config/keyring I/O).
    pub fn new(initial_location: Location) -> Self {
        let settings = Settings::default();
        let resolved = settings.resolve_folder_settings(&initial_location);
        set_active_motion_preference(settings.motion_preference);
        let (remote_edit_tx, remote_edit_rx) = mpsc::unbounded();
        Self {
            history: History::with_initial(initial_location.clone()),
            current_location: initial_location,
            entries: Arc::new(Vec::new()),
            selection: Selection::new(),
            trash_active: false,
            trash_items: Arc::new(Vec::new()),
            trash_selection: HashSet::new(),
            trash_selection_anchor: None,
            trash_loading: false,
            trash_revision: 0,
            trash_sort_column: TrashSortColumn::DeletedAt,
            trash_sort_direction: SortDirection::Descending,
            settings,
            settings_open: false,
            settings_warnings: Vec::new(),
            icon_config: IconConfig::default(),
            icon_resolver: Arc::new(IconResolver::new()),
            active_jobs: Vec::new(),
            completed_jobs: Vec::new(),
            undo_stack: UndoStack::new(),
            preview_state: None,
            search_query: None,
            network_status: NetworkMountStatus::Disconnected,
            connecting_since: None,
            remote: RemoteSessions::in_memory(),
            remote_prompt: None,
            connecting: None,
            status_message: None,
            status_revision: 0,
            status_is_completion: false,
            listing_revision: 0,
            last_arrival: NavigationDirection::Initial,
            last_change_origin: EntryChangeOrigin::UserAction,
            pending_change_origin: EntryChangeOrigin::UserAction,
            highlighted_inserts: Vec::new(),
            recent_deletions: Vec::new(),
            sort_column: resolved.sort_column,
            sort_direction: resolved.sort_direction,
            show_hidden: resolved.show_hidden,
            folders_first: resolved.folders_first,
            hidden_last: resolved.hidden_last,
            view_mode: resolved.view_mode,
            panels: PanelLayout::default(),
            dir_watcher: None,
            volume_mounts: Arc::new(Vec::new()),
            volume_mounts_locked: false,
            remote_cache: Arc::new(RemoteFileCache::new()),
            remote_stream_server: Arc::new(OnceLock::new()),
            remote_edit_watchers: HashMap::new(),
            remote_edit_tx,
            remote_edit_rx: Some(remote_edit_rx),
            remote_edit_debounce: Duration::from_millis(500),
            uploading_remote_edits: HashSet::new(),
            pending_remote_edit_resync: HashSet::new(),
            remote_opener: None,
            stream_launcher_override: None,
            settings_path: None,
            icons_path: None,
            foreground: None,
            pending_navigation: None,
            reconnect_target: None,
            connect_generation: 0,
            read_failure_shown: false,
            load_task: None,
            trash_task: None,
            watch_task: None,
            remote_edit_task: None,
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
        if let Some(path) = default_icons_path() {
            state.icons_path = Some(path.clone());
            match IconConfig::load_from(&path) {
                Ok(icon_config) => {
                    state.icon_config = icon_config;
                }
                Err(err) if state.status_message.is_none() => {
                    state.status_message = Some(format!("Cannot load icon preferences: {err}"));
                }
                Err(_) => {}
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
        let resolved = settings.resolve_folder_settings(&self.current_location);
        self.show_hidden = resolved.show_hidden;
        self.folders_first = resolved.folders_first;
        self.hidden_last = resolved.hidden_last;
        self.sort_column = resolved.sort_column;
        self.sort_direction = resolved.sort_direction;
        self.view_mode = resolved.view_mode;
        self.panels.sidebar_open = settings.show_sidebar;
        set_active_motion_preference(settings.motion_preference);
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
        set_active_motion_preference(next.motion_preference);
        self.settings = next;
        Ok(())
    }

    /// Resets settings to defaults and persists them if `settings_path` is configured.
    pub fn reset_settings(&mut self) -> Result<(), String> {
        self.update_settings(|settings| {
            *settings = Settings::default();
            Ok(())
        })?;
        let resolved = self
            .settings
            .resolve_folder_settings(&self.current_location);
        self.view_mode = resolved.view_mode;
        self.sort_column = resolved.sort_column;
        self.sort_direction = resolved.sort_direction;
        self.folders_first = resolved.folders_first;
        self.hidden_last = resolved.hidden_last;
        if self.show_hidden != resolved.show_hidden {
            self.show_hidden = resolved.show_hidden;
            self.reload_entries();
        } else {
            self.re_sort_entries();
        }
        self.panels.sidebar_open = self.settings.show_sidebar;
        self.settings_warnings.clear();
        Ok(())
    }

    /// Associates an `icons.toml` file path without touching disk (for tests).
    pub fn with_icons_path(mut self, path: Option<PathBuf>) -> Self {
        self.icons_path = path;
        self
    }

    /// Associates a custom [`IconResolver`] (for tests with temporary icon theme directories).
    pub fn with_icon_resolver(mut self, resolver: Arc<IconResolver>) -> Self {
        self.icon_resolver = resolver;
        self
    }

    /// Returns the configured `icons.toml` persistence path, if any.
    pub fn icons_path(&self) -> Option<&Path> {
        self.icons_path.as_deref()
    }

    /// Updates `icons_path` (`None` disables writing `icons.toml` to disk).
    pub fn set_icons_path(&mut self, path: Option<PathBuf>) {
        self.icons_path = path;
    }

    /// Loads icon preferences and presets from `path` and sets it as the persistence target.
    pub fn load_icons_from(&mut self, path: &Path) -> Result<(), String> {
        let config = IconConfig::load_from(path)
            .map_err(|err| format!("Cannot load icon preferences: {err}"))?;
        self.icon_config = config;
        self.icons_path = Some(path.to_path_buf());
        self.icon_resolver.clear_cache();
        Ok(())
    }

    /// Applies `f` to a copy of `icon_config`, persists if `icons_path` is set, and commits.
    pub fn update_icon_config(
        &mut self,
        f: impl FnOnce(&mut IconConfig) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut next = self.icon_config.clone();
        f(&mut next)?;
        if let Some(path) = &self.icons_path {
            next.save_to(path)
                .map_err(|err| format!("Cannot save icon preferences: {err}"))?;
        }
        self.icon_config = next;
        self.icon_resolver.clear_cache();
        Ok(())
    }

    pub fn set_icon_base(&mut self, source: IconSource) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.base = source;
            Ok(())
        })
    }

    pub fn set_icon_group_override(
        &mut self,
        group: IconGroup,
        source: Option<IconSource>,
    ) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.set_group_override(group, source);
            Ok(())
        })
    }

    pub fn set_icon_type_override(
        &mut self,
        type_key: &str,
        source: Option<IconSource>,
    ) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.set_type_override(type_key, source);
            Ok(())
        })
    }

    pub fn set_icon_base_color(&mut self, color: Option<IconColor>) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.set_base_color(color);
            Ok(())
        })
    }

    pub fn set_icon_group_color_override(
        &mut self,
        group: IconGroup,
        color: Option<IconColor>,
    ) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.set_group_color_override(group, color);
            Ok(())
        })
    }

    pub fn set_icon_type_color_override(
        &mut self,
        type_key: &str,
        color: Option<IconColor>,
    ) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.active.set_type_color_override(type_key, color);
            Ok(())
        })
    }

    pub fn save_icon_preset(&mut self, name: impl Into<String>) -> Result<(), String> {
        self.update_icon_config(|cfg| cfg.save_preset(name))
    }

    pub fn apply_icon_preset(&mut self, name: &str) -> Result<(), String> {
        self.update_icon_config(|cfg| cfg.apply_preset(name))
    }

    pub fn rename_icon_preset(
        &mut self,
        old_name: &str,
        new_name: impl Into<String>,
    ) -> Result<(), String> {
        self.update_icon_config(|cfg| cfg.rename_preset(old_name, new_name))
    }

    pub fn delete_icon_preset(&mut self, name: &str) -> Result<bool, String> {
        let mut deleted = false;
        self.update_icon_config(|cfg| {
            deleted = cfg.delete_preset(name);
            Ok(())
        })?;
        Ok(deleted)
    }

    pub fn reset_icon_preferences(&mut self) -> Result<(), String> {
        self.update_icon_config(|cfg| {
            cfg.reset_active();
            Ok(())
        })
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

    pub fn places(&self) -> Vec<PlaceEntry> {
        self.settings.resolved_places()
    }

    pub fn is_place_pinned(&self, path: &Path) -> bool {
        self.places()
            .iter()
            .any(|entry| entry.path().is_some_and(|p| p == path))
    }

    pub fn add_place(&mut self, name: impl Into<String>, path: PathBuf) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("Place name cannot be empty.".into());
        }
        self.update_settings(|s| {
            s.add_place(name, path);
            Ok(())
        })
    }

    pub fn rename_place(&mut self, index: usize, name: impl Into<String>) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("Place name cannot be empty.".into());
        }
        self.update_settings(|s| {
            if s.rename_place(index, name) {
                Ok(())
            } else {
                Err("Place cannot be renamed.".into())
            }
        })
    }

    pub fn remove_place(&mut self, index: usize) -> Result<PlaceEntry, String> {
        let mut removed = None;
        self.update_settings(|s| {
            removed = Some(s.remove_place(index).map_err(str::to_string)?);
            Ok(())
        })?;
        Ok(removed.expect("checked above"))
    }

    pub fn reorder_places(&mut self, from: usize, to: usize) -> Result<(), String> {
        self.update_settings(|s| {
            if s.reorder_places(from, to) {
                Ok(())
            } else {
                Err("Place index out of range.".into())
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
        if let Some(mut rx) = self.remote_edit_rx.take() {
            let this = cx.weak_entity();
            self.remote_edit_task = Some(cx.spawn(async move |_, cx| {
                while let Some(location) = rx.next().await {
                    let updated = this.update(cx, |state, cx| {
                        sync_remote_edit(state, location);
                        cx.notify();
                    });
                    if updated.is_err() {
                        break;
                    }
                }
            }));
        }
    }

    pub(crate) fn stream_server_cell(&self) -> Arc<OnceLock<Result<RemoteStreamServer, String>>> {
        Arc::clone(&self.remote_stream_server)
    }

    pub(crate) fn remote_opener(&self) -> RemoteExternalOpener {
        self.remote_opener
            .clone()
            .unwrap_or_else(|| Arc::new(default_remote_opener))
    }

    pub(crate) fn stream_launcher_override(&self) -> Option<StreamLauncherResolver> {
        self.stream_launcher_override.clone()
    }

    pub(crate) fn remote_edit_sender(&self) -> mpsc::UnboundedSender<Location> {
        self.remote_edit_tx.clone()
    }

    pub(crate) fn remote_edit_debounce(&self) -> Duration {
        self.remote_edit_debounce
    }

    pub(crate) fn has_remote_edit_watcher(&self, location: &Location) -> bool {
        self.remote_edit_watchers.contains_key(location)
    }

    pub(crate) fn insert_remote_edit_watcher(
        &mut self,
        location: Location,
        watcher: RemoteEditWatcher,
    ) {
        self.remote_edit_watchers.insert(location, watcher);
    }

    pub(crate) fn begin_remote_edit_upload(&mut self, location: &Location) -> bool {
        if self.uploading_remote_edits.contains(location) {
            self.pending_remote_edit_resync.insert(location.clone());
            return false;
        }
        self.uploading_remote_edits.insert(location.clone());
        true
    }

    pub(crate) fn finish_remote_edit_upload(&mut self, location: &Location) -> bool {
        self.uploading_remote_edits.remove(location);
        self.pending_remote_edit_resync.remove(location)
    }

    #[doc(hidden)]
    pub fn set_remote_opener_for_test(&mut self, opener: RemoteExternalOpener) {
        self.remote_opener = Some(opener);
    }

    #[doc(hidden)]
    pub fn set_stream_launcher_for_test(&mut self, resolver: StreamLauncherResolver) {
        self.stream_launcher_override = Some(resolver);
    }

    #[doc(hidden)]
    pub fn set_remote_edit_debounce_for_test(&mut self, debounce: Duration) {
        self.remote_edit_debounce = debounce;
    }

    /// Updates cached local volume mount points from a background refresh unless locked by tests.
    pub fn set_volume_mounts(&mut self, mounts: Vec<PathBuf>) {
        if self.volume_mounts_locked {
            return;
        }
        if *self.volume_mounts != mounts {
            self.volume_mounts = Arc::new(mounts);
        }
    }

    /// Returns cached local volume mount points used for same- vs cross-volume drop decisions.
    pub fn volume_mounts(&self) -> &[PathBuf] {
        &self.volume_mounts
    }

    #[doc(hidden)]
    pub fn set_volume_mounts_for_test(&mut self, mounts: Vec<PathBuf>) {
        self.volume_mounts_locked = true;
        self.volume_mounts = Arc::new(mounts);
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
        if self.trash_active {
            self.reload_trash();
            return;
        }
        self.pending_change_origin = EntryChangeOrigin::UserAction;
        self.begin_load(self.current_location.clone(), Arrival::Reload);
    }

    /// Reloads from a filesystem watcher or background remote poll (no accent insert flash).
    pub fn reload_entries_from_background(&mut self) {
        self.pending_change_origin = EntryChangeOrigin::WatcherOrPoll;
        self.begin_load(self.current_location.clone(), Arrival::Reload);
    }

    /// Refreshes the current listing (alias for [`Self::reload_entries`]).
    pub fn refresh_current(&mut self) {
        self.reload_entries();
    }

    /// Opens the in-app trash browser and starts loading trashed items.
    pub fn open_trash(&mut self) {
        self.load_task = None;
        self.pending_navigation = None;
        self.trash_active = true;
        self.trash_selection.clear();
        self.trash_selection_anchor = None;
        self.preview_state = None;
        self.reload_trash();
    }

    /// Leaves the trash browser and refreshes the underlying folder listing.
    pub fn exit_trash(&mut self) {
        if !self.trash_active {
            return;
        }
        self.trash_active = false;
        self.trash_task = None;
        self.trash_loading = false;
        self.trash_selection.clear();
        self.trash_selection_anchor = None;
        self.reload_entries();
    }

    /// Reloads the system trash items (on the background executor when attached).
    pub fn reload_trash(&mut self) {
        let Some(foreground) = &self.foreground else {
            self.finish_trash_load(list_trash());
            return;
        };
        self.trash_loading = true;
        let this = foreground.this.clone();
        let background = foreground.cx.background_executor().clone();
        self.trash_task = Some(foreground.cx.spawn(async move |cx| {
            let result = background.spawn(async move { list_trash() }).await;
            this.update(cx, |state, cx| {
                state.finish_trash_load(result);
                cx.notify();
            })
            .ok();
        }));
    }

    fn finish_trash_load(&mut self, result: io::Result<Vec<TrashItem>>) {
        self.trash_loading = false;
        match result {
            Ok(mut items) => {
                sort_trash_items(
                    &mut items,
                    self.trash_sort_column,
                    self.trash_sort_direction,
                );
                self.set_trash_items(items);
                if std::mem::take(&mut self.read_failure_shown) {
                    self.clear_status();
                }
            }
            Err(err) => {
                self.read_failure_shown = true;
                self.set_status(format!("Cannot list trash: {err}"));
            }
        }
    }

    /// Replaces the listed trash items while preserving selection by `TrashItem::id`.
    pub fn set_trash_items(&mut self, items: Vec<TrashItem>) {
        self.trash_revision = self.trash_revision.wrapping_add(1);
        let valid_ids: HashSet<OsString> = items.iter().map(|i| i.id.clone()).collect();
        self.trash_selection.retain(|id| valid_ids.contains(id));
        if !self
            .trash_selection_anchor
            .as_ref()
            .is_some_and(|a| valid_ids.contains(a))
        {
            self.trash_selection_anchor = None;
        }
        self.trash_items = Arc::new(items);
    }

    /// Toggles sorting on a trash view column.
    pub fn toggle_trash_sort(&mut self, column: TrashSortColumn) {
        if self.trash_sort_column == column {
            self.trash_sort_direction = match self.trash_sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.trash_sort_column = column;
            self.trash_sort_direction = match column {
                TrashSortColumn::DeletedAt => SortDirection::Descending,
                TrashSortColumn::Name | TrashSortColumn::OriginalLocation => {
                    SortDirection::Ascending
                }
            };
        }
        let mut items = (*self.trash_items).clone();
        sort_trash_items(
            &mut items,
            self.trash_sort_column,
            self.trash_sort_direction,
        );
        self.set_trash_items(items);
    }

    pub fn selected_trash_items(&self) -> Vec<TrashItem> {
        if self.trash_selection.is_empty() {
            return Vec::new();
        }
        self.trash_items
            .iter()
            .filter(|item| self.trash_selection.contains(&item.id))
            .cloned()
            .collect()
    }

    pub fn selected_trash_index(&self) -> Option<usize> {
        if self.trash_selection.is_empty() {
            return None;
        }
        self.trash_items
            .iter()
            .position(|item| self.trash_selection.contains(&item.id))
    }

    pub fn select_trash_only(&mut self, id: OsString) {
        self.trash_selection.clear();
        self.trash_selection_anchor = Some(id.clone());
        self.trash_selection.insert(id);
    }

    pub fn toggle_trash_item(&mut self, id: OsString) {
        if !self.trash_selection.remove(&id) {
            self.trash_selection.insert(id.clone());
            self.trash_selection_anchor = Some(id);
        }
    }

    pub fn select_trash_index(&mut self, index: usize) {
        if let Some(item) = self.trash_items.get(index) {
            self.select_trash_only(item.id.clone());
        }
    }

    pub fn select_trash_range_to(&mut self, target_index: usize, keep_existing: bool) {
        let Some(target_item) = self.trash_items.get(target_index) else {
            return;
        };
        let target_id = target_item.id.clone();
        let anchor_idx = self
            .trash_selection_anchor
            .as_ref()
            .and_then(|anchor| self.trash_items.iter().position(|item| &item.id == anchor));
        let (start, end, preserved_anchor) = match anchor_idx {
            Some(a_idx) => {
                let anchor_id = self.trash_items[a_idx].id.clone();
                if a_idx <= target_index {
                    (a_idx, target_index, anchor_id)
                } else {
                    (target_index, a_idx, anchor_id)
                }
            }
            None => (target_index, target_index, target_id),
        };
        if !keep_existing {
            self.trash_selection.clear();
        }
        for item in &self.trash_items[start..=end] {
            self.trash_selection.insert(item.id.clone());
        }
        self.trash_selection_anchor = Some(preserved_anchor);
    }

    pub fn select_all_trash_items(&mut self) {
        for item in self.trash_items.iter() {
            self.trash_selection.insert(item.id.clone());
        }
        if self.trash_selection_anchor.is_none() {
            if let Some(first) = self.trash_items.first() {
                self.trash_selection_anchor = Some(first.id.clone());
            }
        }
    }

    pub fn clear_trash_selection(&mut self) {
        self.trash_selection.clear();
        self.trash_selection_anchor = None;
    }

    pub fn select_next_trash(&mut self) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        let next_ix = match self.selected_trash_index() {
            Some(ix) => (ix + 1).min(self.trash_items.len() - 1),
            None => 0,
        };
        self.select_trash_index(next_ix);
        Some(next_ix)
    }

    pub fn select_previous_trash(&mut self) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        let prev_ix = match self.selected_trash_index() {
            Some(ix) => ix.saturating_sub(1),
            None => self.trash_items.len() - 1,
        };
        self.select_trash_index(prev_ix);
        Some(prev_ix)
    }

    pub fn select_first_trash(&mut self) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        self.select_trash_index(0);
        Some(0)
    }

    pub fn select_last_trash(&mut self) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        let last_ix = self.trash_items.len() - 1;
        self.select_trash_index(last_ix);
        Some(last_ix)
    }

    pub fn select_page_up_trash(&mut self, page_size: usize) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        let ix = self.selected_trash_index().unwrap_or(0);
        let target = ix.saturating_sub(page_size);
        self.select_trash_index(target);
        Some(target)
    }

    pub fn select_page_down_trash(&mut self, page_size: usize) -> Option<usize> {
        if self.trash_items.is_empty() {
            return None;
        }
        let ix = self.selected_trash_index().unwrap_or(0);
        let target = (ix + page_size).min(self.trash_items.len() - 1);
        self.select_trash_index(target);
        Some(target)
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
        if self.trash_active {
            self.exit_trash();
            return true;
        }
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
        let target_settings = self.settings.resolve_folder_settings(&target);
        let options = ListOptions {
            show_hidden: target_settings.show_hidden,
            sort_column: target_settings.sort_column,
            sort_direction: target_settings.sort_direction,
            folders_first: target_settings.folders_first,
            hidden_last: target_settings.hidden_last,
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
            self.set_network_status(NetworkMountStatus::Connecting);
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
        let origin = std::mem::take(&mut self.pending_change_origin);
        match result {
            Ok(entries) => {
                self.last_arrival = match arrival {
                    Arrival::Visit => NavigationDirection::Visit,
                    Arrival::Back => NavigationDirection::Back,
                    Arrival::Forward => NavigationDirection::Forward,
                    Arrival::Reload => NavigationDirection::Reload,
                };
                self.last_change_origin = origin;
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
                let next_net = match err.kind() {
                    io::ErrorKind::TimedOut => Some(NetworkMountStatus::TimedOut),
                    io::ErrorKind::ResourceBusy => Some(NetworkMountStatus::MountBusy),
                    _ if is_network_location(&target) => Some(match &target {
                        Location::Remote { profile_id, .. } => {
                            match self.remote.state(profile_id) {
                                Some(ConnectionState::Connected) => NetworkMountStatus::Connected,
                                Some(ConnectionState::Reconnecting) => {
                                    NetworkMountStatus::Connecting
                                }
                                Some(ConnectionState::Disconnected) | None => {
                                    NetworkMountStatus::Disconnected
                                }
                            }
                        }
                        Location::Local(_) => NetworkMountStatus::Disconnected,
                    }),
                    _ => None,
                };
                if let Some(status) = next_net {
                    self.set_network_status(status);
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
        self.trash_active = false;
        self.trash_task = None;
        self.trash_loading = false;
        let resolved = self.settings.resolve_folder_settings(&target);
        self.view_mode = resolved.view_mode;
        self.sort_column = resolved.sort_column;
        self.sort_direction = resolved.sort_direction;
        self.show_hidden = resolved.show_hidden;
        self.folders_first = resolved.folders_first;
        self.hidden_last = resolved.hidden_last;
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
                if this
                    .update(cx, |state, _| state.reload_entries_from_background())
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn re_sort_entries(&mut self) {
        let mut entries = (*self.entries).clone();
        sort_entries_with(
            &mut entries,
            self.sort_column,
            self.sort_direction,
            self.folders_first,
            self.hidden_last,
        );
        self.set_entries(entries);
    }

    /// Toggles sorting on the given column for the active scope (current folder or global).
    /// Clicking the active column inverts direction; clicking another resets to Ascending.
    pub fn toggle_sort(&mut self, column: SortColumn) {
        let direction = if self.sort_column == column {
            match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            }
        } else {
            SortDirection::Ascending
        };
        self.set_current_sort(column, direction);
    }

    /// Toggles the visibility of hidden files for the active scope and reloads the directory listing.
    pub fn toggle_hidden(&mut self) {
        self.set_current_show_hidden(!self.show_hidden);
    }

    /// Sets global hidden file visibility, persists when configured, and reloads the directory
    /// listing if the effective value for the current folder changed.
    pub fn set_show_hidden(&mut self, show: bool) {
        let clear_overrides =
            self.settings.folder_settings_scope == FolderSettingsScope::AllFolders;
        if self.settings.show_hidden != show || clear_overrides {
            if let Err(err) = self.update_settings(|s| {
                s.show_hidden = show;
                if clear_overrides {
                    s.clear_all_folder_show_hidden_overrides();
                }
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        let effective = self
            .settings
            .resolve_folder_settings(&self.current_location)
            .show_hidden;
        if self.show_hidden != effective {
            self.show_hidden = effective;
            self.reload_entries();
        }
    }

    /// Sets global `folders_first` placement, persists, and re-sorts when effective for the current folder.
    pub fn set_folders_first(&mut self, folders_first: bool) {
        let clear_overrides =
            self.settings.folder_settings_scope == FolderSettingsScope::AllFolders;
        if self.settings.folders_first != folders_first || clear_overrides {
            if let Err(err) = self.update_settings(|s| {
                s.folders_first = folders_first;
                if clear_overrides {
                    s.clear_all_folder_folders_first_overrides();
                }
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        let effective = self
            .settings
            .resolve_folder_settings(&self.current_location)
            .folders_first;
        if self.folders_first != effective {
            self.folders_first = effective;
            self.re_sort_entries();
        }
    }

    /// Sets global `hidden_last` placement, persists, and re-sorts when effective for the current folder.
    pub fn set_hidden_last(&mut self, hidden_last: bool) {
        let clear_overrides =
            self.settings.folder_settings_scope == FolderSettingsScope::AllFolders;
        if self.settings.hidden_last != hidden_last || clear_overrides {
            if let Err(err) = self.update_settings(|s| {
                s.hidden_last = hidden_last;
                if clear_overrides {
                    s.clear_all_folder_hidden_last_overrides();
                }
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        let effective = self
            .settings
            .resolve_folder_settings(&self.current_location)
            .hidden_last;
        if self.hidden_last != effective {
            self.hidden_last = effective;
            self.re_sort_entries();
        }
    }

    /// Sets the global sort column and direction, persists when configured, and re-sorts entries
    /// when effective for the current folder.
    pub fn set_sort(&mut self, column: SortColumn, direction: SortDirection) {
        let order = SortOrder { column, direction };
        let clear_overrides =
            self.settings.folder_settings_scope == FolderSettingsScope::AllFolders;
        if self.settings.default_sort != order || clear_overrides {
            if let Err(err) = self.update_settings(|s| {
                s.default_sort = order;
                if clear_overrides {
                    s.clear_all_folder_sort_overrides();
                }
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        let effective = self
            .settings
            .resolve_folder_settings(&self.current_location);
        self.sort_column = effective.sort_column;
        self.sort_direction = effective.sort_direction;
        self.re_sort_entries();
    }

    /// Sets the global listing view mode and persists it when configured.
    pub fn set_view_mode(&mut self, mode: ViewMode) {
        let clear_overrides =
            self.settings.folder_settings_scope == FolderSettingsScope::AllFolders;
        if self.settings.view_mode != mode || clear_overrides {
            if let Err(err) = self.update_settings(|s| {
                s.view_mode = mode;
                if clear_overrides {
                    s.clear_all_folder_view_mode_overrides();
                }
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        self.view_mode = self
            .settings
            .resolve_folder_settings(&self.current_location)
            .view_mode;
    }

    /// Sets whether explorer view/sort/hidden changes apply per folder or to all folders.
    pub fn set_folder_settings_scope(&mut self, scope: FolderSettingsScope) -> Result<(), String> {
        self.update_settings(|s| {
            s.folder_settings_scope = scope;
            Ok(())
        })
    }

    /// Sets view mode from the explorer UI (context menu, header switcher, shortcuts),
    /// persisting either for the current folder or globally according to `folder_settings_scope`.
    pub fn set_current_view_mode(&mut self, mode: ViewMode) {
        match self.settings.folder_settings_scope {
            FolderSettingsScope::PerFolder => {
                let location = self.current_location.clone();
                if let Err(err) = self.update_settings(|s| {
                    s.set_folder_view_mode(&location, mode);
                    Ok(())
                }) {
                    self.set_status(err);
                }
                self.view_mode = mode;
            }
            FolderSettingsScope::AllFolders => {
                self.set_view_mode(mode);
            }
        }
    }

    /// Sets sort column and direction from the explorer UI according to `folder_settings_scope`.
    pub fn set_current_sort(&mut self, column: SortColumn, direction: SortDirection) {
        match self.settings.folder_settings_scope {
            FolderSettingsScope::PerFolder => {
                let location = self.current_location.clone();
                if let Err(err) = self.update_settings(|s| {
                    s.set_folder_sort(&location, column, direction);
                    Ok(())
                }) {
                    self.set_status(err);
                }
                self.sort_column = column;
                self.sort_direction = direction;
                self.re_sort_entries();
            }
            FolderSettingsScope::AllFolders => {
                self.set_sort(column, direction);
            }
        }
    }

    /// Updates only the sort column in the explorer UI while keeping the current direction.
    pub fn set_current_sort_column(&mut self, column: SortColumn) {
        self.set_current_sort(column, self.sort_direction);
    }

    /// Updates only the sort direction in the explorer UI while keeping the current column.
    pub fn set_current_sort_direction(&mut self, direction: SortDirection) {
        self.set_current_sort(self.sort_column, direction);
    }

    /// Sets hidden-files visibility from the explorer UI according to `folder_settings_scope`.
    pub fn set_current_show_hidden(&mut self, show: bool) {
        match self.settings.folder_settings_scope {
            FolderSettingsScope::PerFolder => {
                let location = self.current_location.clone();
                if let Err(err) = self.update_settings(|s| {
                    s.set_folder_show_hidden(&location, show);
                    Ok(())
                }) {
                    self.set_status(err);
                }
                if self.show_hidden != show {
                    self.show_hidden = show;
                    self.reload_entries();
                }
            }
            FolderSettingsScope::AllFolders => {
                self.set_show_hidden(show);
            }
        }
    }

    /// Sets `folders_first` from the explorer UI according to `folder_settings_scope`.
    pub fn set_current_folders_first(&mut self, folders_first: bool) {
        match self.settings.folder_settings_scope {
            FolderSettingsScope::PerFolder => {
                let location = self.current_location.clone();
                if let Err(err) = self.update_settings(|s| {
                    s.set_folder_folders_first(&location, folders_first);
                    Ok(())
                }) {
                    self.set_status(err);
                }
                if self.folders_first != folders_first {
                    self.folders_first = folders_first;
                    self.re_sort_entries();
                }
            }
            FolderSettingsScope::AllFolders => {
                self.set_folders_first(folders_first);
            }
        }
    }

    /// Sets `hidden_last` from the explorer UI according to `folder_settings_scope`.
    pub fn set_current_hidden_last(&mut self, hidden_last: bool) {
        match self.settings.folder_settings_scope {
            FolderSettingsScope::PerFolder => {
                let location = self.current_location.clone();
                if let Err(err) = self.update_settings(|s| {
                    s.set_folder_hidden_last(&location, hidden_last);
                    Ok(())
                }) {
                    self.set_status(err);
                }
                if self.hidden_last != hidden_last {
                    self.hidden_last = hidden_last;
                    self.re_sort_entries();
                }
            }
            FolderSettingsScope::AllFolders => {
                self.set_hidden_last(hidden_last);
            }
        }
    }

    /// Toggles `folders_first` for the active scope and re-sorts the listing.
    pub fn toggle_folders_first(&mut self) {
        self.set_current_folders_first(!self.folders_first);
    }

    /// Toggles `hidden_last` for the active scope and re-sorts the listing.
    pub fn toggle_hidden_last(&mut self) {
        self.set_current_hidden_last(!self.hidden_last);
    }

    /// Promotes the active folder's view, sort, and hidden-file settings to global defaults
    /// and clears per-folder overrides so every folder shares them.
    pub fn apply_current_folder_settings_to_all(&mut self) {
        let resolved = ResolvedFolderSettings {
            view_mode: self.view_mode,
            sort_column: self.sort_column,
            sort_direction: self.sort_direction,
            show_hidden: self.show_hidden,
            folders_first: self.folders_first,
            hidden_last: self.hidden_last,
        };
        if let Err(err) = self.update_settings(|s| {
            s.apply_folder_settings_to_all(resolved);
            Ok(())
        }) {
            self.set_status(err);
        } else {
            self.set_completion_status("Applied folder settings to all folders");
        }
    }

    /// Shows or hides the sidebar and persists the choice when configured.
    pub fn set_sidebar_open(&mut self, open: bool) {
        if self.settings.show_sidebar != open {
            if let Err(err) = self.update_settings(|s| {
                s.show_sidebar = open;
                Ok(())
            }) {
                self.set_status(err);
            }
        }
        self.panels.sidebar_open = open;
    }

    pub fn toggle_sidebar(&mut self) {
        self.set_sidebar_open(!self.panels.sidebar_open);
    }

    /// Cycles through `Details -> Grid -> CompactList -> Details` for the active scope.
    pub fn cycle_view_mode(&mut self) -> ViewMode {
        let next = match self.view_mode {
            ViewMode::Details => ViewMode::Grid,
            ViewMode::Grid => ViewMode::CompactList,
            ViewMode::CompactList => ViewMode::Details,
        };
        self.set_current_view_mode(next);
        next
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
                self.set_network_status(NetworkMountStatus::Connecting);
                return;
            }
        }
        let next = match &self.current_location {
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
        self.set_network_status(next);
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
        self.set_network_status(NetworkMountStatus::Connecting);
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
        if let Some(Ok(server)) = self.remote_stream_server.get() {
            server.unregister_profile(profile_id);
        }
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
        self.listing_revision = self.listing_revision.wrapping_add(1);
        let now = Instant::now();
        self.highlighted_inserts
            .retain(|(_, ts)| now.saturating_duration_since(*ts) < JOB_COMPLETE_HOLD);
        self.recent_deletions
            .retain(|(_, ts)| now.saturating_duration_since(*ts) < JOB_COMPLETE_HOLD);
        self.entries = Arc::new(entries);
        let listed: Vec<Location> = self.entries.iter().map(|e| self.location_of(e)).collect();
        self.selection.reload_locations(&listed);
    }

    /// Marks user-created, renamed, or pasted locations to receive the `accent` entrance flash.
    pub fn mark_highlighted_insert(&mut self, location: Location) {
        let now = Instant::now();
        self.highlighted_inserts.retain(|(loc, _)| loc != &location);
        self.highlighted_inserts.push((location, now));
    }

    /// Marks user-trashed or deleted locations so `ListView` animates their departure throw.
    pub fn mark_recent_deletions(&mut self, locations: impl IntoIterator<Item = Location>) {
        let now = Instant::now();
        for loc in locations {
            self.recent_deletions
                .retain(|(existing, _)| existing != &loc);
            self.recent_deletions.push((loc, now));
        }
    }

    /// Returns selected entries in listing order, working correctly for both local and remote
    /// locations (`Selection::selected_in_order` constructs local locations and misses remote ones).
    pub fn selected_entries(&self) -> Vec<&Entry> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|e| self.selection.contains(&self.location_of(e)))
            .collect()
    }

    /// Returns the `Location` of each selected entry in listing order.
    pub fn selected_locations(&self) -> Vec<Location> {
        if self.selection.is_empty() {
            return Vec::new();
        }
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
        if self.selection.is_empty() {
            return None;
        }
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

    /// Moves selection up by one grid row (`cols` items), staying on the first row when already there.
    pub fn select_up_by(&mut self, cols: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let cols = cols.max(1);
        let target = match self.selected_index() {
            Some(ix) if ix >= cols => ix - cols,
            Some(ix) => ix,
            None => self.entries.len() - 1,
        };
        self.select_index(target);
        Some(target)
    }

    /// Moves selection down by one grid row (`cols` items), clamping to the last entry when
    /// moving from a higher row onto a shorter final row, and staying put on the last row.
    pub fn select_down_by(&mut self, cols: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let cols = cols.max(1);
        let last_ix = self.entries.len() - 1;
        let last_row = last_ix / cols;
        let target = match self.selected_index() {
            Some(ix) if ix / cols < last_row => (ix + cols).min(last_ix),
            Some(ix) => ix,
            None => 0,
        };
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
        self.status_revision = self.status_revision.wrapping_add(1);
        self.status_is_completion = false;
    }

    /// Sets a completion status message that pops into the footer (`460 ms`, from `0.7`).
    pub fn set_completion_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
        self.status_revision = self.status_revision.wrapping_add(1);
        self.status_is_completion = true;
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_is_completion = false;
    }

    /// Appends a new active job.
    pub fn add_job(&mut self, job: JobStatus) {
        self.active_jobs.push(job);
    }

    /// Completes a job and keeps a transient summary record for the `secondary` 1s completion fade,
    /// while removing it from `active_jobs` immediately.
    pub fn complete_job(&mut self, id: u64) {
        if let Some(pos) = self.active_jobs.iter().position(|j| j.id == id) {
            let job = self.active_jobs.remove(pos);
            self.completed_jobs.push(CompletedJob {
                id: job.id,
                title: job.title,
                kind: job.kind,
                completed_at: Instant::now(),
            });
        }
    }

    /// Prunes expired completion pills once their 1s hold + fade-out has elapsed.
    pub fn prune_completed_jobs(&mut self, now: Instant) {
        let total = JOB_COMPLETE_HOLD + QUICK;
        self.completed_jobs
            .retain(|j| now.saturating_duration_since(j.completed_at) < total);
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

    /// Updates network mount status and tracks when `Connecting` began for the 300ms bar delay.
    pub fn set_network_status(&mut self, status: NetworkMountStatus) {
        if status == NetworkMountStatus::Connecting {
            self.connecting_since.get_or_insert_with(Instant::now);
        } else {
            self.connecting_since = None;
        }
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

/// Sorts trash items by `column` and `direction`.
pub fn sort_trash_items(
    items: &mut [TrashItem],
    column: TrashSortColumn,
    direction: SortDirection,
) {
    items.sort_by(|a, b| {
        let ord = match column {
            TrashSortColumn::Name => a
                .name
                .to_string_lossy()
                .to_lowercase()
                .cmp(&b.name.to_string_lossy().to_lowercase()),
            TrashSortColumn::OriginalLocation => a
                .original_parent
                .to_string_lossy()
                .to_lowercase()
                .cmp(&b.original_parent.to_string_lossy().to_lowercase()),
            TrashSortColumn::DeletedAt => a.time_deleted.cmp(&b.time_deleted),
        };
        let directed = match direction {
            SortDirection::Ascending => ord,
            SortDirection::Descending => ord.reverse(),
        };
        directed
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// The reason without the `I/O error:` prefix `BackendError` adds.
fn describe(err: &BackendError) -> String {
    match err {
        BackendError::Io(err) => err.to_string(),
        BackendError::Unsupported => "the server does not support this".into(),
    }
}
