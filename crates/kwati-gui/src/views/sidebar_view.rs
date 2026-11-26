//! Sidebar view (owned by guinet-worker for #61).
//!
//! Renders the unified quick-access Places list (standard folders, pinned folders, and Trash),
//! the device's mounted drives with capacity indicators, and mounted network shares.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, transparent_black, AnyElement, Context, Div, Entity, EventEmitter,
    ExternalPaths, FontWeight, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, Pixels, Point,
    Stateful, StyleRefinement, Subscription, Task, Window,
};
use kwati_core::{
    device_volumes, eject, is_trash_browsing_supported, mount, network_mounts, unmounted_drives,
    ConnectionState, DriveError, IconPreferences, IconResolver, Location, NetworkMount, PlaceEntry,
    PlaceKind, ResolvedIcon, UnmountedDrive, Volume,
};

use crate::actions::{open_system_trash, ManageServers};
use crate::components::{
    animated_icon_sized, icon, icon_for_with, render_resolved_icon, resolve_icon_color, Ellipsize,
    IconMotion, IconName, SpecialFolders, TextInput, Tooltip,
};
use crate::motion::{
    color_lerp, is_reduced_motion, lerp_f32, request_frame_if_active, Curve, MotionKind,
    MotionTracker, INSTANT, PRESS_SCALE, QUICK,
};
use crate::remote::protocol_label;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, TextRole};
use crate::views::drag_drop::{
    collect_volume_mounts, resolve_drop_operation, resolve_drop_operation_for_drag,
    validate_external_drop, validate_external_drop_fast, validate_internal_drop,
    validate_internal_drop_fast, DropOperation,
};
use crate::views::list_view::DragEntries;

/// Formats byte quantities into human-readable strings (B, KB, MB, GB, TB).
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Distinguishes folder entries from the built-in Trash entry in the unified Places list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarPlaceKind {
    Folder(PathBuf),
    Trash,
}

/// An item in the unified sidebar Places list (standard folders, user-pinned folders, and Trash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPlace {
    pub name: String,
    pub kind: SidebarPlaceKind,
    pub icon: IconName,
}

impl SidebarPlace {
    pub fn new(name: impl Into<String>, path: PathBuf, icon: IconName) -> Self {
        Self {
            name: name.into(),
            kind: SidebarPlaceKind::Folder(path),
            icon,
        }
    }

    pub fn trash() -> Self {
        Self {
            name: "Trash".into(),
            kind: SidebarPlaceKind::Trash,
            icon: IconName::Trash,
        }
    }

    pub fn from_entry(entry: &PlaceEntry, specials: &SpecialFolders) -> Self {
        match entry {
            PlaceEntry::Trash => Self::trash(),
            PlaceEntry::Folder { name, path } => {
                let resolved = if path == Path::new("/") {
                    match icon_for_with(path, true, specials) {
                        IconName::Folder => IconName::Drive,
                        other => other,
                    }
                } else {
                    icon_for_with(path, true, specials)
                };
                Self::new(name.clone(), path.clone(), resolved)
            }
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            SidebarPlaceKind::Folder(path) => Some(path.as_path()),
            SidebarPlaceKind::Trash => None,
        }
    }

    pub fn is_trash(&self) -> bool {
        matches!(self.kind, SidebarPlaceKind::Trash)
    }

    pub fn is_removable(&self) -> bool {
        !self.is_trash()
    }

    pub fn is_missing(&self) -> bool {
        match &self.kind {
            SidebarPlaceKind::Folder(path) => !path.exists(),
            SidebarPlaceKind::Trash => false,
        }
    }
}

/// Gets the current user's home directory across platforms.
pub fn system_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Returns standard operating system places that exist on the filesystem.
pub fn standard_places() -> Vec<SidebarPlace> {
    let specials = SpecialFolders::from_system();
    let mut places: Vec<SidebarPlace> = kwati_core::standard_places()
        .into_iter()
        .map(|place| {
            let icon = match icon_for_with(&place.path, true, &specials) {
                IconName::Folder => match place.kind {
                    PlaceKind::Home => IconName::FolderHome,
                    PlaceKind::Desktop => IconName::FolderDesktop,
                    PlaceKind::Documents => IconName::FolderDocuments,
                    PlaceKind::Downloads => IconName::FolderDownloads,
                    PlaceKind::Music => IconName::FolderMusic,
                    PlaceKind::Pictures => IconName::FolderPictures,
                    PlaceKind::Videos => IconName::FolderVideos,
                },
                resolved => resolved,
            };
            SidebarPlace::new(place.name, place.path, icon)
        })
        .collect();

    if places.is_empty() {
        places.push(SidebarPlace::new(
            "Root",
            PathBuf::from("/"),
            IconName::Drive,
        ));
    }

    places
}

/// A storage volume or drive listed in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarVolume {
    pub id: String,
    pub name: String,
    pub mount_point: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub is_mounted: bool,
    pub is_busy: bool,
    pub busy_reason: Option<String>,
}

impl From<Volume> for SidebarVolume {
    fn from(volume: Volume) -> Self {
        Self::new(
            volume.mount_point.to_string_lossy().into_owned(),
            volume.name,
            volume.mount_point,
            volume.total_space,
            volume.available_space,
            volume.is_removable,
        )
    }
}

impl SidebarVolume {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        mount_point: PathBuf,
        total_bytes: u64,
        available_bytes: u64,
        is_removable: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            mount_point,
            total_bytes,
            available_bytes,
            is_removable,
            is_mounted: true,
            is_busy: false,
            busy_reason: None,
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }

    pub fn used_percentage(&self) -> u8 {
        if self.total_bytes == 0 {
            0
        } else {
            let used = self.used_bytes() as f64;
            let total = self.total_bytes as f64;
            ((used / total) * 100.0).min(100.0) as u8
        }
    }

    pub fn capacity_summary(&self) -> String {
        let free_str = format_bytes(self.available_bytes);
        let total_str = format_bytes(self.total_bytes);
        format!("{} free of {}", free_str, total_str)
    }
}

/// Outcome of attempting to eject a removable drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EjectResult {
    SafeToRemove {
        volume_name: String,
        unmounted_path: PathBuf,
    },
    Busy {
        volume_name: String,
        reason: String,
    },
    NotFound,
}

/// Outcome of attempting to mount an unmounted drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountResult {
    Mounted {
        volume_name: String,
        mount_point: PathBuf,
    },
    Failed(String),
}

/// Connection status for a saved remote server profile in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerConnectionStatus {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
}

impl ServerConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "Conectado",
            Self::Connecting => "Conectando",
            Self::Reconnecting => "Reconectando",
            Self::Disconnected => "Desconectado",
        }
    }
}

/// Snapshot of a saved remote server profile for sidebar rendering and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarServerItem {
    pub id: String,
    pub name: String,
    pub protocol: &'static str,
    pub host: String,
    pub port: u16,
    pub status: ServerConnectionStatus,
}

/// Computes the sidebar connection status of `profile_id` from `AppState`.
pub fn server_connection_status(state: &AppState, profile_id: &str) -> ServerConnectionStatus {
    if state.connecting.as_deref() == Some(profile_id) {
        return ServerConnectionStatus::Connecting;
    }
    match state.remote.state(profile_id) {
        Some(ConnectionState::Connected) => ServerConnectionStatus::Connected,
        Some(ConnectionState::Reconnecting) => ServerConnectionStatus::Reconnecting,
        Some(ConnectionState::Disconnected) | None => ServerConnectionStatus::Disconnected,
    }
}

/// Returns all saved remote server profiles with protocol and connection status.
pub fn server_items(state: &AppState) -> Vec<SidebarServerItem> {
    state
        .remote
        .profiles()
        .iter()
        .map(|p| SidebarServerItem {
            id: p.id.clone(),
            name: p.name.clone(),
            protocol: protocol_label(p.protocol),
            host: p.host.clone(),
            port: p.port,
            status: server_connection_status(state, &p.id),
        })
        .collect()
}

/// How often drives and shares are listed again, so a plugged-in disk or a newly mounted
/// share shows up without a restart.
const DEVICE_POLL: Duration = Duration::from_secs(3);

/// Backend operations used by [`SidebarView`] to list, mount, and safely eject drives.
pub trait DriveBackend: Send + Sync + 'static {
    fn list_mounted(&self) -> Vec<Volume>;
    fn list_unmounted(&self) -> Result<Vec<UnmountedDrive>, DriveError>;
    fn list_network(&self) -> Vec<NetworkMount>;
    fn mount(&self, drive: &UnmountedDrive) -> Result<PathBuf, DriveError>;
    fn eject(&self, mount_point: &Path) -> Result<(), DriveError>;
    fn collect_mounts(&self) -> Vec<PathBuf> {
        let mut mounts: Vec<PathBuf> = self
            .list_mounted()
            .into_iter()
            .map(|v| v.mount_point)
            .chain(self.list_network().into_iter().map(|m| m.mount_point))
            .collect();
        mounts.sort();
        mounts.dedup();
        mounts
    }
}

/// Default [`DriveBackend`] backed by `kwati_core` OS/D-Bus/diskutil/Win32 APIs.
#[derive(Debug, Default)]
pub struct SystemDriveBackend;

impl DriveBackend for SystemDriveBackend {
    fn list_mounted(&self) -> Vec<Volume> {
        device_volumes()
    }

    fn list_unmounted(&self) -> Result<Vec<UnmountedDrive>, DriveError> {
        unmounted_drives()
    }

    fn list_network(&self) -> Vec<NetworkMount> {
        network_mounts()
    }

    fn mount(&self, drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        mount(drive)
    }

    fn eject(&self, mount_point: &Path) -> Result<(), DriveError> {
        eject(mount_point)
    }

    fn collect_mounts(&self) -> Vec<PathBuf> {
        collect_volume_mounts()
    }
}

type DeviceSnapshot = (
    Vec<Volume>,
    Vec<UnmountedDrive>,
    Vec<NetworkMount>,
    Vec<PathBuf>,
);

fn poll_device_snapshot(backend: &dyn DriveBackend) -> DeviceSnapshot {
    (
        backend.list_mounted(),
        backend.list_unmounted().unwrap_or_default(),
        backend.list_network(),
        backend.collect_mounts(),
    )
}

