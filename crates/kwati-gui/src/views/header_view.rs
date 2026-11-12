//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, rgba, Context, CursorStyle, Entity, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Window,
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

    /// Sets the text inside the address input.
    pub fn set_address_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.address_input.update(cx, |input, cx| {
            input.set_text(text, cx);
        });
    }

    /// Synchronizes the address input text with the state's current location path.
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

    /// Validates the entered path and navigates to it if it exists and is a directory.
    /// If missing, displays an error in the status bar and keeps current listing intact.
    pub fn commit_address(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.address_input.read(cx).text().trim().to_string();
        if typed.is_empty() {
            return;
        }

        let target_path = PathBuf::from(&typed);
        if target_path.exists() && target_path.is_dir() {
            self.app_state.update(cx, |state, _cx| {
                state.navigate_to(target_path.clone().into());
                state.clear_status();
            });
            self.sync_address_input(cx);
        } else {
            let err_msg = format!("Directory not found: {}", typed);
            self.app_state.update(cx, |state, _cx| {
                state.set_status(err_msg);
            });
        }
        cx.notify();
    }

    /// Reverts the address input back to the current directory path and clears status error.
    pub fn revert_address(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.sync_address_input(cx);
        self.app_state.update(cx, |state, _cx| {
            state.clear_status();
        });
        cx.notify();
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
            // Address bar with Enter and Escape key listeners
            .child(
                div()
                    .flex_1()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "enter" {
                            this.commit_address(window, cx);
                        } else if event.keystroke.key == "escape" {
                            this.revert_address(window, cx);
                        }
                    }))
                    .child(self.address_input.clone()),
            )
    }
}
