//! File list view (owned by guinav-worker for #7, #12).
//!
//! Uses `uniform_list` to render only visible rows, with support for columns
//! (name, size, date), sort headers, and scrollbar synchronization.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui::{
    div, point, prelude::*, px, relative, transparent_black, uniform_list, Context, CursorStyle,
    Div, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollStrategy, Stateful,
    UniformListScrollHandle, Window,
};
use kwati_core::{Entry, Location, SortColumn, SortDirection, ViewMode};

use crate::actions::navigation::{
    ClearSelection, GoBack, GoForward, GoUp, OpenContextMenu, OpenSelected, PageDown, PageUp,
    Refresh, SelectAll, SelectFirst, SelectLast, SelectNext, SelectPrevious, ToggleHidden,
};
use crate::actions::operations::get_location_clipboard;
use crate::components::interactive::focus_ring_color;
use crate::components::{animated_icon_sized, Ellipsize, IconMotion, IconName, ScrollbarState};
use crate::motion::{
    color_lerp, is_reduced_motion, lerp_f32, request_frame_if_active, Curve, MotionKind,
    MotionTracker, DELETE_THROW, DELETE_THROW_PX, INSTANT, LIST_CASCADE_CAP, LIST_CASCADE_STEP,
    LIST_ENTER_OFFSET_PX, PRESS_SCALE, ROW_INSERT_START_SCALE, SETTLE,
};
use crate::state::{entry_location, AppState, EntryChangeOrigin, NavigationDirection};
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ListMotionKey {
    Hover(Location),
    Press(Location),
    SelectBg(Location),
    FocusRing(Location),
    CutFade(Location),
    InsertEnter(Location),
    FolderOpen(Location),
    HeaderHover(u8),
}

#[derive(Clone, Debug)]
struct ExitingRow {
    name: String,
    is_dir: bool,
    is_hidden: bool,
    size_str: String,
    date_str: String,
    top_px: f32,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct ReorderOffset {
    from_delta_px: f32,
    started_at: Instant,
}

/// Events emitted by [`ListView`] to [`super::MainView`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListViewEvent {
    OpenContextMenu { position: Point<Pixels> },
}

/// View rendering directory entries in Details, Compact List, or Grid mode with a draggable scrollbar.
pub struct ListView {
    pub app_state: Entity<AppState>,
    pub focus_handle: FocusHandle,
    pub scroll_handle: UniformListScrollHandle,
    pub scrollbar_state: ScrollbarState,
    pub row_height: Pixels,
    grid_columns: usize,
    last_view_mode: ViewMode,
    rubber_band: Option<RubberBand>,
    hovered_location: Option<Location>,
    pressed_location: Option<Location>,
    hovered_header: Option<u8>,
    last_listing_revision: u64,
    last_location: Option<Location>,
    prev_entries: Vec<(Location, Entry)>,
    prev_deletion_watermark: Option<Instant>,
    nav_anim: Option<(Instant, NavigationDirection)>,
    reorder_offsets: HashMap<Location, ReorderOffset>,
    flash_inserts: HashMap<Location, Instant>,
    calm_inserts: HashMap<Location, Instant>,
    exiting_rows: Vec<ExitingRow>,
    exiting_scroll_top: usize,
    motion: MotionTracker<ListMotionKey>,
}

impl EventEmitter<ListViewEvent> for ListView {}

