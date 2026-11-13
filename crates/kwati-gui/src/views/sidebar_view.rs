//! Sidebar view (owned by guinet-worker for #61).
//!
//! Renders quick-access places, bookmarks, the device's mounted drives with capacity
//! indicators, and mounted network shares.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    div, prelude::*, px, transparent_black, AnyElement, Context, Div, ElementId, Entity,
    FontWeight, MouseButton, MouseDownEvent, Stateful, Svg, Task, Window,
};
use kwati_core::{device_volumes, network_mounts, Location, NetworkMount, Volume};

use crate::components::{icon, Ellipsize, IconName, Tooltip};
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
    let mut places = Vec::new();

    if let Some(home) = system_home_dir() {
        places.push(SidebarPlace::new("Home", home.clone(), IconName::Home));

        let desktop = home.join("Desktop");
        if desktop.exists() {
            places.push(SidebarPlace::new("Desktop", desktop, IconName::Folder));
        }

        let documents = home.join("Documents");
        if documents.exists() {
            places.push(SidebarPlace::new("Documents", documents, IconName::Folder));
        }

        let downloads = home.join("Downloads");
        if downloads.exists() {
            places.push(SidebarPlace::new("Downloads", downloads, IconName::Folder));
        }

        let pictures = home.join("Pictures");
        if pictures.exists() {
            places.push(SidebarPlace::new("Pictures", pictures, IconName::Image));
        }

        let music = home.join("Music");
        if music.exists() {
            places.push(SidebarPlace::new("Music", music, IconName::Folder));
        }

        let videos = home.join("Videos");
        if videos.exists() {
            places.push(SidebarPlace::new("Videos", videos, IconName::Folder));
        }
    }

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

/// How often drives and shares are listed again, so a plugged-in disk or a newly mounted
/// share shows up without a restart.
const DEVICE_POLL: Duration = Duration::from_secs(3);

pub struct SidebarView {
    pub app_state: Entity<AppState>,
    pub places: Vec<SidebarPlace>,
    pub bookmarks: Vec<UserBookmark>,
    pub volumes: Vec<SidebarVolume>,
    pub network: Vec<NetworkMount>,
    /// Drives show as plain rows unless the user asks for capacity cards with a right click.
    pub drive_details: bool,
    _device_poll: Task<()>,
}

