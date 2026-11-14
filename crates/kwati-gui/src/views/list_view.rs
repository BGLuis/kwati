//! File list view (owned by guinav-worker for #7, #12).
//!
//! Uses `uniform_list` to render only visible rows, with support for columns
//! (name, size, date), sort headers, and scrollbar synchronization.

use std::collections::HashSet;

use gpui::{
    div, prelude::*, px, relative, transparent_black, uniform_list, Context, CursorStyle, Entity,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollStrategy, UniformListScrollHandle, Window,
};
use kwati_core::{Entry, Location, SortColumn, SortDirection};

use crate::actions::navigation::{
    ClearSelection, GoBack, GoForward, GoUp, OpenSelected, PageDown, PageUp, Refresh, SelectAll,
    SelectFirst, SelectLast, SelectNext, SelectPrevious, ToggleHidden,
};
use crate::components::{icon, Ellipsize, IconName, ScrollbarState};
use crate::state::{entry_location, AppState};
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

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

/// Active rubber-band drag rectangle in window coordinates.
#[derive(Clone, Debug)]
struct RubberBand {
    start: Point<Pixels>,
    current: Point<Pixels>,
    base_selection: Option<HashSet<Location>>,
}

impl RubberBand {
    fn rect(&self) -> (Pixels, Pixels, Pixels, Pixels) {
        let left = self.start.x.min(self.current.x);
        let top = self.start.y.min(self.current.y);
        let right = self.start.x.max(self.current.x);
        let bottom = self.start.y.max(self.current.y);
        (left, top, right, bottom)
    }

    fn is_drag(&self) -> bool {
        let (left, top, right, bottom) = self.rect();
        (right - left) > px(2.) || (bottom - top) > px(2.)
    }
}

