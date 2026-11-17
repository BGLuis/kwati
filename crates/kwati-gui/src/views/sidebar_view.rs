//! Sidebar view (owned by guinet-worker for #61).
//!
//! Renders quick-access places, bookmarks, the device's mounted drives with capacity
//! indicators, and mounted network shares.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, transparent_black, AnyElement, Context, Div, Entity, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, Stateful, Subscription, Task, Window,
};
use kwati_core::{
    device_volumes, network_mounts, ConnectionState, Location, NetworkMount, PlaceKind, Volume,
};

use crate::actions::ManageServers;
use crate::components::{
    animated_icon_sized, icon, icon_for_with, Ellipsize, IconMotion, IconName, SpecialFolders,
    TextInput, Tooltip,
};
use crate::motion::{
    color_lerp, is_reduced_motion, lerp_f32, request_frame_if_active, Curve, MotionKind,
    MotionTracker, INSTANT, PRESS_SCALE, QUICK,
};
use crate::remote::protocol_label;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, TextRole};

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

/// A standard quick-access place (Home, Documents, Downloads, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPlace {
    pub name: String,
    pub path: PathBuf,
    pub icon: IconName,
}

impl SidebarPlace {
    pub fn new(name: impl Into<String>, path: PathBuf, icon: IconName) -> Self {
        Self {
            name: name.into(),
            path,
            icon,
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

/// A user-defined pinned bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserBookmark {
    pub name: String,
    pub path: PathBuf,
}

impl UserBookmark {
    pub fn new(name: impl Into<String>, path: PathBuf) -> Self {
        Self {
            name: name.into(),
            path,
        }
    }

    /// Returns true if the bookmarked target directory/file no longer exists.
    pub fn is_missing(&self) -> bool {
        !self.path.exists()
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebarKey {
    Place(usize),
    Bookmark(usize),
    AddBookmark,
    ManageServers,
    AddServer,
    Server(usize),
    Drive(usize),
    DriveEject(usize),
    NetworkShare(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebarMotionKey {
    Hover(SidebarKey),
    Press(SidebarKey),
    ServerDraw(usize),
    EjectHop(usize),
}

pub struct SidebarView {
    pub app_state: Entity<AppState>,
    pub places: Vec<SidebarPlace>,
    pub bookmarks: Vec<UserBookmark>,
    pub volumes: Vec<SidebarVolume>,
    pub network: Vec<NetworkMount>,
    /// Drives show as plain rows unless the user asks for capacity cards with a right click.
    pub drive_details: bool,
    editing_bookmark: Option<usize>,
    rename_input: Entity<TextInput>,
    hovered: HashSet<SidebarKey>,
    pressed: HashSet<SidebarKey>,
    last_server_states: Vec<(String, ServerConnectionStatus)>,
    motion: MotionTracker<SidebarMotionKey>,
    _state_sub: Subscription,
    _device_poll: Task<()>,
}

impl SidebarView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let initial_bookmarks = app_state
            .read(cx)
            .settings
            .bookmarks
            .iter()
            .map(|b| UserBookmark::new(&b.name, b.path.clone()))
            .collect();
        let rename_input = cx.new(|cx| TextInput::with_placeholder("Nome do favorito", cx));
        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            this.sync_bookmarks_from_state(app_state.read(cx));
            cx.notify();
        });
        let device_poll = cx.spawn(async move |this, cx| loop {
            let (volumes, network) = cx
                .background_executor()
                .spawn(async { (device_volumes(), network_mounts()) })
                .await;
            let listed = this.update(cx, |this, cx| this.set_devices(volumes, network, cx));
            if listed.is_err() {
                break;
            }
            cx.background_executor().timer(DEVICE_POLL).await;
        });
        Self {
            app_state,
            places: standard_places(),
            bookmarks: initial_bookmarks,
            volumes: Vec::new(),
            network: Vec::new(),
            drive_details: false,
            editing_bookmark: None,
            rename_input,
            hovered: HashSet::new(),
            pressed: HashSet::new(),
            last_server_states: Vec::new(),
            motion: MotionTracker::new(),
            _state_sub: state_sub,
            _device_poll: device_poll,
        }
    }

    /// Synchronizes `self.bookmarks` with `state.settings.bookmarks`.
    pub fn sync_bookmarks_from_state(&mut self, state: &AppState) {
        let synced: Vec<UserBookmark> = state
            .settings
            .bookmarks
            .iter()
            .map(|b| UserBookmark::new(&b.name, b.path.clone()))
            .collect();
        if self.bookmarks != synced {
            self.bookmarks = synced;
            if let Some(idx) = self.editing_bookmark {
                if idx >= self.bookmarks.len() {
                    self.editing_bookmark = None;
                }
            }
        }
    }

    fn set_devices(
        &mut self,
        volumes: Vec<Volume>,
        network: Vec<NetworkMount>,
        cx: &mut Context<Self>,
    ) {
        let volumes: Vec<SidebarVolume> = volumes.into_iter().map(SidebarVolume::from).collect();
        if volumes != self.volumes || network != self.network {
            self.volumes = volumes;
            self.network = network;
            cx.notify();
        }
    }

    // --- Bookmarks management ---

    pub fn add_bookmark(&mut self, name: impl Into<String>, path: PathBuf) {
        self.bookmarks.push(UserBookmark::new(name, path));
    }

    pub fn remove_bookmark(&mut self, index: usize) -> Option<UserBookmark> {
        if index < self.bookmarks.len() {
            Some(self.bookmarks.remove(index))
        } else {
            None
        }
    }

    pub fn reorder_bookmark(&mut self, from: usize, to: usize) -> bool {
        if from >= self.bookmarks.len() || to >= self.bookmarks.len() {
            return false;
        }
        let item = self.bookmarks.remove(from);
        self.bookmarks.insert(to, item);
        true
    }

    /// Adds a bookmark via `AppState::add_bookmark` and persists/syncs it.
    pub fn add_bookmark_in_state(
        &mut self,
        name: impl Into<String>,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.add_bookmark(name, path));
        if res.is_ok() {
            let state = self.app_state.read(cx);
            self.sync_bookmarks_from_state(state);
        }
        cx.notify();
        res
    }

    /// Removes a bookmark via `AppState::remove_bookmark` and persists/syncs it.
    pub fn remove_bookmark_in_state(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.remove_bookmark(index).map(|_| ()));
        if res.is_ok() {
            if self.editing_bookmark == Some(index) {
                self.editing_bookmark = None;
            }
            let state = self.app_state.read(cx);
            self.sync_bookmarks_from_state(state);
        }
        cx.notify();
        res
    }

