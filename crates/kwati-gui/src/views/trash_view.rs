//! Trash browser view (#63).
//!
//! Lists trashed items with their original location and deletion date, lets the user
//! restore selected items (reporting destination conflicts without overwriting), and
//! offers Empty Trash behind a destructive confirmation dialog.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    div, prelude::*, px, relative, transparent_black, uniform_list, Context, CursorStyle, Entity,
    EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollStrategy, UniformListScrollHandle, Window,
};
use kwati_core::{SortDirection, TrashItem};

use crate::actions::navigation::{
    ClearSelection, GoBack, PageDown, PageUp, Refresh, SelectAll, SelectFirst, SelectLast,
    SelectNext, SelectPrevious,
};
use crate::actions::operations::{
    apply_restore_trash_outcome, restore_trash_items_op, RestoreTrashOutcome,
};
use crate::components::interactive::focus_ring_color;
use crate::components::{animated_icon_sized, Ellipsize, IconMotion, IconName, ScrollbarState};
use crate::motion::{
    color_lerp, is_reduced_motion, lerp_f32, request_frame_if_active, Curve, MotionKind,
    MotionTracker, INSTANT, PRESS_SCALE,
};
use crate::state::{AppState, TrashSortColumn};
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};
use crate::views::list_view::format_modified;