fn home_fallback_dir() -> PathBuf {
    system_home_dir()
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebarKey {
    Place(usize),
    AddPlace,
    ManageServers,
    AddServer,
    Server(usize),
    Drive(usize),
    DriveEject(usize),
    UnmountedDrive(usize),
    DriveMount(usize),
    RefreshDrives,
    NetworkShare(usize),
}

#[derive(Clone)]
struct DragSidebarPlace {
    index: usize,
    name: String,
    icon: IconName,
    is_trash: bool,
    is_missing: bool,
}

struct DragPlacePreview {
    name: String,
    icon: IconName,
    is_trash: bool,
    is_missing: bool,
    resolved_icon: Option<ResolvedIcon>,
    icon_color: Option<gpui::Hsla>,
}

impl Render for DragPlacePreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let glyph = if self.is_trash {
            animated_icon_sized(IconName::Trash, IconMotion::Rest, size::ICON, theme.text)
        } else if self.is_missing {
            animated_icon_sized(IconName::Alert, IconMotion::Rest, size::ICON, theme.danger)
        } else if let (Some(resolved), Some(color)) = (&self.resolved_icon, self.icon_color) {
            render_resolved_icon(resolved, self.icon, IconMotion::Rest, size::ICON, color)
        } else {
            animated_icon_sized(self.icon, IconMotion::Rest, size::ICON, theme.text)
        };

        div()
            .h(size::SIDEBAR_ROW)
            .px(space::S8)
            .rounded(radius::CONTROL)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.accent)
            .shadow_sm()
            .opacity(0.92)
            .flex()
            .items_center()
            .gap(space::S8)
            .text_body()
            .text_color(if self.is_missing {
                theme.danger
            } else {
                theme.text
            })
            .child(glyph)
            .child(self.name.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarViewEvent {
    DropTransfer {
        sources: Vec<Location>,
        dest_dir: Location,
        operation: DropOperation,
    },
    TrashEntries(Vec<Location>),
    OpenPlaceContextMenu {
        index: usize,
        is_trash: bool,
        position: Point<Pixels>,
    },
}

impl EventEmitter<SidebarViewEvent> for SidebarView {}

fn style_drag_over_local_dir(
    style: StyleRefinement,
    dragged: &DragEntries,
    dest: &Path,
    modifiers: Modifiers,
    mounts: &[PathBuf],
    theme: &Theme,
) -> StyleRefinement {
    let dest_loc = Location::Local(dest.to_path_buf());
    let op = resolve_drop_operation_for_drag(dragged, &dest_loc, modifiers, mounts);
    if validate_internal_drop_fast(dragged, &dest_loc, op).is_ok() {
        style
            .bg(theme.surface)
            .border_1()
            .border_color(theme.accent)
    } else {
        style
    }
}

fn style_drag_over_external_local_dir(
    style: StyleRefinement,
    paths: &ExternalPaths,
    dest: &Path,
    theme: &Theme,
) -> StyleRefinement {
    let dest_loc = Location::Local(dest.to_path_buf());
    if validate_external_drop_fast(paths.paths(), &dest_loc).is_ok() {
        style
            .bg(theme.surface)
            .border_1()
            .border_color(theme.accent)
    } else {
        style
    }
}

#[allow(clippy::too_many_arguments)]
fn style_drag_over_place_entry(
    style: StyleRefinement,
    dragged: &DragEntries,
    is_trash: bool,
    is_missing: bool,
    dest_path: Option<&Path>,
    modifiers: Modifiers,
    mounts: &[PathBuf],
    theme: &Theme,
) -> StyleRefinement {
    if is_trash {
        return style
            .bg(theme.surface)
            .border_1()
            .border_color(theme.danger);
    }
    if is_missing {
        return style;
    }
    if let Some(dest) = dest_path {
        return style_drag_over_local_dir(style, dragged, dest, modifiers, mounts, theme);
    }
    style
}

fn style_drag_over_pin(
    style: StyleRefinement,
    dragged: &DragEntries,
    pinned_paths: &[PathBuf],
    theme: &Theme,
    expand_height: bool,
) -> StyleRefinement {
    if let Some((path, _)) = dragged.single_local_folder() {
        if !pinned_paths.iter().any(|p| p.as_path() == path) {
            let styled = style
                .bg(theme.surface)
                .border_1()
                .border_color(theme.accent);
            return if expand_height {
                styled.h(size::SIDEBAR_ROW)
            } else {
                styled
            };
        }
    }
    style
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebarMotionKey {
    Hover(SidebarKey),
    Press(SidebarKey),
    ServerDraw(usize),
    EjectHop(usize),
    RefreshSpin,
}

pub struct SidebarView {
    pub app_state: Entity<AppState>,
    pub places: Vec<SidebarPlace>,
    pub volumes: Vec<SidebarVolume>,
    pub unmounted_drives: Vec<UnmountedDrive>,
    pub network: Vec<NetworkMount>,
    /// Drives show as rows with their free space unless the user asks for capacity cards with
    /// a right click.
    pub drive_details: bool,
    drive_backend: Arc<dyn DriveBackend>,
    busy_volumes: HashMap<PathBuf, String>,
    ejecting_mounts: HashSet<PathBuf>,
    mounting_drives: HashSet<String>,
    editing_place: Option<usize>,
    rename_input: Entity<TextInput>,
    hovered: HashSet<SidebarKey>,
    pressed: HashSet<SidebarKey>,
    last_server_states: Vec<(String, ServerConnectionStatus)>,
    motion: MotionTracker<SidebarMotionKey>,
    resolved_icon_cache: HashMap<(IconName, u16), ResolvedIcon>,
    cached_icon_prefs: IconPreferences,
    _state_sub: Subscription,
    _device_poll: Task<()>,
    device_refresh: Option<Task<()>>,
}

impl SidebarView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::with_drive_backend(app_state, Arc::new(SystemDriveBackend), cx)
    }

    /// Creates a [`SidebarView`] with a custom [`DriveBackend`] (e.g. for tests).
    pub fn with_drive_backend(
        app_state: Entity<AppState>,
        drive_backend: Arc<dyn DriveBackend>,
        cx: &mut Context<Self>,
    ) -> Self {
        let specials = SpecialFolders::from_system();
        let initial_places = app_state
            .read(cx)
            .places()
            .iter()
            .map(|entry| SidebarPlace::from_entry(entry, &specials))
            .collect();
        let rename_input = cx.new(|cx| TextInput::with_placeholder("Nome do local", cx));
        let initial_icon_prefs = app_state.read(cx).icon_config.active.clone();
        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            this.sync_places_from_state(app_state.read(cx));
            cx.notify();
        });
        let poll_backend = Arc::clone(&drive_backend);
        let device_poll = cx.spawn(async move |this, cx| loop {
            let backend = Arc::clone(&poll_backend);
            let (volumes, unmounted, network, mounts) = cx
                .background_executor()
                .spawn(async move { poll_device_snapshot(backend.as_ref()) })
                .await;
            let listed = this.update(cx, |this, cx| {
                this.app_state
                    .update(cx, |state, _| state.set_volume_mounts(mounts));
                this.set_devices_snapshot(volumes, unmounted, network, cx);
            });
            if listed.is_err() {
                break;
            }
            cx.background_executor().timer(DEVICE_POLL).await;
        });
        Self {
            app_state,
            places: initial_places,
            volumes: Vec::new(),
            unmounted_drives: Vec::new(),
            network: Vec::new(),
            drive_details: false,
            drive_backend,
            busy_volumes: HashMap::new(),
            ejecting_mounts: HashSet::new(),
            mounting_drives: HashSet::new(),
            editing_place: None,
            rename_input,
            hovered: HashSet::new(),
            pressed: HashSet::new(),
            last_server_states: Vec::new(),
            motion: MotionTracker::new(),
            resolved_icon_cache: HashMap::new(),
            cached_icon_prefs: initial_icon_prefs,
            _state_sub: state_sub,
            _device_poll: device_poll,
            device_refresh: None,
        }
    }

    fn cached_resolved_icon(
        &mut self,
        builtin: IconName,
        icon_size: Pixels,
        prefs: &IconPreferences,
        resolver: &IconResolver,
    ) -> ResolvedIcon {
        if *prefs != self.cached_icon_prefs {
            self.cached_icon_prefs = prefs.clone();
            self.resolved_icon_cache.clear();
        }
        let size_px = (icon_size / px(1.0)).round().max(1.0) as u16;
        let key = (builtin, size_px);
        if let Some(resolved) = self.resolved_icon_cache.get(&key) {
            return resolved.clone();
        }
        let resolved = resolver.resolve(prefs, builtin.key_name(), size_px);
        self.resolved_icon_cache.insert(key, resolved.clone());
        resolved
    }

    /// Synchronizes `self.places` with `state.places()`.
    pub fn sync_places_from_state(&mut self, state: &AppState) {
        let specials = SpecialFolders::from_system();
        let synced: Vec<SidebarPlace> = state
            .places()
            .iter()
            .map(|entry| SidebarPlace::from_entry(entry, &specials))
            .collect();
        if self.places != synced {
            self.places = synced;
            if let Some(idx) = self.editing_place {
                if self.places.get(idx).is_none_or(|p| p.is_trash()) {
                    self.editing_place = None;
                }
            }
        }
    }

    /// Lists drives and shares now instead of waiting for the next poll.
    pub fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        let backend = Arc::clone(&self.drive_backend);
        self.device_refresh = Some(cx.spawn(async move |this, cx| {
            let (volumes, unmounted, network, mounts) = cx
                .background_executor()
                .spawn(async move { poll_device_snapshot(backend.as_ref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.app_state
                    .update(cx, |state, _| state.set_volume_mounts(mounts));
                this.set_devices_snapshot(volumes, unmounted, network, cx);
            });
        }));
    }

    pub fn refresh_devices_with(
        &mut self,
        list: impl FnOnce() -> (Vec<Volume>, Vec<NetworkMount>) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.device_refresh = Some(cx.spawn(async move |this, cx| {
            let (volumes, network) = cx.background_executor().spawn(async move { list() }).await;
            let _ = this.update(cx, |this, cx| {
                this.set_devices_snapshot(volumes, Vec::new(), network, cx);
            });
        }));
    }

    fn set_devices_snapshot(
        &mut self,
        volumes: Vec<Volume>,
        unmounted: Vec<UnmountedDrive>,
        network: Vec<NetworkMount>,
        cx: &mut Context<Self>,
    ) {
        self.busy_volumes
            .retain(|mp, _| volumes.iter().any(|v| &v.mount_point == mp));

        let volumes: Vec<SidebarVolume> = volumes
            .into_iter()
            .map(|v| {
                let mut sv = SidebarVolume::from(v);
                if let Some(reason) = self.busy_volumes.get(&sv.mount_point) {
                    sv.is_busy = true;
                    sv.busy_reason = Some(reason.clone());
                }
                sv
            })
            .collect();

        if volumes != self.volumes || unmounted != self.unmounted_drives || network != self.network
        {
            self.volumes = volumes;
            self.unmounted_drives = unmounted;
            self.network = network;
            cx.notify();
        }
    }

    // --- Unified Places management ---

    /// Adds a place via `AppState::add_place` and persists/syncs it.
    pub fn add_place_in_state(
        &mut self,
        name: impl Into<String>,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.add_place(name, path));
        if res.is_ok() {
            let state = self.app_state.read(cx);
            self.sync_places_from_state(state);
        }
        cx.notify();
        res
    }

    /// Removes a place via `AppState::remove_place` and persists/syncs it. Trash cannot be removed.
    pub fn remove_place_in_state(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.remove_place(index).map(|_| ()));
        if res.is_ok() {
            if self.editing_place == Some(index) {
                self.editing_place = None;
            }
            let state = self.app_state.read(cx);
            self.sync_places_from_state(state);
        }
        cx.notify();
        res
    }

    /// Renames a folder place via `AppState::rename_place` and persists/syncs it.
    pub fn rename_place_in_state(
        &mut self,
        index: usize,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.rename_place(index, name));
        if res.is_ok() {
            self.editing_place = None;
            let state = self.app_state.read(cx);
            self.sync_places_from_state(state);
        }
        cx.notify();
        res
    }

    /// Moves any place (including Trash) from `from` to `to` and persists/syncs it.
    pub fn reorder_place_in_state(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.reorder_places(from, to));
        if res.is_ok() {
            self.editing_place = None;
            let state = self.app_state.read(cx);
            self.sync_places_from_state(state);
        }
        cx.notify();
        res
    }

    /// Pins the current local folder into the unified Places list.
    pub fn pin_current_location(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let current = self.app_state.read(cx).current_location.clone();
        let Location::Local(path) = current else {
            return Err("Apenas pastas locais podem ser fixadas nos locais.".into());
        };
        self.pin_folder(path, cx)
    }

    /// Adds `path` to Places under its folder name, unless it is already there.
    pub fn pin_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
        if self.places.iter().any(|p| p.path() == Some(path.as_path())) {
            return Ok(());
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        self.add_place_in_state(name, path, cx)
    }

    pub fn begin_rename_place(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.places.get(index) else {
            return;
        };
        if place.is_trash() {
            return;
        }
        let current_name = place.name.clone();
        self.editing_place = Some(index);
        self.rename_input.update(cx, |input, cx| {
            input.set_text(current_name, cx);
        });
        self.rename_input.read(cx).focus_handle.focus(window);
        cx.notify();
    }

    pub fn commit_rename_place(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.editing_place else {
            return;
        };
        let new_name = self.rename_input.read(cx).text().trim().to_string();
        if new_name.is_empty() {
            self.editing_place = None;
            cx.notify();
            return;
        }
        let _ = self.rename_place_in_state(index, new_name, cx);
    }

    pub fn cancel_rename_place(&mut self, cx: &mut Context<Self>) {
        self.editing_place = None;
        cx.notify();
    }

    pub fn open_place_at(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(place) = self.places.get(index).cloned() else {
            return;
        };
        if place.is_trash() {
            self.open_trash(cx);
        } else if let Some(path) = place.path() {
            self.navigate_to_path(path, cx);
        }
    }

    fn handle_local_dir_entries_drop(
        &mut self,
        dragged: &DragEntries,
        dest: &Path,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let dest_loc = Location::Local(dest.to_path_buf());
        let mounts = self.app_state.read(cx).volume_mounts.clone();
        let sources = dragged.locations();
        let op = resolve_drop_operation(&sources, &dest_loc, modifiers, &mounts);
        match validate_internal_drop(dragged, &dest_loc, op) {
            Ok(()) => {
                cx.emit(SidebarViewEvent::DropTransfer {
                    sources,
                    dest_dir: dest_loc,
                    operation: op,
                });
            }
            Err(reason) => {
                if let Some(msg) = reason.status_message() {
                    self.app_state.update(cx, |state, _| {
                        state.set_status(msg);
                    });
                    cx.notify();
                }
            }
        }
    }

    /// Copies external OS files dropped onto a sidebar directory target into `dest_path`.
    pub fn handle_external_paths_drop(
        &mut self,
        paths: &[PathBuf],
        dest_path: &Path,
        cx: &mut Context<Self>,
    ) {
        let dest_loc = Location::Local(dest_path.to_path_buf());
        match validate_external_drop(paths, &dest_loc) {
            Ok(()) => {
                let sources = paths.iter().cloned().map(Location::Local).collect();
                cx.emit(SidebarViewEvent::DropTransfer {
                    sources,
                    dest_dir: dest_loc,
                    operation: DropOperation::Copy,
                });
            }
            Err(reason) => {
                if let Some(msg) = reason.status_message() {
                    self.app_state.update(cx, |state, _| {
                        state.set_status(msg);
                    });
                    cx.notify();
                }
            }
        }
    }

    fn handle_place_entries_drop(
        &mut self,
        dragged: &DragEntries,
        is_trash: bool,
        is_missing: bool,
        dest_path: Option<&Path>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if is_trash {
            let locations = dragged.locations();
            if !locations.is_empty() {
                cx.emit(SidebarViewEvent::TrashEntries(locations));
            }
            return;
        }
        if is_missing {
            return;
        }
        if let Some(dest) = dest_path {
            self.handle_local_dir_entries_drop(dragged, dest, modifiers, cx);
        }
    }

    fn handle_pin_drop(&mut self, dragged: &DragEntries, cx: &mut Context<Self>) {
        if let Some((path, _)) = dragged.single_local_folder() {
            if let Err(err) = self.pin_folder(path.to_path_buf(), cx) {
                self.app_state.update(cx, |state, _| {
                    state.set_status(err);
                });
            }
        } else if !dragged.items.is_empty() {
            self.app_state.update(cx, |state, _| {
                state.set_status("Apenas pastas locais podem ser fixadas em Places.");
            });
        }
    }

    /// Initiates a connection to the saved server profile `profile_id`.
    pub fn connect_server(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _| {
            state.connect(profile_id, None, false);
        });
        cx.notify();
    }

    // --- Volumes and Eject management ---

    pub fn add_volume(&mut self, volume: SidebarVolume) {
        self.volumes.push(volume);
    }

    fn mark_volume_busy(&mut self, mount_point: &Path, reason: String) {
        self.busy_volumes
            .insert(mount_point.to_path_buf(), reason.clone());
        for vol in &mut self.volumes {
            if vol.mount_point == mount_point {
                vol.is_busy = true;
                vol.busy_reason = Some(reason.clone());
            }
        }
    }

    /// Safely ejects a mounted removable drive on the GPUI background executor.
    ///
    /// If `AppState::current_location` is currently inside `mount_point`, navigates to the
    /// user's home folder first so Kwatí itself does not hold the volume busy.
    pub fn eject_drive(
        &mut self,
        mount_point: PathBuf,
        volume_name: String,
        cx: &mut Context<Self>,
    ) {
        if !self.ejecting_mounts.insert(mount_point.clone()) {
            return;
        }
        cx.notify();

        let should_leave = {
            let state = self.app_state.read(cx);
            matches!(&state.current_location, Location::Local(p) if p.starts_with(&mount_point))
        };
        if should_leave {
            let home = home_fallback_dir();
            self.app_state.update(cx, |state, cx| {
                // Drop any directory watcher on the drive immediately so OS handles close before eject.
                state.dir_watcher = None;
                state.navigate_to(Location::Local(home));
                cx.notify();
            });
        }

        let backend = Arc::clone(&self.drive_backend);
        cx.spawn(async move |this, cx| {
            let eject_target = mount_point.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    backend.eject(&eject_target)?;
                    Ok::<_, DriveError>(poll_device_snapshot(backend.as_ref()))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.ejecting_mounts.remove(&mount_point);
                match result {
                    Ok((volumes, unmounted, network, mounts)) => {
                        this.busy_volumes.remove(&mount_point);
                        this.app_state.update(cx, |state, _| {
                            state.set_volume_mounts(mounts);
                            state.set_completion_status(format!(
                                "Ejected {volume_name}. Safe to remove."
                            ));
                        });
                        this.set_devices_snapshot(volumes, unmounted, network, cx);
                        cx.notify();
                    }
                    Err(DriveError::Busy(reason)) => {
                        this.mark_volume_busy(&mount_point, reason.clone());
                        this.app_state.update(cx, |state, _| {
                            state.set_status(format!("Cannot eject {volume_name}: {reason}"));
                        });
                        cx.notify();
                    }
                    Err(DriveError::NotFound(reason)) => {
                        this.busy_volumes.remove(&mount_point);
                        this.app_state.update(cx, |state, _| {
                            state.set_status(format!("Cannot eject {volume_name}: {reason}"));
                        });
                        this.refresh_devices(cx);
                        cx.notify();
                    }
                    Err(err) => {
                        this.app_state.update(cx, |state, _| {
                            state.set_status(format!("Cannot eject {volume_name}: {err}"));
                        });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Mounts an unmounted removable drive on the GPUI background executor, refreshes the
    /// sidebar drives, and navigates to the new mount point.
    pub fn mount_drive(&mut self, drive: UnmountedDrive, cx: &mut Context<Self>) {
        if !self.mounting_drives.insert(drive.id.clone()) {
            return;
        }
        cx.notify();

        let backend = Arc::clone(&self.drive_backend);
        let drive_for_bg = drive.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mount_point = backend.mount(&drive_for_bg)?;
                    let snapshot = poll_device_snapshot(backend.as_ref());
                    Ok::<_, DriveError>((mount_point, snapshot))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.mounting_drives.remove(&drive.id);
                match result {
                    Ok((mount_point, (volumes, unmounted, network, mounts))) => {
                        this.app_state.update(cx, |state, _| {
                            state.set_volume_mounts(mounts);
                            state.set_completion_status(format!("Mounted {}", drive.name));
                            state.navigate_to(Location::Local(mount_point));
                        });
                        this.set_devices_snapshot(volumes, unmounted, network, cx);
                        cx.notify();
                    }
                    Err(err) => {
                        this.app_state.update(cx, |state, _| {
                            state.set_status(format!("Cannot mount {}: {err}", drive.name));
                        });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn navigate_to_path(&self, path: &Path, cx: &mut Context<Self>) {
        let location = Location::Local(path.to_path_buf());
        self.app_state.update(cx, |state, _cx| {
            state.navigate_to(location);
        });
    }

    /// Opens the in-app trash view on supported platforms, or the OS trash on macOS.
    pub fn open_trash(&mut self, cx: &mut Context<Self>) {
        self.open_trash_with(is_trash_browsing_supported(), open_system_trash, cx);
    }

    pub fn open_trash_with(
        &mut self,
        browsing_supported: bool,
        open_system: impl FnOnce() -> std::io::Result<()>,
        cx: &mut Context<Self>,
    ) {
        if browsing_supported {
            self.app_state.update(cx, |state, _| {
                state.open_trash();
            });
        } else if let Err(err) = open_system() {
            self.app_state.update(cx, |state, _| {
                state.set_status(format!("Cannot open system trash: {err}"));
            });
        }
        cx.notify();
    }
}

impl SidebarView {
    fn set_hover(&mut self, key: SidebarKey, active: bool, cx: &mut Context<Self>) {
        let changed = if active {
            self.hovered.insert(key)
        } else {
            self.pressed.remove(&key);
            self.hovered.remove(&key)
        };
        if changed {
            cx.notify();
        }
    }

    fn set_press(&mut self, key: SidebarKey, active: bool, cx: &mut Context<Self>) {
        let changed = if active {
            self.pressed.insert(key)
        } else {
            self.pressed.remove(&key)
        };
        if changed {
            cx.notify();
        }
    }

    fn step_row_factors(&mut self, key: SidebarKey, now: Instant, reduced: bool) -> (f32, f32) {
        let hover_target = if self.hovered.contains(&key) {
            1.0
        } else {
            0.0
        };
        let press_target = if self.pressed.contains(&key) {
            1.0
        } else {
            0.0
        };
        let hover_t = self.motion.step_scalar(
            SidebarMotionKey::Hover(key),
            0.0,
            hover_target,
            INSTANT,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let press_t = self.motion.step_scalar(
            SidebarMotionKey::Press(key),
            0.0,
            press_target,
            INSTANT,
            Curve::Spring,
            MotionKind::MovementOrScale,
            now,
            reduced,
        );
        (hover_t, press_t)
    }

    fn sync_server_draws(&mut self, servers: &[SidebarServerItem], now: Instant) {
        let snapshot: Vec<(String, ServerConnectionStatus)> =
            servers.iter().map(|s| (s.id.clone(), s.status)).collect();
        if self.last_server_states != snapshot {
            for (ix, (id, status)) in snapshot.iter().enumerate() {
                let prev = self
                    .last_server_states
                    .iter()
                    .find(|(pid, _)| pid == id)
                    .map(|(_, st)| *st);
                if prev != Some(*status) {
                    self.motion.trigger_pulse(
                        SidebarMotionKey::ServerDraw(ix),
                        QUICK,
                        MotionKind::MovementOrScale,
                        now,
                    );
                }
            }
            self.last_server_states = snapshot;
        }
    }

    fn toggle_drive_details(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drive_details = !self.drive_details;
        cx.notify();
    }

    /// A drive as a row like a place, with its free space on a second line.
    #[allow(clippy::too_many_arguments)]
    fn drive_row(
        &mut self,
        ix: usize,
        vol: SidebarVolume,
        current_loc: &Location,
        trash_active: bool,
        now: Instant,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let is_active =
            !trash_active && matches!(current_loc, Location::Local(p) if p == &vol.mount_point);
        let tooltip = if vol.is_mounted {
            match &vol.busy_reason {
                Some(reason) if vol.is_busy => format!(
                    "{} — {} ({reason})",
                    vol.mount_point.display(),
                    vol.capacity_summary()
                ),
                _ => format!("{} — {}", vol.mount_point.display(), vol.capacity_summary()),
            }
        } else {
            format!("{} (not mounted)", vol.name)
        };
        let path = vol.mount_point.clone();
        let row_key = SidebarKey::Drive(ix);
        let (hover_t, press_t) = self.step_row_factors(row_key, now, reduced);
        let row_bg = if is_active {
            theme.selection.into()
        } else {
            color_lerp(transparent_black(), theme.border, hover_t)
        };
        let drive_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, press_t)
        };
        let eject_key = SidebarKey::DriveEject(ix);
        let (eject_hover, eject_press) = if vol.is_removable && vol.is_mounted {
            self.step_row_factors(eject_key, now, reduced)
        } else {
            (0.0, 0.0)
        };
        let eject_pulse = self
            .motion
            .pulse_progress(&SidebarMotionKey::EjectHop(ix), now, reduced)
            .map(|t| (t * std::f32::consts::PI).sin())
            .unwrap_or(0.0);
        let eject_motion = if !reduced && (eject_pulse > 1e-3 || eject_hover > 1e-3) {
            IconMotion::State(eject_pulse.max(eject_hover))
        } else {
            IconMotion::Rest
        };
        let eject_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, eject_press)
        };
        let eject_name = vol.name.clone();
        let eject_mount_point = vol.mount_point.clone();
        let is_ejecting = self.ejecting_mounts.contains(&eject_mount_point);

        let free_space = (vol.is_mounted && vol.total_bytes > 0).then(|| vol.capacity_summary());
        let app_state_drag = self.app_state.clone();
        let path_drag_over = path.clone();
        let path_drop = path.clone();
        let path_ext_over = path.clone();
        let path_ext_drop = path.clone();

        div()
            .id(("drive-row", ix))
            .debug_selector(move || format!("drive-row-{ix}"))
            .min_h(size::SIDEBAR_ROW)
            .py(space::S2)
            .px(space::S8)
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .bg(row_bg)
            .text_body()
            .text_color(if vol.is_mounted {
                theme.text
            } else {
                theme.text_muted
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S8)
            .tooltip(Tooltip::text(tooltip))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.set_hover(row_key, *hovered, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(space::S8)
                    .child(animated_icon_sized(
                        IconName::Drive,
                        IconMotion::Rest,
                        size::ICON * drive_scale,
                        if vol.is_busy {
                            theme.warning
                        } else {
                            theme.text
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().ellipsize().child(vol.name))
                            .when_some(free_space, |col, free| {
                                col.child(
                                    div()
                                        .ellipsize()
                                        .text_label()
                                        .text_color(theme.text_muted)
                                        .child(free),
                                )
                            }),
                    ),
            )
            .when(vol.is_removable && vol.is_mounted, |el| {
                el.child(
                    div()
                        .id(("drive-eject", ix))
                        .debug_selector(move || format!("drive-eject-{ix}"))
                        .p(space::S2)
                        .rounded(radius::CONTROL)
                        .bg(color_lerp(
                            transparent_black(),
                            theme.surface_raised,
                            eject_hover,
                        ))
                        .tooltip(Tooltip::text(if is_ejecting {
                            "Ejecting…"
                        } else {
                            "Eject"
                        }))
                        .on_hover(cx.listener(move |this, hovered, _, cx| {
                            if *hovered && !this.hovered.contains(&eject_key) {
                                this.motion.trigger_pulse(
                                    SidebarMotionKey::EjectHop(ix),
                                    QUICK,
                                    MotionKind::MovementOrScale,
                                    Instant::now(),
                                );
                            }
                            this.set_hover(eject_key, *hovered, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_press(eject_key, true, cx);
                                this.motion.trigger_pulse(
                                    SidebarMotionKey::EjectHop(ix),
                                    QUICK,
                                    MotionKind::MovementOrScale,
                                    Instant::now(),
                                );
                                this.eject_drive(eject_mount_point.clone(), eject_name.clone(), cx);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.set_press(eject_key, false, cx);
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.set_press(eject_key, false, cx);
                            }),
                        )
                        .child(animated_icon_sized(
                            IconName::Eject,
                            eject_motion,
                            size::ICON * eject_scale,
                            if vol.is_busy {
                                theme.warning
                            } else {
                                theme.text_muted
                            },
                        )),
                )
            })
            .when(vol.is_mounted, |s| {
                s.drag_over::<DragEntries>(move |style, dragged, window, cx| {
                    let theme = Theme::get(cx);
                    let mounts = app_state_drag.read(cx).volume_mounts.clone();
                    style_drag_over_local_dir(
                        style,
                        dragged,
                        &path_drag_over,
                        window.modifiers(),
                        &mounts,
                        &theme,
                    )
                })
                .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                    this.handle_local_dir_entries_drop(dragged, &path_drop, window.modifiers(), cx);
                }))
                .drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                    let theme = Theme::get(cx);
                    style_drag_over_external_local_dir(style, paths, &path_ext_over, &theme)
                })
                .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                    this.handle_external_paths_drop(paths.paths(), &path_ext_drop, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(row_key, true, cx);
                        this.navigate_to_path(&path, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(row_key, false, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(row_key, false, cx);
                    }),
                )
            })
    }

    fn mount_drive_button(
        &mut self,
        ux: usize,
        drive: UnmountedDrive,
        now: Instant,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let mount_key = SidebarKey::DriveMount(ux);
        let (mount_hover, _) = self.step_row_factors(mount_key, now, reduced);
        let is_mounting = self.mounting_drives.contains(&drive.id);

        div()
            .id(("drive-mount", ux))
            .debug_selector(move || format!("drive-mount-{ux}"))
            .px(space::S8)
            .py(space::S2)
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .bg(color_lerp(theme.surface, theme.border, mount_hover))
            .text_label()
            .text_color(theme.text)
            .tooltip(Tooltip::text("Mount"))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.set_hover(mount_key, *hovered, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_press(mount_key, true, cx);
                    this.mount_drive(drive.clone(), cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_press(mount_key, false, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_press(mount_key, false, cx);
                }),
            )
            .child(if is_mounting { "Mounting…" } else { "Mount" })
    }

    fn unmounted_drive_subtitle(drive: &UnmountedDrive) -> String {
        match (drive.total_space > 0, drive.file_system.is_empty()) {
            (true, false) => format!(
                "{} · {}",
                format_bytes(drive.total_space),
                drive.file_system
            ),
            (true, true) => format_bytes(drive.total_space),
            (false, false) => format!("Not mounted · {}", drive.file_system),
            (false, true) => "Not mounted".to_string(),
        }
    }

    /// An unmounted removable drive row with a Mount action.
    fn unmounted_drive_row(
        &mut self,
        ux: usize,
        drive: UnmountedDrive,
        now: Instant,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let row_key = SidebarKey::UnmountedDrive(ux);
        let (hover_t, _) = self.step_row_factors(row_key, now, reduced);
        let subtitle = Self::unmounted_drive_subtitle(&drive);
        let tooltip = format!("{} ({}) — not mounted", drive.name, drive.device);
        let drive_name = drive.name.clone();
        let mount_button = self.mount_drive_button(ux, drive, now, reduced, cx);

        div()
            .id(("unmounted-drive-row", ux))
            .debug_selector(move || format!("unmounted-drive-row-{ux}"))
            .min_h(size::SIDEBAR_ROW)
            .py(space::S2)
            .px(space::S8)
            .rounded(radius::CONTROL)
            .bg(color_lerp(transparent_black(), theme.border, hover_t))
            .text_body()
            .text_color(theme.text_muted)
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S8)
            .tooltip(Tooltip::text(tooltip))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.set_hover(row_key, *hovered, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(space::S8)
                    .child(animated_icon_sized(
                        IconName::Drive,
                        IconMotion::Rest,
                        size::ICON,
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().ellipsize().child(drive_name))
                            .child(
                                div()
                                    .ellipsize()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(mount_button)
    }

    /// Icons only, each named by its tooltip. Section titles become dividers, and drive
    /// actions wait for the full sidebar.
    fn render_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx);
        let now = Instant::now();
        let (current_loc, trash_active, servers, reduced, icon_prefs, icon_resolver) = {
            let state = self.app_state.read(cx);
            (
                state.current_location.clone(),
                state.trash_active,
                server_items(state),
                is_reduced_motion(state.settings.motion_preference),
                state.icon_config.active.clone(),
                state.icon_resolver.clone(),
            )
        };
        self.sync_server_draws(&servers, now);

        let places = self.places.clone();
        let volumes = self.volumes.clone();
        let unmounted_drives = self.unmounted_drives.clone();
        let network = self.network.clone();
        let pinned_paths: Vec<PathBuf> = places
            .iter()
            .filter_map(|p| p.path().map(Path::to_path_buf))
            .collect();

        let place_rows: Vec<_> = places
            .iter()
            .enumerate()
            .map(|(ix, place)| {
                let key = SidebarKey::Place(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let is_trash = place.is_trash();
                let is_missing = place.is_missing();
                let path_opt = place.path().map(Path::to_path_buf);
                let path_for_drag_over = path_opt.clone();
                let path_for_drop = path_opt.clone();
                let path_for_ext_over = path_opt.clone();
                let path_for_ext_drop = path_opt.clone();
                let app_state_place_drag = self.app_state.clone();
                let is_active = if is_trash {
                    trash_active
                } else {
                    !trash_active
                        && matches!((&current_loc, &path_opt), (Location::Local(cur), Some(p)) if cur == p)
                };
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let (glyph_name, glyph_color, tooltip_label) = if is_trash {
                    (IconName::Trash, theme.text.into(), place.name.clone())
                } else if is_missing {
                    (
                        IconName::Alert,
                        theme.danger.into(),
                        format!("{} (missing)", place.name),
                    )
                } else if place.icon.is_folder() {
                    (
                        place.icon,
                        resolve_icon_color(place.icon.key_name(), &icon_prefs, &theme),
                        place.name.clone(),
                    )
                } else {
                    (place.icon, theme.text.into(), place.name.clone())
                };
                let glyph_motion = if !reduced
                    && (is_trash || (glyph_name.is_folder() && !is_missing))
                    && (is_active || hover_t > 0.01)
                {
                    IconMotion::State(if is_active { 1.0 } else { hover_t })
                } else {
                    IconMotion::Rest
                };
                let drag_payload = DragSidebarPlace {
                    index: ix,
                    name: place.name.clone(),
                    icon: place.icon,
                    is_trash,
                    is_missing,
                };
                let app_state_drag_preview = self.app_state.clone();
                let resolved_place_icon = if !is_trash && !is_missing && glyph_name.is_folder() {
                    Some(self.cached_resolved_icon(
                        glyph_name,
                        size::ICON * scale,
                        &icon_prefs,
                        &icon_resolver,
                    ))
                } else {
                    None
                };

                div()
                    .id(("place", ix))
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .tooltip(Tooltip::text(tooltip_label))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .on_drag(drag_payload, move |dragged, _, _, cx| {
                        let (resolved_icon, icon_color) = if !dragged.is_trash
                            && !dragged.is_missing
                            && dragged.icon.is_folder()
                        {
                            let theme = Theme::get(cx);
                            let state = app_state_drag_preview.read(cx);
                            let prefs = &state.icon_config.active;
                            let resolved = state.icon_resolver.resolve(
                                prefs,
                                dragged.icon.key_name(),
                                (size::ICON / px(1.0)).round().max(1.0) as u16,
                            );
                            let color = resolve_icon_color(dragged.icon.key_name(), prefs, &theme);
                            (Some(resolved), Some(color))
                        } else {
                            (None, None)
                        };
                        cx.new(|_| DragPlacePreview {
                            name: dragged.name.clone(),
                            icon: dragged.icon,
                            is_trash: dragged.is_trash,
                            is_missing: dragged.is_missing,
                            resolved_icon,
                            icon_color,
                        })
                    })
                    .drag_over::<DragSidebarPlace>(move |style, dragged, _, cx| {
                        if dragged.index == ix {
                            return style;
                        }
                        let theme = Theme::get(cx);
                        let style = style.bg(theme.surface).border_color(theme.accent);
                        if dragged.index < ix {
                            style.border_b_2()
                        } else {
                            style.border_t_2()
                        }
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragSidebarPlace, _, cx| {
                        if dragged.index != ix {
                            let _ = this.reorder_place_in_state(dragged.index, ix, cx);
                        }
                    }))
                    .drag_over::<DragEntries>(move |style, dragged, window, cx| {
                        let theme = Theme::get(cx);
                        let mounts = app_state_place_drag.read(cx).volume_mounts.clone();
                        style_drag_over_place_entry(
                            style,
                            dragged,
                            is_trash,
                            is_missing,
                            path_for_drag_over.as_deref(),
                            window.modifiers(),
                            &mounts,
                            &theme,
                        )
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                        this.handle_place_entries_drop(
                            dragged,
                            is_trash,
                            is_missing,
                            path_for_drop.as_deref(),
                            window.modifiers(),
                            cx,
                        );
                    }))
                    .when(!is_trash && !is_missing, |el| {
                        el.drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                            if let Some(dest) = path_for_ext_over.as_deref() {
                                let theme = Theme::get(cx);
                                style_drag_over_external_local_dir(style, paths, dest, &theme)
                            } else {
                                style
                            }
                        })
                        .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                            if let Some(dest) = path_for_ext_drop.as_deref() {
                                this.handle_external_paths_drop(paths.paths(), dest, cx);
                            }
                        }))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(SidebarViewEvent::OpenPlaceContextMenu {
                                index: ix,
                                is_trash,
                                position: event.position,
                            });
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_trash {
                            this.open_trash(cx);
                        } else if let Some(path) = &path_opt {
                            this.navigate_to_path(path, cx);
                        }
                    }))
                    .child(if let Some(resolved) = resolved_place_icon {
                        render_resolved_icon(
                            &resolved,
                            glyph_name,
                            glyph_motion,
                            size::ICON * scale,
                            glyph_color,
                        )
                    } else {
                        animated_icon_sized(glyph_name, glyph_motion, size::ICON * scale, glyph_color)
                    })
            })
            .collect();

        let drive_rows: Vec<_> = volumes
            .iter()
            .enumerate()
            .map(|(ix, vol)| {
                let key = SidebarKey::Drive(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = vol.mount_point.clone();
                let path_drag_over = path.clone();
                let path_drop = path.clone();
                let path_ext_over = path.clone();
                let path_ext_drop = path.clone();
                let app_state_drive_drag = self.app_state.clone();
                let is_mounted = vol.is_mounted;
                let is_active = !trash_active
                    && matches!(&current_loc, Location::Local(p) if p == &vol.mount_point);
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let label = if is_mounted {
                    format!("{} — {}", vol.name, vol.capacity_summary())
                } else {
                    format!("{} (not mounted)", vol.name)
                };
                let color = if vol.is_busy {
                    theme.warning
                } else if is_mounted {
                    theme.text
                } else {
                    theme.text_muted
                };
                div()
                    .id(("drive", ix))
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .tooltip(Tooltip::text(label))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .when(is_mounted, |s| {
                        s.drag_over::<DragEntries>(move |style, dragged, window, cx| {
                            let theme = Theme::get(cx);
                            let mounts = app_state_drive_drag.read(cx).volume_mounts.clone();
                            style_drag_over_local_dir(
                                style,
                                dragged,
                                &path_drag_over,
                                window.modifiers(),
                                &mounts,
                                &theme,
                            )
                        })
                        .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                            this.handle_local_dir_entries_drop(
                                dragged,
                                &path_drop,
                                window.modifiers(),
                                cx,
                            );
                        }))
                        .drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                            let theme = Theme::get(cx);
                            style_drag_over_external_local_dir(style, paths, &path_ext_over, &theme)
                        })
                        .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                            this.handle_external_paths_drop(paths.paths(), &path_ext_drop, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.set_press(key, true, cx);
                                this.navigate_to_path(&path, cx);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                        )
                    })
                    .child(animated_icon_sized(
                        IconName::Drive,
                        IconMotion::Rest,
                        size::ICON * scale,
                        color,
                    ))
            })
            .collect();

        let unmounted_drive_rows: Vec<_> = unmounted_drives
            .iter()
            .enumerate()
            .map(|(ux, drive)| {
                let key = SidebarKey::UnmountedDrive(ux);
                let (hover_t, _) = self.step_row_factors(key, now, reduced);
                let label = format!("{} ({}) — not mounted", drive.name, drive.device);
                div()
                    .id(("unmounted-drive", ux))
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .bg(color_lerp(transparent_black(), theme.border, hover_t))
                    .tooltip(Tooltip::text(label))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .child(animated_icon_sized(
                        IconName::Drive,
                        IconMotion::Rest,
                        size::ICON,
                        theme.text_muted,
                    ))
            })
            .collect();

        let share_rows: Vec<_> = network
            .iter()
            .enumerate()
            .map(|(ix, share)| {
                let key = SidebarKey::NetworkShare(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = share.mount_point.clone();
                let path_drag_over = path.clone();
                let path_drop = path.clone();
                let path_ext_over = path.clone();
                let path_ext_drop = path.clone();
                let app_state_share_drag = self.app_state.clone();
                let is_active = !trash_active
                    && matches!(&current_loc, Location::Local(p) if p == &share.mount_point);
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let resolved_share_icon = self.cached_resolved_icon(
                    IconName::FolderNetwork,
                    size::ICON * scale,
                    &icon_prefs,
                    &icon_resolver,
                );
                div()
                    .id(("share", ix))
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .tooltip(Tooltip::text(share.name.clone()))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .drag_over::<DragEntries>(move |style, dragged, window, cx| {
                        let theme = Theme::get(cx);
                        let mounts = app_state_share_drag.read(cx).volume_mounts.clone();
                        style_drag_over_local_dir(
                            style,
                            dragged,
                            &path_drag_over,
                            window.modifiers(),
                            &mounts,
                            &theme,
                        )
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                        this.handle_local_dir_entries_drop(
                            dragged,
                            &path_drop,
                            window.modifiers(),
                            cx,
                        );
                    }))
                    .drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                        let theme = Theme::get(cx);
                        style_drag_over_external_local_dir(style, paths, &path_ext_over, &theme)
                    })
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.handle_external_paths_drop(paths.paths(), &path_ext_drop, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.navigate_to_path(&path, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .child(render_resolved_icon(
                        &resolved_share_icon,
                        IconName::FolderNetwork,
                        IconMotion::Rest,
                        size::ICON * scale,
                        resolve_icon_color(IconName::FolderNetwork.key_name(), &icon_prefs, &theme),
                    ))
            })
            .collect();

        let server_rows: Vec<_> = servers
            .iter()
            .enumerate()
            .map(|(ix, server)| {
                let key = SidebarKey::Server(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let server_id = server.id.clone();
                let is_active = !trash_active
                    && matches!(
                        &current_loc,
                        Location::Remote { profile_id, .. } if profile_id == &server.id
                    );
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let color = match server.status {
                    ServerConnectionStatus::Connected => theme.secondary,
                    ServerConnectionStatus::Connecting | ServerConnectionStatus::Reconnecting => {
                        theme.warning
                    }
                    ServerConnectionStatus::Disconnected => theme.text,
                };
                let draw_motion = self
                    .motion
                    .pulse_progress(&SidebarMotionKey::ServerDraw(ix), now, reduced)
                    .map_or(IconMotion::Rest, |t| {
                        IconMotion::Draw(Curve::EaseOut.sample(t))
                    });
                let tooltip = format!(
                    "{} ({}) — {}",
                    server.name,
                    server.protocol,
                    server.status.label()
                );
                div()
                    .id(("server", ix))
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .tooltip(Tooltip::text(tooltip))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.connect_server(&server_id, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .child(animated_icon_sized(
                        IconName::Network,
                        draw_motion,
                        size::ICON * scale,
                        color,
                    ))
            })
            .collect();

        let (manage_hover, manage_press) =
            self.step_row_factors(SidebarKey::ManageServers, now, reduced);
        let manage_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, manage_press)
        };
        let animating = self.motion.has_active(now, reduced);
        self.motion.prune_settled(now, reduced);
        request_frame_if_active(animating, window);

        let divider = || div().h(px(1.)).mx(space::S4).bg(theme.border);
        let compact_pin_paths = pinned_paths.clone();

        div()
            .id("compact-sidebar")
            .size_full()
            .p(space::S8)
            .bg(theme.surface_raised)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap_1()
            .overflow_hidden()
            .children(place_rows)
            .child(
                div()
                    .id("compact-places-pin-drop")
                    .w_full()
                    .h(px(6.))
                    .rounded(radius::CONTROL)
                    .drag_over::<DragEntries>(move |style, dragged, _, cx| {
                        let theme = Theme::get(cx);
                        style_drag_over_pin(style, dragged, &compact_pin_paths, &theme, true)
                    })
                    .on_drop(cx.listener(|this, dragged: &DragEntries, _, cx| {
                        this.handle_pin_drop(dragged, cx);
                    })),
            )
            .when(!volumes.is_empty() || !unmounted_drives.is_empty(), |s| {
                s.child(divider())
            })
            .children(drive_rows)
            .children(unmounted_drive_rows)
            .when(!network.is_empty(), |s| s.child(divider()))
            .children(share_rows)
            .child(divider())
            .children(server_rows)
            .child(
                div()
                    .id("compact-manage-servers")
                    .h(size::SIDEBAR_ROW)
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(color_lerp(transparent_black(), theme.border, manage_hover))
                    .tooltip(Tooltip::text("Gerenciar servidores"))
                    .on_hover(cx.listener(|this, hovered, _, cx| {
                        this.set_hover(SidebarKey::ManageServers, *hovered, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_press(SidebarKey::ManageServers, true, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_press(SidebarKey::ManageServers, false, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_press(SidebarKey::ManageServers, false, cx);
                        }),
                    )
                    .child(animated_icon_sized(
                        IconName::Plus,
                        IconMotion::Rest,
                        size::ICON * manage_scale,
                        theme.text_muted,
                    ))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ManageServers), cx)),
            )
            .into_any_element()
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = self
            .app_state
            .read(cx)
            .panels
            .resolve(window.viewport_size().width)
            .sidebar_compact;
        if compact {
            return self.render_compact(window, cx);
        }

        let theme = Theme::get(cx);
        let now = Instant::now();
        let places = self.places.clone();
        let volumes = self.volumes.clone();
        let unmounted_drives = self.unmounted_drives.clone();
        let network = self.network.clone();
        let drive_details = self.drive_details;
        let editing_place = self.editing_place;
        let (current_loc, trash_active, servers, reduced, icon_prefs, icon_resolver) = {
            let state = self.app_state.read(cx);
            (
                state.current_location.clone(),
                state.trash_active,
                server_items(state),
                is_reduced_motion(state.settings.motion_preference),
                state.icon_config.active.clone(),
                state.icon_resolver.clone(),
            )
        };
        self.sync_server_draws(&servers, now);
        let can_pin_current = !trash_active
            && matches!(&current_loc, Location::Local(p) if !places.iter().any(|place| place.path() == Some(p.as_path())));

        let (add_place_hover, add_place_press) =
            self.step_row_factors(SidebarKey::AddPlace, now, reduced);
        let add_place_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, add_place_press)
        };

        let pinned_paths: Vec<PathBuf> = places
            .iter()
            .filter_map(|p| p.path().map(Path::to_path_buf))
            .collect();

        let place_rows: Vec<AnyElement> = places
            .into_iter()
            .enumerate()
            .map(|(ix, place)| {
                let is_trash = place.is_trash();
                let is_missing = place.is_missing();
                let path_opt = place.path().map(Path::to_path_buf);
                let path_for_drag_over = path_opt.clone();
                let path_for_drop = path_opt.clone();
                let path_for_ext_over = path_opt.clone();
                let path_for_ext_drop = path_opt.clone();
                let app_state_place_drag = self.app_state.clone();

                if !is_trash && editing_place == Some(ix) {
                    return div()
                        .id(("place-edit", ix))
                        .h(size::SIDEBAR_ROW)
                        .px(space::S4)
                        .rounded(radius::CONTROL)
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.accent)
                        .flex()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                            if ev.keystroke.key == "enter" {
                                cx.stop_propagation();
                                this.commit_rename_place(cx);
                            } else if ev.keystroke.key == "escape" {
                                cx.stop_propagation();
                                this.cancel_rename_place(cx);
                            }
                        }))
                        .child(self.rename_input.clone())
                        .into_any_element();
                }

                let key = SidebarKey::Place(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let is_active = if is_trash {
                    trash_active
                } else {
                    !trash_active
                        && matches!((&current_loc, &path_opt), (Location::Local(cur), Some(p)) if cur == p)
                };
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let icon_motion = if !reduced
                    && (is_trash || (place.icon.is_folder() && !is_missing))
                    && (is_active || hover_t > 0.01)
                {
                    IconMotion::State(if is_active { 1.0 } else { hover_t })
                } else {
                    IconMotion::Rest
                };
                let color = if is_trash {
                    theme.text.into()
                } else if is_missing {
                    theme.danger.into()
                } else if place.icon.is_folder() {
                    resolve_icon_color(place.icon.key_name(), &icon_prefs, &theme)
                } else {
                    theme.text.into()
                };
                let drag_payload = DragSidebarPlace {
                    index: ix,
                    name: place.name.clone(),
                    icon: place.icon,
                    is_trash,
                    is_missing,
                };
                let app_state_drag_preview = self.app_state.clone();
                let resolved_place_icon = if !is_trash && !is_missing && place.icon.is_folder() {
                    Some(self.cached_resolved_icon(
                        place.icon,
                        size::ICON * scale,
                        &icon_prefs,
                        &icon_resolver,
                    ))
                } else {
                    None
                };

                div()
                    .id(("place-row", ix))
                    .debug_selector(move || format!("place-row-{ix}"))
                    .h(size::SIDEBAR_ROW)
                    .px(space::S8)
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .text_body()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S4)
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .on_drag(drag_payload, move |dragged, _, _, cx| {
                        let (resolved_icon, icon_color) = if !dragged.is_trash
                            && !dragged.is_missing
                            && dragged.icon.is_folder()
                        {
                            let theme = Theme::get(cx);
                            let state = app_state_drag_preview.read(cx);
                            let prefs = &state.icon_config.active;
                            let resolved = state.icon_resolver.resolve(
                                prefs,
                                dragged.icon.key_name(),
                                (size::ICON / px(1.0)).round().max(1.0) as u16,
                            );
                            let color = resolve_icon_color(dragged.icon.key_name(), prefs, &theme);
                            (Some(resolved), Some(color))
                        } else {
                            (None, None)
                        };
                        cx.new(|_| DragPlacePreview {
                            name: dragged.name.clone(),
                            icon: dragged.icon,
                            is_trash: dragged.is_trash,
                            is_missing: dragged.is_missing,
                            resolved_icon,
                            icon_color,
                        })
                    })
                    .drag_over::<DragSidebarPlace>(move |style, dragged, _, cx| {
                        if dragged.index == ix {
                            return style;
                        }
                        let theme = Theme::get(cx);
                        let style = style.bg(theme.surface).border_color(theme.accent);
                        if dragged.index < ix {
                            style.border_b_2()
                        } else {
                            style.border_t_2()
                        }
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragSidebarPlace, _, cx| {
                        if dragged.index != ix {
                            let _ = this.reorder_place_in_state(dragged.index, ix, cx);
                        }
                    }))
                    .drag_over::<DragEntries>(move |style, dragged, window, cx| {
                        let theme = Theme::get(cx);
                        let mounts = app_state_place_drag.read(cx).volume_mounts.clone();
                        style_drag_over_place_entry(
                            style,
                            dragged,
                            is_trash,
                            is_missing,
                            path_for_drag_over.as_deref(),
                            window.modifiers(),
                            &mounts,
                            &theme,
                        )
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                        this.handle_place_entries_drop(
                            dragged,
                            is_trash,
                            is_missing,
                            path_for_drop.as_deref(),
                            window.modifiers(),
                            cx,
                        );
                    }))
                    .when(!is_trash && !is_missing, |el| {
                        el.drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                            if let Some(dest) = path_for_ext_over.as_deref() {
                                let theme = Theme::get(cx);
                                style_drag_over_external_local_dir(style, paths, dest, &theme)
                            } else {
                                style
                            }
                        })
                        .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                            if let Some(dest) = path_for_ext_drop.as_deref() {
                                this.handle_external_paths_drop(paths.paths(), dest, cx);
                            }
                        }))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(space::S8)
                            .child(if is_trash {
                                animated_icon_sized(
                                    IconName::Trash,
                                    icon_motion,
                                    size::ICON * scale,
                                    color,
                                )
                            } else if is_missing {
                                animated_icon_sized(
                                    IconName::Alert,
                                    IconMotion::Rest,
                                    size::ICON * scale,
                                    color,
                                )
                            } else if let Some(resolved) = resolved_place_icon {
                                render_resolved_icon(
                                    &resolved,
                                    place.icon,
                                    icon_motion,
                                    size::ICON * scale,
                                    color,
                                )
                            } else {
                                animated_icon_sized(
                                    place.icon,
                                    icon_motion,
                                    size::ICON * scale,
                                    color,
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .ellipsize()
                                    .text_color(if is_missing {
                                        theme.danger
                                    } else {
                                        theme.text
                                    })
                                    .child(place.name),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(SidebarViewEvent::OpenPlaceContextMenu {
                                index: ix,
                                is_trash,
                                position: event.position,
                            });
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_trash {
                            this.open_trash(cx);
                        } else if let Some(path) = &path_opt {
                            this.navigate_to_path(path, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect();

        let (manage_hover, manage_press) =
            self.step_row_factors(SidebarKey::ManageServers, now, reduced);
        let manage_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, manage_press)
        };
        let manage_motion = if !reduced && manage_hover > 0.01 {
            IconMotion::State(manage_hover)
        } else {
            IconMotion::Rest
        };

        let (add_srv_hover, add_srv_press) =
            self.step_row_factors(SidebarKey::AddServer, now, reduced);
        let add_srv_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, add_srv_press)
        };

        let server_rows: Vec<_> = servers
            .iter()
            .enumerate()
            .map(|(ix, server)| {
                let key = SidebarKey::Server(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let server_id = server.id.clone();
                let is_active = !trash_active
                    && matches!(
                        &current_loc,
                        Location::Remote { profile_id, .. } if profile_id == &server.id
                    );
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let dot_color = match server.status {
                    ServerConnectionStatus::Connected => theme.secondary,
                    ServerConnectionStatus::Connecting | ServerConnectionStatus::Reconnecting => {
                        theme.warning
                    }
                    ServerConnectionStatus::Disconnected => theme.border_strong,
                };
                let draw_motion = self
                    .motion
                    .pulse_progress(&SidebarMotionKey::ServerDraw(ix), now, reduced)
                    .map_or(IconMotion::Rest, |t| {
                        IconMotion::Draw(Curve::EaseOut.sample(t))
                    });
                let tooltip = format!(
                    "{} ({}://{}:{}) — {}",
                    server.name,
                    server.protocol.to_ascii_lowercase(),
                    server.host,
                    server.port,
                    server.status.label()
                );

                div()
                    .id(("server-row", ix))
                    .h(size::SIDEBAR_ROW)
                    .px(space::S8)
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .text_body()
                    .text_color(theme.text)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S8)
                    .tooltip(Tooltip::text(tooltip))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(space::S8)
                            .child(
                                div()
                                    .w(px(7.))
                                    .h(px(7.))
                                    .rounded_full()
                                    .flex_none()
                                    .bg(dot_color),
                            )
                            .child(animated_icon_sized(
                                IconName::Network,
                                draw_motion,
                                size::ICON * scale,
                                theme.text,
                            ))
                            .child(div().flex_1().ellipsize().child(server.name.clone())),
                    )
                    .child(
                        div()
                            .px(space::S4)
                            .rounded(radius::CONTROL)
                            .bg(theme.surface)
                            .text_label()
                            .text_color(theme.text_muted)
                            .child(server.protocol),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.connect_server(&server_id, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
            })
            .collect();

        let drive_elements: Vec<_> = volumes
            .iter()
            .cloned()
            .enumerate()
            .map(|(ix, vol)| {
                if !drive_details {
                    return self
                        .drive_row(ix, vol, &current_loc, trash_active, now, reduced, cx)
                        .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
                        .into_any_element();
                }
                let is_mounted = vol.is_mounted;
                let is_busy = vol.is_busy;
                let busy_reason = vol.busy_reason.clone();
                let is_removable = vol.is_removable;
                let capacity_text = vol.capacity_summary();
                let used_pct = vol.used_percentage();
                let p_nav = vol.mount_point.clone();
                let eject_mount_point = vol.mount_point.clone();
                let eject_name = vol.name.clone();
                let p_drag_over = p_nav.clone();
                let p_drop = p_nav.clone();
                let p_ext_over = p_nav.clone();
                let p_ext_drop = p_nav.clone();
                let app_state_card_drag = self.app_state.clone();
                let is_active = !trash_active && current_loc.path() == vol.mount_point;

                div()
                    .id(("drive-card", ix))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
                    .when(is_mounted, |card| {
                        card.drag_over::<DragEntries>(move |style, dragged, window, cx| {
                            let theme = Theme::get(cx);
                            let mounts = app_state_card_drag.read(cx).volume_mounts.clone();
                            style_drag_over_local_dir(
                                style,
                                dragged,
                                &p_drag_over,
                                window.modifiers(),
                                &mounts,
                                &theme,
                            )
                        })
                        .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                            this.handle_local_dir_entries_drop(
                                dragged,
                                &p_drop,
                                window.modifiers(),
                                cx,
                            );
                        }))
                        .drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                            let theme = Theme::get(cx);
                            style_drag_over_external_local_dir(style, paths, &p_ext_over, &theme)
                        })
                        .on_drop(cx.listener(
                            move |this, paths: &ExternalPaths, _, cx| {
                                this.handle_external_paths_drop(paths.paths(), &p_ext_drop, cx);
                            },
                        ))
                    })
                    .p_2()
                    .rounded_md()
                    .bg(if is_active {
                        theme.selection
                    } else {
                        transparent_black().into()
                    })
                    .border_1()
                    .border_color(if is_active {
                        theme.accent
                    } else {
                        theme.border
                    })
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.navigate_to_path(&p_nav, cx);
                                        }),
                                    )
                                    .child(icon(IconName::Drive).text_color(theme.text))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(vol.name),
                                    ),
                            )
                            .when(is_removable && is_mounted, |p| {
                                p.child(
                                    div()
                                        .id(("drive-card-eject", ix))
                                        .p(space::S2)
                                        .rounded(radius::CONTROL)
                                        .cursor_pointer()
                                        .tooltip(Tooltip::text("Eject"))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.eject_drive(
                                                    eject_mount_point.clone(),
                                                    eject_name.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(animated_icon_sized(
                                            IconName::Eject,
                                            IconMotion::Rest,
                                            size::ICON,
                                            if is_busy {
                                                theme.warning
                                            } else {
                                                theme.text_muted
                                            },
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(capacity_text),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_sm()
                            .bg(theme.border)
                            .child(
                                div()
                                    .w(gpui::DefiniteLength::Fraction(
                                        (used_pct as f32 / 100.0).min(1.0),
                                    ))
                                    .h_full()
                                    .rounded_sm()
                                    .bg(if used_pct > 90 {
                                        theme.danger
                                    } else {
                                        theme.accent
                                    }),
                            ),
                    )
                    .when(is_busy, |p| {
                        p.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S4)
                                .text_size(px(11.))
                                .text_color(theme.warning)
                                .child(icon(IconName::Alert).text_color(theme.warning))
                                .child(
                                    busy_reason
                                        .unwrap_or_else(|| "In use by open files".to_string()),
                                ),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        let unmounted_drive_elements: Vec<AnyElement> = unmounted_drives
            .iter()
            .cloned()
            .enumerate()
            .map(|(ux, drive)| {
                if !drive_details {
                    return self
                        .unmounted_drive_row(ux, drive, now, reduced, cx)
                        .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
                        .into_any_element();
                }
                let subtitle = Self::unmounted_drive_subtitle(&drive);
                let drive_name = drive.name.clone();
                let mount_button = self.mount_drive_button(ux, drive, now, reduced, cx);
                div()
                    .id(("unmounted-drive-card", ux))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(icon(IconName::Drive).text_color(theme.text_muted))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.text_muted)
                                            .child(drive_name),
                                    ),
                            )
                            .child(mount_button),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(subtitle),
                    )
                    .into_any_element()
            })
            .collect();

        let (refresh_drives_hover, refresh_drives_press) =
            self.step_row_factors(SidebarKey::RefreshDrives, now, reduced);
        let refresh_drives_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, refresh_drives_press)
        };
        let refresh_drives_motion = self
            .motion
            .pulse_progress(&SidebarMotionKey::RefreshSpin, now, reduced)
            .map_or(IconMotion::Rest, IconMotion::Pulse);

        let network_rows: Vec<_> = network
            .iter()
            .enumerate()
            .map(|(ix, share)| {
                let key = SidebarKey::NetworkShare(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let is_active = !trash_active
                    && matches!(&current_loc, Location::Local(p) if p == &share.mount_point);
                let row_bg = if is_active {
                    theme.selection.into()
                } else {
                    color_lerp(transparent_black(), theme.border, hover_t)
                };
                let scale = if reduced {
                    1.0
                } else {
                    lerp_f32(1.0, PRESS_SCALE, press_t)
                };
                let path = share.mount_point.clone();
                let path_drag_over = path.clone();
                let path_drop = path.clone();
                let path_ext_over = path.clone();
                let path_ext_drop = path.clone();
                let app_state_net_drag = self.app_state.clone();

                let resolved_share_icon = self.cached_resolved_icon(
                    IconName::FolderNetwork,
                    size::ICON * scale,
                    &icon_prefs,
                    &icon_resolver,
                );
                div()
                    .id(("network-share", ix))
                    .h(size::SIDEBAR_ROW)
                    .px(space::S8)
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .bg(row_bg)
                    .text_body()
                    .text_color(theme.text)
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .tooltip(Tooltip::text(
                        share.mount_point.to_string_lossy().into_owned(),
                    ))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .drag_over::<DragEntries>(move |style, dragged, window, cx| {
                        let theme = Theme::get(cx);
                        let mounts = app_state_net_drag.read(cx).volume_mounts.clone();
                        style_drag_over_local_dir(
                            style,
                            dragged,
                            &path_drag_over,
                            window.modifiers(),
                            &mounts,
                            &theme,
                        )
                    })
                    .on_drop(cx.listener(move |this, dragged: &DragEntries, window, cx| {
                        this.handle_local_dir_entries_drop(
                            dragged,
                            &path_drop,
                            window.modifiers(),
                            cx,
                        );
                    }))
                    .drag_over::<ExternalPaths>(move |style, paths, _, cx| {
                        let theme = Theme::get(cx);
                        style_drag_over_external_local_dir(style, paths, &path_ext_over, &theme)
                    })
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.handle_external_paths_drop(paths.paths(), &path_ext_drop, cx);
                    }))
                    .child(render_resolved_icon(
                        &resolved_share_icon,
                        IconName::FolderNetwork,
                        IconMotion::Rest,
                        size::ICON * scale,
                        resolve_icon_color(IconName::FolderNetwork.key_name(), &icon_prefs, &theme),
                    ))
                    .child(div().flex_1().ellipsize().child(share.name.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.navigate_to_path(&path, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_press(key, false, cx)),
                    )
            })
            .collect();

        let animating = self.motion.has_active(now, reduced);
        self.motion.prune_settled(now, reduced);
        request_frame_if_active(animating, window);
        let header_pin_paths = pinned_paths.clone();
        let footer_pin_paths = pinned_paths;

        div()
            .size_full()
            .p(space::S8)
            .bg(theme.surface_raised)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap_4()
            .overflow_hidden()
            // --- PLACES SECTION ---
            .child(
                div()
                    .id("sidebar-places-section")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .on_drop(cx.listener(|this, dragged: &DragEntries, _, cx| {
                        this.handle_pin_drop(dragged, cx);
                    }))
                    .child(
                        div()
                            .id("sidebar-places-header")
                            .debug_selector(|| "sidebar-places-header".into())
                            .px(space::S8)
                            .py(space::S4)
                            .rounded(radius::CONTROL)
                            .flex()
                            .items_center()
                            .justify_between()
                            .drag_over::<DragEntries>(move |style, dragged, _, cx| {
                                let theme = Theme::get(cx);
                                style_drag_over_pin(
                                    style,
                                    dragged,
                                    &header_pin_paths,
                                    &theme,
                                    false,
                                )
                            })
                            .on_drop(cx.listener(|this, dragged: &DragEntries, _, cx| {
                                this.handle_pin_drop(dragged, cx);
                            }))
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child("Places"),
                            )
                            .when(can_pin_current, |hdr| {
                                hdr.child(
                                    div()
                                        .id("sidebar-add-place")
                                        .debug_selector(|| "sidebar-add-place".into())
                                        .p(space::S2)
                                        .rounded(radius::CONTROL)
                                        .cursor_pointer()
                                        .bg(color_lerp(
                                            transparent_black(),
                                            theme.border,
                                            add_place_hover,
                                        ))
                                        .tooltip(Tooltip::text("Adicionar pasta atual aos locais"))
                                        .on_hover(cx.listener(|this, hovered, _, cx| {
                                            this.set_hover(SidebarKey::AddPlace, *hovered, cx);
                                        }))
                                        .child(animated_icon_sized(
                                            IconName::Plus,
                                            IconMotion::Rest,
                                            size::ICON * add_place_scale,
                                            theme.text_muted,
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.set_press(SidebarKey::AddPlace, true, cx);
                                                let _ = this.pin_current_location(cx);
                                            }),
                                        )
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.set_press(SidebarKey::AddPlace, false, cx);
                                            }),
                                        )
                                        .on_mouse_up_out(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.set_press(SidebarKey::AddPlace, false, cx);
                                            }),
                                        ),
                                )
                            }),
                    )
                    .children(place_rows)
                    .child(
                        div()
                            .id("sidebar-places-pin-drop")
                            .debug_selector(|| "sidebar-places-pin-drop".into())
                            .w_full()
                            .h(px(6.))
                            .rounded(radius::CONTROL)
                            .drag_over::<DragEntries>(move |style, dragged, _, cx| {
                                let theme = Theme::get(cx);
                                style_drag_over_pin(style, dragged, &footer_pin_paths, &theme, true)
                            })
                            .on_drop(cx.listener(|this, dragged: &DragEntries, _, cx| {
                                this.handle_pin_drop(dragged, cx);
                            })),
                    ),
            )
            // --- SERVERS SECTION ---
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px(space::S8)
                            .py(space::S4)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child("Servidores"),
                            )
                            .child(
                                div()
                                    .id("sidebar-manage-servers")
                                    .px(space::S4)
                                    .py(space::S2)
                                    .rounded(radius::CONTROL)
                                    .cursor_pointer()
                                    .bg(color_lerp(transparent_black(), theme.border, manage_hover))
                                    .flex()
                                    .items_center()
                                    .gap(space::S4)
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .tooltip(Tooltip::text("Gerenciar servidores"))
                                    .on_hover(cx.listener(|this, hovered, _, cx| {
                                        this.set_hover(SidebarKey::ManageServers, *hovered, cx);
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_press(SidebarKey::ManageServers, true, cx);
                                        }),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_press(SidebarKey::ManageServers, false, cx);
                                        }),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_press(SidebarKey::ManageServers, false, cx);
                                        }),
                                    )
                                    .child(animated_icon_sized(
                                        IconName::Settings,
                                        manage_motion,
                                        size::ICON * manage_scale,
                                        theme.text_muted,
                                    ))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ManageServers), cx);
                                    }),
                            ),
                    )
                    .when(servers.is_empty(), |sec| {
                        sec.child(
                            div()
                                .id("sidebar-add-server")
                                .h(size::SIDEBAR_ROW)
                                .px(space::S8)
                                .rounded(radius::CONTROL)
                                .cursor_pointer()
                                .bg(color_lerp(transparent_black(), theme.border, add_srv_hover))
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .text_body()
                                .text_color(theme.text_muted)
                                .on_hover(cx.listener(|this, hovered, _, cx| {
                                    this.set_hover(SidebarKey::AddServer, *hovered, cx);
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_press(SidebarKey::AddServer, true, cx);
                                    }),
                                )
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_press(SidebarKey::AddServer, false, cx);
                                    }),
                                )
                                .on_mouse_up_out(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_press(SidebarKey::AddServer, false, cx);
                                    }),
                                )
                                .child(animated_icon_sized(
                                    IconName::Plus,
                                    IconMotion::Rest,
                                    size::ICON * add_srv_scale,
                                    theme.text_muted,
                                ))
                                .child("Adicionar servidor…")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ManageServers), cx);
                                }),
                        )
                    })
                    .children(server_rows),
            )
            // --- VOLUMES & DRIVES SECTION ---
            .when(
                !volumes.is_empty() || !unmounted_drives.is_empty(),
                |parent| {
                    parent.child(
                        div()
                            .flex()
                            .flex_col()
                            .when(drive_details, |s| s.gap_2())
                            .when(!drive_details, |s| s.gap_1())
                            .child(
                                div()
                                    .px(space::S8)
                                    .py(space::S4)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(Self::toggle_drive_details),
                                    )
                                    .child(
                                        div()
                                            .text_label()
                                            .text_color(theme.text_muted)
                                            .child("Drives"),
                                    )
                                    .child(
                                        div()
                                            .id("sidebar-refresh-drives")
                                            .p(space::S2)
                                            .rounded(radius::CONTROL)
                                            .cursor_pointer()
                                            .bg(color_lerp(
                                                transparent_black(),
                                                theme.border,
                                                refresh_drives_hover,
                                            ))
                                            .tooltip(Tooltip::text("Refresh drives"))
                                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                                this.set_hover(
                                                    SidebarKey::RefreshDrives,
                                                    *hovered,
                                                    cx,
                                                );
                                            }))
                                            .child(animated_icon_sized(
                                                IconName::Refresh,
                                                refresh_drives_motion,
                                                size::ICON * refresh_drives_scale,
                                                theme.text_muted,
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.set_press(
                                                        SidebarKey::RefreshDrives,
                                                        true,
                                                        cx,
                                                    );
                                                    this.motion.trigger_pulse(
                                                        SidebarMotionKey::RefreshSpin,
                                                        QUICK,
                                                        MotionKind::MovementOrScale,
                                                        Instant::now(),
                                                    );
                                                    this.refresh_devices(cx);
                                                }),
                                            )
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_press(
                                                        SidebarKey::RefreshDrives,
                                                        false,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .on_mouse_up_out(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_press(
                                                        SidebarKey::RefreshDrives,
                                                        false,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    ),
                            )
                            .children(drive_elements)
                            .children(unmounted_drive_elements),
                    )
                },
            )
            // --- NETWORK SECTION ---
            .when(!network.is_empty(), |parent| {
                parent.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .px(space::S8)
                                .py(space::S4)
                                .text_label()
                                .text_color(theme.text_muted)
                                .child("Network"),
                        )
                        .children(network_rows),
                )
            })
            .into_any_element()
    }
}
