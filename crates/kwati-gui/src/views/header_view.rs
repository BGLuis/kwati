//! Header and address bar view (owned by guinav-worker for #9, #10).
//!
//! Provides navigation buttons (Back, Forward, Up) and the interactive address bar.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    div, prelude::*, transparent_black, Context, CursorStyle, Entity, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Stateful, Window,
};

use kwati_core::{ConnectionState, Location, ViewMode};

use crate::actions::navigation::{SetViewCompact, SetViewDetails, SetViewGrid, TogglePreview};
use crate::actions::{DisconnectRemote, OpenSettings, ReconnectRemote, RefreshListing};
use crate::components::interactive::hover_bg;
use crate::components::{animated_icon_sized, IconMotion, IconName, TextInput, Tooltip};
use crate::motion::{
    color_lerp, is_reduced_motion, lerp_f32, request_frame_if_active, Curve, MotionKind,
    MotionTracker, INSTANT, PRESS_SCALE, QUICK,
};
use crate::remote::protocol_label;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};
use crate::views::search_view::ToggleSearch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HeaderControl {
    Back,
    Forward,
    Up,
    Refresh,
    Reconnect,
    Disconnect,
    ViewDetails,
    ViewGrid,
    ViewCompact,
    Search,
    Preview,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HeaderMotionKey {
    Hover(HeaderControl),
    Press(HeaderControl),
    RefreshSpin,
    RemoteDraw,
    MountDraw,
}

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
    hovered: HashSet<HeaderControl>,
    pressed: HashSet<HeaderControl>,
    last_remote_state: Option<RemoteBadgeState>,
    last_network_badge: bool,
    motion: MotionTracker<HeaderMotionKey>,
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
            hovered: HashSet::new(),
            pressed: HashSet::new(),
            last_remote_state: None,
            last_network_badge: false,
            motion: MotionTracker::new(),
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
                self.address_input.update(cx, |input, cx| {
                    input.trigger_shake(cx);
                });
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
    fn control_factors(
        &mut self,
        ctrl: HeaderControl,
        enabled: bool,
        now: Instant,
        reduced: bool,
    ) -> (f32, f32) {
        let hover_target = if enabled && self.hovered.contains(&ctrl) {
            1.0
        } else {
            0.0
        };
        let press_target = if enabled && self.pressed.contains(&ctrl) {
            1.0
        } else {
            0.0
        };
        let hover_t = self.motion.step_scalar(
            HeaderMotionKey::Hover(ctrl),
            0.0,
            hover_target,
            INSTANT,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let press_t = self.motion.step_scalar(
            HeaderMotionKey::Press(ctrl),
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

    fn set_hover(&mut self, ctrl: HeaderControl, active: bool, cx: &mut Context<Self>) {
        let changed = if active {
            self.hovered.insert(ctrl)
        } else {
            self.pressed.remove(&ctrl);
            self.hovered.remove(&ctrl)
        };
        if changed {
            cx.notify();
        }
    }

    fn set_press(&mut self, ctrl: HeaderControl, active: bool, cx: &mut Context<Self>) {
        let changed = if active {
            self.pressed.insert(ctrl)
        } else {
            self.pressed.remove(&ctrl)
        };
        if changed {
            cx.notify();
        }
    }

    pub fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.set_view_mode(mode);
            cx.notify();
        });
        cx.notify();
    }

    fn view_switcher_segment(
        &self,
        spec: (
            &'static str,
            HeaderControl,
            ViewMode,
            IconName,
            &'static str,
        ),
        active: bool,
        motion: (f32, f32),
        reduced: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, ctrl, mode, icon_name, tooltip_label) = spec;
        let (hover_t, press_t) = motion;
        let bg = if active {
            theme.selection.into()
        } else {
            hover_bg(transparent_black(), hover_t, theme)
        };
        let color = if active {
            theme.text
        } else {
            color_lerp(theme.text_muted, theme.text, hover_t).into()
        };
        let scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, press_t.clamp(0.0, 1.0))
        };

        div()
            .id(id)
            .w(gpui::px(26.))
            .h(gpui::px(20.))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .rounded(gpui::px(2.))
            .bg(bg)
            .cursor(CursorStyle::PointingHand)
            .tooltip(Tooltip::text(tooltip_label))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.set_hover(ctrl, *hovered, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_press(ctrl, true, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_press(ctrl, false, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_press(ctrl, false, cx);
                }),
            )
            .on_click(move |_, window, cx| {
                let action: Box<dyn gpui::Action> = match mode {
                    ViewMode::Details => Box::new(SetViewDetails),
                    ViewMode::Grid => Box::new(SetViewGrid),
                    ViewMode::CompactList => Box::new(SetViewCompact),
                };
                window.dispatch_action(action, cx);
            })
            .child(animated_icon_sized(
                icon_name,
                IconMotion::Rest,
                size::ICON * scale,
                color,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn icon_button(
        &self,
        id: &'static str,
        ctrl: HeaderControl,
        icon_name: IconName,
        enabled: bool,
        active: bool,
        hover_t: f32,
        press_t: f32,
        reduced: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let rest_bg: gpui::Hsla = if active {
            theme.selection.into()
        } else {
            transparent_black()
        };
        let bg = if active {
            rest_bg
        } else {
            hover_bg(rest_bg, hover_t, theme)
        };
        let scale = if reduced || !enabled {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, press_t.clamp(0.0, 1.0))
        };
        let icon_motion = if enabled && !reduced && hover_t > 0.01 {
            IconMotion::State(hover_t)
        } else {
            IconMotion::Rest
        };
        let color = if enabled {
            theme.text
        } else {
            theme.border_strong
        };

        div()
            .id(id)
            .w(size::ICON + space::S4 * 2.0)
            .h(size::ICON + space::S2 * 2.0)
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .rounded(radius::CONTROL)
            .bg(bg)
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .when(enabled, |el| {
                el.on_hover(cx.listener(move |this, hovered, _, cx| {
                    this.set_hover(ctrl, *hovered, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(ctrl, true, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(ctrl, false, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_press(ctrl, false, cx);
                    }),
                )
            })
            .child(animated_icon_sized(
                icon_name,
                icon_motion,
                size::ICON * scale,
                color,
            ))
    }
}

impl Render for HeaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let now = Instant::now();
        let (
            can_go_back,
            can_go_forward,
            preview_open,
            settings_open,
            view_mode,
            remote_info,
            show_network_badge,
            can_go_up,
            reduced,
        ) = {
            let state = self.app_state.read(cx);
            let can_go_up = state
                .current_location
                .parent()
                .map(|p| !p.path().as_os_str().is_empty())
                .unwrap_or(false);
            (
                state.history.can_go_back(),
                state.history.can_go_forward(),
                state.panels.preview_open,
                state.settings_open,
                state.view_mode,
                remote_header_info(state),
                is_network_mount_badge_visible(state),
                can_go_up,
                is_reduced_motion(state.settings.motion_preference),
            )
        };

        let remote_state_now = remote_info.as_ref().map(|info| info.state);
        if remote_state_now != self.last_remote_state {
            self.last_remote_state = remote_state_now;
            if remote_state_now.is_some() {
                self.motion.trigger_pulse(
                    HeaderMotionKey::RemoteDraw,
                    QUICK,
                    MotionKind::MovementOrScale,
                    now,
                );
            }
        }
        if show_network_badge != self.last_network_badge {
            self.last_network_badge = show_network_badge;
            if show_network_badge {
                self.motion.trigger_pulse(
                    HeaderMotionKey::MountDraw,
                    QUICK,
                    MotionKind::MovementOrScale,
                    now,
                );
            }
        }

        let (back_hover, back_press) =
            self.control_factors(HeaderControl::Back, can_go_back, now, reduced);
        let (fwd_hover, fwd_press) =
            self.control_factors(HeaderControl::Forward, can_go_forward, now, reduced);
        let (up_hover, up_press) = self.control_factors(HeaderControl::Up, can_go_up, now, reduced);
        let (refresh_hover, refresh_press) =
            self.control_factors(HeaderControl::Refresh, remote_info.is_some(), now, reduced);
        let (_, reconnect_press) = self.control_factors(
            HeaderControl::Reconnect,
            remote_info.as_ref().is_some_and(|i| i.show_reconnect),
            now,
            reduced,
        );
        let (disconnect_hover, disconnect_press) = self.control_factors(
            HeaderControl::Disconnect,
            remote_info.as_ref().is_some_and(|i| i.show_disconnect),
            now,
            reduced,
        );
        let (view_details_hover, view_details_press) =
            self.control_factors(HeaderControl::ViewDetails, true, now, reduced);
        let (view_grid_hover, view_grid_press) =
            self.control_factors(HeaderControl::ViewGrid, true, now, reduced);
        let (view_compact_hover, view_compact_press) =
            self.control_factors(HeaderControl::ViewCompact, true, now, reduced);
        let (search_hover, search_press) =
            self.control_factors(HeaderControl::Search, true, now, reduced);
        let (preview_hover, preview_press) =
            self.control_factors(HeaderControl::Preview, true, now, reduced);
        let (settings_hover, settings_press) =
            self.control_factors(HeaderControl::Settings, true, now, reduced);

        let refresh_motion = self
            .motion
            .pulse_progress(&HeaderMotionKey::RefreshSpin, now, reduced)
            .map_or(IconMotion::Rest, IconMotion::Pulse);
        let remote_draw_motion = self
            .motion
            .pulse_progress(&HeaderMotionKey::RemoteDraw, now, reduced)
            .map_or(IconMotion::Rest, |t| {
                IconMotion::Draw(Curve::EaseOut.sample(t))
            });
        let mount_draw_motion = self
            .motion
            .pulse_progress(&HeaderMotionKey::MountDraw, now, reduced)
            .map_or(IconMotion::Rest, |t| {
                IconMotion::Draw(Curve::EaseOut.sample(t))
            });

        let animating = self.motion.has_active(now, reduced);
        self.motion.prune_settled(now, reduced);
        request_frame_if_active(animating, window);

        let back_btn = self
            .icon_button(
                "header-back",
                HeaderControl::Back,
                IconName::ArrowLeft,
                can_go_back,
                false,
                back_hover,
                back_press,
                reduced,
                &theme,
                cx,
            )
            .when(can_go_back, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev, window, cx| {
                        this.set_press(HeaderControl::Back, true, cx);
                        this.on_back_click(ev, window, cx);
                    }),
                )
            });

        let fwd_btn = self
            .icon_button(
                "header-forward",
                HeaderControl::Forward,
                IconName::ArrowRight,
                can_go_forward,
                false,
                fwd_hover,
                fwd_press,
                reduced,
                &theme,
                cx,
            )
            .when(can_go_forward, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev, window, cx| {
                        this.set_press(HeaderControl::Forward, true, cx);
                        this.on_forward_click(ev, window, cx);
                    }),
                )
            });

        let up_btn = self
            .icon_button(
                "header-up",
                HeaderControl::Up,
                IconName::ArrowUp,
                can_go_up,
                false,
                up_hover,
                up_press,
                reduced,
                &theme,
                cx,
            )
            .when(can_go_up, |s| {
                s.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev, window, cx| {
                        this.set_press(HeaderControl::Up, true, cx);
                        this.on_up_click(ev, window, cx);
                    }),
                )
            });

        let view_switcher = div()
            .id("header-view-switcher")
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .p(space::S2)
            .gap(space::S2)
            .rounded(radius::CONTROL)
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border_strong)
            .child(self.view_switcher_segment(
                (
                    "view-mode-details",
                    HeaderControl::ViewDetails,
                    ViewMode::Details,
                    IconName::ViewList,
                    "Details view (Ctrl+1)",
                ),
                view_mode == ViewMode::Details,
                (view_details_hover, view_details_press),
                reduced,
                &theme,
                cx,
            ))
            .child(self.view_switcher_segment(
                (
                    "view-mode-grid",
                    HeaderControl::ViewGrid,
                    ViewMode::Grid,
                    IconName::ViewGrid,
                    "Grid view (Ctrl+2)",
                ),
                view_mode == ViewMode::Grid,
                (view_grid_hover, view_grid_press),
                reduced,
                &theme,
                cx,
            ))
            .child(self.view_switcher_segment(
                (
                    "view-mode-compact",
                    HeaderControl::ViewCompact,
                    ViewMode::CompactList,
                    IconName::ViewCompact,
                    "Compact list view (Ctrl+3)",
                ),
                view_mode == ViewMode::CompactList,
                (view_compact_hover, view_compact_press),
                reduced,
                &theme,
                cx,
            ));

        let search_btn = self
            .icon_button(
                "header-search",
                HeaderControl::Search,
                IconName::Search,
                true,
                false,
                search_hover,
                search_press,
                reduced,
                &theme,
                cx,
            )
            .tooltip(Tooltip::text("Buscar (Ctrl+F)"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSearch), cx));

        let preview_btn = self
            .icon_button(
                "toggle-preview",
                HeaderControl::Preview,
                IconName::PanelRight,
                true,
                preview_open,
                preview_hover,
                preview_press,
                reduced,
                &theme,
                cx,
            )
            .tooltip(Tooltip::text(if preview_open {
                "Hide preview (Alt+P)"
            } else {
                "Show preview (Alt+P)"
            }))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePreview), cx));

        let settings_btn = self
            .icon_button(
                "header-settings",
                HeaderControl::Settings,
                IconName::Settings,
                true,
                settings_open,
                settings_hover,
                settings_press,
                reduced,
                &theme,
                cx,
            )
            .tooltip(Tooltip::text("Configurações (Ctrl+,)"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx));

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
            .child(back_btn)
            .child(fwd_btn)
            .child(up_btn)
            // Remote connection badge & controls
            .when_some(remote_info, |el, info| {
                let badge_color = match info.state {
                    RemoteBadgeState::Connected => theme.secondary,
                    RemoteBadgeState::Reconnecting => theme.warning,
                    RemoteBadgeState::Disconnected => theme.danger,
                };
                let refresh_bg = color_lerp(theme.surface_raised, theme.border, refresh_hover);
                let refresh_icon_size = if reduced {
                    size::ICON
                } else {
                    size::ICON * lerp_f32(1.0, PRESS_SCALE, refresh_press)
                };
                let disconnect_bg =
                    color_lerp(theme.surface_raised, theme.border, disconnect_hover);
                let reconnect_bg =
                    color_lerp(theme.accent, theme.surface_raised, reconnect_press * 0.15);
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
                        .child(animated_icon_sized(
                            IconName::Network,
                            remote_draw_motion,
                            size::ICON,
                            badge_color,
                        ))
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
                            .bg(refresh_bg)
                            .border_1()
                            .border_color(theme.border)
                            .cursor(CursorStyle::PointingHand)
                            .flex()
                            .items_center()
                            .gap(space::S4)
                            .text_label()
                            .text_color(theme.text)
                            .tooltip(Tooltip::text("Atualizar listagem (F5)"))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                this.set_hover(HeaderControl::Refresh, *hovered, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Refresh, true, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Refresh, false, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Refresh, false, cx);
                                }),
                            )
                            .child(animated_icon_sized(
                                IconName::Refresh,
                                refresh_motion,
                                refresh_icon_size,
                                theme.text,
                            ))
                            .child("Atualizar")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.motion.trigger_pulse(
                                    HeaderMotionKey::RefreshSpin,
                                    QUICK,
                                    MotionKind::MovementOrScale,
                                    Instant::now(),
                                );
                                window.dispatch_action(Box::new(RefreshListing), cx);
                                cx.notify();
                            })),
                    )
                })
                .when(info.show_reconnect, |el| {
                    el.child(
                        div()
                            .id("header-reconnect")
                            .px(space::S8)
                            .py(space::S2)
                            .rounded(radius::CONTROL)
                            .bg(reconnect_bg)
                            .cursor(CursorStyle::PointingHand)
                            .text_label()
                            .text_color(theme.on_accent)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Reconnect, true, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Reconnect, false, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Reconnect, false, cx);
                                }),
                            )
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
                            .bg(disconnect_bg)
                            .border_1()
                            .border_color(theme.border)
                            .cursor(CursorStyle::PointingHand)
                            .text_label()
                            .text_color(if disconnect_press > 0.5 && !reduced {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                this.set_hover(HeaderControl::Disconnect, *hovered, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Disconnect, true, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Disconnect, false, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_press(HeaderControl::Disconnect, false, cx);
                                }),
                            )
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
                        .child(animated_icon_sized(
                            IconName::Network,
                            mount_draw_motion,
                            size::ICON,
                            theme.warning,
                        ))
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
            .child(view_switcher)
            .child(search_btn)
            .child(preview_btn)
            .child(settings_btn)
    }
}