/// View rendering directory entries in a windowed list with a draggable scrollbar.
pub struct ListView {
    pub app_state: Entity<AppState>,
    pub focus_handle: FocusHandle,
    pub scroll_handle: UniformListScrollHandle,
    pub scrollbar_state: ScrollbarState,
    pub row_height: Pixels,
    rubber_band: Option<RubberBand>,
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
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            scrollbar_state: ScrollbarState::default(),
            row_height: size::ROW,
            rubber_band: None,
        }
    }

    fn effective_row_height(&self, entry_count: usize) -> Pixels {
        if entry_count == 0 {
            return self.row_height;
        }
        self.scroll_handle
            .0
            .borrow()
            .last_item_size
            .map(|s| s.contents.height / (entry_count as f32))
            .filter(|h| *h > px(0.))
            .unwrap_or(self.row_height)
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

    fn update_selection(
        &mut self,
        cx: &mut Context<Self>,
        select: impl FnOnce(&mut AppState) -> Option<usize>,
        after: impl FnOnce(&Self, usize),
    ) {
        let new_ix = self.app_state.update(cx, |state, cx| {
            let prev = state.selected_index();
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

    pub fn on_entry_click(
        &mut self,
        ix: usize,
        entry: Entry,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        cx.stop_propagation();

        if event.button != MouseButton::Left {
            return;
        }

        if event.click_count == 2 && !event.modifiers.shift && !event.modifiers.secondary() {
            self.open_entry(entry, cx);
            return;
        }

        self.app_state.update(cx, |state, cx| {
            let location = state.location_of(&entry);
            if event.modifiers.shift {
                state.select_range_to(ix, event.modifiers.secondary());
                cx.notify();
            } else if event.modifiers.secondary() {
                state.toggle_select_location(location);
                cx.notify();
            } else if state.selection.len() != 1 || !state.selection.contains(&location) {
                state.selection.select_only(location);
                cx.notify();
            }
        });
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.select_all_entries();
            cx.notify();
        });
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            if !state.selection.is_empty() {
                state.clear_selection();
                cx.notify();
            }
        });
        cx.notify();
    }

    fn row_index_at(&self, position: Point<Pixels>, entry_count: usize) -> Option<usize> {
        let state = self.scroll_handle.0.borrow();
        let list_bounds = state.base_handle.bounds();
        let scroll_offset = state.base_handle.offset();
        drop(state);
        let row_h = self.effective_row_height(entry_count);
        if position.x < list_bounds.left()
            || position.x > list_bounds.right()
            || position.y < list_bounds.top()
            || position.y > list_bounds.bottom()
        {
            return None;
        }
        let rel_y = position.y - list_bounds.top() - scroll_offset.y;
        if rel_y < px(0.) || row_h <= px(0.) {
            return None;
        }
        let ix = (rel_y / row_h).floor() as usize;
        (ix < entry_count).then_some(ix)
    }

    fn on_list_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        let entry_count = self.app_state.read(cx).entries.len();
        if self.row_index_at(event.position, entry_count).is_some() {
            return;
        }
        self.focus_handle.focus(window);
        let base_selection = if event.modifiers.secondary() {
            Some(self.app_state.read(cx).selection.locations().clone())
        } else {
            self.app_state.update(cx, |state, cx| {
                if !state.selection.is_empty() {
                    state.clear_selection();
                    cx.notify();
                }
            });
            None
        };
        self.rubber_band = Some(RubberBand {
            start: event.position,
            current: event.position,
            base_selection,
        });
        cx.notify();
    }

    fn on_list_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ref mut rb) = self.rubber_band else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.rubber_band = None;
            cx.notify();
            return;
        }
        rb.current = event.position;
        if !rb.is_drag() {
            return;
        }

        let (left, top, right, bottom) = rb.rect();
        let base_selection = rb.base_selection.clone();
        let (list_bounds, scroll_offset) = {
            let state = self.scroll_handle.0.borrow();
            (state.base_handle.bounds(), state.base_handle.offset())
        };
        let entry_count = self.app_state.read(cx).entries.len();
        let row_h = self.effective_row_height(entry_count);

        if list_bounds.size.width > px(0.)
            && right > list_bounds.left()
            && left < list_bounds.right()
        {
            self.app_state.update(cx, |state, cx| {
                let touched = (0..state.entries.len()).filter(|&ix| {
                    let row_top = list_bounds.top() + row_h * (ix as f32) + scroll_offset.y;
                    let row_bottom = row_top + row_h;
                    row_bottom > top && row_top < bottom
                });
                state.select_indices(touched, base_selection.as_ref());
                cx.notify();
            });
        }
        cx.notify();
    }

    fn on_list_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rubber_band.take().is_some() {
            cx.notify();
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_next, Self::scroll_to_item);
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_previous, Self::scroll_to_item);
    }

    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_first, |this, _| this.scroll_to_top());
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_last, |this, _| this.scroll_to_bottom());
    }

    pub fn select_page_up(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, |s| s.select_page_up(20), Self::scroll_to_item);
    }

    pub fn select_page_down(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, |s| s.select_page_down(20), Self::scroll_to_item);
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let entry_opt = self.app_state.read(cx).selected_entry().cloned();
        if let Some(entry) = entry_opt {
            self.open_entry(entry, cx);
        }
        cx.notify();
    }

    /// Enters a folder, or opens a local file with the system's default application. Remote
    /// files cannot be opened yet, so they get a notice instead.
    fn open_entry(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let is_remote = self.app_state.read(cx).is_remote();
        if entry.is_dir() {
            self.app_state.update(cx, |state, _cx| {
                let location = state.location_of(&entry);
                state.navigate_to(location);
            });
        } else if is_remote {
            self.app_state.update(cx, |state, _cx| {
                state.set_status(REMOTE_FILE_NOTICE);
            });
        } else {
            cx.open_with_system(entry.path());
        }
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

pub const REMOTE_FILE_NOTICE: &str = "Files on remote locations cannot be opened or previewed yet";

const SIZE_COLUMN: Pixels = px(72.);
const DATE_COLUMN: Pixels = px(128.);
const HIDDEN_TOGGLE_COLUMN: Pixels = px(80.);
const SCROLLBAR_WIDTH: Pixels = px(10.);

impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let state = self.app_state.read(cx);
        let entries = state.entries.clone();
        let selection = state.selection.clone();
        let current = state.current_location.clone();
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
        let (row_selected, row_hover, row_text, row_muted, row_accent) = (
            theme.selection,
            theme.surface_raised,
            theme.text,
            theme.text_muted,
            theme.accent,
        );
        let row_fonts = fonts.clone();

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
            .bg(theme.surface)
            .key_context("ListView")
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
                    // Name column header
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(theme.text))
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
                            .w(SIZE_COLUMN)
                            .flex()
                            .justify_end()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(theme.text))
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
                            .w(DATE_COLUMN)
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|s| s.text_color(theme.text))
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
                            .w(HIDDEN_TOGGLE_COLUMN)
                            .flex()
                            .justify_center()
                            .rounded(radius::CONTROL)
                            .cursor(CursorStyle::PointingHand)
                            .bg(if show_hidden {
                                theme.selection
                            } else {
                                transparent_black().into()
                            })
                            .text_color(if show_hidden {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .hover(|s| s.bg(theme.surface_raised))
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
                        div()
                            .relative()
                            .flex_1()
                            .h_full()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::on_list_mouse_down),
                            )
                            .on_mouse_move(cx.listener(Self::on_list_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_list_mouse_up))
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(Self::on_list_mouse_up),
                            )
                            .child(
                                uniform_list(
                                    "directory_entries",
                                    entry_count,
                                    cx.processor(move |_this: &mut ListView, range, _window, cx| {
                                        let mut items: Vec<gpui::AnyElement> = Vec::new();
                                        for ix in range {
                                            if let Some(entry) = entries_clone.get(ix) {
                                                let entry: &Entry = entry;
                                                let loc: Location =
                                                    entry_location(&current, entry);
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
                                                        .w_full()
                                                        .h(size::ROW)
                                                        .px(space::S8)
                                                        .gap(space::S8)
                                                        .rounded(radius::CONTROL)
                                                        .items_center()
                                                        .cursor_pointer()
                                                        .when(is_selected, |s| s.bg(row_selected))
                                                        .when(!is_selected, |s| {
                                                            s.hover(|s| s.bg(row_hover))
                                                        })
                                                        .text_body()
                                                        .text_color(if is_hidden && !is_selected {
                                                            row_muted
                                                        } else {
                                                            row_text
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(
                                                                move |this,
                                                                      event: &MouseDownEvent,
                                                                      window,
                                                                      cx| {
                                                                    this.on_entry_click(
                                                                        ix,
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
                                                                .min_w_0()
                                                                .overflow_hidden()
                                                                .whitespace_nowrap()
                                                                .text_ellipsis()
                                                                .flex()
                                                                .items_center()
                                                                .gap(space::S8)
                                                                .child(if is_dir {
                                                                    icon(IconName::Folder)
                                                                        .text_color(row_accent)
                                                                } else {
                                                                    icon(IconName::File).text_color(
                                                                        if is_hidden && !is_selected
                                                                        {
                                                                            row_muted
                                                                        } else {
                                                                            row_text
                                                                        },
                                                                    )
                                                                })
                                                                .child(div().ellipsize().child(name)),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(SIZE_COLUMN)
                                                                .flex()
                                                                .justify_end()
                                                                .text_data(&row_fonts)
                                                                .text_color(if is_selected {
                                                                    row_text
                                                                } else {
                                                                    row_muted
                                                                })
                                                                .child(size_str),
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
                                                                .child(date_str),
                                                        )
                                                        .child(div().w(HIDDEN_TOGGLE_COLUMN))
                                                        .into_any_element(),
                                                );
                                            }
                                        }
                                        items
                                    }),
                                )
                                .track_scroll(self.scroll_handle.clone())
                                .size_full(),
                            )
                            .when_some(
                                self.rubber_band.as_ref().filter(|rb| rb.is_drag()),
                                |container, rb| {
                                    let list_bounds =
                                        self.scroll_handle.0.borrow().base_handle.bounds();
                                    let (left, top, right, bottom) = rb.rect();
                                    let rel_left = (left - list_bounds.left()).max(px(0.));
                                    let rel_top = (top - list_bounds.top()).max(px(0.));
                                    let rel_right =
                                        (right - list_bounds.left()).min(list_bounds.size.width);
                                    let rel_bottom = (bottom - list_bounds.top())
                                        .min(list_bounds.size.height);
                                    let w = (rel_right - rel_left).max(px(0.));
                                    let h = (rel_bottom - rel_top).max(px(0.));
                                    let mut fill = theme.accent;
                                    fill.a = 0.16;
                                    container.child(
                                        div()
                                            .absolute()
                                            .left(rel_left)
                                            .top(rel_top)
                                            .w(w)
                                            .h(h)
                                            .border_1()
                                            .border_color(theme.accent)
                                            .bg(fill)
                                            .rounded(px(2.)),
                                    )
                                },
                            ),
                    )
                    .child(
                        // Scrollbar track & thumb
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
                                        theme.text_muted
                                    } else {
                                        theme.border_strong
                                    })
                                    .hover(|s| s.bg(theme.text_muted))
                                    .cursor(CursorStyle::Arrow),
                            ),
                    ),
            )
    }
}
