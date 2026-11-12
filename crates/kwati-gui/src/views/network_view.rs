//! Network mount indicator and banner (owned by guinet-worker for #34).
//!
//! Shows network status, timeouts, cancellation, and retry actions when browsing network shares.

use gpui::{div, prelude::*, px, Context, Entity, MouseButton, MouseDownEvent, Window};
use kwati_core::{volume_kind, Location, VolumeKind};

use crate::state::{AppState, NetworkMountStatus};
use crate::theme::Theme;

pub const DEFAULT_NETWORK_TIMEOUT_SECS: u32 = 10;

/// Checks if a location lives on a network mount or remote share.
pub fn is_network_location(location: &Location) -> bool {
    match location {
        Location::Remote { .. } => true,
        Location::Local(path) => {
            let path_str = path.to_string_lossy();
            if path_str.starts_with("\\\\") || path_str.starts_with("//") {
                return true;
            }
            volume_kind(path) == VolumeKind::Network
        }
    }
}

pub struct NetworkView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
    pub timeout_secs: u32,
}

impl NetworkView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
            timeout_secs: DEFAULT_NETWORK_TIMEOUT_SECS,
        }
    }

    /// Updates the configured network timeout in seconds (without requiring a restart).
    pub fn set_timeout_secs(&mut self, timeout: u32) {
        self.timeout_secs = timeout.max(1);
    }

    /// Applies timeout from settings.
    pub fn apply_settings_timeout(&mut self, timeout: u32) {
        self.set_timeout_secs(timeout);
    }

    /// Triggers a retry of the network mount connection or read.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::Connecting;
            state.set_status("Retrying network connection...");
        });
        cx.notify();
    }

    /// Cancels an in-progress or timed-out network operation.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::Disconnected;
            state.set_status("Network operation cancelled");
        });
        cx.notify();
    }

    /// Marks the network status as TimedOut.
    pub fn handle_read_timeout(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::TimedOut;
        });
        cx.notify();
    }

    /// Marks the network status as MountBusy.
    pub fn handle_mount_busy(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::MountBusy;
        });
        cx.notify();
    }

    /// Marks the network status as Connected.
    pub fn handle_connected(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::Connected;
        });
        cx.notify();
    }

    fn on_retry(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.retry(cx);
    }

    fn on_cancel(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
    }
}

impl Render for NetworkView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, current_loc) = {
            let state = self.app_state.read(cx);
            (state.network_status, state.current_location.clone())
        };

        let is_network = is_network_location(&current_loc);

        match status {
            NetworkMountStatus::Disconnected => {
                if is_network {
                    div()
                        .w_full()
                        .px_3()
                        .py_1()
                        .bg(self.theme.surface_raised)
                        .text_color(self.theme.text)
                        .text_size(px(12.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("🌐 Network Share")
                        .child(
                            div()
                                .text_color(self.theme.text_muted)
                                .child(current_loc.path().to_string_lossy().to_string()),
                        )
                } else {
                    div()
                }
            }
            NetworkMountStatus::Connecting => div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .w_full()
                .px_3()
                .py_1()
                .bg(self.theme.surface_raised)
                .text_color(self.theme.text)
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("🌐")
                        .child("Connecting to network share..."),
                )
                .child(
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded_sm()
                        .bg(self.theme.surface_raised)
                        .text_color(self.theme.text_muted)
                        .cursor_pointer()
                        .child("Cancel")
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::on_cancel)),
                ),
            NetworkMountStatus::Connected => {
                if is_network {
                    div()
                        .w_full()
                        .px_3()
                        .py_1()
                        .bg(self.theme.surface_raised)
                        .text_color(self.theme.secondary)
                        .text_size(px(12.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("🌐 Connected:")
                        .child(
                            div()
                                .text_color(self.theme.text)
                                .child(current_loc.path().to_string_lossy().to_string()),
                        )
                } else {
                    div()
                }
            }
            NetworkMountStatus::TimedOut => div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .w_full()
                .px_3()
                .py_1()
                .bg(self.theme.surface_raised)
                .text_color(self.theme.danger)
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("⚠️")
                        .child(format!(
                            "Network connection timed out ({}s). Slow or unresponsive mount.",
                            self.timeout_secs
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py(px(2.))
                                .rounded_sm()
                                .bg(self.theme.accent)
                                .text_color(self.theme.on_accent)
                                .cursor_pointer()
                                .child("Retry")
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_retry)),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(2.))
                                .rounded_sm()
                                .bg(self.theme.surface_raised)
                                .text_color(self.theme.text_muted)
                                .cursor_pointer()
                                .child("Cancel")
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_cancel)),
                        ),
                ),
            NetworkMountStatus::MountBusy => div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .w_full()
                .px_3()
                .py_1()
                .bg(self.theme.surface_raised)
                .text_color(self.theme.warning)
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child("⏳")
                        .child("Network mount is busy. Waiting for server response..."),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py(px(2.))
                                .rounded_sm()
                                .bg(self.theme.accent)
                                .text_color(self.theme.on_accent)
                                .cursor_pointer()
                                .child("Retry")
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_retry)),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(2.))
                                .rounded_sm()
                                .bg(self.theme.surface_raised)
                                .text_color(self.theme.text_muted)
                                .cursor_pointer()
                                .child("Cancel")
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_cancel)),
                        ),
                ),
        }
    }
}
