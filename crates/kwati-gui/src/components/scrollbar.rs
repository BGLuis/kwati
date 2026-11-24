//! # Scrollbar Component
//!
//! **Chosen Option:** Reusable draggable GPUI 0.2.2 Scrollbar element.
//! - **Source:** Native GPUI 0.2.2 element implementation
//! - **Crate Version:** `gpui = "=0.2.2"`
//! - **License:** Apache-2.0
//! - **Capabilities:** Draggable thumb, track click navigation, ratio calculations for
//!   `uniform_list` and scrollable containers, custom styling tokens.
//! - **Platforms:** Passes build on Linux (Wayland/X11), macOS, and Windows.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, size, transparent_black, Bounds, Context,
    CursorStyle, DispatchPhase, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Rgba, ScrollStrategy, UniformListScrollHandle, Window,
};

use crate::theme::Theme;

/// Configuration and current scroll position for a Scrollbar.
#[derive(Debug, Clone)]
pub struct ScrollbarState {
    pub total_items: usize,
    pub visible_items: usize,
    pub current_offset: usize,
    pub is_dragging: bool,
    pub drag_start_y: Pixels,
    pub drag_start_offset: usize,
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self {
            total_items: 0,
            visible_items: 0,
            current_offset: 0,
            is_dragging: false,
            drag_start_y: px(0.),
            drag_start_offset: 0,
        }
    }
}

impl ScrollbarState {
    pub fn new(total_items: usize, visible_items: usize) -> Self {
        Self {
            total_items,
            visible_items,
            current_offset: 0,
            is_dragging: false,
            drag_start_y: px(0.),
            drag_start_offset: 0,
        }
    }

    pub fn set_metrics(&mut self, total_items: usize, visible_items: usize, offset: usize) {
        self.total_items = total_items;
        self.visible_items = visible_items;
        self.current_offset = offset.min(total_items.saturating_sub(visible_items));
    }

    pub fn can_scroll(&self) -> bool {
        self.total_items > self.visible_items && self.visible_items > 0
    }

    pub fn scroll_ratio(&self) -> f32 {
        if !self.can_scroll() {
            return 0.0;
        }
        let max_offset = (self.total_items - self.visible_items) as f32;
        (self.current_offset as f32 / max_offset).clamp(0.0, 1.0)
    }

    pub fn thumb_ratio(&self) -> f32 {
        if self.total_items == 0 {
            return 1.0;
        }
        (self.visible_items as f32 / self.total_items as f32).clamp(0.05, 1.0)
    }
}

/// Live scroll metrics derived from a [`UniformListScrollHandle`].
#[derive(Debug, Clone, Copy, Default)]
pub struct UniformListScrollMetrics {
    pub viewport_height: Pixels,
    pub content_height: Pixels,
    pub scroll_px: Pixels,
    pub max_scroll_px: Pixels,
    pub visible_items: usize,
    pub current_top: usize,
    pub thumb_ratio: f32,
    pub scroll_ratio: f32,
    pub can_scroll: bool,
}

/// Interactive drag and bounds state for a `uniform_list` scrollbar.
#[derive(Debug, Clone, Default)]
pub struct ScrollbarDrag {
    pub is_dragging: bool,
    pub drag_start_y: Pixels,
    pub drag_start_scroll_px: Pixels,
    pub is_hovered: bool,
    pub track_bounds: Bounds<Pixels>,
}

/// Computes live pixel and item scroll metrics for a `uniform_list` handle.
///
/// GPUI's `UniformList` does not populate `ScrollHandle::child_bounds`, so
/// `base_handle.logical_scroll_top()` always returns `0`. Reading `offset().y`
/// and `last_item_size` / `bounds()` gives the true scroll position.
pub fn uniform_list_scroll_metrics(
    scroll_handle: &UniformListScrollHandle,
    row_height: Pixels,
    item_count: usize,
    fallback_visible: usize,
) -> UniformListScrollMetrics {
    let state = scroll_handle.0.borrow();
    let viewport_height = state
        .last_item_size
        .map(|s| s.item.height)
        .filter(|h| *h > px(0.))
        .unwrap_or_else(|| state.base_handle.bounds().size.height)
        .max(px(0.));

    let content_height = if item_count == 0 {
        px(0.)
    } else if row_height > px(0.) {
        row_height * (item_count as f32)
    } else {
        state
            .last_item_size
            .map(|s| s.contents.height)
            .unwrap_or(px(0.))
    };

    let visible_items = if viewport_height > px(0.) && row_height > px(0.) {
        ((viewport_height / row_height).floor() as usize).max(1)
    } else {
        fallback_visible
    };

    let max_scroll_px = (content_height - viewport_height).max(px(0.));
    let mut scroll_px = (-state.base_handle.offset().y).clamp(px(0.), max_scroll_px);
    if let Some(deferred) = state.deferred_scroll_to_item {
        if viewport_height > px(0.) && row_height > px(0.) && item_count > 0 {
            let mut ix = deferred.item_index.min(item_count - 1);
            if state.y_flipped {
                ix = item_count.saturating_sub(ix + 1);
            }
            let item_top = row_height * (ix as f32);
            let item_bottom = item_top + row_height;
            let offset_px = row_height * (deferred.offset as f32);
            let needs_scroll = deferred.scroll_strict
                || item_top < scroll_px + offset_px
                || item_bottom > scroll_px + viewport_height;
            if needs_scroll {
                let target = match deferred.strategy {
                    ScrollStrategy::Top => item_top - offset_px,
                    ScrollStrategy::Center => {
                        let item_center = item_top + row_height / 2.0;
                        let vp_center = offset_px + (viewport_height - offset_px) / 2.0;
                        item_center - vp_center
                    }
                    ScrollStrategy::Bottom => item_bottom - viewport_height + offset_px,
                };
                scroll_px = target.clamp(px(0.), max_scroll_px);
            }
        }
    }
    let can_scroll = viewport_height > px(0.) && max_scroll_px > px(0.);

    let current_top = if row_height > px(0.) {
        ((scroll_px / row_height).floor() as usize).min(item_count.saturating_sub(visible_items))
    } else {
        0
    };

    let thumb_ratio = if !can_scroll || content_height <= px(0.) {
        1.0
    } else {
        (viewport_height / content_height).clamp(0.05, 1.0)
    };

    let scroll_ratio = if !can_scroll {
        0.0
    } else if max_scroll_px - scroll_px <= px(0.05) {
        1.0
    } else {
        (scroll_px / max_scroll_px).clamp(0.0, 1.0)
    };

    UniformListScrollMetrics {
        viewport_height,
        content_height,
        scroll_px,
        max_scroll_px,
        visible_items,
        current_top,
        thumb_ratio,
        scroll_ratio,
        can_scroll,
    }
}

