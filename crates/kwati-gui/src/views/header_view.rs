//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use gpui::{div, prelude::*, px, rgba, Context, Entity, MouseButton, MouseDownEvent, Window};

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
        let address_input =
            cx.new(|cx| TextInput::with_placeholder("Type path and press Enter...", cx));
        Self {
            app_state,
            address_input,
            theme: Theme::dark(),
        }
    }

    fn on_up_click(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            if let Some(parent) = state.current_location.path().parent() {
                state.navigate_to(parent.into());
            }
        });
    }

    fn on_back_click(&mut self, _: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            if let Some(prev) = state.history.back().cloned() {
                state.current_location = prev;
                state.selection.clear();
            }
        });
    }

    fn on_forward_click(
        &mut self,
        _: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app_state.update(cx, |state, _cx| {
            if let Some(next) = state.history.forward().cloned() {
                state.current_location = next;
                state.selection.clear();
            }
        });
    }
}

impl Render for HeaderView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let _path_str = state.current_location.path().to_string_lossy().to_string();
        let can_go_back = state.history.can_go_back();
        let can_go_forward = state.history.can_go_forward();
        let can_go_up = state.current_location.path().parent().is_some();

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
                    .cursor_pointer()
                    .text_color(if can_go_back {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .hover(|s| s.bg(rgba(0xffffff14)))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_back_click))
                    .child("◀"),
            )
            // Forward button
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(if can_go_forward {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .hover(|s| s.bg(rgba(0xffffff14)))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_forward_click))
                    .child("▶"),
            )
            // Up button
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(if can_go_up {
                        self.theme.text_primary
                    } else {
                        self.theme.text_muted
                    })
                    .bg(rgba(0x00000000))
                    .hover(|s| s.bg(rgba(0xffffff14)))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_up_click))
                    .child("▲ Up"),
            )
            // Address bar
            .child(div().flex_1().child(self.address_input.clone()))
    }
}
