//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use gpui::{
    div, prelude::*, px, rgba, Context, CursorStyle, Entity, MouseButton, MouseDownEvent, Window,
};

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

pub struct HeaderView {
    pub app_state: Entity<AppState>,
    pub address_input: Entity<TextInput>,
    pub theme: Theme,
}

impl HeaderView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let initial_path = app_state
            .read(cx)
            .current_location
            .path()
            .to_string_lossy()
            .to_string();
        let address_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("Type path and press Enter...", cx);
            input.set_text(initial_path, cx);
            input
        });
        Self {
            app_state,
            address_input,
            theme: Theme::dark(),
        }
    }

    pub fn sync_address_input(&mut self, cx: &mut Context<Self>) {
        let path_str = self
            .app_state
            .read(cx)
            .current_location
            .path()
            .to_string_lossy()
            .to_string();
        self.address_input.update(cx, |input, cx| {
            input.set_text(path_str, cx);
        });
    }

    fn on_up_click(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.app_state.update(cx, |state, _cx| state.go_up());
        if moved {
            self.sync_address_input(cx);
            cx.notify();
        }
    }

    fn on_back_click(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.app_state.update(cx, |state, _cx| state.go_back());
        if moved {
            self.sync_address_input(cx);
            cx.notify();
        }
    }

    fn on_forward_click(
        &mut self,
        _: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moved = self.app_state.update(cx, |state, _cx| state.go_forward());
        if moved {
            self.sync_address_input(cx);
            cx.notify();
        }
    }
}

impl Render for HeaderView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let can_go_back = state.history.can_go_back();
        let can_go_forward = state.history.can_go_forward();
        let can_go_up = state
            .current_location
            .path()
            .parent()
            .map(|p| !p.as_os_str().is_empty())
            .unwrap_or(false);

        div()
            .flex()
            .flex_row()
            .h(px(40.))
            .w_full()
            .px_3()
            .py_1()
            .items_center()
            .gap_2()
            .bg(self.theme.bg_header)
            .border_b_1()
            .border_color(self.theme.border)
            // Back button
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor(if can_go_back {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .text_color(if can_go_back {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .when(can_go_back, |s| s.hover(|h| h.bg(rgba(0xffffff14))))
                    .when(can_go_back, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_back_click))
                    })
                    .child("◀"),
            )
            // Forward button
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor(if can_go_forward {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .text_color(if can_go_forward {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .when(can_go_forward, |s| s.hover(|h| h.bg(rgba(0xffffff14))))
                    .when(can_go_forward, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_forward_click))
                    })
                    .child("▶"),
            )
            // Up button
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor(if can_go_up {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .text_color(if can_go_up {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .when(can_go_up, |s| s.hover(|h| h.bg(rgba(0xffffff14))))
                    .when(can_go_up, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_up_click))
                    })
                    .child("▲ Up"),
            )
            // Address bar
            .child(div().flex_1().child(self.address_input.clone()))
    }
}
