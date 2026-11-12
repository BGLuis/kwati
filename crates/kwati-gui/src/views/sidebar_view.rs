//! Sidebar view (owned by guinet-worker for #61).
//!
//! Renders quick-access places, bookmarks, and mounted/removable storage drives.

use gpui::{div, prelude::*, px, rgba, Context, Entity, Window};

use crate::state::AppState;
use crate::theme::Theme;

pub struct SidebarView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
}

impl SidebarView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(200.))
            .h_full()
            .p_3()
            .bg(self.theme.bg_sidebar)
            .border_r_1()
            .border_color(self.theme.border)
            .flex()
            .flex_col()
            .gap_4()
            // Places Section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.theme.text_muted)
                            .child("PLACES"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff0d)))
                            .text_size(px(13.))
                            .text_color(self.theme.text_primary)
                            .child("🏠 Home"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff0d)))
                            .text_size(px(13.))
                            .text_color(self.theme.text_primary)
                            .child("📁 Documents"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff0d)))
                            .text_size(px(13.))
                            .text_color(self.theme.text_primary)
                            .child("📥 Downloads"),
                    ),
            )
            // Drives Section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.theme.text_muted)
                            .child("DRIVES"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff0d)))
                            .text_size(px(13.))
                            .text_color(self.theme.text_primary)
                            .child("💾 Root"),
                    ),
            )
    }
}
