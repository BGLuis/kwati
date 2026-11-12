//! File list view (owned by guinav-worker for #7, #12).
//!
//! Uses `uniform_list` to render only visible rows, with support for columns
//! (name, size, date), sort headers, and scrollbar synchronization.

use gpui::{
    div, prelude::*, px, relative, rgb, rgba, uniform_list, Context, CursorStyle, Entity,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollStrategy,
    UniformListScrollHandle, Window,
};
use kwati_core::{Entry, Location};

use crate::components::ScrollbarState;
use crate::state::AppState;
use crate::theme::Theme;

/// View rendering directory entries in a windowed list with a draggable scrollbar.
pub struct ListView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
    pub scroll_handle: UniformListScrollHandle,
    pub scrollbar_state: ScrollbarState,
    pub row_height: Pixels,
}

impl ListView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
            scroll_handle: UniformListScrollHandle::new(),
            scrollbar_state: ScrollbarState::default(),
            row_height: px(26.),
        }
    }

    /// Scrolls to bring the item at index `ix` into view.
    pub fn scroll_to_item(&self, ix: usize) {
        self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
    }

    /// Scrolls strictly to the top item.
    pub fn scroll_to_top(&self) {
        self.scroll_handle
            .scroll_to_item_strict(0, ScrollStrategy::Top);
    }

    /// Scrolls strictly to the bottom item.
    pub fn scroll_to_bottom(&self) {
        let total = self.scrollbar_state.total_items;
        if total > 0 {
            self.scroll_handle
                .scroll_to_item_strict(total.saturating_sub(1), ScrollStrategy::Bottom);
        }
    }

    pub fn on_entry_click(
        &mut self,
        entry: Entry,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Left && event.click_count == 2 {
            // Double click: open folder
            if entry.is_dir() {
                self.app_state.update(cx, |state, _cx| {
                    state.navigate_to(entry.location());
                });
            }
        } else if event.button == MouseButton::Left {
            // Single click: select entry
            self.app_state.update(cx, |state, _cx| {
                state.selection.select_only(entry.location());
            });
        }
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

impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let entries = state.entries.clone();
        let selection = state.selection.clone();
        let entry_count = entries.len();

        let current_top = self
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .logical_scroll_top()
            .0;
        let visible_items = 25;
        self.scrollbar_state
            .set_metrics(entry_count, visible_items, current_top);

        let entries_clone = entries.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.bg_content)
            // Column Headers
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(28.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.bg_header)
                    .text_color(self.theme.text_muted)
                    .text_size(px(12.))
                    .child(div().flex_1().child("Name"))
                    .child(div().w(px(100.)).child("Size"))
                    .child(div().w(px(150.)).child("Modified")),
            )
            // Row list + Scrollbar container
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "directory_entries",
                            entry_count,
                            cx.processor(move |_this: &mut ListView, range, _window, cx| {
                                let mut items: Vec<gpui::AnyElement> = Vec::new();
                                for ix in range {
                                    if let Some(entry) = entries_clone.get(ix) {
                                        let entry: &Entry = entry;
                                        let loc: Location = entry.location();
                                        let is_selected = selection.contains(&loc);
                                        let is_dir = entry.is_dir();
                                        let name = entry.display_name();
                                        let entry_for_click = entry.clone();

                                        items.push(
                                            div()
                                                .id(ix)
                                                .flex()
                                                .flex_row()
                                                .h(px(26.))
                                                .px_3()
                                                .items_center()
                                                .cursor_pointer()
                                                .bg(if is_selected {
                                                    rgba(0x89b4fa26)
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .hover(|s| s.bg(rgba(0xffffff0d)))
                                                .text_size(px(13.))
                                                .text_color(if is_dir {
                                                    rgb(0x89b4fa)
                                                } else {
                                                    rgb(0xcdd6f4)
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        move |this,
                                                              event: &MouseDownEvent,
                                                              window,
                                                              cx| {
                                                            this.on_entry_click(
                                                                entry_for_click.clone(),
                                                                event,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .flex()
                                                        .items_center()
                                                        .gap_2()
                                                        .child(if is_dir { "📁" } else { "📄" })
                                                        .child(name),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(100.))
                                                        .text_color(rgb(0x6c7086))
                                                        .child(if is_dir { "--" } else { "" }),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(150.))
                                                        .text_color(rgb(0x6c7086))
                                                        .child(""),
                                                )
                                                .into_any_element(),
                                        );
                                    }
                                }
                                items
                            }),
                        )
                        .track_scroll(self.scroll_handle.clone())
                        .flex_1()
                        .h_full(),
                    )
                    .child(
                        // Scrollbar track & thumb
                        div()
                            .w(px(10.))
                            .h_full()
                            .bg(rgba(0x00000014))
                            .border_l_1()
                            .border_color(self.theme.border)
                            .relative()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::on_scrollbar_mouse_down),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(Self::on_scrollbar_mouse_up),
                            )
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
                                        rgba(0xffffff80)
                                    } else {
                                        rgba(0xffffff40)
                                    })
                                    .hover(|s| s.bg(rgba(0xffffff70)))
                                    .cursor(CursorStyle::Arrow),
                            ),
                    ),
            )
    }
}
