//! Sidebar view (owned by guinet-worker for #61).
//!
//! Renders quick-access places, bookmarks, and mounted/removable storage drives with
//! capacity indicators, eject button, busy detection, and safe-to-remove feedback.

use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, rgb, rgba, Context, Entity, FontWeight, MouseButton, Window};
use kwati_core::Location;

use crate::state::AppState;
use crate::theme::Theme;

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
    pub icon: &'static str,
}

impl SidebarPlace {
    pub fn new(name: impl Into<String>, path: PathBuf, icon: &'static str) -> Self {
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
        places.push(SidebarPlace::new("Home", home.clone(), "🏠"));

        let desktop = home.join("Desktop");
        if desktop.exists() {
            places.push(SidebarPlace::new("Desktop", desktop, "🖥️"));
        }

        let documents = home.join("Documents");
        if documents.exists() {
            places.push(SidebarPlace::new("Documents", documents, "📁"));
        }

        let downloads = home.join("Downloads");
        if downloads.exists() {
            places.push(SidebarPlace::new("Downloads", downloads, "📥"));
        }

        let pictures = home.join("Pictures");
        if pictures.exists() {
            places.push(SidebarPlace::new("Pictures", pictures, "🖼️"));
        }

        let music = home.join("Music");
        if music.exists() {
            places.push(SidebarPlace::new("Music", music, "🎵"));
        }

        let videos = home.join("Videos");
        if videos.exists() {
            places.push(SidebarPlace::new("Videos", videos, "🎬"));
        }
    }

    if places.is_empty() {
        places.push(SidebarPlace::new("Root", PathBuf::from("/"), "💾"));
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

pub struct SidebarView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
    pub places: Vec<SidebarPlace>,
    pub bookmarks: Vec<UserBookmark>,
    pub volumes: Vec<SidebarVolume>,
}

impl SidebarView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
            places: standard_places(),
            bookmarks: Vec::new(),
            volumes: Vec::new(),
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

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let places = self.places.clone();
        let bookmarks = self.bookmarks.clone();
        let volumes = self.volumes.clone();
        let current_loc = self.app_state.read(cx).current_location.clone();