fn set_uniform_list_scroll_y(scroll_handle: &UniformListScrollHandle, offset_y: Pixels) {
    let mut state = scroll_handle.0.borrow_mut();
    state.deferred_scroll_to_item = None;
    let current_x = state.base_handle.offset().x;
    state.base_handle.set_offset(point(current_x, offset_y));
}

fn thumb_geometry(
    track_bounds: Bounds<Pixels>,
    thumb_ratio: f32,
    scroll_ratio: f32,
) -> (Pixels, Pixels, Pixels) {
    let track_h = track_bounds.size.height.max(px(0.));
    let min_thumb = px(16.).min(track_h);
    let thumb_h = (track_h * thumb_ratio).clamp(min_thumb, track_h);
    let usable_h = (track_h - thumb_h).max(px(0.));
    let thumb_top = track_bounds.top() + usable_h * scroll_ratio.clamp(0.0, 1.0);
    (thumb_top, thumb_h, usable_h)
}

/// Renders a synchronized vertical scrollbar for a `uniform_list`.
///
/// Painting via `canvas` after the sibling `uniform_list` prepaints ensures the
/// thumb reads the post-layout scroll offset (including deferred `scroll_to_item`
/// updates) in the same frame, and window-level drag listeners keep dragging
/// smooth even when the pointer moves horizontally outside the narrow track.
pub fn uniform_list_scrollbar(
    scroll_handle: UniformListScrollHandle,
    drag: Rc<RefCell<ScrollbarDrag>>,
    row_height: Pixels,
    item_count: usize,
    fallback_visible: usize,
    width: Pixels,
    theme: &Theme,
) -> impl IntoElement {
    let thumb_color = theme.border_strong;
    let thumb_hover_color = theme.text_muted;
    let border_color = theme.border;

    let drag_for_prepaint = Rc::clone(&drag);
    let drag_for_paint = drag;

    div()
        .w(width)
        .h_full()
        .border_l_1()
        .border_color(border_color)
        .child(
            canvas(
                move |track_bounds, window, _cx| {
                    drag_for_prepaint.borrow_mut().track_bounds = track_bounds;
                    window.insert_hitbox(track_bounds, HitboxBehavior::Normal)
                },
                move |track_bounds, hitbox, window, _cx| {
                    let metrics = uniform_list_scroll_metrics(
                        &scroll_handle,
                        row_height,
                        item_count,
                        fallback_visible,
                    );
                    if !metrics.can_scroll {
                        drag_for_paint.borrow_mut().is_dragging = false;
                        return;
                    }

                    let (thumb_top, thumb_h, _) =
                        thumb_geometry(track_bounds, metrics.thumb_ratio, metrics.scroll_ratio);
                    let thumb_bounds = Bounds::new(
                        point(track_bounds.left(), thumb_top),
                        size(track_bounds.size.width, thumb_h),
                    );

                    let is_dragging = drag_for_paint.borrow().is_dragging;
                    let is_hovered = hitbox.is_hovered(window)
                        && thumb_bounds.contains(&window.mouse_position());
                    drag_for_paint.borrow_mut().is_hovered = is_hovered;

                    if hitbox.is_hovered(window) || is_dragging {
                        window.set_cursor_style(CursorStyle::Arrow, &hitbox);
                    }

                    let color = if is_dragging || is_hovered {
                        thumb_hover_color
                    } else {
                        thumb_color
                    };
                    window.paint_quad(fill(thumb_bounds, color).corner_radii(px(2.)));

                    let current_view = window.current_view();

                    let scroll_down = scroll_handle.clone();
                    let drag_down = Rc::clone(&drag_for_paint);
                    let hitbox_down = hitbox.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble
                            && event.button == MouseButton::Left
                            && hitbox_down.is_hovered(window)
                        {
                            cx.stop_propagation();
                            let cur = uniform_list_scroll_metrics(
                                &scroll_down,
                                row_height,
                                item_count,
                                fallback_visible,
                            );
                            if !cur.can_scroll {
                                return;
                            }
                            let (cur_thumb_top, cur_thumb_h, cur_usable_h) =
                                thumb_geometry(track_bounds, cur.thumb_ratio, cur.scroll_ratio);
                            let on_thumb = event.position.y >= cur_thumb_top
                                && event.position.y <= cur_thumb_top + cur_thumb_h;

                            let start_scroll_px = if on_thumb {
                                cur.scroll_px
                            } else {
                                let click_y = event.position.y - track_bounds.top();
                                let target_top =
                                    (click_y - cur_thumb_h / 2.0).clamp(px(0.), cur_usable_h);
                                let ratio = if cur_usable_h > px(0.) {
                                    target_top / cur_usable_h
                                } else {
                                    0.0
                                };
                                let target_scroll_px = cur.max_scroll_px * ratio;
                                set_uniform_list_scroll_y(&scroll_down, -target_scroll_px);
                                target_scroll_px
                            };

                            let mut d = drag_down.borrow_mut();
                            d.is_dragging = true;
                            d.drag_start_y = event.position.y;
                            d.drag_start_scroll_px = start_scroll_px;
                            cx.notify(current_view);
                        }
                    });

                    let scroll_move = scroll_handle;
                    let drag_move = Rc::clone(&drag_for_paint);
                    let hitbox_move = hitbox;
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        let is_dragging = drag_move.borrow().is_dragging;
                        if is_dragging {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            if event.pressed_button != Some(MouseButton::Left) {
                                drag_move.borrow_mut().is_dragging = false;
                                cx.notify(current_view);
                                return;
                            }
                            cx.stop_propagation();
                            let cur = uniform_list_scroll_metrics(
                                &scroll_move,
                                row_height,
                                item_count,
                                fallback_visible,
                            );
                            if !cur.can_scroll {
                                return;
                            }
                            let (_, _, cur_usable_h) =
                                thumb_geometry(track_bounds, cur.thumb_ratio, cur.scroll_ratio);
                            if cur_usable_h <= px(0.) {
                                return;
                            }
                            let d = drag_move.borrow();
                            let delta_y = event.position.y - d.drag_start_y;
                            let delta_ratio = delta_y / cur_usable_h;
                            let new_scroll_px = (d.drag_start_scroll_px
                                + cur.max_scroll_px * delta_ratio)
                                .clamp(px(0.), cur.max_scroll_px);
                            drop(d);
                            set_uniform_list_scroll_y(&scroll_move, -new_scroll_px);
                            cx.notify(current_view);
                        } else if phase == DispatchPhase::Bubble {
                            let now_hovered = hitbox_move.is_hovered(window)
                                && thumb_bounds.contains(&event.position);
                            let mut d = drag_move.borrow_mut();
                            if d.is_hovered != now_hovered {
                                d.is_hovered = now_hovered;
                                cx.notify(current_view);
                            }
                        }
                    });

                    let drag_up = drag_for_paint;
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                        if phase == DispatchPhase::Capture
                            && event.button == MouseButton::Left
                            && drag_up.borrow().is_dragging
                        {
                            drag_up.borrow_mut().is_dragging = false;
                            cx.notify(current_view);
                        }
                    });
                },
            )
            .size_full(),
        )
}

