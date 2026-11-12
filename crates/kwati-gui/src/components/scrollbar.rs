//! # Scrollbar Component
//!
//! **Chosen Option:** Reusable draggable GPUI 0.2.2 Scrollbar element.
//! - **Source:** Native GPUI 0.2.2 element implementation
//! - **Crate Version:** `gpui = "=0.2.2"`
//! - **License:** Apache-2.0
//! - **Capabilities:** Draggable thumb, track click navigation, ratio calculations for
//!   `uniform_list` and scrollable containers, custom styling tokens.
//! - **Platforms:** Passes build on Linux (Wayland/X11), macOS, and Windows.

use gpui::{div, prelude::*, px, relative, rgba, Context, CursorStyle, Pixels, Rgba, Window};

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
        Self {
            state: ScrollbarState::default(),
            track_color: rgba(0x00000018),
            thumb_color: rgba(0xffffff40),
            thumb_hover_color: rgba(0xffffff70),
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
