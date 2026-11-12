//! Network mount indicator and banner (owned by guinet-worker for #34).
//!
//! Shows network status, timeouts, and a retry button when browsing network shares.

use gpui::{div, prelude::*, px, rgb, rgba, Context, Entity, MouseButton, MouseDownEvent, Window};

use crate::state::{AppState, NetworkMountStatus};
use crate::theme::Theme;

pub struct NetworkView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
}

impl NetworkView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
        }
    }

    fn on_retry(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.network_status = NetworkMountStatus::Connecting;
        });
    }
}

impl Render for NetworkView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.app_state.read(cx).network_status;

        match status {
            NetworkMountStatus::Disconnected => div(),
            NetworkMountStatus::Connecting => div()
                .w_full()
                .px_3()
                .py_1()
                .bg(rgba(0x89b4fa20))
                .text_color(self.theme.accent)
                .text_size(px(12.))
                .child("Connecting to remote share..."),
            NetworkMountStatus::Connected => div(),
            NetworkMountStatus::TimedOut => div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .w_full()
                .px_3()
                .py_1()
                .bg(rgba(0xf38ba830))
                .text_color(self.theme.text_error)
                .text_size(px(12.))
                .child("Network connection timed out.")
                .child(
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded_sm()
                        .bg(self.theme.accent)
                        .text_color(rgb(0x181825))
                        .cursor_pointer()
                        .child("Retry")
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::on_retry)),
                ),
            NetworkMountStatus::MountBusy => div()
                .w_full()
                .px_3()
                .py_1()
                .bg(rgba(0xf9e2af30))
                .text_color(self.theme.text_warning)
                .text_size(px(12.))
                .child("Mount is busy..."),
        }
    }
}
