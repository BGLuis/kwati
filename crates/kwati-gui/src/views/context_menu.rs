//! Context menu view for file list entries and empty space (#55).

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Context, CursorStyle, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, Window,
};
use kwati_core::{volume_kind, Location, VolumeKind};

use crate::actions::operations::get_location_clipboard;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

/// Actions offered by the file list context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Open,
    Rename,
    Copy,
    Cut,
    Paste,
    Remove,
    NewFolder,
    Refresh,
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
pub fn build_context_menu_entries(
    selected: &[Location],
    has_clipboard: bool,
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
        vec![
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
        ]
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

/// Floating context menu anchored at the pointer or focused list row.
pub struct ContextMenuView {
    app_state: Entity<AppState>,
    focus_handle: FocusHandle,
    is_open: bool,
    position: Point<Pixels>,
    entries: Vec<ContextMenuEntry>,
    focused_index: Option<usize>,
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
        let selected = self.app_state.read(cx).selected_locations();
        let has_clipboard = get_location_clipboard().is_some();
        self.entries = build_context_menu_entries(&selected, has_clipboard);
        self.position = position;
        self.focused_index = self.first_enabled_index();
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

impl Render for ContextMenuView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return div().into_any_element();
        }

        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let focused = self.focused_index;

        let mut card = div()
            .id("context-menu-card")
            .w(px(210.))
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
                    let label_color = if !enabled {
                        theme.text_muted
                    } else if destructive {
                        theme.danger
                    } else {
                        theme.text
                    };

                    let row = div()
                        .id(("context-menu-item", ix))
                        .h(size::ROW)
                        .px(space::S8)
                        .rounded(radius::CONTROL)
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_body()
                        .text_color(label_color)
                        .when(is_focused && enabled, |el| el.bg(theme.selection))
                        .when(enabled, |el| {
                            el.cursor(CursorStyle::PointingHand)
                                .hover(|s| s.bg(theme.selection))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.trigger(action, cx);
                                    }),
                                )
                        })
                        .child(*label)
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
                        .child(card),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}