impl Focusable for ListView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ListView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let (initial_rev, initial_loc, initial_entries, initial_mode) = {
            let state = app_state.read(cx);
            let cur = state.current_location.clone();
            let snaps = state
                .entries
                .iter()
                .map(|e| (entry_location(&cur, e), e.clone()))
                .collect();
            (state.listing_revision, cur, snaps, state.view_mode)
        };
        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            scrollbar_state: ScrollbarState::default(),
            row_height: base_row_height(initial_mode),
            grid_columns: DEFAULT_GRID_COLUMNS,
            last_view_mode: initial_mode,
            rubber_band: None,
            hovered_location: None,
            pressed_location: None,
            hovered_header: None,
            last_listing_revision: initial_rev,
            last_location: Some(initial_loc),
            prev_entries: initial_entries,
            prev_deletion_watermark: None,
            nav_anim: None,
            reorder_offsets: HashMap::new(),
            flash_inserts: HashMap::new(),
            calm_inserts: HashMap::new(),
            exiting_rows: Vec::new(),
            exiting_scroll_top: 0,
            motion: MotionTracker::new(),
        }
    }

    /// Active column count when rendering in [`ViewMode::Grid`].
    pub fn grid_columns(&self) -> usize {
        self.grid_columns
    }

    fn current_view_mode(&self, cx: &gpui::App) -> ViewMode {
        self.app_state.read(cx).view_mode
    }

    fn active_grid_columns(&self) -> usize {
        let width = self
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width;
        if width > px(0.) {
            grid_columns_for_width(width)
        } else {
            self.grid_columns
        }
    }

    fn effective_row_height(&self, view_mode: ViewMode, item_count: usize) -> Pixels {
        let fallback = base_row_height(view_mode);
        if item_count == 0 {
            return fallback;
        }
        self.scroll_handle
            .0
            .borrow()
            .last_item_size
            .map(|s| s.contents.height / (item_count as f32))
            .filter(|h| *h > px(0.))
            .unwrap_or(fallback)
    }

    fn visible_row_count(&self, view_mode: ViewMode, item_count: usize) -> usize {
        let list_h = self
            .scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        let row_h = self.effective_row_height(view_mode, item_count);
        if list_h > px(0.) && row_h > px(0.) {
            ((list_h / row_h).floor() as usize).max(1)
        } else if view_mode == ViewMode::Grid {
            6
        } else {
            25
        }
    }

    /// Scrolls to bring the entry at index `ix` into view (converting to row index in Grid mode).
    pub fn scroll_to_item(&self, ix: usize) {
        let row_ix = if self.last_view_mode == ViewMode::Grid {
            ix / self.grid_columns.max(1)
        } else {
            ix
        };
        self.scroll_handle
            .scroll_to_item(row_ix, ScrollStrategy::Top);
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

    pub fn on_entry_right_click(
        &mut self,
        entry: Entry,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        cx.stop_propagation();

        self.app_state.update(cx, |state, cx| {
            let location = state.location_of(&entry);
            if !state.selection.contains(&location) {
                state.selection.select_only(location);
                cx.notify();
            }
        });
        cx.emit(ListViewEvent::OpenContextMenu {
            position: event.position,
        });
    }

    fn on_list_right_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        let (entry_count, view_mode) = {
            let state = self.app_state.read(cx);
            (state.entries.len(), state.view_mode)
        };
        if let Some(ix) = self.entry_index_at(event.position, entry_count, view_mode) {
            if let Some(entry) = self.app_state.read(cx).entries.get(ix).cloned() {
                self.on_entry_right_click(entry, event, window, cx);
                return;
            }
        }
        self.app_state.update(cx, |state, cx| {
            if !state.selection.is_empty() {
                state.clear_selection();
                cx.notify();
            }
        });
        cx.emit(ListViewEvent::OpenContextMenu {
            position: event.position,
        });
    }

    pub fn open_context_menu_from_keyboard(&mut self, cx: &mut Context<Self>) {
        let (entry_count, selected_ix, view_mode) = {
            let state = self.app_state.read(cx);
            (state.entries.len(), state.selected_index(), state.view_mode)
        };
        let cols = self.active_grid_columns();
        let item_count = if view_mode == ViewMode::Grid {
            entry_count.div_ceil(cols)
        } else {
            entry_count
        };
        let row_h = self.effective_row_height(view_mode, item_count);
        let (list_bounds, scroll_offset) = {
            let scroll = self.scroll_handle.0.borrow();
            (scroll.base_handle.bounds(), scroll.base_handle.offset())
        };
        let position = if list_bounds.size.width > px(0.) && list_bounds.size.height > px(0.) {
            match (view_mode, selected_ix) {
                (ViewMode::Grid, Some(ix)) => {
                    let row = ix / cols;
                    let col = ix % cols;
                    let tile_x = list_bounds.left()
                        + GRID_PAD_X
                        + (GRID_TILE_WIDTH + GRID_GAP) * (col as f32)
                        + GRID_TILE_WIDTH / 2.0;
                    let tile_y = list_bounds.top()
                        + row_h * (row as f32)
                        + GRID_GAP / 2.0
                        + GRID_TILE_HEIGHT / 2.0
                        + scroll_offset.y;
                    point(
                        tile_x.clamp(list_bounds.left(), list_bounds.right()),
                        tile_y.clamp(list_bounds.top(), list_bounds.bottom()),
                    )
                }
                (_, Some(ix)) => point(
                    list_bounds.left() + px(120.),
                    (list_bounds.top() + row_h * ((ix + 1) as f32) + scroll_offset.y)
                        .clamp(list_bounds.top(), list_bounds.bottom()),
                ),
                (_, None) => point(list_bounds.left() + px(16.), list_bounds.top() + px(16.)),
            }
        } else {
            point(px(120.), px(120.))
        };
        cx.emit(ListViewEvent::OpenContextMenu { position });
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

    fn entry_index_at(
        &self,
        position: Point<Pixels>,
        entry_count: usize,
        view_mode: ViewMode,
    ) -> Option<usize> {
        let state = self.scroll_handle.0.borrow();
        let list_bounds = state.base_handle.bounds();
        let scroll_offset = state.base_handle.offset();
        drop(state);
        if position.x < list_bounds.left()
            || position.x > list_bounds.right()
            || position.y < list_bounds.top()
            || position.y > list_bounds.bottom()
        {
            return None;
        }
        let rel_y = position.y - list_bounds.top() - scroll_offset.y;
        if rel_y < px(0.) {
            return None;
        }
        if view_mode == ViewMode::Grid {
            let cols = self.active_grid_columns();
            let row_count = entry_count.div_ceil(cols);
            let row_h = self.effective_row_height(view_mode, row_count);
            if row_h <= px(0.) {
                return None;
            }
            let row = (rel_y / row_h).floor() as usize;
            let in_row_y = rel_y - row_h * (row as f32);
            let tile_top_pad = GRID_GAP / 2.0;
            if in_row_y < tile_top_pad || in_row_y > tile_top_pad + GRID_TILE_HEIGHT {
                return None;
            }
            let rel_x = position.x - list_bounds.left() - GRID_PAD_X;
            if rel_x < px(0.) {
                return None;
            }
            let stride_x = GRID_TILE_WIDTH + GRID_GAP;
            let col = (rel_x / stride_x).floor() as usize;
            if col >= cols {
                return None;
            }
            let in_col_x = rel_x - stride_x * (col as f32);
            if in_col_x > GRID_TILE_WIDTH {
                return None;
            }
            let ix = row * cols + col;
            return (ix < entry_count).then_some(ix);
        }
        let row_h = self.effective_row_height(view_mode, entry_count);
        if row_h <= px(0.) {
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
        let (entry_count, view_mode) = {
            let state = self.app_state.read(cx);
            (state.entries.len(), state.view_mode)
        };
        if self
            .entry_index_at(event.position, entry_count, view_mode)
            .is_some()
        {
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
        let (entry_count, view_mode) = {
            let state = self.app_state.read(cx);
            (state.entries.len(), state.view_mode)
        };

        if list_bounds.size.width > px(0.)
            && right > list_bounds.left()
            && left < list_bounds.right()
        {
            if view_mode == ViewMode::Grid {
                let cols = self.active_grid_columns();
                let row_count = entry_count.div_ceil(cols);
                let row_h = self.effective_row_height(view_mode, row_count);
                if row_h > px(0.) && row_count > 0 {
                    let top_rel = (top - list_bounds.top() - scroll_offset.y).max(px(0.));
                    let bottom_rel = (bottom - list_bounds.top() - scroll_offset.y).max(px(0.));
                    let r_start = ((top_rel / row_h).floor() as usize).min(row_count - 1);
                    let r_end = ((bottom_rel / row_h).floor() as usize).min(row_count - 1);
                    let stride_x = GRID_TILE_WIDTH + GRID_GAP;
                    let mut touched = Vec::new();
                    for r in r_start..=r_end {
                        let tile_top = list_bounds.top()
                            + row_h * (r as f32)
                            + GRID_GAP / 2.0
                            + scroll_offset.y;
                        let tile_bottom = tile_top + GRID_TILE_HEIGHT;
                        if tile_bottom <= top || tile_top >= bottom {
                            continue;
                        }
                        for c in 0..cols {
                            let ix = r * cols + c;
                            if ix >= entry_count {
                                break;
                            }
                            let tile_left = list_bounds.left() + GRID_PAD_X + stride_x * (c as f32);
                            let tile_right = tile_left + GRID_TILE_WIDTH;
                            if tile_right > left && tile_left < right {
                                touched.push(ix);
                            }
                        }
                    }
                    self.app_state.update(cx, |state, cx| {
                        state.select_indices(touched, base_selection.as_ref());
                        cx.notify();
                    });
                }
            } else {
                let row_h = self.effective_row_height(view_mode, entry_count);
                if row_h > px(0.) && entry_count > 0 {
                    let top_rel = (top - list_bounds.top() - scroll_offset.y).max(px(0.));
                    let bottom_rel = (bottom - list_bounds.top() - scroll_offset.y).max(px(0.));
                    let r_start = ((top_rel / row_h).floor() as usize).min(entry_count - 1);
                    let r_end = ((bottom_rel / row_h).floor() as usize).min(entry_count - 1);
                    self.app_state.update(cx, |state, cx| {
                        let touched = (r_start..=r_end).filter(|&ix| {
                            let row_top = list_bounds.top() + row_h * (ix as f32) + scroll_offset.y;
                            let row_bottom = row_top + row_h;
                            row_bottom > top && row_top < bottom
                        });
                        state.select_indices(touched, base_selection.as_ref());
                        cx.notify();
                    });
                }
            }
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
        if self.current_view_mode(cx) == ViewMode::Grid {
            let cols = self.active_grid_columns();
            self.update_selection(cx, |s| s.select_down_by(cols), Self::scroll_to_item);
        } else {
            self.update_selection(cx, AppState::select_next, Self::scroll_to_item);
        }
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.current_view_mode(cx) == ViewMode::Grid {
            let cols = self.active_grid_columns();
            self.update_selection(cx, |s| s.select_up_by(cols), Self::scroll_to_item);
        } else {
            self.update_selection(cx, AppState::select_previous, Self::scroll_to_item);
        }
    }

    pub fn select_right(&mut self, cx: &mut Context<Self>) {
        if self.current_view_mode(cx) == ViewMode::Grid {
            self.update_selection(cx, AppState::select_next, Self::scroll_to_item);
        }
    }

    pub fn select_left(&mut self, cx: &mut Context<Self>) {
        if self.current_view_mode(cx) == ViewMode::Grid {
            self.update_selection(cx, AppState::select_previous, Self::scroll_to_item);
        }
    }

    fn shift_extend_selection(&mut self, key: &str, cx: &mut Context<Self>) {
        let view_mode = self.current_view_mode(cx);
        let cols = self.active_grid_columns();
        let target_opt = self.app_state.update(cx, |state, cx| {
            let len = state.entries.len();
            if len == 0 {
                return None;
            }
            let cursor = active_selection_cursor(state);
            let target = match (view_mode, key) {
                (ViewMode::Grid, "right") => cursor.map_or(0, |ix| (ix + 1).min(len - 1)),
                (ViewMode::Grid, "left") => cursor.map_or(len - 1, |ix| ix.saturating_sub(1)),
                (ViewMode::Grid, "down") => {
                    let last_ix = len - 1;
                    let last_row = last_ix / cols;
                    cursor.map_or(0, |ix| {
                        if ix / cols < last_row {
                            (ix + cols).min(last_ix)
                        } else {
                            ix
                        }
                    })
                }
                (ViewMode::Grid, "up") => {
                    cursor.map_or(len - 1, |ix| if ix >= cols { ix - cols } else { ix })
                }
                (_, "down") => cursor.map_or(0, |ix| (ix + 1).min(len - 1)),
                (_, "up") => cursor.map_or(len - 1, |ix| ix.saturating_sub(1)),
                _ => return None,
            };
            state.select_range_to(target, false);
            cx.notify();
            Some(target)
        });
        if let Some(target) = target_opt {
            self.scroll_to_item(target);
        }
        cx.notify();
    }

    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_first, |this, _| this.scroll_to_top());
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        self.update_selection(cx, AppState::select_last, |this, _| this.scroll_to_bottom());
    }

    pub fn select_page_up(&mut self, cx: &mut Context<Self>) {
        let view_mode = self.current_view_mode(cx);
        let cols = self.active_grid_columns();
        let entry_count = self.app_state.read(cx).entries.len();
        let item_count = if view_mode == ViewMode::Grid {
            entry_count.div_ceil(cols)
        } else {
            entry_count
        };
        let step = if view_mode == ViewMode::Grid {
            self.visible_row_count(view_mode, item_count) * cols
        } else {
            20
        };
        self.update_selection(cx, |s| s.select_page_up(step), Self::scroll_to_item);
    }

    pub fn select_page_down(&mut self, cx: &mut Context<Self>) {
        let view_mode = self.current_view_mode(cx);
        let cols = self.active_grid_columns();
        let entry_count = self.app_state.read(cx).entries.len();
        let item_count = if view_mode == ViewMode::Grid {
            entry_count.div_ceil(cols)
        } else {
            entry_count
        };
        let step = if view_mode == ViewMode::Grid {
            self.visible_row_count(view_mode, item_count) * cols
        } else {
            20
        };
        self.update_selection(cx, |s| s.select_page_down(step), Self::scroll_to_item);
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
            if let Err(err) = crate::actions::open_with_system(entry.path()) {
                self.app_state.update(cx, |state, _cx| {
                    state.set_status(format!("Cannot open {}: {err}", entry.path().display()));
                });
            }
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

pub const COMPACT_ROW_HEIGHT: Pixels = px(22.);
pub const GRID_TILE_WIDTH: Pixels = px(104.);
pub const GRID_TILE_HEIGHT: Pixels = px(96.);
pub const GRID_ICON_SIZE: Pixels = px(40.);
pub const GRID_PAD_X: Pixels = space::S12;
pub const GRID_GAP: Pixels = space::S8;
pub const GRID_ROW_HEIGHT: Pixels = px(104.);
const DEFAULT_GRID_COLUMNS: usize = 4;

/// Computes how many `104 px` tiles fit horizontally inside `width` with `12 px` outer
/// padding and `8 px` tile spacing.
pub fn grid_columns_for_width(width: Pixels) -> usize {
    if width <= px(0.) {
        return DEFAULT_GRID_COLUMNS;
    }
    let usable = (width - GRID_PAD_X * 2.0).max(GRID_TILE_WIDTH);
    (((usable + GRID_GAP) / (GRID_TILE_WIDTH + GRID_GAP)).floor() as usize).max(1)
}

fn base_row_height(mode: ViewMode) -> Pixels {
    match mode {
        ViewMode::Details => size::ROW,
        ViewMode::CompactList => COMPACT_ROW_HEIGHT,
        ViewMode::Grid => GRID_ROW_HEIGHT,
    }
}

fn active_selection_cursor(state: &AppState) -> Option<usize> {
    let anchor_ix = state.selection.anchor().and_then(|anchor| {
        state
            .entries
            .iter()
            .position(|e| &state.location_of(e) == anchor)
    });
    let first_selected = state.selected_index();
    let last_selected = state
        .entries
        .iter()
        .rposition(|e| state.selection.contains(&state.location_of(e)));
    match (anchor_ix, first_selected, last_selected) {
        (Some(a), Some(first), Some(last)) => Some(if a == first { last } else { first }),
        (_, Some(first), _) => Some(first),
        _ => None,
    }
}

struct ItemVisuals {
    bg: Hsla,
    border_col: Hsla,
    opacity: f32,
    x_offset: f32,
    y_offset: f32,
    icon_scale: f32,
    icon_motion: IconMotion,
    glyph: IconName,
    glyph_col: Hsla,
    text_col: Hsla,
}

impl ListView {
    #[allow(clippy::too_many_arguments)]
    fn compute_item_visuals(
        &mut self,
        ix: usize,
        entry: &Entry,
        loc: &Location,
        is_selected: bool,
        is_focused_row: bool,
        is_cut: bool,
        scroll_top_entry: usize,
        now: Instant,
        reduced: bool,
        theme: &Theme,
    ) -> ItemVisuals {
        let is_hovered = self.hovered_location.as_ref() == Some(loc);
        let is_pressed = self.pressed_location.as_ref() == Some(loc);
        let is_dir = entry.is_dir();
        let is_hidden = entry.is_hidden;

        let hover_t = self.motion.step_scalar(
            ListMotionKey::Hover(loc.clone()),
            0.0,
            if is_hovered { 1.0 } else { 0.0 },
            INSTANT,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let press_t = self.motion.step_scalar(
            ListMotionKey::Press(loc.clone()),
            0.0,
            if is_pressed { 1.0 } else { 0.0 },
            INSTANT,
            Curve::Spring,
            MotionKind::MovementOrScale,
            now,
            reduced,
        );
        let select_t = self.motion.step_scalar(
            ListMotionKey::SelectBg(loc.clone()),
            if is_selected { 1.0 } else { 0.0 },
            if is_selected { 1.0 } else { 0.0 },
            INSTANT,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let focus_t = self.motion.step_scalar(
            ListMotionKey::FocusRing(loc.clone()),
            0.0,
            if is_focused_row { 1.0 } else { 0.0 },
            INSTANT,
            Curve::Spring,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let cut_t = self.motion.step_scalar(
            ListMotionKey::CutFade(loc.clone()),
            0.0,
            if is_cut { 1.0 } else { 0.0 },
            INSTANT,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let folder_open_t = if is_dir {
            self.motion.step_scalar(
                ListMotionKey::FolderOpen(loc.clone()),
                0.0,
                if is_hovered { 1.0 } else { 0.0 },
                INSTANT,
                Curve::Spring,
                MotionKind::MovementOrScale,
                now,
                reduced,
            )
        } else {
            0.0
        };

        let (mut x_offset, mut item_opacity) = if let Some((start, dir)) = self.nav_anim {
            let rel_ix = ix.saturating_sub(scroll_top_entry);
            let delay = LIST_CASCADE_STEP * (rel_ix.min(LIST_CASCADE_CAP) as u32);
            let elapsed = now.saturating_duration_since(start);
            let sign = match dir {
                NavigationDirection::Back => -1.0,
                _ => 1.0,
            };
            if elapsed < delay {
                (sign * LIST_ENTER_OFFSET_PX, 0.0)
            } else {
                let raw_t = (elapsed - delay).as_secs_f32() / SETTLE.as_secs_f32();
                let spring = Curve::Spring.sample(raw_t.clamp(0.0, 1.0));
                let op = Curve::EaseOut.sample(raw_t.clamp(0.0, 1.0));
                (sign * LIST_ENTER_OFFSET_PX * (1.0 - spring), op)
            }
        } else {
            (0.0, 1.0)
        };

        let mut insert_flash_a = 0.0_f32;
        let mut insert_box_scale = 1.0_f32;
        if let Some(&ts) = self.flash_inserts.get(loc) {
            let t = (now.saturating_duration_since(ts).as_secs_f32() / SETTLE.as_secs_f32())
                .clamp(0.0, 1.0);
            let spring = Curve::Spring.sample(t);
            x_offset += -24.0 * (1.0 - spring);
            insert_box_scale = lerp_f32(ROW_INSERT_START_SCALE, 1.0, spring);
            insert_flash_a = 0.22 * (1.0 - Curve::EaseOut.sample(t));
            item_opacity *= Curve::EaseOut.sample(t);
            let _ = self.motion.step_scalar(
                ListMotionKey::InsertEnter(loc.clone()),
                0.0,
                1.0,
                SETTLE,
                Curve::Spring,
                MotionKind::MovementOrScale,
                now,
                reduced,
            );
        } else if let Some(&ts) = self.calm_inserts.get(loc) {
            let t = (now.saturating_duration_since(ts).as_secs_f32() / SETTLE.as_secs_f32())
                .clamp(0.0, 1.0);
            let spring = Curve::Spring.sample(t);
            x_offset += -18.0 * (1.0 - spring);
            item_opacity *= Curve::EaseOut.sample(t);
        }

        let y_offset = if let Some(ro) = self.reorder_offsets.get(loc) {
            let t = (now.saturating_duration_since(ro.started_at).as_secs_f32()
                / SETTLE.as_secs_f32())
            .clamp(0.0, 1.0);
            ro.from_delta_px * (1.0 - Curve::Spring.sample(t))
        } else {
            0.0
        };

        item_opacity *= lerp_f32(1.0, 0.5, cut_t);

        let base_hover = color_lerp(transparent_black(), theme.surface_raised, hover_t);
        let mut bg: Hsla = color_lerp(base_hover, theme.selection, select_t);
        if insert_flash_a > 0.005 {
            bg = color_lerp(bg, theme.accent, insert_flash_a.min(0.35));
        }
        let border_col = focus_ring_color(transparent_black(), focus_t, theme);
        let icon_scale = if reduced {
            1.0
        } else {
            lerp_f32(1.0, PRESS_SCALE, press_t) * insert_box_scale
        };
        let icon_motion = if is_dir {
            if !reduced && folder_open_t > 0.01 {
                IconMotion::State(folder_open_t)
            } else {
                IconMotion::Rest
            }
        } else if !reduced && hover_t > 0.01 {
            IconMotion::State(hover_t)
        } else {
            IconMotion::Rest
        };
        let glyph = if is_dir {
            IconName::Folder
        } else {
            IconName::File
        };
        let glyph_col: Hsla = if is_dir {
            theme.accent.into()
        } else if is_hidden && !is_selected {
            theme.text_muted.into()
        } else {
            theme.text.into()
        };
        let text_col: Hsla = if is_hidden && !is_selected {
            theme.text_muted.into()
        } else {
            theme.text.into()
        };

        ItemVisuals {
            bg,
            border_col,
            opacity: item_opacity.clamp(0.0, 1.0),
            x_offset,
            y_offset,
            icon_scale,
            icon_motion,
            glyph,
            glyph_col,
            text_col,
        }
    }

    fn attach_entry_interactions(
        el: Stateful<Div>,
        ix: usize,
        loc: Location,
        entry: Entry,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let loc_hover = loc.clone();
        let loc_press = loc;
        let entry_for_click = entry.clone();
        let entry_for_rclick = entry;

        el.on_hover(cx.listener(move |this, hovered, _, cx| {
            if *hovered {
                this.hovered_location = Some(loc_hover.clone());
                cx.notify();
            } else if this.hovered_location.as_ref() == Some(&loc_hover) {
                this.hovered_location = None;
                this.pressed_location = None;
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.pressed_location = Some(loc_press.clone());
                this.on_entry_click(ix, entry_for_click.clone(), event, window, cx);
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                if this.pressed_location.is_some() {
                    this.pressed_location = None;
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                if this.pressed_location.is_some() {
                    this.pressed_location = None;
                    cx.notify();
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.on_entry_right_click(entry_for_rclick.clone(), event, window, cx);
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_details_row(
        &mut self,
        ix: usize,
        entry: &Entry,
        loc: &Location,
        is_selected: bool,
        vis: ItemVisuals,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_dir = entry.is_dir();
        let name = entry.display_name();
        let size_str = format_size(entry.size, is_dir);
        let date_str = format_modified(entry.modified);
        let secondary_col: Hsla = if is_selected {
            theme.text.into()
        } else {
            theme.text_muted.into()
        };

        let base = div()
            .id(ix)
            .relative()
            .left(px(vis.x_offset))
            .top(px(vis.y_offset))
            .flex()
            .flex_row()
            .w_full()
            .h(size::ROW)
            .px(space::S8)
            .gap(space::S8)
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(vis.border_col)
            .bg(vis.bg)
            .opacity(vis.opacity)
            .items_center()
            .cursor_pointer()
            .text_body()
            .text_color(vis.text_col);

        Self::attach_entry_interactions(base, ix, loc.clone(), entry.clone(), cx)
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
                    .child(animated_icon_sized(
                        vis.glyph,
                        vis.icon_motion,
                        size::ICON * vis.icon_scale,
                        vis.glyph_col,
                    ))
                    .child(div().ellipsize().child(name)),
            )
            .child(
                div()
                    .w(SIZE_COLUMN)
                    .flex()
                    .justify_end()
                    .text_data(fonts)
                    .text_color(secondary_col)
                    .child(size_str),
            )
            .child(
                div()
                    .w(DATE_COLUMN)
                    .text_data(fonts)
                    .text_color(secondary_col)
                    .child(date_str),
            )
            .child(div().w(HIDDEN_TOGGLE_COLUMN))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_compact_row(
        &mut self,
        ix: usize,
        entry: &Entry,
        loc: &Location,
        vis: ItemVisuals,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = entry.display_name();
        let base = div()
            .id(ix)
            .relative()
            .left(px(vis.x_offset))
            .top(px(vis.y_offset))
            .flex()
            .flex_row()
            .w_full()
            .h(COMPACT_ROW_HEIGHT)
            .px(space::S8)
            .gap(space::S8)
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(vis.border_col)
            .bg(vis.bg)
            .opacity(vis.opacity)
            .items_center()
            .cursor_pointer()
            .text_body()
            .text_color(vis.text_col);

        Self::attach_entry_interactions(base, ix, loc.clone(), entry.clone(), cx)
            .child(animated_icon_sized(
                vis.glyph,
                vis.icon_motion,
                size::ICON * vis.icon_scale,
                vis.glyph_col,
            ))
            .child(div().flex_1().ellipsize().child(name))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_grid_tile(
        &mut self,
        ix: usize,
        entry: &Entry,
        loc: &Location,
        vis: ItemVisuals,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let name = entry.display_name();
        let base = div()
            .id(ix)
            .relative()
            .left(px(vis.x_offset))
            .top(px(vis.y_offset))
            .w(GRID_TILE_WIDTH)
            .h(GRID_TILE_HEIGHT)
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .p(space::S8)
            .gap(space::S4)
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(vis.border_col)
            .bg(vis.bg)
            .opacity(vis.opacity)
            .overflow_hidden()
            .cursor_pointer()
            .text_body()
            .text_color(vis.text_col);

        Self::attach_entry_interactions(base, ix, loc.clone(), entry.clone(), cx)
            .child(
                div()
                    .size(GRID_ICON_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .child(animated_icon_sized(
                        vis.glyph,
                        vis.icon_motion,
                        GRID_ICON_SIZE * vis.icon_scale,
                        vis.glyph_col,
                    )),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .line_height(px(16.))
                    .whitespace_normal()
                    .line_clamp(2)
                    .text_ellipsis()
                    .overflow_hidden()
                    .text_center()
                    .child(name),
            )
            .into_any_element()
    }

    /// Detects navigation, sort/filter reorders, user/watcher inserts, and deletions on revision change.
    #[allow(clippy::too_many_arguments)]
    fn sync_listing_transitions(
        &mut self,
        state: &AppState,
        view_mode: ViewMode,
        cols: usize,
        scroll_top: usize,
        row_h_px: f32,
        now: Instant,
        reduced: bool,
    ) {
        if state.listing_revision == self.last_listing_revision
            && self.last_location.as_ref() == Some(&state.current_location)
        {
            return;
        }

        let dir_changed = self.last_location.as_ref() != Some(&state.current_location);
        self.last_listing_revision = state.listing_revision;
        self.last_location = Some(state.current_location.clone());

        let new_entries: Vec<(Location, Entry)> = state
            .entries
            .iter()
            .map(|e| (entry_location(&state.current_location, e), e.clone()))
            .collect();

        if dir_changed
            || matches!(
                state.last_arrival,
                NavigationDirection::Visit
                    | NavigationDirection::Back
                    | NavigationDirection::Forward
            )
        {
            self.hovered_location = None;
            self.pressed_location = None;
            self.reorder_offsets.clear();
            self.flash_inserts.clear();
            self.calm_inserts.clear();
            self.exiting_rows.clear();
            self.motion.clear();
            self.nav_anim = if reduced
                || matches!(
                    state.last_arrival,
                    NavigationDirection::Initial | NavigationDirection::Reload
                ) {
                None
            } else {
                Some((now, state.last_arrival))
            };
            self.prev_entries = new_entries;
            return;
        }

        // Same directory update (sort, filter, insert, or delete).
        if !reduced {
            let entry_scroll_top = if view_mode == ViewMode::Grid {
                scroll_top * cols
            } else {
                scroll_top
            };
            let win_span = if view_mode == ViewMode::Grid {
                cols * 12
            } else {
                64
            };
            let win_start = entry_scroll_top.saturating_sub(32);
            let win_end = (entry_scroll_top + win_span).min(new_entries.len());

            let prev_index: HashMap<&Location, usize> = self
                .prev_entries
                .iter()
                .enumerate()
                .map(|(ix, (loc, _))| (loc, ix))
                .collect();

            // Record recent deletions that were near the visible viewport so they can throw right + fade.
            let del_set: HashSet<&Location> = state
                .recent_deletions
                .iter()
                .filter(|(_, ts)| self.prev_deletion_watermark.is_none_or(|wm| *ts > wm))
                .map(|(loc, _)| loc)
                .collect();
            if let Some(max_ts) = state.recent_deletions.iter().map(|(_, ts)| *ts).max() {
                self.prev_deletion_watermark = Some(max_ts);
            }

            if !del_set.is_empty() && view_mode != ViewMode::Grid {
                self.exiting_scroll_top = scroll_top;
                for (old_ix, (loc, entry)) in self.prev_entries.iter().enumerate() {
                    if del_set.contains(loc)
                        && !new_entries.iter().any(|(nloc, _)| nloc == loc)
                        && old_ix >= scroll_top.saturating_sub(8)
                        && old_ix < scroll_top + 48
                    {
                        self.exiting_rows.push(ExitingRow {
                            name: entry.display_name(),
                            is_dir: entry.is_dir(),
                            is_hidden: entry.is_hidden,
                            size_str: format_size(entry.size, entry.is_dir()),
                            date_str: format_modified(entry.modified),
                            top_px: (old_ix as isize - scroll_top as isize) as f32 * row_h_px,
                            started_at: now,
                        });
                    }
                }
            }

            let flash_set: HashSet<&Location> = state
                .highlighted_inserts
                .iter()
                .map(|(loc, _)| loc)
                .collect();

            for (new_ix, (loc, _)) in new_entries.iter().enumerate().take(win_end).skip(win_start) {
                if let Some(&old_ix) = prev_index.get(loc) {
                    if old_ix != new_ix {
                        let delta_rows = if view_mode == ViewMode::Grid {
                            (old_ix / cols) as isize - (new_ix / cols) as isize
                        } else {
                            old_ix as isize - new_ix as isize
                        };
                        if delta_rows != 0 {
                            self.reorder_offsets.insert(
                                loc.clone(),
                                ReorderOffset {
                                    from_delta_px: delta_rows as f32 * row_h_px,
                                    started_at: now,
                                },
                            );
                        }
                    }
                } else if flash_set.contains(loc)
                    || state.last_change_origin == EntryChangeOrigin::UserAction
                {
                    self.flash_inserts.insert(loc.clone(), now);
                } else {
                    self.calm_inserts.insert(loc.clone(), now);
                }
            }
        }

        self.prev_entries = new_entries;
    }

    fn prune_list_motion(&mut self, now: Instant, reduced: bool) {
        self.motion.prune_settled(now, reduced);
        self.reorder_offsets
            .retain(|_, r| !reduced && now.saturating_duration_since(r.started_at) < SETTLE);
        self.flash_inserts
            .retain(|_, ts| !reduced && now.saturating_duration_since(*ts) < SETTLE);
        self.calm_inserts
            .retain(|_, ts| !reduced && now.saturating_duration_since(*ts) < SETTLE);
        self.exiting_rows
            .retain(|r| !reduced && now.saturating_duration_since(r.started_at) < DELETE_THROW);
        if let Some((start, _)) = self.nav_anim {
            let max_nav = LIST_CASCADE_STEP * (LIST_CASCADE_CAP as u32) + SETTLE;
            if reduced || now.saturating_duration_since(start) >= max_nav {
                self.nav_anim = None;
            }
        }
    }

    fn has_active_list_motion(&self, now: Instant, reduced: bool) -> bool {
        if reduced {
            return false;
        }
        self.motion.has_active(now, reduced)
            || self.nav_anim.is_some()
            || !self.reorder_offsets.is_empty()
            || !self.flash_inserts.is_empty()
            || !self.calm_inserts.is_empty()
            || !self.exiting_rows.is_empty()
    }
}

impl Render for ListView {
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

        let view_mode = self.current_view_mode(cx);
        let cols = self.active_grid_columns();
        self.grid_columns = cols;
        self.row_height = base_row_height(view_mode);

        if self.last_view_mode != view_mode {
            self.last_view_mode = view_mode;
            self.exiting_rows.clear();
            self.reorder_offsets.clear();
            self.hovered_location = None;
            self.pressed_location = None;
            if let Some(ix) = self.app_state.read(cx).selected_index() {
                self.scroll_to_item(ix);
            } else {
                self.scroll_to_top();
            }
        }

        let row_h_px = self.row_height / px(1.);

        let (
            entries,
            selection,
            current,
            sort_column,
            sort_direction,
            show_hidden,
            reduced,
            focused_location,
        ) = {
            let state: &AppState = self.app_state.read(cx);
            let r = is_reduced_motion(state.settings.motion_preference);
            self.sync_listing_transitions(state, view_mode, cols, current_top, row_h_px, now, r);
            (
                state.entries.clone(),
                state.selection.clone(),
                state.current_location.clone(),
                state.sort_column,
                state.sort_direction,
                state.show_hidden,
                r,
                state.selection.anchor().cloned().or_else(|| {
                    state
                        .selected_index()
                        .and_then(|ix| state.entries.get(ix))
                        .map(|e| entry_location(&state.current_location, e))
                }),
            )
        };
        let entry_count = entries.len();
        let list_item_count = if view_mode == ViewMode::Grid {
            entry_count.div_ceil(cols)
        } else {
            entry_count
        };
        let visible_items = self.visible_row_count(view_mode, list_item_count);
        self.scrollbar_state
            .set_metrics(list_item_count, visible_items, current_top);

        let cut_sources: HashSet<Location> = get_location_clipboard()
            .filter(|c| c.is_cut)
            .map(|c| c.sources.into_iter().collect())
            .unwrap_or_default();
        let list_focused = self.focus_handle.is_focused(window);

        let header_hovers = [
            self.motion.step_scalar(
                ListMotionKey::HeaderHover(0),
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
                ListMotionKey::HeaderHover(1),
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
                ListMotionKey::HeaderHover(2),
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
            self.motion.step_scalar(
                ListMotionKey::HeaderHover(3),
                0.0,
                if self.hovered_header == Some(3) {
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

        let entries_clone = entries;
        let (row_text, row_muted, row_accent) = (theme.text, theme.text_muted, theme.accent);
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

        let exiting_elements: Vec<_> = if view_mode == ViewMode::Grid {
            Vec::new()
        } else {
            self.exiting_rows
                .iter()
                .map(|ex| {
                    let t = (now.saturating_duration_since(ex.started_at).as_secs_f32()
                        / DELETE_THROW.as_secs_f32())
                    .clamp(0.0, 1.0);
                    let exit_eased = Curve::Exit.sample(t);
                    let x_throw = DELETE_THROW_PX * exit_eased;
                    let opacity = (1.0 - exit_eased).clamp(0.0, 1.0);
                    let scroll_shift_px =
                        (self.exiting_scroll_top as isize - current_top as isize) as f32 * row_h_px;
                    let top_px = ex.top_px + scroll_shift_px;
                    let glyph = if ex.is_dir {
                        IconName::Folder
                    } else {
                        IconName::File
                    };
                    let glyph_col = if ex.is_dir {
                        row_accent
                    } else if ex.is_hidden {
                        row_muted
                    } else {
                        row_text
                    };

                    div()
                        .absolute()
                        .left(px(x_throw))
                        .top(px(top_px))
                        .w_full()
                        .h(base_row_height(view_mode))
                        .px(space::S8)
                        .gap(space::S8)
                        .flex()
                        .flex_row()
                        .items_center()
                        .rounded(radius::CONTROL)
                        .opacity(opacity)
                        .text_body()
                        .text_color(if ex.is_hidden { row_muted } else { row_text })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .child(animated_icon_sized(
                                    glyph,
                                    IconMotion::Rest,
                                    size::ICON,
                                    glyph_col,
                                ))
                                .child(div().ellipsize().child(ex.name.clone())),
                        )
                        .when(view_mode == ViewMode::Details, |row| {
                            row.child(
                                div()
                                    .w(SIZE_COLUMN)
                                    .flex()
                                    .justify_end()
                                    .text_data(&fonts)
                                    .text_color(row_muted)
                                    .child(ex.size_str.clone()),
                            )
                            .child(
                                div()
                                    .w(DATE_COLUMN)
                                    .text_data(&fonts)
                                    .text_color(row_muted)
                                    .child(ex.date_str.clone()),
                            )
                            .child(div().w(HIDDEN_TOGGLE_COLUMN))
                        })
                })
                .collect()
        };

        let theme_for_rows = theme;
        let scroll_top_entry = if view_mode == ViewMode::Grid {
            current_top * cols
        } else {
            current_top
        };

        let root = div()
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
            .on_action(cx.listener(|this, _: &OpenContextMenu, _window, cx| {
                this.open_context_menu_from_keyboard(cx)
            }))
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
                let shift = event.keystroke.modifiers.shift;
                match event.keystroke.key.as_str() {
                    "down" if shift => this.shift_extend_selection("down", cx),
                    "up" if shift => this.shift_extend_selection("up", cx),
                    "left" if shift => this.shift_extend_selection("left", cx),
                    "right" if shift => this.shift_extend_selection("right", cx),
                    "down" => this.select_next(cx),
                    "up" => this.select_previous(cx),
                    "left" => this.select_left(cx),
                    "right" => this.select_right(cx),
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
            // Column Headers Bar (shown only in Details mode where Size/Modified columns exist)
            .when(view_mode == ViewMode::Details, |root| {
                root.child(
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
                                .id("col-sort-name")
                                .flex_1()
                                .flex()
                                .items_center()
                                .cursor(CursorStyle::PointingHand)
                                .text_color(color_lerp(
                                    theme.text_muted,
                                    theme.text,
                                    header_hovers[0],
                                ))
                                .on_hover(cx.listener(|this, hovered, _, cx| {
                                    this.hovered_header = if *hovered { Some(0) } else { None };
                                    cx.notify();
                                }))
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
                                .id("col-sort-size")
                                .w(SIZE_COLUMN)
                                .flex()
                                .justify_end()
                                .items_center()
                                .cursor(CursorStyle::PointingHand)
                                .text_color(color_lerp(
                                    theme.text_muted,
                                    theme.text,
                                    header_hovers[1],
                                ))
                                .on_hover(cx.listener(|this, hovered, _, cx| {
                                    this.hovered_header = if *hovered { Some(1) } else { None };
                                    cx.notify();
                                }))
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
                                .id("col-sort-date")
                                .w(DATE_COLUMN)
                                .flex()
                                .items_center()
                                .cursor(CursorStyle::PointingHand)
                                .text_color(color_lerp(
                                    theme.text_muted,
                                    theme.text,
                                    header_hovers[2],
                                ))
                                .on_hover(cx.listener(|this, hovered, _, cx| {
                                    this.hovered_header = if *hovered { Some(2) } else { None };
                                    cx.notify();
                                }))
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
                                .id("col-toggle-hidden")
                                .w(HIDDEN_TOGGLE_COLUMN)
                                .flex()
                                .justify_center()
                                .rounded(radius::CONTROL)
                                .cursor(CursorStyle::PointingHand)
                                .bg(if show_hidden {
                                    theme.selection.into()
                                } else {
                                    color_lerp(
                                        transparent_black(),
                                        theme.surface_raised,
                                        header_hovers[3],
                                    )
                                })
                                .text_color(if show_hidden {
                                    theme.text
                                } else {
                                    color_lerp(theme.text_muted, theme.text, header_hovers[3])
                                        .into()
                                })
                                .on_hover(cx.listener(|this, hovered, _, cx| {
                                    this.hovered_header = if *hovered { Some(3) } else { None };
                                    cx.notify();
                                }))
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
            })
            // Row / Grid list + Scrollbar container
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
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_list_mouse_down))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(Self::on_list_right_click),
                            )
                            .on_mouse_move(cx.listener(Self::on_list_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_list_mouse_up))
                            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_list_mouse_up))
                            .child(
                                uniform_list(
                                    "directory_entries",
                                    list_item_count,
                                    cx.processor(
                                        move |this: &mut ListView,
                                              range: std::ops::Range<usize>,
                                              _window,
                                              cx| {
                                            let mut items: Vec<gpui::AnyElement> =
                                                Vec::with_capacity(range.len());
                                            match view_mode {
                                                ViewMode::Details | ViewMode::CompactList => {
                                                    for ix in range {
                                                        if let Some(entry) = entries_clone.get(ix) {
                                                            let loc =
                                                                entry_location(&current, entry);
                                                            let is_selected =
                                                                selection.contains(&loc);
                                                            let is_focused_row = list_focused
                                                                && focused_location.as_ref()
                                                                    == Some(&loc);
                                                            let is_cut = cut_sources.contains(&loc);
                                                            let vis = this.compute_item_visuals(
                                                                ix,
                                                                entry,
                                                                &loc,
                                                                is_selected,
                                                                is_focused_row,
                                                                is_cut,
                                                                scroll_top_entry,
                                                                now,
                                                                reduced,
                                                                &theme_for_rows,
                                                            );
                                                            if view_mode == ViewMode::Details {
                                                                items.push(
                                                                    this.render_details_row(
                                                                        ix,
                                                                        entry,
                                                                        &loc,
                                                                        is_selected,
                                                                        vis,
                                                                        &theme_for_rows,
                                                                        &row_fonts,
                                                                        cx,
                                                                    ),
                                                                );
                                                            } else {
                                                                items.push(
                                                                    this.render_compact_row(
                                                                        ix, entry, &loc, vis, cx,
                                                                    ),
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                ViewMode::Grid => {
                                                    for row_ix in range {
                                                        let start = row_ix * cols;
                                                        let end = (start + cols).min(entry_count);
                                                        let mut row_tiles = Vec::with_capacity(
                                                            end.saturating_sub(start),
                                                        );
                                                        for ix in start..end {
                                                            if let Some(entry) =
                                                                entries_clone.get(ix)
                                                            {
                                                                let loc =
                                                                    entry_location(&current, entry);
                                                                let is_selected =
                                                                    selection.contains(&loc);
                                                                let is_focused_row = list_focused
                                                                    && focused_location.as_ref()
                                                                        == Some(&loc);
                                                                let is_cut =
                                                                    cut_sources.contains(&loc);
                                                                let vis = this
                                                                    .compute_item_visuals(
                                                                        ix,
                                                                        entry,
                                                                        &loc,
                                                                        is_selected,
                                                                        is_focused_row,
                                                                        is_cut,
                                                                        scroll_top_entry,
                                                                        now,
                                                                        reduced,
                                                                        &theme_for_rows,
                                                                    );
                                                                row_tiles.push(
                                                                    this.render_grid_tile(
                                                                        ix, entry, &loc, vis, cx,
                                                                    ),
                                                                );
                                                            }
                                                        }
                                                        items.push(
                                                            div()
                                                                .flex()
                                                                .flex_row()
                                                                .items_start()
                                                                .w_full()
                                                                .h(GRID_ROW_HEIGHT)
                                                                .px(GRID_PAD_X)
                                                                .py(GRID_GAP / 2.0)
                                                                .gap(GRID_GAP)
                                                                .children(row_tiles)
                                                                .into_any_element(),
                                                        );
                                                    }
                                                }
                                            }
                                            items
                                        },
                                    ),
                                )
                                .track_scroll(self.scroll_handle.clone())
                                .size_full(),
                            )
                            .children(exiting_elements)
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
                                    let rel_bottom =
                                        (bottom - list_bounds.top()).min(list_bounds.size.height);
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
            );

        self.prune_list_motion(now, reduced);
        let animating = self.has_active_list_motion(now, reduced);
        request_frame_if_active(animating, window);
        root
    }
}