    /// Renames a bookmark via `AppState::rename_bookmark` and persists/syncs it.
    pub fn rename_bookmark_in_state(
        &mut self,
        index: usize,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self
            .app_state
            .update(cx, |state, _| state.rename_bookmark(index, name));
        if res.is_ok() {
            self.editing_bookmark = None;
            let state = self.app_state.read(cx);
            self.sync_bookmarks_from_state(state);
        }
        cx.notify();
        res
    }

    /// Pins the current local folder into `AppState::settings.bookmarks`.
    pub fn pin_current_location(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let current = self.app_state.read(cx).current_location.clone();
        let Location::Local(path) = current else {
            return Err("Apenas pastas locais podem ser fixadas nos favoritos.".into());
        };
        if self.bookmarks.iter().any(|b| b.path == path) {
            return Ok(());
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        self.add_bookmark_in_state(name, path, cx)
    }

    pub fn begin_rename_bookmark(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bm) = self.bookmarks.get(index) else {
            return;
        };
        let current_name = bm.name.clone();
        self.editing_bookmark = Some(index);
        self.rename_input.update(cx, |input, cx| {
            input.set_text(current_name, cx);
        });
        self.rename_input.read(cx).focus_handle.focus(window);
        cx.notify();
    }

    pub fn commit_rename_bookmark(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.editing_bookmark else {
            return;
        };
        let new_name = self.rename_input.read(cx).text().trim().to_string();
        if new_name.is_empty() {
            self.editing_bookmark = None;
            cx.notify();
            return;
        }
        let _ = self.rename_bookmark_in_state(index, new_name, cx);
    }