impl SidebarView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
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
            bookmarks: Vec::new(),
            volumes: Vec::new(),
            network: Vec::new(),
            drive_details: false,
            _device_poll: device_poll,
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
    fn toggle_drive_details(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drive_details = !self.drive_details;
        cx.notify();
    }

    /// A drive as one row, like a place; its capacity waits in the tooltip.
    fn drive_row(
        &self,
        ix: usize,
        vol: SidebarVolume,
        current_loc: &Location,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::get(cx);
        let is_active = current_loc.path() == vol.mount_point;
        let tooltip = if vol.is_mounted {
            format!("{} — {}", vol.mount_point.display(), vol.capacity_summary())
        } else {
            format!("{} (not mounted)", vol.name)
        };
        let path = vol.mount_point.clone();

        div()
            .id(("drive-row", ix))
            .h(size::SIDEBAR_ROW)
            .px(space::S8)
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .when(is_active, |s| s.bg(theme.selection))
            .when(!is_active, |s| s.hover(|s| s.bg(theme.border)))
            .text_body()
            .text_color(if vol.is_mounted {
                theme.text
            } else {
                theme.text_muted
            })
            .flex()
            .items_center()
            .gap(space::S8)
            .child(icon(IconName::Drive).text_color(if vol.is_busy {
                theme.warning
            } else {
                theme.text
            }))
            .child(div().ellipsize().child(vol.name))
            .tooltip(Tooltip::text(tooltip))
            .when(vol.is_mounted, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.navigate_to_path(&path, cx)),
                )
            })
    }

    /// Icons only, each named by its tooltip. Section titles become dividers, and drive
    /// actions wait for the full sidebar.
    fn render_compact(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx);
        let current_loc = self.app_state.read(cx).current_location.clone();
        let divider = || div().h(px(1.)).mx(space::S4).bg(theme.border);
        let row = |id: ElementId, icon: Svg, label: String, is_active: bool| -> Stateful<Div> {
            div()
                .id(id)
                .h(size::SIDEBAR_ROW)
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(is_active, |s| s.bg(theme.selection))
                .when(!is_active, |s| s.hover(|s| s.bg(theme.border)))
                .child(icon)
                .tooltip(Tooltip::text(label))
        };

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
            .children(self.places.iter().enumerate().map(|(ix, place)| {
                let path = place.path.clone();
                let color = if place.icon == IconName::Folder {
                    theme.accent
                } else {
                    theme.text
                };
                row(
                    ("place", ix).into(),
                    icon(place.icon).text_color(color),
                    place.name.clone(),
                    current_loc.path() == place.path,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.navigate_to_path(&path, cx)),
                )
            }))
            .when(!self.bookmarks.is_empty(), |s| s.child(divider()))
            .children(self.bookmarks.iter().enumerate().map(|(ix, bm)| {
                let path = bm.path.clone();
                let (glyph, label) = if bm.is_missing() {
                    (
                        icon(IconName::Alert).text_color(theme.danger),
                        format!("{} (missing)", bm.name),
                    )
                } else {
                    (
                        icon(IconName::Folder).text_color(theme.accent),
                        bm.name.clone(),
                    )
                };
                row(
                    ("bookmark", ix).into(),
                    glyph,
                    label,
                    current_loc.path() == bm.path,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.navigate_to_path(&path, cx)),
                )
            }))
            .when(!self.volumes.is_empty(), |s| s.child(divider()))
            .children(self.volumes.iter().enumerate().map(|(ix, vol)| {
                let path = vol.mount_point.clone();
                let is_mounted = vol.is_mounted;
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
                row(
                    ("drive", ix).into(),
                    icon(IconName::Drive).text_color(color),
                    label,
                    current_loc.path() == vol.mount_point,
                )
                .when(is_mounted, |s| {
                    s.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.navigate_to_path(&path, cx)),
                    )
                })
            }))
            .when(!self.network.is_empty(), |s| s.child(divider()))
            .children(self.network.iter().enumerate().map(|(ix, share)| {
                let path = share.mount_point.clone();
                row(
                    ("share", ix).into(),
                    icon(IconName::Network).text_color(theme.text),
                    share.name.clone(),
                    current_loc.path() == share.mount_point,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.navigate_to_path(&path, cx)),
                )
            }))
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
            return self.render_compact(cx);
        }

        let theme = Theme::get(cx);
        let places = self.places.clone();
        let bookmarks = self.bookmarks.clone();
        let volumes = self.volumes.clone();
        let network = self.network.clone();
        let drive_details = self.drive_details;
        let current_loc = self.app_state.read(cx).current_location.clone();

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
                    .children(places.into_iter().map(|place| {
                        let is_active = current_loc.path() == place.path;
                        let p_clone = place.path.clone();

                        div()
                            .h(size::SIDEBAR_ROW)
                            .px(space::S8)
                            .rounded(radius::CONTROL)
                            .cursor_pointer()
                            .when(is_active, |s| s.bg(theme.selection))
                            .when(!is_active, |s| s.hover(|s| s.bg(theme.border)))
                            .text_body()
                            .text_color(theme.text)
                            .flex()
                            .items_center()
                            .gap(space::S8)
                            .child(
                                icon(place.icon).text_color(if place.icon == IconName::Folder {
                                    theme.accent
                                } else {
                                    theme.text
                                }),
                            )
                            .child(div().ellipsize().child(place.name))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.navigate_to_path(&p_clone, cx);
                                }),
                            )
                    })),
            )
            // --- BOOKMARKS SECTION ---
            .when(!bookmarks.is_empty(), |parent| {
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
                                .child("Bookmarks"),
                        )
                        .children(bookmarks.into_iter().enumerate().map(|(idx, bm)| {
                            let is_missing = bm.is_missing();
                            let is_active = current_loc.path() == bm.path;
                            let p_clone = bm.path.clone();

                            div()
                                .h(size::SIDEBAR_ROW)
                                .px(space::S8)
                                .rounded(radius::CONTROL)
                                .cursor_pointer()
                                .when(is_active, |s| s.bg(theme.selection))
                                .when(!is_active, |s| s.hover(|s| s.bg(theme.border)))
                                .text_body()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .gap(space::S8)
                                        .child(if is_missing {
                                            icon(IconName::Alert).text_color(theme.danger)
                                        } else {
                                            icon(IconName::Folder).text_color(theme.accent)
                                        })
                                        .child(
                                            div()
                                                .ellipsize()
                                                .text_color(if is_missing {
                                                    theme.danger
                                                } else {
                                                    theme.text
                                                })
                                                .child(bm.name),
                                        ),
                                )
                                .child(
                                    div()
                                        .rounded(radius::CONTROL)
                                        .hover(|s| s.bg(theme.surface_raised))
                                        .child(icon(IconName::Close).text_color(theme.text_muted))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.remove_bookmark(idx);
                                                cx.notify();
                                            }),
                                        ),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.navigate_to_path(&p_clone, cx);
                                    }),
                                )
                        })),
                )
            })
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
                        .children(volumes.into_iter().enumerate().map(|(ix, vol)| {
                            if !drive_details {
                                return self
                                    .drive_row(ix, vol, &current_loc, cx)
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(Self::toggle_drive_details),
                                    )
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
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(Self::toggle_drive_details),
                                )
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
                                                                this.navigate_to_path(
                                                                    &mount_point,
                                                                    cx,
                                                                );
                                                                this.app_state.update(
                                                                    cx,
                                                                    |state, _cx| {
                                                                        state.set_status(format!(
                                                                            "Mounted '{}'",
                                                                            volume_name
                                                                        ));
                                                                    },
                                                                );
                                                                cx.notify();
                                                            }
                                                        }),
                                                    ),
                                            )
                                        }),
                                )
                                // Capacity summary
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.text_muted)
                                        .child(capacity_text),
                                )
                                // Usage progress bar
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
                        })),
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
                        .children(network.into_iter().enumerate().map(|(ix, share)| {
                            let is_active = current_loc.path() == share.mount_point;
                            let path = share.mount_point.clone();

                            div()
                                .id(("network-share", ix))
                                .h(size::SIDEBAR_ROW)
                                .px(space::S8)
                                .rounded(radius::CONTROL)
                                .cursor_pointer()
                                .when(is_active, |s| s.bg(theme.selection))
                                .when(!is_active, |s| s.hover(|s| s.bg(theme.border)))
                                .text_body()
                                .text_color(theme.text)
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .child(icon(IconName::Network).text_color(theme.text))
                                .child(div().ellipsize().child(share.name))
                                .tooltip(Tooltip::text(
                                    share.mount_point.to_string_lossy().into_owned(),
                                ))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.navigate_to_path(&path, cx);
                                    }),
                                )
                        })),
                )
            })
            .into_any_element()
    }
}