/// Events emitted by [`TrashView`] to [`super::MainView`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashViewEvent {
    ConfirmEmptyTrash {
        items: Vec<TrashItem>,
    },
    RestoreConflict {
        item_name: String,
        destination: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TrashMotionKey {
    Hover(OsString),
    Press(OsString),
    SelectBg(OsString),
    FocusRing(OsString),
    HeaderHover(u8),
}

const DATE_COLUMN: Pixels = px(140.);
const SCROLLBAR_WIDTH: Pixels = px(10.);

/// View rendering system trash items with Restore and Empty Trash controls.
pub struct TrashView {
    pub app_state: Entity<AppState>,
    pub focus_handle: FocusHandle,
    pub scroll_handle: UniformListScrollHandle,
    pub scrollbar_state: ScrollbarState,
    hovered_id: Option<OsString>,
    pressed_id: Option<OsString>,
    hovered_header: Option<u8>,
    pending_restore: bool,
    op_generation: u64,
    motion: MotionTracker<TrashMotionKey>,
}

impl EventEmitter<TrashViewEvent> for TrashView {}

impl Focusable for TrashView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TrashView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            scrollbar_state: ScrollbarState::default(),
            hovered_id: None,
            pressed_id: None,
            hovered_header: None,
            pending_restore: false,
            op_generation: 0,
            motion: MotionTracker::new(),
        }
    }

    pub fn scroll_to_item(&self, ix: usize) {
        self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
    }

    pub fn scroll_to_top(&self) {
        self.scroll_handle
            .scroll_to_item_strict(0, ScrollStrategy::Top);
    }

    pub fn scroll_to_bottom(&self) {
        let total = self.scrollbar_state.total_items;
        if total > 0 {
            self.scroll_handle
                .scroll_to_item_strict(total.saturating_sub(1), ScrollStrategy::Bottom);
        }
    }

    fn update_selection(
        &mut self,
        cx: &mut Context<Self>,
        select: impl FnOnce(&mut AppState) -> Option<usize>,
        after: impl FnOnce(&Self, usize),
    ) {
        let new_ix = self.app_state.update(cx, |state, cx| {
            let prev = state.selected_trash_index();
            let ix = select(state);
            if ix.is_some() && ix != prev {
                cx.notify();
            }
            ix
        });
        if let Some(ix) = new_ix {
            after(self, ix);
        }
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_next_trash, Self::scroll_to_item);
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_previous_trash, Self::scroll_to_item);
    }

    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_first_trash, |this, _| {
            this.scroll_to_top()
        });
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_last_trash, |this, _| {
            this.scroll_to_bottom()
        });
    }

    pub fn select_page_up(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, |s| s.select_page_up_trash(20), Self::scroll_to_item);
    }

    pub fn select_page_down(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, |s| s.select_page_down_trash(20), Self::scroll_to_item);
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.select_all_trash_items();
            cx.notify();
        });
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            if !state.trash_selection.is_empty() {
                state.clear_trash_selection();
                cx.notify();
            }
        });
        cx.notify();
    }

    fn on_row_click(
        &mut self,
        ix: usize,
        id: OsString,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        cx.stop_propagation();
        if event.button != MouseButton::Left {
            return;
        }
        self.app_state.update(cx, |state, cx| {
            if event.modifiers.shift {
                state.select_trash_range_to(ix, event.modifiers.secondary());
                cx.notify();
            } else if event.modifiers.secondary() {
                state.toggle_trash_item(id);
                cx.notify();
            } else if state.trash_selection.len() != 1 || !state.trash_selection.contains(&id) {
                state.select_trash_only(id);
                cx.notify();
            }
        });
        cx.notify();
    }

    /// Restores the currently selected trash items on the background executor.
    pub fn restore_selected(&mut self, cx: &mut Context<Self>) {
        if self.pending_restore {
            return;
        }
        let selected = self.app_state.read(cx).selected_trash_items();
        if selected.is_empty() {
            return;
        }

        self.pending_restore = true;
        self.op_generation = self.op_generation.wrapping_add(1);
        let gen = self.op_generation;
        cx.notify();

        let items_bg = selected.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { restore_trash_items_op(&items_bg) })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.op_generation != gen {
                    return;
                }
                this.pending_restore = false;
                this.app_state.update(cx, |state, cx| {
                    apply_restore_trash_outcome(state, &selected, &outcome);
                    cx.notify();
                });
                if let RestoreTrashOutcome::Conflict {
                    item_name,
                    destination,
                } = outcome
                {
                    cx.emit(TrashViewEvent::RestoreConflict {
                        item_name,
                        destination,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Emits [`TrashViewEvent::ConfirmEmptyTrash`] so [`super::MainView`] opens the confirmation modal.
    pub fn request_empty_trash(&mut self, cx: &mut Context<Self>) {
        if self.pending_restore {
            return;
        }
        let items = (*self.app_state.read(cx).trash_items).clone();
        if items.is_empty() {
            return;
        }
        cx.emit(TrashViewEvent::ConfirmEmptyTrash { items });
    }

    fn on_sort_column_click(&mut self, column: TrashSortColumn, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.toggle_trash_sort(column);
            cx.notify();
        });
        cx.notify();
    }

    fn on_scrollbar_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.scrollbar_state.can_scroll() {
            return;
        }
        self.scrollbar_state.is_dragging = true;
        self.scrollbar_state.drag_start_y = event.position.y;
        self.scrollbar_state.drag_start_offset = self.scrollbar_state.current_offset;

        let track_height = px(600.);
        let ratio = (event.position.y / track_height).clamp(0.0, 1.0);
        let max_offset = self
            .scrollbar_state
            .total_items
            .saturating_sub(self.scrollbar_state.visible_items);
        let target = ((ratio * max_offset as f32).round() as usize).min(max_offset);
        self.scrollbar_state.current_offset = target;

        if target >= max_offset {
            self.scroll_to_bottom();
        } else {
            self.scroll_handle
                .scroll_to_item_strict(target, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn on_scrollbar_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scrollbar_state.is_dragging = false;
        cx.notify();
    }

    fn on_scrollbar_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.scrollbar_state.is_dragging || !self.scrollbar_state.can_scroll() {
            return;
        }
        let track_height = px(600.);
        let thumb_ratio = self.scrollbar_state.thumb_ratio();
        let usable_track = track_height * (1.0 - thumb_ratio);
        if usable_track <= px(0.) {
            return;
        }
        let delta_y = event.position.y - self.scrollbar_state.drag_start_y;
        let delta_ratio = delta_y / usable_track;
        let max_offset = self
            .scrollbar_state
            .total_items
            .saturating_sub(self.scrollbar_state.visible_items);
        let delta_offset = (delta_ratio * max_offset as f32).round() as isize;
        let new_offset = (self.scrollbar_state.drag_start_offset as isize + delta_offset)
            .clamp(0, max_offset as isize) as usize;
        self.scrollbar_state.current_offset = new_offset;

        if new_offset >= max_offset {
            self.scroll_to_bottom();
        } else {
            self.scroll_handle
                .scroll_to_item_strict(new_offset, ScrollStrategy::Top);
        }
        cx.notify();
    }
}

