//! Context menu view for file list entries and empty space (#55).

use std::time::Instant;

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Context, CursorStyle, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, Window,
};
use kwati_core::{volume_kind, Location, VolumeKind};

use crate::actions::operations::get_location_clipboard;
use crate::components::{animated_icon, popup_entrance_metrics, IconMotion, IconName};
use crate::motion::{
    color_lerp, is_reduced_motion, request_frame_if_active, Curve, MotionKind, MotionTracker,
    INSTANT, MENU_CASCADE_STEP, QUICK,
};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

/// Actions offered by the file list or sidebar Places context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Open,
    Rename,
    Bookmark,
    Copy,
    Cut,
    Paste,
    Remove,
    NewFolder,
    Refresh,
    SidebarOpen(usize),
    SidebarRename(usize),
    SidebarRemove(usize),
}

/// An item or separator inside the context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuEntry {
    Item {
        action: ContextMenuAction,
        label: &'static str,
        shortcut: Option<&'static str>,
        enabled: bool,
        destructive: bool,
    },
    Separator,
}

/// Events emitted by [`ContextMenuView`] to [`super::MainView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuEvent {
    Execute(ContextMenuAction),
    Closed,
}

/// Builds the context menu entries applicable to the current selection and clipboard state.
/// `folder_bookmarked` is `Some` when the selection is a single local folder, telling whether
/// the sidebar already lists it.
pub fn build_context_menu_entries(
    selected: &[Location],
    has_clipboard: bool,
    folder_bookmarked: Option<bool>,
) -> Vec<ContextMenuEntry> {
    if selected.is_empty() {
        return vec![
            ContextMenuEntry::Item {
                action: ContextMenuAction::Paste,
                label: "Paste",
                shortcut: Some("Ctrl+V"),
                enabled: has_clipboard,
                destructive: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::NewFolder,
                label: "New Folder",
                shortcut: Some("Ctrl+Shift+N"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item {
                action: ContextMenuAction::Refresh,
                label: "Refresh",
                shortcut: Some("F5"),
                enabled: true,
                destructive: false,
            },
        ];
    }

    let trash_available = selected
        .iter()
        .all(|loc| matches!(loc, Location::Local(path) if volume_kind(path) == VolumeKind::Local));
    let remove_label = if trash_available {
        "Move to Trash"
    } else {
        "Delete Permanently"
    };

    if selected.len() == 1 {
        let mut entries = vec![
            ContextMenuEntry::Item {
                action: ContextMenuAction::Open,
                label: "Open",
                shortcut: Some("Enter"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::Rename,
                label: "Rename",
                shortcut: Some("F2"),
                enabled: true,
                destructive: false,
            },
        ];
        if let Some(bookmarked) = folder_bookmarked {
            entries.push(ContextMenuEntry::Item {
                action: ContextMenuAction::Bookmark,
                label: "Add to Places",
                shortcut: None,
                enabled: !bookmarked,
                destructive: false,
            });
        }
        entries.extend([
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item {
                action: ContextMenuAction::Copy,
                label: "Copy",
                shortcut: Some("Ctrl+C"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::Cut,
                label: "Cut",
                shortcut: Some("Ctrl+X"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::Paste,
                label: "Paste",
                shortcut: Some("Ctrl+V"),
                enabled: has_clipboard,
                destructive: false,
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item {
                action: ContextMenuAction::Remove,
                label: remove_label,
                shortcut: Some("Delete"),
                enabled: true,
                destructive: true,
            },
        ]);
        entries
    } else {
        vec![
            ContextMenuEntry::Item {
                action: ContextMenuAction::Copy,
                label: "Copy",
                shortcut: Some("Ctrl+C"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Item {
                action: ContextMenuAction::Cut,
                label: "Cut",
                shortcut: Some("Ctrl+X"),
                enabled: true,
                destructive: false,
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Item {
                action: ContextMenuAction::Remove,
                label: remove_label,
                shortcut: Some("Delete"),
                enabled: true,
                destructive: true,
            },
        ]
    }
}

/// Builds the context menu entries for a sidebar Places item at `index`.
/// Trash can be opened, while folders can be opened, renamed, or removed from Places.
pub fn build_sidebar_place_menu_entries(index: usize, is_trash: bool) -> Vec<ContextMenuEntry> {
    if is_trash {
        return vec![ContextMenuEntry::Item {
            action: ContextMenuAction::SidebarOpen(index),
            label: "Open",
            shortcut: None,
            enabled: true,
            destructive: false,
        }];
    }
    vec![
        ContextMenuEntry::Item {
            action: ContextMenuAction::SidebarOpen(index),
            label: "Open",
            shortcut: None,
            enabled: true,
            destructive: false,
        },
        ContextMenuEntry::Item {
            action: ContextMenuAction::SidebarRename(index),
            label: "Rename",
            shortcut: None,
            enabled: true,
            destructive: false,
        },
        ContextMenuEntry::Separator,
        ContextMenuEntry::Item {
            action: ContextMenuAction::SidebarRemove(index),
            label: "Remove from Places",
            shortcut: None,
            enabled: true,
            destructive: false,
        },
    ]
}

/// Floating context menu anchored at the pointer or focused list row.
pub struct ContextMenuView {
    app_state: Entity<AppState>,
    focus_handle: FocusHandle,
    is_open: bool,
    position: Point<Pixels>,
    entries: Vec<ContextMenuEntry>,
    focused_index: Option<usize>,
    hovered_index: Option<usize>,
    opened_at: Option<Instant>,
    motion: MotionTracker<usize>,
}

impl EventEmitter<ContextMenuEvent> for ContextMenuView {}

impl Focusable for ContextMenuView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ContextMenuView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            is_open: false,
            position: point(px(0.), px(0.)),
            entries: Vec::new(),
            focused_index: None,
            hovered_index: None,
            opened_at: None,
            motion: MotionTracker::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn entries(&self) -> &[ContextMenuEntry] {
        &self.entries
    }

    pub fn position(&self) -> Point<Pixels> {
        self.position
    }

    /// Opens the menu at `position` with items matching the current selection and clipboard.
    pub fn open_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.app_state.read(cx);
        let selected = state.selected_locations();
        let folder_bookmarked = match (state.selected_entries().as_slice(), selected.as_slice()) {
            ([entry], [Location::Local(path)]) if entry.is_dir() => {
                Some(state.is_place_pinned(path))
            }
            _ => None,
        };
        let has_clipboard = get_location_clipboard().is_some();
        self.entries = build_context_menu_entries(&selected, has_clipboard, folder_bookmarked);
        self.position = position;
        self.focused_index = self.first_enabled_index();
        self.hovered_index = None;
        self.opened_at = Some(Instant::now());
        self.motion.clear();
        self.is_open = true;
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Opens the menu at `position` for a sidebar Places item.
    pub fn open_for_sidebar_place_at(
        &mut self,
        index: usize,
        is_trash: bool,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.entries = build_sidebar_place_menu_entries(index, is_trash);
        self.position = position;
        self.focused_index = self.first_enabled_index();
        self.hovered_index = None;
        self.opened_at = Some(Instant::now());
        self.motion.clear();
        self.is_open = true;
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Closes the menu and notifies listeners to restore focus.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        self.is_open = false;
        self.entries.clear();
        self.focused_index = None;
        self.hovered_index = None;
        self.opened_at = None;
        self.motion.clear();
        cx.emit(ContextMenuEvent::Closed);
        cx.notify();
    }

    fn trigger(&mut self, action: ContextMenuAction, cx: &mut Context<Self>) {
        self.is_open = false;
        self.entries.clear();
        self.focused_index = None;
        cx.emit(ContextMenuEvent::Execute(action));
        cx.notify();
    }

    fn first_enabled_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| matches!(e, ContextMenuEntry::Item { enabled: true, .. }))
    }

    fn move_focus(&mut self, forward: bool, cx: &mut Context<Self>) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let start = self.focused_index.unwrap_or(0);
        for step in 1..=len {
            let candidate = if forward {
                (start + step) % len
            } else {
                (start + len - (step % len)) % len
            };
            if matches!(
                self.entries.get(candidate),
                Some(ContextMenuEntry::Item { enabled: true, .. })
            ) {
                self.focused_index = Some(candidate);
                cx.notify();
                return;
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.close(cx);
            }
            "down" => {
                cx.stop_propagation();
                self.move_focus(true, cx);
            }
            "up" => {
                cx.stop_propagation();
                self.move_focus(false, cx);
            }
            "enter" | "space" => {
                cx.stop_propagation();
                if let Some(ix) = self.focused_index {
                    if let Some(ContextMenuEntry::Item {
                        action,
                        enabled: true,
                        ..
                    }) = self.entries.get(ix).copied()
                    {
                        self.trigger(action, cx);
                    }
                }
            }
            _ => {}
        }
    }
}

fn menu_action_icon(action: ContextMenuAction) -> Option<IconName> {
    match action {
        ContextMenuAction::Open => Some(IconName::Folder),
        ContextMenuAction::Copy => Some(IconName::Copy),
        ContextMenuAction::Remove => Some(IconName::Trash),
        ContextMenuAction::NewFolder => Some(IconName::Folder),
        ContextMenuAction::Refresh => Some(IconName::Refresh),
        ContextMenuAction::SidebarRemove(_) => Some(IconName::Close),
        ContextMenuAction::Rename
        | ContextMenuAction::Bookmark
        | ContextMenuAction::Cut
        | ContextMenuAction::Paste
        | ContextMenuAction::SidebarOpen(_)
        | ContextMenuAction::SidebarRename(_) => None,
    }
}

impl Render for ContextMenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return div().into_any_element();
        }

        let now = Instant::now();
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let reduced = is_reduced_motion(self.app_state.read(cx).settings.motion_preference);
        let focused = self.focused_index;

        let open_elapsed = self
            .opened_at
            .map_or(QUICK, |t| now.saturating_duration_since(t));
        let open_dur = if reduced { INSTANT } else { QUICK };
        let open_t = (open_elapsed.as_secs_f32() / open_dur.as_secs_f32()).clamp(0.0, 1.0);
        let (card_scale, card_opacity) = popup_entrance_metrics(open_t, reduced);

        const CARD_W: f32 = 210.0;
        let visible_w = px(CARD_W * card_scale);

        let mut card = div()
            .id("context-menu-card")
            .w(px(CARD_W))
            .py(space::S4)
            .px(space::S4)
            .rounded(radius::CONTROL)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .flex()
            .flex_col()
            .gap(space::S2)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        let mut cascade_animating = open_t < 1.0;
        let mut item_slot: usize = 0;

        for (ix, entry) in self.entries.iter().enumerate() {
            match entry {
                ContextMenuEntry::Separator => {
                    card = card.child(div().h(px(1.)).my(space::S2).bg(theme.border));
                }
                ContextMenuEntry::Item {
                    action,
                    label,
                    shortcut,
                    enabled,
                    destructive,
                } => {
                    let action = *action;
                    let enabled = *enabled;
                    let destructive = *destructive;
                    let is_focused = focused == Some(ix);
                    let is_hovered = self.hovered_index == Some(ix);
                    let active_t = self.motion.step_scalar(
                        ix,
                        0.0,
                        if enabled && (is_focused || is_hovered) {
                            1.0
                        } else {
                            0.0
                        },
                        INSTANT,
                        Curve::EaseOut,
                        MotionKind::OpacityOrColor,
                        now,
                        reduced,
                    );
                    let icon_t = self.motion.step_scalar(
                        10_000 + ix,
                        0.0,
                        if enabled && is_hovered { 1.0 } else { 0.0 },
                        QUICK,
                        Curve::Spring,
                        MotionKind::MovementOrScale,
                        now,
                        reduced,
                    );

                    let (item_opacity, item_dx) = if reduced {
                        (card_opacity, 0.0)
                    } else {
                        let delay = MENU_CASCADE_STEP.saturating_mul(item_slot as u32);
                        let since_delay = open_elapsed.saturating_sub(delay);
                        let p = (since_delay.as_secs_f32() / QUICK.as_secs_f32()).clamp(0.0, 1.0);
                        if p < 1.0 {
                            cascade_animating = true;
                        }
                        let spring = Curve::Spring.sample(p);
                        (Curve::EaseOut.sample(p), -6.0 * (1.0 - spring))
                    };
                    item_slot += 1;

                    let label_color = if !enabled {
                        theme.text_muted
                    } else if destructive {
                        theme.danger
                    } else {
                        theme.text
                    };
                    let row_bg = color_lerp(theme.surface_raised, theme.selection, active_t);

                    let icon_el = menu_action_icon(action).map(|icon_name| {
                        let motion = if matches!(icon_name, IconName::Refresh) {
                            if icon_t > 0.01 && icon_t < 0.99 {
                                IconMotion::Pulse(icon_t)
                            } else {
                                IconMotion::Rest
                            }
                        } else {
                            IconMotion::State(icon_t)
                        };
                        animated_icon(icon_name, motion, label_color)
                    });

                    let row = div()
                        .id(("context-menu-item", ix))
                        .debug_selector(move || format!("context-menu-item-{ix}"))
                        .relative()
                        .left(px(item_dx))
                        .opacity(item_opacity)
                        .h(size::ROW)
                        .px(space::S8)
                        .rounded(radius::CONTROL)
                        .bg(row_bg)
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_body()
                        .text_color(label_color)
                        .when(enabled, |el| {
                            el.cursor(CursorStyle::PointingHand)
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.hovered_index = Some(ix);
                                    } else if this.hovered_index == Some(ix) {
                                        this.hovered_index = None;
                                    }
                                    cx.notify();
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.trigger(action, cx);
                                    }),
                                )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .children(icon_el)
                                .child(*label),
                        )
                        .when_some(*shortcut, |el, key| {
                            el.child(
                                div()
                                    .text_data(&fonts)
                                    .text_color(theme.text_muted)
                                    .child(key),
                            )
                        });

                    card = card.child(row);
                }
            }
        }

        request_frame_if_active(
            cascade_animating || self.motion.has_active(now, reduced),
            window,
        );

        let animated_card = div()
            .w(visible_w)
            .overflow_hidden()
            .opacity(card_opacity)
            .child(card);

        deferred(
            div()
                .id("context-menu-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::on_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }),
                )
                .child(
                    anchored()
                        .position(self.position)
                        .snap_to_window_with_margin(px(8.))
                        .child(animated_card),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}