        div()
            .w(px(220.))
            .h_full()
            .p_3()
            .bg(self.theme.bg_sidebar)
            .border_r_1()
            .border_color(self.theme.border)
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
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(self.theme.text_muted)
                            .child("PLACES"),
                    )
                    .children(places.into_iter().map(|place| {
                        let is_active = current_loc.path() == place.path;
                        let p_clone = place.path.clone();

                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if is_active {
                                rgba(0x89b4fa20)
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| s.bg(rgba(0xffffff0d)))
                            .text_size(px(13.))
                            .text_color(if is_active {
                                self.theme.accent
                            } else {
                                self.theme.text_primary
                            })
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(place.icon)
                            .child(place.name)
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
                                .text_size(px(11.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(self.theme.text_muted)
                                .child("BOOKMARKS"),
                        )
                        .children(bookmarks.into_iter().enumerate().map(|(idx, bm)| {
                            let is_missing = bm.is_missing();
                            let is_active = current_loc.path() == bm.path;
                            let p_clone = bm.path.clone();

                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if is_active {
                                    rgba(0x89b4fa20)
                                } else {
                                    rgba(0x00000000)
                                })
                                .hover(|s| s.bg(rgba(0xffffff0d)))
                                .text_size(px(13.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(if is_missing { "⚠️" } else { "🔖" })
                                        .child(
                                            div()
                                                .text_color(if is_missing {
                                                    self.theme.text_error
                                                } else if is_active {
                                                    self.theme.accent
                                                } else {
                                                    self.theme.text_primary
                                                })
                                                .child(bm.name),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_1()
                                        .text_size(px(11.))
                                        .text_color(self.theme.text_muted)
                                        .hover(|s| s.text_color(self.theme.text_error))
                                        .child("✕")
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
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(self.theme.text_muted)
                                .child("DRIVES & STORAGE"),
                        )
                        .children(volumes.into_iter().map(|vol| {
                            let vol_id = vol.id.clone();
                            let is_removable = vol.is_removable;
                            let is_mounted = vol.is_mounted;
                            let is_busy = vol.is_busy;
                            let capacity_text = vol.capacity_summary();
                            let used_pct = vol.used_percentage();
                            let p_nav = vol.mount_point.clone();
                            let p_eject = vol.mount_point.clone();
                            let is_active = current_loc.path() == vol.mount_point;

                            div()
                                .p_2()
                                .rounded_md()
                                .bg(if is_active {
                                    rgba(0x89b4fa15)
                                } else {
                                    rgba(0xffffff05)
                                })
                                .border_1()
                                .border_color(if is_active {
                                    self.theme.accent
                                } else {
                                    self.theme.border
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
                                                .child(if is_removable { "🔌" } else { "💾" })
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(self.theme.text_primary)
                                                        .child(vol.name),
                                                ),
                                        )
                                        .when(is_removable && is_mounted, |p| {
                                            let v_id = vol_id.clone();
                                            p.child(
                                                div()
                                                    .px_2()
                                                    .py(px(1.))
                                                    .rounded_sm()
                                                    .bg(self.theme.bg_header)
                                                    .hover(|s| s.bg(rgba(0xf38ba830)))
                                                    .text_size(px(11.))
                                                    .text_color(self.theme.text_secondary)
                                                    .cursor_pointer()
                                                    .child("⏏ Eject")
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            let loc = this
                                                                .app_state
                                                                .read(cx)
                                                                .current_location
                                                                .clone();
                                                            match this.eject_volume(&v_id, &loc) {
                                                                EjectResult::SafeToRemove {
                                                                    volume_name,
                                                                    ..
                                                                } => {
                                                                    this.app_state.update(
                                                                        cx,
                                                                        |state, _cx| {
                                                                            if state
                                                                                .current_location
                                                                                .path()
                                                                                .starts_with(
                                                                                    &p_eject,
                                                                                )
                                                                            {
                                                                                let home = system_home_dir().unwrap_or_else(|| PathBuf::from("/"));
                                                                                state.navigate_to(Location::Local(home));
                                                                            }
                                                                            state.set_status(format!("'{}' is safe to remove", volume_name));
                                                                        },
                                                                    );
                                                                    cx.notify();
                                                                }
                                                                EjectResult::Busy {
                                                                    volume_name,
                                                                    reason,
                                                                } => {
                                                                    this.app_state.update(
                                                                        cx,
                                                                        |state, _cx| {
                                                                            state.set_status(format!("Cannot eject '{}': {}", volume_name, reason));
                                                                        },
                                                                    );
                                                                    cx.notify();
                                                                }
                                                                EjectResult::NotFound => {}
                                                            }
                                                        }),
                                                    ),
                                            )
                                        })
                                        .when(!is_mounted, |p| {
                                            let v_id = vol_id.clone();
                                            p.child(
                                                div()
                                                    .px_2()
                                                    .py(px(1.))
                                                    .rounded_sm()
                                                    .bg(self.theme.accent)
                                                    .text_color(rgb(0x181825))
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
                                        .text_color(self.theme.text_muted)
                                        .child(capacity_text),
                                )
                                // Usage progress bar
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(4.))
                                        .rounded_sm()
                                        .bg(self.theme.border)
                                        .child(
                                            div()
                                                .w(gpui::DefiniteLength::Fraction(
                                                    (used_pct as f32 / 100.0).min(1.0),
                                                ))
                                                .h_full()
                                                .rounded_sm()
                                                .bg(if used_pct > 90 {
                                                    self.theme.text_error
                                                } else {
                                                    self.theme.accent
                                                }),
                                        ),
                                )
                                .when(is_busy, |p| {
                                    p.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(self.theme.text_warning)
                                            .child("⚠️ In use by open files"),
                                    )
                                })
                        })),
                )
            })
    }
}