/// A draggable vertical scrollbar view.
pub struct Scrollbar {
    pub state: ScrollbarState,
    pub track_color: Rgba,
    pub thumb_color: Rgba,
    pub thumb_hover_color: Rgba,
    pub width: Pixels,
}

impl Default for Scrollbar {
    fn default() -> Self {
        let theme = Theme::default();
        Self {
            state: ScrollbarState::default(),
            track_color: transparent_black().into(),
            thumb_color: theme.border_strong,
            thumb_hover_color: theme.text_muted,
            width: px(8.),
        }
    }
}

impl Scrollbar {
    pub fn new(state: ScrollbarState) -> Self {
        Self {
            state,
            ..Default::default()
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }
}

impl Render for Scrollbar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.can_scroll() {
            return div().w(self.width).h_full();
        }

        let thumb_ratio = self.state.thumb_ratio();
        let scroll_ratio = self.state.scroll_ratio();

        div()
            .w(self.width)
            .h_full()
            .bg(self.track_color)
            .rounded_sm()
            .relative()
            .child(
                div()
                    .w_full()
                    .h(relative(thumb_ratio))
                    .top(relative(scroll_ratio * (1.0 - thumb_ratio)))
                    .rounded_sm()
                    .bg(self.thumb_color)
                    .hover(|s| s.bg(self.thumb_hover_color))
                    .cursor(CursorStyle::Arrow),
            )
    }
}
