//! Widths of the sidebar and preview panes: what the user dragged them to, and what fits the
//! window. The file list takes whatever is left and never drops below [`LIST_MIN`].

use gpui::{px, Pixels};

pub const SIDEBAR_COMPACT: Pixels = px(48.);
pub const SIDEBAR_MIN: Pixels = px(160.);
pub const SIDEBAR_DEFAULT: Pixels = px(200.);
pub const SIDEBAR_MAX: Pixels = px(360.);
pub const PREVIEW_MIN: Pixels = px(200.);
pub const PREVIEW_DEFAULT: Pixels = px(280.);
pub const PREVIEW_MAX: Pixels = px(600.);
/// Room for the size, date, and hidden columns plus a readable stretch of the name column.
pub const LIST_MIN: Pixels = px(480.);
/// The narrowest window: a compact sidebar beside the list, with the preview hidden.
pub const WINDOW_MIN_WIDTH: Pixels = px(540.);
pub const WINDOW_MIN_HEIGHT: Pixels = px(360.);

/// Dragging the sidebar edge left of this point collapses it to icons.
const SIDEBAR_SNAP: Pixels = px(104.);
/// Dragging the preview edge closer than this to the window's right edge closes the preview.
const PREVIEW_SNAP: Pixels = px(100.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarWidth {
    Compact,
    Expanded(Pixels),
}

/// The pane widths the user asked for. The window may be too narrow to honour them, so the
/// widths on screen come from [`PanelLayout::resolve`] and the request is kept for when the
/// window grows again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelLayout {
    pub sidebar: SidebarWidth,
    pub preview: Pixels,
    /// Closing the preview keeps its width for when it opens again.
    pub preview_open: bool,
}

/// Pane widths that fit a given window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedLayout {
    pub sidebar: Pixels,
    pub sidebar_compact: bool,
    /// `None` when the preview is closed or the window is too narrow to show it beside the
    /// list.
    pub preview: Option<Pixels>,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            sidebar: SidebarWidth::Expanded(SIDEBAR_DEFAULT),
            preview: PREVIEW_DEFAULT,
            preview_open: true,
        }
    }
}

impl PanelLayout {
    /// Gives way in order of least loss: the preview narrows, then the sidebar narrows, then
    /// the sidebar drops to icons, and only then the preview hides.
    pub fn resolve(&self, window_width: Pixels) -> ResolvedLayout {
        let room = window_width - LIST_MIN;
        let preview = self.preview;

        if !self.preview_open {
            return match self.sidebar {
                SidebarWidth::Expanded(sidebar) if SIDEBAR_MIN <= room => ResolvedLayout {
                    sidebar: sidebar.min(room),
                    sidebar_compact: false,
                    preview: None,
                },
                _ => ResolvedLayout {
                    sidebar: SIDEBAR_COMPACT,
                    sidebar_compact: true,
                    preview: None,
                },
            };
        }

        if let SidebarWidth::Expanded(sidebar) = self.sidebar {
            if sidebar + PREVIEW_MIN <= room {
                return ResolvedLayout {
                    sidebar,
                    sidebar_compact: false,
                    preview: Some(preview.min(room - sidebar)),
                };
            }
            if SIDEBAR_MIN + PREVIEW_MIN <= room {
                return ResolvedLayout {
                    sidebar: room - PREVIEW_MIN,
                    sidebar_compact: false,
                    preview: Some(PREVIEW_MIN),
                };
            }
        }

        if SIDEBAR_COMPACT + PREVIEW_MIN <= room {
            return ResolvedLayout {
                sidebar: SIDEBAR_COMPACT,
                sidebar_compact: true,
                preview: Some(preview.min(room - SIDEBAR_COMPACT)),
            };
        }

        ResolvedLayout {
            sidebar: SIDEBAR_COMPACT,
            sidebar_compact: true,
            preview: None,
        }
    }

    /// Sets the sidebar from the pointer's distance to the left edge of the window. The
    /// sidebar stops widening where the list and a minimal preview would no longer fit.
    pub fn drag_sidebar(&mut self, pointer: Pixels, window_width: Pixels) {
        if pointer < SIDEBAR_SNAP {
            self.sidebar = SidebarWidth::Compact;
            return;
        }
        let preview = if self.preview_open {
            PREVIEW_MIN
        } else {
            px(0.)
        };
        let fits = window_width - LIST_MIN - preview;
        let max = SIDEBAR_MAX.min(fits).max(SIDEBAR_MIN);
        self.sidebar = SidebarWidth::Expanded(pointer.clamp(SIDEBAR_MIN, max));
    }

    pub fn toggle_preview(&mut self) {
        self.preview_open = !self.preview_open;
    }

    /// Sets the preview from the pointer's distance to the right edge of the window, closing
    /// it near that edge. The preview stops widening where it would push the list below its
    /// minimum.
    pub fn drag_preview(&mut self, pointer_from_right: Pixels, window_width: Pixels) {
        if pointer_from_right < PREVIEW_SNAP {
            self.preview_open = false;
            return;
        }
        let sidebar = self.resolve(window_width).sidebar;
        let fits = window_width - LIST_MIN - sidebar;
        let max = PREVIEW_MAX.min(fits).max(PREVIEW_MIN);
        self.preview = pointer_from_right.clamp(PREVIEW_MIN, max);
    }
}
