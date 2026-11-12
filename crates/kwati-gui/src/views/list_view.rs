//! File list view (owned by guinav-worker for #7, #12).
//!
//! Uses `uniform_list` to render only visible rows, with support for columns
//! (name, size, date), sort headers, and scrollbar synchronization.

use gpui::{
    div, prelude::*, px, relative, rgb, rgba, uniform_list, Context, CursorStyle, Entity,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollStrategy, UniformListScrollHandle, Window,
};
use kwati_core::{Entry, Location, SortColumn, SortDirection};

use crate::actions::navigation::{
    GoBack, GoForward, GoUp, OpenSelected, PageDown, PageUp, Refresh, SelectFirst, SelectLast,
    SelectNext, SelectPrevious, ToggleHidden,
};
use crate::components::ScrollbarState;
use crate::state::AppState;
use crate::theme::Theme;

/// Formats a file size in human-readable units (B, KB, MB, GB, TB).
pub fn format_size(bytes: Option<u64>, is_dir: bool) -> String {
    if is_dir {
        return "--".to_string();
    }
    let Some(b) = bytes else {
        return "--".to_string();
    };
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if b < KB {
        format!("{} B", b)
    } else if b < MB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else if b < GB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b < TB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else {
        format!("{:.1} TB", b as f64 / TB as f64)
    }
}

/// Formats a `SystemTime` timestamp into UTC `YYYY-MM-DD HH:MM`.
pub fn format_modified(time: Option<std::time::SystemTime>) -> String {
    let Some(time) = time else {
        return "--".to_string();
    };
    let Ok(duration) = time.duration_since(std::time::UNIX_EPOCH) else {
        return "--".to_string();
    };
    let secs = duration.as_secs();
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;

    // Civil calendar algorithm
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hours, minutes)
}

/// View rendering directory entries in a windowed list with a draggable scrollbar.
pub struct ListView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
    pub focus_handle: FocusHandle,
    pub scroll_handle: UniformListScrollHandle,
    pub scrollbar_state: ScrollbarState,
    pub row_height: Pixels,
}