impl Render for TrashView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let now = Instant::now();
        let current_top = self
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .logical_scroll_top()
            .0;

        let (
            items,
            selection,
            anchor_id,
            sort_column,
            sort_direction,
            trash_loading,
            reduced,
            focused_id,
        ) = {
            let state = self.app_state.read(cx);
            let focused_id = state.trash_selection_anchor.clone().or_else(|| {
                state
                    .selected_trash_index()
                    .and_then(|ix| state.trash_items.get(ix))
                    .map(|item| item.id.clone())
            });
            (
                state.trash_items.clone(),
                state.trash_selection.clone(),
                state.trash_selection_anchor.clone(),
                state.trash_sort_column,
                state.trash_sort_direction,
                state.trash_loading,
                is_reduced_motion(state.settings.motion_preference),
                focused_id,
            )
        };
        let _ = anchor_id;

        let item_count = items.len();
        let selected_count = selection.len();
        let visible_items = 25;
        self.scrollbar_state
            .set_metrics(item_count, visible_items, current_top);

        let view_focused = self.focus_handle.is_focused(window);
        let can_restore = selected_count > 0 && !trash_loading && !self.pending_restore;
        let can_empty = item_count > 0 && !trash_loading && !self.pending_restore;

        let header_hovers = [
            self.motion.step_scalar(
                TrashMotionKey::HeaderHover(0),
                0.0,
                if self.hovered_header == Some(0) {
                    1.0
                } else {
                    0.0
                },
                INSTANT,
                Curve::EaseOut,
                MotionKind::OpacityOrColor,
                now,
                reduced,
            ),
            self.motion.step_scalar(
                TrashMotionKey::HeaderHover(1),
                0.0,
                if self.hovered_header == Some(1) {
                    1.0
                } else {
                    0.0
                },
                INSTANT,
                Curve::EaseOut,
                MotionKind::OpacityOrColor,
                now,
                reduced,
            ),
            self.motion.step_scalar(
                TrashMotionKey::HeaderHover(2),
                0.0,
                if self.hovered_header == Some(2) {
                    1.0
                } else {
                    0.0
                },
                INSTANT,
                Curve::EaseOut,
                MotionKind::OpacityOrColor,
                now,
                reduced,
            ),
        ];

        let sort_indicator = |col: TrashSortColumn| -> &'static str {
            if sort_column == col {
                match sort_direction {
                    SortDirection::Ascending => " ▲",
                    SortDirection::Descending => " ▼",
                }
            } else {
                ""
            }
        };

        let items_clone = items.clone();
        let selection_clone: HashSet<OsString> = selection;
        let (row_selected, row_hover, row_text, row_muted) = (
            theme.selection,
            theme.surface_raised,
            theme.text,
            theme.text_muted,
        );
        let row_fonts = fonts.clone();
        let theme_for_rows = theme;

        let body: gpui::AnyElement = if item_count == 0 {
            div()
                .flex_1()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::S8)
                .text_body()
                .text_color(theme.text_muted)
                .child(animated_icon_sized(
                    IconName::Trash,
                    IconMotion::Rest,
                    size::ICON * 2.0,
                    theme.text_muted,
                ))
                .child(if trash_loading {
                    "Loading trash..."
                } else {
                    "Trash is empty"
                })
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .size_full()
                .overflow_hidden()
                .child(
                    div().relative().flex_1().h_full().child(
                        uniform_list(
                            "trash_entries",
                            item_count,
                            cx.processor(
                                move |this: &mut TrashView,
                                      range: std::ops::Range<usize>,
                                      _window,
                                      cx| {
                                    let mut rows: Vec<gpui::AnyElement> = Vec::new();
                                    for ix in range {
                                        if let Some(item) = items_clone.get(ix) {
                                            let id = item.id.clone();
                                            let is_selected = selection_clone.contains(&id);
                                            let is_focused_row =
                                                view_focused && focused_id.as_ref() == Some(&id);
                                            let is_hovered = this.hovered_id.as_ref() == Some(&id);
                                            let is_pressed = this.pressed_id.as_ref() == Some(&id);
                                            let name = item.display_name();
                                            let origin =
                                                item.original_parent.to_string_lossy().into_owned();
                                            let deleted_str = format_modified(item.deleted_at());

                                            let hover_t = this.motion.step_scalar(
                                                TrashMotionKey::Hover(id.clone()),
                                                0.0,
                                                if is_hovered { 1.0 } else { 0.0 },
                                                INSTANT,
                                                Curve::EaseOut,
                                                MotionKind::OpacityOrColor,
                                                now,
                                                reduced,
                                            );
                                            let press_t = this.motion.step_scalar(
                                                TrashMotionKey::Press(id.clone()),
                                                0.0,
                                                if is_pressed { 1.0 } else { 0.0 },
                                                INSTANT,
                                                Curve::Spring,
                                                MotionKind::MovementOrScale,
                                                now,
                                                reduced,
                                            );
                                            let select_t = this.motion.step_scalar(
                                                TrashMotionKey::SelectBg(id.clone()),
                                                if is_selected { 1.0 } else { 0.0 },
                                                if is_selected { 1.0 } else { 0.0 },
                                                INSTANT,
                                                Curve::EaseOut,
                                                MotionKind::OpacityOrColor,
                                                now,
                                                reduced,
                                            );
                                            let focus_t = this.motion.step_scalar(
                                                TrashMotionKey::FocusRing(id.clone()),
                                                0.0,
                                                if is_focused_row { 1.0 } else { 0.0 },
                                                INSTANT,
                                                Curve::Spring,
                                                MotionKind::OpacityOrColor,
                                                now,
                                                reduced,
                                            );

                                            let base_hover =
                                                color_lerp(transparent_black(), row_hover, hover_t);
                                            let bg = color_lerp(base_hover, row_selected, select_t);
                                            let border_col = focus_ring_color(
                                                transparent_black(),
                                                focus_t,
                                                &theme_for_rows,
                                            );
                                            let icon_scale = if reduced {
                                                1.0
                                            } else {
                                                lerp_f32(1.0, PRESS_SCALE, press_t)
                                            };

                                            let id_hover = id.clone();
                                            let id_press = id.clone();
                                            let id_click = id.clone();

                                            rows.push(
                                                div()
                                                    .id(ix)
                                                    .flex()
                                                    .flex_row()
                                                    .w_full()
                                                    .h(size::ROW)
                                                    .px(space::S8)
                                                    .gap(space::S8)
                                                    .rounded(radius::CONTROL)
                                                    .border_1()
                                                    .border_color(border_col)
                                                    .bg(bg)
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .text_body()
                                                    .text_color(row_text)
                                                    .on_hover(cx.listener(
                                                        move |this, hovered, _, cx| {
                                                            if *hovered {
                                                                this.hovered_id =
                                                                    Some(id_hover.clone());
                                                                cx.notify();
                                                            } else if this.hovered_id.as_ref()
                                                                == Some(&id_hover)
                                                            {
                                                                this.hovered_id = None;
                                                                this.pressed_id = None;
                                                                cx.notify();
                                                            }
                                                        },
                                                    ))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(
                                                            move |this,
                                                                  event: &MouseDownEvent,
                                                                  window,
                                                                  cx| {
                                                                this.pressed_id =
                                                                    Some(id_press.clone());
                                                                this.on_row_click(
                                                                    ix,
                                                                    id_click.clone(),
                                                                    event,
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        ),
                                                    )
                                                    .on_mouse_up(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if this.pressed_id.is_some() {
                                                                this.pressed_id = None;
                                                                cx.notify();
                                                            }
                                                        }),
                                                    )
                                                    .on_mouse_up_out(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if this.pressed_id.is_some() {
                                                                this.pressed_id = None;
                                                                cx.notify();
                                                            }
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .items_center()
                                                            .gap(space::S8)
                                                            .child(animated_icon_sized(
                                                                IconName::File,
                                                                IconMotion::Rest,
                                                                size::ICON * icon_scale,
                                                                row_text,
                                                            ))
                                                            .child(div().ellipsize().child(name)),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .text_data(&row_fonts)
                                                            .text_color(if is_selected {
                                                                row_text
                                                            } else {
                                                                row_muted
                                                            })
                                                            .ellipsize()
                                                            .child(origin),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(DATE_COLUMN)
                                                            .text_data(&row_fonts)
                                                            .text_color(if is_selected {
                                                                row_text
                                                            } else {
                                                                row_muted
                                                            })
                                                            .child(deleted_str),
                                                    )
                                                    .into_any_element(),
                                            );
                                        }
                                    }
                                    rows
                                },
                            ),
                        )
                        .track_scroll(self.scroll_handle.clone())
                        .size_full(),
                    ),
                )
                .child(
                    div()
                        .w(SCROLLBAR_WIDTH)
                        .h_full()
                        .border_l_1()
                        .border_color(theme.border)
                        .relative()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(Self::on_scrollbar_mouse_down),
                        )
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::on_scrollbar_mouse_up))
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(Self::on_scrollbar_mouse_up),
                        )
                        .on_mouse_move(cx.listener(Self::on_scrollbar_mouse_move))
                        .child(
                            div()
                                .w_full()
                                .h(relative(self.scrollbar_state.thumb_ratio()))
                                .top(relative(
                                    self.scrollbar_state.scroll_ratio()
                                        * (1.0 - self.scrollbar_state.thumb_ratio()),
                                ))
                                .rounded_sm()
                                .bg(if self.scrollbar_state.is_dragging {
                                    theme.text_muted
                                } else {
                                    theme.border_strong
                                })
                                .hover(|s| s.bg(theme.text_muted))
                                .cursor(CursorStyle::Arrow),
                        ),
                )
                .into_any_element()
        };

        self.motion.prune_settled(now, reduced);
        request_frame_if_active(self.motion.has_active(now, reduced), window);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .key_context("TrashView")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| this.select_all(cx)))
            .on_action(
                cx.listener(|this, _: &ClearSelection, _window, cx| this.clear_selection(cx)),
            )
            .on_action(cx.listener(|this, _: &SelectNext, _window, cx| this.select_next(cx)))
            .on_action(
                cx.listener(|this, _: &SelectPrevious, _window, cx| this.select_previous(cx)),
            )
            .on_action(cx.listener(|this, _: &SelectFirst, _window, cx| this.select_first(cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _window, cx| this.select_last(cx)))
            .on_action(cx.listener(|this, _: &PageUp, _window, cx| this.select_page_up(cx)))
            .on_action(cx.listener(|this, _: &PageDown, _window, cx| this.select_page_down(cx)))
            .on_action(cx.listener(|this, _: &GoBack, _window, cx| {
                this.app_state.update(cx, |state, _| {
                    state.exit_trash();
                });
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Refresh, _window, cx| {
                this.app_state.update(cx, |state, _| {
                    state.reload_trash();
                });
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "down" => this.select_next(cx),
                    "up" => this.select_previous(cx),
                    "home" => this.select_first(cx),
                    "end" => this.select_last(cx),
                    "pageup" => this.select_page_up(cx),
                    "pagedown" => this.select_page_down(cx),
                    "backspace" => {
                        this.app_state.update(cx, |state, _| state.exit_trash());
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            // Action bar
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(size::TOOLBAR)
                    .px(space::S8)
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S8)
                            .child(animated_icon_sized(
                                IconName::Trash,
                                IconMotion::Rest,
                                size::ICON,
                                theme.text,
                            ))
                            .child(
                                div()
                                    .text_body()
                                    .text_color(theme.text)
                                    .child(format!("Trash ({item_count})")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S8)
                            .child(
                                div()
                                    .id("trash-restore-btn")
                                    .px(space::S8)
                                    .py(space::S4)
                                    .rounded(radius::CONTROL)
                                    .bg(if can_restore {
                                        theme.accent
                                    } else {
                                        theme.surface_raised
                                    })
                                    .text_label()
                                    .text_color(if can_restore {
                                        theme.on_accent
                                    } else {
                                        theme.text_muted
                                    })
                                    .cursor(if can_restore {
                                        CursorStyle::PointingHand
                                    } else {
                                        CursorStyle::Arrow
                                    })
                                    .child("Restore")
                                    .when(can_restore, |btn| {
                                        btn.on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.restore_selected(cx);
                                            }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("trash-empty-btn")
                                    .flex()
                                    .items_center()
                                    .gap(space::S4)
                                    .px(space::S8)
                                    .py(space::S4)
                                    .rounded(radius::CONTROL)
                                    .bg(if can_empty {
                                        theme.danger
                                    } else {
                                        theme.surface_raised
                                    })
                                    .text_label()
                                    .text_color(if can_empty {
                                        theme.on_accent
                                    } else {
                                        theme.text_muted
                                    })
                                    .cursor(if can_empty {
                                        CursorStyle::PointingHand
                                    } else {
                                        CursorStyle::Arrow
                                    })
                                    .child(animated_icon_sized(
                                        IconName::Trash,
                                        IconMotion::Rest,
                                        size::ICON,
                                        if can_empty {
                                            theme.on_accent
                                        } else {
                                            theme.text_muted
                                        },
                                    ))
                                    .child("Empty Trash")
                                    .when(can_empty, |btn| {
                                        btn.on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.request_empty_trash(cx);
                                            }),
                                        )
                                    }),
                            ),
                    ),
            )
            // Column headers
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(size::ROW)
                    .pl(space::S8)
                    .pr(space::S8 + SCROLLBAR_WIDTH)
                    .gap(space::S8)
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.surface_raised)
                    .text_color(theme.text_muted)
                    .text_label()
                    .child(
                        div()
                            .id("trash-col-sort-name")
                            .flex_1()
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .text_color(color_lerp(theme.text_muted, theme.text, header_hovers[0]))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                this.hovered_header = if *hovered { Some(0) } else { None };
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(TrashSortColumn::Name, cx);
                                }),
                            )
                            .child(format!("Name{}", sort_indicator(TrashSortColumn::Name))),
                    )
                    .child(
                        div()
                            .id("trash-col-sort-origin")
                            .flex_1()
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .text_color(color_lerp(theme.text_muted, theme.text, header_hovers[1]))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                this.hovered_header = if *hovered { Some(1) } else { None };
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(
                                        TrashSortColumn::OriginalLocation,
                                        cx,
                                    );
                                }),
                            )
                            .child(format!(
                                "Original Location{}",
                                sort_indicator(TrashSortColumn::OriginalLocation)
                            )),
                    )
                    .child(
                        div()
                            .id("trash-col-sort-date")
                            .w(DATE_COLUMN)
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .text_color(color_lerp(theme.text_muted, theme.text, header_hovers[2]))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                this.hovered_header = if *hovered { Some(2) } else { None };
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(TrashSortColumn::DeletedAt, cx);
                                }),
                            )
                            .child(format!(
                                "Date Deleted{}",
                                sort_indicator(TrashSortColumn::DeletedAt)
                            )),
                    ),
            )
            .child(body)
    }
}
