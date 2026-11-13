//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, Context, CursorStyle, Entity, KeyDownEvent, MouseButton, MouseDownEvent,
    SharedString, Window,
};

use kwati_core::Location;

use crate::actions::navigation::TogglePreview;
use crate::components::TextInput;
use crate::components::{icon, IconName, Tooltip};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

pub struct HeaderView {
    pub app_state: Entity<AppState>,
    pub address_input: Entity<TextInput>,
    // The address field is rewritten only when this changes, so a refresh of the same
    // directory does not erase a path being typed.
    shown_location: Location,
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
        let shown_location = app_state.read(cx).current_location.clone();
        cx.observe(&app_state, |this, app_state, cx| {
            let location = app_state.read(cx).current_location.clone();
            if this.shown_location != location {
                this.shown_location = location;
                this.sync_address_input(cx);
            }
        })
        .detach();
        Self {
            app_state,
            address_input,
            shown_location,
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
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let state = self.app_state.read(cx);
        let can_go_back = state.history.can_go_back();
        let can_go_forward = state.history.can_go_forward();
        let preview_open = state.panels.preview_open;
        let can_go_up = state
            .current_location
            .path()
            .parent()
            .map(|p| !p.as_os_str().is_empty())
            .unwrap_or(false);

        div()
            .flex()
            .flex_row()
            .h(size::TOOLBAR)
            .w_full()
            .pl(space::S12)
            .pr(space::S8)
            .items_center()
            .gap(space::S12)
            .bg(theme.bg)
            .border_b_1()
            .border_color(theme.border)
            // Back button
            .child(
                div()
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(if can_go_back {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(can_go_back, |s| s.hover(|h| h.bg(theme.surface_raised)))
                    .when(can_go_back, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_back_click))
                    })
                    .child(icon(IconName::ArrowLeft).text_color(if can_go_back {
                        theme.text
                    } else {
                        theme.border_strong
                    })),
            )
            // Forward button
            .child(
                div()
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(if can_go_forward {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(can_go_forward, |s| s.hover(|h| h.bg(theme.surface_raised)))
                    .when(can_go_forward, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_forward_click))
                    })
                    .child(icon(IconName::ArrowRight).text_color(if can_go_forward {
                        theme.text
                    } else {
                        theme.border_strong
                    })),
            )
            // Up button
            .child(
                div()
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(if can_go_up {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(can_go_up, |s| s.hover(|h| h.bg(theme.surface_raised)))
                    .when(can_go_up, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(Self::on_up_click))
                    })
                    .child(icon(IconName::ArrowUp).text_color(if can_go_up {
                        theme.text
                    } else {
                        theme.border_strong
                    })),
            )
            // Address bar with Enter and Escape key listeners
            .child(
                div()
                    .flex_1()
                    .text_data(&fonts)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "enter" {
                            this.commit_address(window, cx);
                        } else if event.keystroke.key == "escape" {
                            this.revert_address(window, cx);
                        }
                    }))
                    .child(self.address_input.clone()),
            )
            .child(
                div()
                    .id("toggle-preview")
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(CursorStyle::PointingHand)
                    .when(preview_open, |s| s.bg(theme.selection))
                    .when(!preview_open, |s| s.hover(|h| h.bg(theme.surface_raised)))
                    .child(icon(IconName::PanelRight).text_color(theme.text))
                    .tooltip(Tooltip::text(if preview_open {
                        "Hide preview (Alt+P)"
                    } else {
                        "Show preview (Alt+P)"
                    }))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePreview), cx)),
            )
    }
}