impl Focusable for ListView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ListView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
            focus_handle: cx.focus_handle(),
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
            if entry.is_dir() {
                // Double click on folder enters it and launches no application
                self.app_state.update(cx, |state, _cx| {
                    state.navigate_to(entry.location());
                });
            } else {
                // Double click on file opens with system default application
                cx.open_with_system(entry.path());
            }
        } else if event.button == MouseButton::Left {
            // Single click: select entry
            self.app_state.update(cx, |state, _cx| {
                state.selection.select_only(entry.location());
            });
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let new_ix = self.app_state.update(cx, |state, _cx| state.select_next());
        if let Some(ix) = new_ix {
            self.scroll_to_item(ix);
        }
        cx.notify();
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let new_ix = self
            .app_state
            .update(cx, |state, _cx| state.select_previous());
        if let Some(ix) = new_ix {
            self.scroll_to_item(ix);
        }
        cx.notify();
    }

    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        let new_ix = self.app_state.update(cx, |state, _cx| state.select_first());
        if new_ix.is_some() {
            self.scroll_to_top();
        }
        cx.notify();
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        let new_ix = self.app_state.update(cx, |state, _cx| state.select_last());
        if new_ix.is_some() {
            self.scroll_to_bottom();
        }
        cx.notify();
    }

    pub fn select_page_up(&mut self, cx: &mut Context<Self>) {
        let new_ix = self
            .app_state
            .update(cx, |state, _cx| state.select_page_up(20));
        if let Some(ix) = new_ix {
            self.scroll_to_item(ix);
        }
        cx.notify();
    }

    pub fn select_page_down(&mut self, cx: &mut Context<Self>) {
        let new_ix = self
            .app_state
            .update(cx, |state, _cx| state.select_page_down(20));
        if let Some(ix) = new_ix {
            self.scroll_to_item(ix);
        }
        cx.notify();
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let entry_opt = self.app_state.read(cx).selected_entry().cloned();
        if let Some(entry) = entry_opt {
            if entry.is_dir() {
                self.app_state.update(cx, |state, _cx| {
                    state.navigate_to(entry.location());
                });
            } else {
                cx.open_with_system(entry.path());
            }
        }
        cx.notify();
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| state.go_up());
        cx.notify();
    }

    fn on_sort_column_click(&mut self, column: SortColumn, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.toggle_sort(column);
        });
        cx.notify();
    }

    fn on_toggle_hidden_click(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, _cx| {
            state.toggle_hidden();
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

impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let entries = state.entries.clone();
        let selection = state.selection.clone();
        let entry_count = entries.len();
        let sort_column = state.sort_column;
        let sort_direction = state.sort_direction;
        let show_hidden = state.show_hidden;

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

        let sort_indicator = |col: SortColumn| -> &'static str {
            if sort_column == col {
                match sort_direction {
                    SortDirection::Ascending => " ▲",
                    SortDirection::Descending => " ▼",
                }
            } else {
                ""
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.bg_content)
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(|this, _: &SelectNext, _window, cx| this.select_next(cx)))
            .on_action(
                cx.listener(|this, _: &SelectPrevious, _window, cx| this.select_previous(cx)),
            )
            .on_action(cx.listener(|this, _: &SelectFirst, _window, cx| this.select_first(cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _window, cx| this.select_last(cx)))
            .on_action(cx.listener(|this, _: &PageUp, _window, cx| this.select_page_up(cx)))
            .on_action(cx.listener(|this, _: &PageDown, _window, cx| this.select_page_down(cx)))
            .on_action(cx.listener(|this, _: &OpenSelected, _window, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &GoUp, _window, cx| this.go_up(cx)))
            .on_action(cx.listener(|this, _: &GoBack, _window, cx| {
                this.app_state.update(cx, |state, _cx| {
                    state.go_back();
                });
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoForward, _window, cx| {
                this.app_state.update(cx, |state, _cx| {
                    state.go_forward();
                });
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &ToggleHidden, _window, cx| this.on_toggle_hidden_click(cx)),
            )
            .on_action(cx.listener(|this, _: &Refresh, _window, cx| {
                this.app_state.update(cx, |state, _cx| {
                    state.reload_entries();
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
                    "enter" => this.open_selected(cx),
                    "backspace" => this.go_up(cx),
                    "." => this.on_toggle_hidden_click(cx),
                    "h" if event.keystroke.modifiers.control => this.on_toggle_hidden_click(cx),
                    _ => {}
                }
            }))
            // Column Headers Bar
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
                    // Name column header
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(self.theme.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(SortColumn::Name, cx)
                                }),
                            )
                            .child(format!("Name{}", sort_indicator(SortColumn::Name))),
                    )
                    // Size column header
                    .child(
                        div()
                            .w(px(100.))
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(self.theme.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(SortColumn::Size, cx)
                                }),
                            )
                            .child(format!("Size{}", sort_indicator(SortColumn::Size))),
                    )
                    // Modified column header
                    .child(
                        div()
                            .w(px(150.))
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(self.theme.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.on_sort_column_click(SortColumn::Date, cx)
                                }),
                            )
                            .child(format!("Modified{}", sort_indicator(SortColumn::Date))),
                    )
                    // Hidden toggle button
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor(CursorStyle::PointingHand)
                            .bg(if show_hidden {
                                rgba(0x89b4fa33)
                            } else {
                                rgba(0x00000000)
                            })
                            .text_color(if show_hidden {
                                rgb(0x89b4fa)
                            } else {
                                self.theme.text_muted
                            })
                            .hover(|s| s.bg(rgba(0xffffff14)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.on_toggle_hidden_click(cx)),
                            )
                            .child(if show_hidden {
                                "Hidden: On"
                            } else {
                                "Hidden: Off"
                            }),
                    ),
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
                                        let is_hidden = entry.is_hidden;
                                        let name = entry.display_name();
                                        let size_str = format_size(entry.size, is_dir);
                                        let date_str = format_modified(entry.modified);
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
                                                .text_color(if is_hidden {
                                                    rgb(0x6c7086)
                                                } else if is_dir {
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
                                                        .child(size_str),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(150.))
                                                        .text_color(rgb(0x6c7086))
                                                        .child(date_str),
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