    pub fn cancel_rename_bookmark(&mut self, cx: &mut Context<Self>) {
        self.editing_bookmark = None;
        cx.notify();
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

    /// Ejects a removable volume.
    /// If drive is busy (open file), keeps it listed and reports the busy reason.
    /// If ejecting succeeds and current location is on the drive, navigates to Home first,
    /// removes volume, and returns SafeToRemove.
    pub fn eject_volume(&mut self, volume_id: &str, current_location: &Location) -> EjectResult {
        let Some(pos) = self.volumes.iter().position(|v| v.id == volume_id) else {
            return EjectResult::NotFound;
        };

        let vol = &self.volumes[pos];

        if vol.is_busy {
            let reason = vol
                .busy_reason
                .clone()
                .unwrap_or_else(|| "Drive has active files open in another process".to_string());
            return EjectResult::Busy {
                volume_name: vol.name.clone(),
                reason,
            };
        }

        let vol_name = vol.name.clone();
        let mount_point = vol.mount_point.clone();

        // If currently inside the drive being ejected, move to Home folder first
        let current_path = current_location.path();
        let was_inside = current_path.starts_with(&mount_point);

        // Remove from active volumes list
        self.volumes.remove(pos);

        if was_inside {
            // Signal navigation to Home
            let home = system_home_dir().unwrap_or_else(|| PathBuf::from("/"));
            // (Caller or UI handler updates AppState navigation)
            let _ = home;
        }

        EjectResult::SafeToRemove {
            volume_name: vol_name,
            unmounted_path: mount_point,
        }
    }

    /// Mounts an unmounted volume.
    pub fn mount_volume(&mut self, volume_id: &str) -> MountResult {
        let Some(vol) = self.volumes.iter_mut().find(|v| v.id == volume_id) else {
            return MountResult::Failed("Volume not found".into());
        };

        vol.is_mounted = true;
        MountResult::Mounted {
            volume_name: vol.name.clone(),
            mount_point: vol.mount_point.clone(),
        }
    }

    fn navigate_to_path(&self, path: &Path, cx: &mut Context<Self>) {
        let location = Location::Local(path.to_path_buf());
        self.app_state.update(cx, |state, _cx| {
            state.navigate_to(location);
        });
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

    /// A drive as one row, like a place; its capacity waits in the tooltip.
    fn drive_row(
        &mut self,
        ix: usize,
        vol: SidebarVolume,
        current_loc: &Location,
        now: Instant,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let is_active = matches!(current_loc, Location::Local(p) if p == &vol.mount_point);
        let tooltip = if vol.is_mounted {
            format!("{} — {}", vol.mount_point.display(), vol.capacity_summary())
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
        let eject_id = vol.id.clone();

        div()
            .id(("drive-row", ix))
            .h(size::SIDEBAR_ROW)
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
                    .child(div().ellipsize().child(vol.name)),
            )
            .when(vol.is_removable && vol.is_mounted, |el| {
                el.child(
                    div()
                        .id(("drive-eject", ix))
                        .p(space::S2)
                        .rounded(radius::CONTROL)
                        .bg(color_lerp(
                            transparent_black(),
                            theme.surface_raised,
                            eject_hover,
                        ))
                        .tooltip(Tooltip::text("Ejetar"))
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
                                let cur_loc = this.app_state.read(cx).current_location.clone();
                                match this.eject_volume(&eject_id, &cur_loc) {
                                    EjectResult::SafeToRemove {
                                        volume_name,
                                        unmounted_path: _,
                                    } => {
                                        this.app_state.update(cx, |state, _| {
                                            state.set_completion_status(format!(
                                                "Seguro para remover '{volume_name}'"
                                            ));
                                        });
                                    }
                                    EjectResult::Busy {
                                        volume_name,
                                        reason,
                                    } => {
                                        this.app_state.update(cx, |state, _| {
                                            state.set_status(format!(
                                                "Não foi possível ejetar '{volume_name}': {reason}"
                                            ));
                                        });
                                    }
                                    EjectResult::NotFound => {}
                                }
                                cx.notify();
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
                            theme.text_muted,
                        )),
                )
            })
            .when(vol.is_mounted, |s| {
                s.on_mouse_down(
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

    /// Icons only, each named by its tooltip. Section titles become dividers, and drive
    /// actions wait for the full sidebar.
    fn render_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx);
        let now = Instant::now();
        let (current_loc, servers, reduced) = {
            let state = self.app_state.read(cx);
            (
                state.current_location.clone(),
                server_items(state),
                is_reduced_motion(state.settings.motion_preference),
            )
        };
        self.sync_server_draws(&servers, now);

        let places = self.places.clone();
        let bookmarks = self.bookmarks.clone();
        let volumes = self.volumes.clone();
        let network = self.network.clone();
        let specials = SpecialFolders::from_system();

        let place_rows: Vec<_> = places
            .iter()
            .enumerate()
            .map(|(ix, place)| {
                let key = SidebarKey::Place(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = place.path.clone();
                let is_active = matches!(&current_loc, Location::Local(p) if p == &place.path);
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
                let color = if place.icon.is_folder() {
                    theme.accent
                } else {
                    theme.text
                };
                let glyph_motion =
                    if !reduced && place.icon.is_folder() && (is_active || hover_t > 0.01) {
                        IconMotion::State(if is_active { 1.0 } else { hover_t })
                    } else {
                        IconMotion::Rest
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
                    .tooltip(Tooltip::text(place.name.clone()))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
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
                    .child(animated_icon_sized(
                        place.icon,
                        glyph_motion,
                        size::ICON * scale,
                        color,
                    ))
            })
            .collect();

        let bookmark_rows: Vec<_> = bookmarks
            .iter()
            .enumerate()
            .map(|(ix, bm)| {
                let key = SidebarKey::Bookmark(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = bm.path.clone();
                let is_active = matches!(&current_loc, Location::Local(p) if p == &bm.path);
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
                let (glyph_name, glyph_color, label) = if bm.is_missing() {
                    (
                        IconName::Alert,
                        theme.danger,
                        format!("{} (missing)", bm.name),
                    )
                } else {
                    (
                        icon_for_with(&bm.path, true, &specials),
                        theme.accent,
                        bm.name.clone(),
                    )
                };
                let glyph_motion =
                    if !reduced && glyph_name.is_folder() && (is_active || hover_t > 0.01) {
                        IconMotion::State(if is_active { 1.0 } else { hover_t })
                    } else {
                        IconMotion::Rest
                    };
                div()
                    .id(("bookmark", ix))
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
                    .child(animated_icon_sized(
                        glyph_name,
                        glyph_motion,
                        size::ICON * scale,
                        glyph_color,
                    ))
            })
            .collect();

        let drive_rows: Vec<_> = volumes
            .iter()
            .enumerate()
            .map(|(ix, vol)| {
                let key = SidebarKey::Drive(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = vol.mount_point.clone();
                let is_mounted = vol.is_mounted;
                let is_active = matches!(&current_loc, Location::Local(p) if p == &vol.mount_point);
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
                        s.on_mouse_down(
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

        let share_rows: Vec<_> = network
            .iter()
            .enumerate()
            .map(|(ix, share)| {
                let key = SidebarKey::NetworkShare(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let path = share.mount_point.clone();
                let is_active =
                    matches!(&current_loc, Location::Local(p) if p == &share.mount_point);
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
                    .child(animated_icon_sized(
                        IconName::FolderNetwork,
                        IconMotion::Rest,
                        size::ICON * scale,
                        theme.accent,
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
                let is_active = matches!(
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

        div()
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
            .when(!bookmarks.is_empty(), |s| s.child(divider()))
            .children(bookmark_rows)
            .when(!volumes.is_empty(), |s| s.child(divider()))
            .children(drive_rows)
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
        let bookmarks = self.bookmarks.clone();
        let volumes = self.volumes.clone();
        let network = self.network.clone();
        let specials = SpecialFolders::from_system();
        let drive_details = self.drive_details;
        let editing_bookmark = self.editing_bookmark;
        let (current_loc, servers, reduced) = {
            let state = self.app_state.read(cx);
            (
                state.current_location.clone(),
                server_items(state),
                is_reduced_motion(state.settings.motion_preference),
            )
        };
        self.sync_server_draws(&servers, now);
        let can_pin_current =
            matches!(&current_loc, Location::Local(p) if !bookmarks.iter().any(|b| &b.path == p));

        let place_rows: Vec<_> = places
            .into_iter()
            .enumerate()
            .map(|(ix, place)| {
                let key = SidebarKey::Place(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let is_active = matches!(&current_loc, Location::Local(p) if p == &place.path);
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
                let icon_motion =
                    if !reduced && place.icon.is_folder() && (is_active || hover_t > 0.01) {
                        IconMotion::State(if is_active { 1.0 } else { hover_t })
                    } else {
                        IconMotion::Rest
                    };
                let color = if place.icon.is_folder() {
                    theme.accent
                } else {
                    theme.text
                };
                let p_clone = place.path.clone();

                div()
                    .id(("place-row", ix))
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
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .child(animated_icon_sized(
                        place.icon,
                        icon_motion,
                        size::ICON * scale,
                        color,
                    ))
                    .child(div().ellipsize().child(place.name))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.navigate_to_path(&p_clone, cx);
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

        let (add_bm_hover, add_bm_press) =
            self.step_row_factors(SidebarKey::AddBookmark, now, reduced);
        let add_bm_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, add_bm_press)
        };

        let bookmark_rows: Vec<_> = bookmarks
            .iter()
            .enumerate()
            .map(|(idx, bm)| {
                let is_missing = bm.is_missing();
                let is_active = matches!(&current_loc, Location::Local(p) if p == &bm.path);
                let p_clone = bm.path.clone();

                if editing_bookmark == Some(idx) {
                    return div()
                        .id(("bookmark-edit", idx))
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
                                this.commit_rename_bookmark(cx);
                            } else if ev.keystroke.key == "escape" {
                                cx.stop_propagation();
                                this.cancel_rename_bookmark(cx);
                            }
                        }))
                        .child(self.rename_input.clone())
                        .into_any_element();
                }

                let key = SidebarKey::Bookmark(idx);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
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
                let folder_motion = if !reduced && !is_missing && (is_active || hover_t > 0.01) {
                    IconMotion::State(if is_active { 1.0 } else { hover_t })
                } else {
                    IconMotion::Rest
                };

                div()
                    .id(("bookmark-row", idx))
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
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(space::S8)
                            .child(if is_missing {
                                animated_icon_sized(
                                    IconName::Alert,
                                    IconMotion::Rest,
                                    size::ICON * scale,
                                    theme.danger,
                                )
                            } else {
                                animated_icon_sized(
                                    icon_for_with(&bm.path, true, &specials),
                                    folder_motion,
                                    size::ICON * scale,
                                    theme.accent,
                                )
                            })
                            .child(
                                div()
                                    .ellipsize()
                                    .text_color(if is_missing { theme.danger } else { theme.text })
                                    .child(bm.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S2)
                            .child(
                                div()
                                    .id(("bookmark-rename", idx))
                                    .px(space::S4)
                                    .rounded(radius::CONTROL)
                                    .hover(|s| s.bg(theme.surface_raised))
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .tooltip(Tooltip::text("Renomear favorito"))
                                    .child("Aa")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.begin_rename_bookmark(idx, window, cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("bookmark-remove", idx))
                                    .rounded(radius::CONTROL)
                                    .hover(|s| s.bg(theme.surface_raised))
                                    .tooltip(Tooltip::text("Remover favorito"))
                                    .child(icon(IconName::Close).text_color(theme.text_muted))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let _ = this.remove_bookmark_in_state(idx, cx);
                                        }),
                                    ),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_press(key, true, cx);
                            this.navigate_to_path(&p_clone, cx);
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
                let is_active = matches!(
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
                            .child(div().ellipsize().child(server.name.clone())),
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
                        .drive_row(ix, vol, &current_loc, now, reduced, cx)
                        .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
                        .into_any_element();
                }
                let vol_id = vol.id.clone();
                let is_mounted = vol.is_mounted;
                let is_busy = vol.is_busy;
                let capacity_text = vol.capacity_summary();
                let used_pct = vol.used_percentage();
                let p_nav = vol.mount_point.clone();
                let is_active = current_loc.path() == vol.mount_point;

                div()
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::toggle_drive_details))
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
                            .when(!is_mounted, |p| {
                                let v_id = vol_id.clone();
                                p.child(
                                    div()
                                        .px_2()
                                        .py(px(1.))
                                        .rounded_sm()
                                        .bg(theme.accent)
                                        .text_color(theme.on_accent)
                                        .text_size(px(11.))
                                        .cursor_pointer()
                                        .child("Mount")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                if let MountResult::Mounted {
                                                    mount_point,
                                                    volume_name,
                                                } = this.mount_volume(&v_id)
                                                {
                                                    this.navigate_to_path(&mount_point, cx);
                                                    this.app_state.update(cx, |state, _cx| {
                                                        state.set_status(format!(
                                                            "Mounted '{}'",
                                                            volume_name
                                                        ));
                                                    });
                                                    cx.notify();
                                                }
                                            }),
                                        ),
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
                                .child("In use by open files"),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        let network_rows: Vec<_> = network
            .iter()
            .enumerate()
            .map(|(ix, share)| {
                let key = SidebarKey::NetworkShare(ix);
                let (hover_t, press_t) = self.step_row_factors(key, now, reduced);
                let is_active =
                    matches!(&current_loc, Location::Local(p) if p == &share.mount_point);
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
                    .child(animated_icon_sized(
                        IconName::FolderNetwork,
                        IconMotion::Rest,
                        size::ICON * scale,
                        theme.accent,
                    ))
                    .child(div().ellipsize().child(share.name.clone()))
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
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px(space::S8)
                            .py(space::S4)
                            .text_label()
                            .text_color(theme.text_muted)
                            .child("Places"),
                    )
                    .children(place_rows),
            )
            // --- BOOKMARKS SECTION ---
            .when(!bookmarks.is_empty() || can_pin_current, |parent| {
                parent.child(
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
                                        .child("Bookmarks"),
                                )
                                .when(can_pin_current, |hdr| {
                                    hdr.child(
                                        div()
                                            .id("sidebar-add-bookmark")
                                            .p(space::S2)
                                            .rounded(radius::CONTROL)
                                            .cursor_pointer()
                                            .bg(color_lerp(
                                                transparent_black(),
                                                theme.border,
                                                add_bm_hover,
                                            ))
                                            .tooltip(Tooltip::text(
                                                "Adicionar pasta atual aos favoritos",
                                            ))
                                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                                this.set_hover(
                                                    SidebarKey::AddBookmark,
                                                    *hovered,
                                                    cx,
                                                );
                                            }))
                                            .child(animated_icon_sized(
                                                IconName::Plus,
                                                IconMotion::Rest,
                                                size::ICON * add_bm_scale,
                                                theme.text_muted,
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.set_press(
                                                        SidebarKey::AddBookmark,
                                                        true,
                                                        cx,
                                                    );
                                                    let _ = this.pin_current_location(cx);
                                                }),
                                            )
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_press(
                                                        SidebarKey::AddBookmark,
                                                        false,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .on_mouse_up_out(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.set_press(
                                                        SidebarKey::AddBookmark,
                                                        false,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                }),
                        )
                        .children(bookmark_rows),
                )
            })
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
            .when(!volumes.is_empty(), |parent| {
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
                                .text_label()
                                .text_color(theme.text_muted)
                                .child("Drives")
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(Self::toggle_drive_details),
                                ),
                        )
                        .children(drive_elements),
                )
            })
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
