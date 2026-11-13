//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, Context, CursorStyle, Entity, KeyDownEvent, MouseButton, MouseDownEvent,
    SharedString, Window,
};

use kwati_core::{ConnectionState, Location};

use crate::actions::navigation::TogglePreview;
use crate::actions::{DisconnectRemote, OpenSettings, ReconnectRemote, RefreshListing};
use crate::components::TextInput;
use crate::components::{icon, IconName, Tooltip};
use crate::remote::protocol_label;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};
use crate::views::search_view::ToggleSearch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteBadgeState {
    Connected,
    Reconnecting,
    Disconnected,
}

impl RemoteBadgeState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "Conectado",
            Self::Reconnecting => "Reconectando",
            Self::Disconnected => "Desconectado",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHeaderInfo {
    pub protocol: &'static str,
    pub host: String,
    pub state: RemoteBadgeState,
    pub show_refresh: bool,
    pub show_reconnect: bool,
    pub show_disconnect: bool,
}

/// Computes the remote header badge and control visibility for `state`.
pub fn remote_header_info(state: &AppState) -> Option<RemoteHeaderInfo> {
    let Location::Remote { profile_id, .. } = &state.current_location else {
        return None;
    };
    let profile = state.remote.profile(profile_id);
    let protocol = profile
        .map(|p| protocol_label(p.protocol))
        .unwrap_or("REMOTO");
    let host = profile
        .map(|p| p.host.clone())
        .unwrap_or_else(|| profile_id.clone());

    let badge_state = if state.connecting.as_deref() == Some(profile_id.as_str()) {
        RemoteBadgeState::Reconnecting
    } else {
        match state.remote_state() {
            Some(ConnectionState::Connected) => RemoteBadgeState::Connected,
            Some(ConnectionState::Reconnecting) => RemoteBadgeState::Reconnecting,
            Some(ConnectionState::Disconnected) | None => RemoteBadgeState::Disconnected,
        }
    };

    Some(RemoteHeaderInfo {
        protocol,
        host,
        state: badge_state,
        show_refresh: true,
        show_reconnect: badge_state == RemoteBadgeState::Disconnected,
        show_disconnect: matches!(
            badge_state,
            RemoteBadgeState::Connected | RemoteBadgeState::Reconnecting
        ),
    })
}

/// Whether the header should show the OS network-mount indicator badge.
pub fn is_network_mount_badge_visible(state: &AppState) -> bool {
    !state.is_remote() && state.is_current_network_location()
}

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

    /// Validates the entered path and navigates to it if it exists and is a directory (or
    /// navigates within the active remote server when `current_location` is remote).
    /// If missing locally, displays an error in the status bar and keeps current listing intact.
    pub fn commit_address(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.address_input.read(cx).text().trim().to_string();
        if typed.is_empty() {
            return;
        }

        let remote_profile_id = match &self.app_state.read(cx).current_location {
            Location::Remote { profile_id, .. } => Some(profile_id.clone()),
            Location::Local(_) => None,
        };

        if let Some(profile_id) = remote_profile_id {
            let normalized = if typed.starts_with('/') {
                typed
            } else {
                format!("/{typed}")
            };
            self.app_state.update(cx, |state, _cx| {
                state.navigate_to(Location::Remote {
                    profile_id,
                    path: PathBuf::from(normalized),
                });
                state.clear_status();
            });
            self.sync_address_input(cx);
        } else {
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
        let settings_open = state.settings_open;
        let remote_info = remote_header_info(state);
        let show_network_badge = is_network_mount_badge_visible(state);
        let can_go_up = state
            .current_location
            .parent()
            .map(|p| !p.path().as_os_str().is_empty())
            .unwrap_or(false);

        div()
            .flex()
            .flex_row()
            .h(size::TOOLBAR)
            .w_full()
            .pl(space::S12)
            .pr(space::S8)
            .items_center()
            .gap(space::S8)
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
            // Remote connection badge & controls
            .when_some(remote_info, |el, info| {
                let badge_color = match info.state {
                    RemoteBadgeState::Connected => theme.secondary,
                    RemoteBadgeState::Reconnecting => theme.warning,
                    RemoteBadgeState::Disconnected => theme.danger,
                };
                el.child(
                    div()
                        .id("header-remote-badge")
                        .flex()
                        .items_center()
                        .gap(space::S8)
                        .px(space::S8)
                        .py(space::S2)
                        .rounded(radius::CONTROL)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .child(icon(IconName::Network).text_color(badge_color))
                        .child(
                            div()
                                .text_label()
                                .text_color(theme.text)
                                .child(format!("{} · {}", info.protocol, info.host)),
                        )
                        .child(
                            div()
                                .text_label()
                                .text_color(badge_color)
                                .child(info.state.label()),
                        ),
                )
                .when(info.show_refresh, |el| {
                    el.child(
                        div()
                            .id("header-refresh")
                            .px(space::S8)
                            .py(space::S2)
                            .rounded(radius::CONTROL)
                            .bg(theme.surface_raised)
                            .border_1()
                            .border_color(theme.border)
                            .cursor(CursorStyle::PointingHand)
                            .hover(|h| h.bg(theme.border))
                            .flex()
                            .items_center()
                            .gap(space::S4)
                            .text_label()
                            .text_color(theme.text)
                            .tooltip(Tooltip::text("Atualizar listagem (F5)"))
                            .child(icon(IconName::Refresh).text_color(theme.text))
                            .child("Atualizar")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(RefreshListing), cx);
                            }),
                    )
                })
                .when(info.show_reconnect, |el| {
                    el.child(
                        div()
                            .id("header-reconnect")
                            .px(space::S8)
                            .py(space::S2)
                            .rounded(radius::CONTROL)
                            .bg(theme.accent)
                            .cursor(CursorStyle::PointingHand)
                            .text_label()
                            .text_color(theme.on_accent)
                            .child("Reconectar")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ReconnectRemote), cx);
                            }),
                    )
                })
                .when(info.show_disconnect, |el| {
                    el.child(
                        div()
                            .id("header-disconnect")
                            .px(space::S8)
                            .py(space::S2)
                            .rounded(radius::CONTROL)
                            .bg(theme.surface_raised)
                            .border_1()
                            .border_color(theme.border)
                            .cursor(CursorStyle::PointingHand)
                            .hover(|h| h.bg(theme.border))
                            .text_label()
                            .text_color(theme.text_muted)
                            .child("Desconectar")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(DisconnectRemote), cx);
                            }),
                    )
                })
            })
            // Local OS network-mount indicator badge
            .when(show_network_badge, |el| {
                el.child(
                    div()
                        .id("header-network-mount-badge")
                        .flex()
                        .items_center()
                        .gap(space::S4)
                        .px(space::S8)
                        .py(space::S2)
                        .rounded(radius::CONTROL)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .text_label()
                        .text_color(theme.warning)
                        .tooltip(Tooltip::text("Pasta localizada em montagem de rede"))
                        .child(icon(IconName::Network).text_color(theme.warning))
                        .child("Montagem de rede"),
                )
            })
            // Address bar with Enter and Escape key listeners
            .child(
                div()
                    .flex_1()
                    .min_w_0()
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
            // Search button
            .child(
                div()
                    .id("header-search")
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(CursorStyle::PointingHand)
                    .hover(|h| h.bg(theme.surface_raised))
                    .child(icon(IconName::Search).text_color(theme.text))
                    .tooltip(Tooltip::text("Buscar (Ctrl+F)"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSearch), cx)),
            )
            // Preview toggle button
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
            // Settings button
            .child(
                div()
                    .id("header-settings")
                    .px(space::S4)
                    .py(space::S2)
                    .rounded(radius::CONTROL)
                    .cursor(CursorStyle::PointingHand)
                    .when(settings_open, |s| s.bg(theme.selection))
                    .when(!settings_open, |s| s.hover(|h| h.bg(theme.surface_raised)))
                    .child(icon(IconName::Settings).text_color(theme.text))
                    .tooltip(Tooltip::text("Configurações (Ctrl+,)"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            )
    }
}
