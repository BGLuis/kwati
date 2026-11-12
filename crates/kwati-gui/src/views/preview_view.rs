//! File preview side panel view (owned by inspect-worker for #15).
//!
//! Renders textual previews or image previews (via `img`) for the currently selected file.

use gpui::{div, img, prelude::*, px, Context, Entity, Window};

use crate::state::{AppState, PreviewKind};
use crate::theme::Theme;

pub struct PreviewView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
}

impl PreviewView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
        }
    }
}

impl Render for PreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let preview = state.preview_state.clone();

        let Some(preview_data) = preview else {
            return div()
                .w(px(250.))
                .h_full()
                .p_3()
                .bg(self.theme.bg_panel)
                .border_l_1()
                .border_color(self.theme.border)
                .flex()
                .items_center()
                .justify_center()
                .text_color(self.theme.text_muted)
                .text_size(px(12.))
                .child("No file selected");
        };

        let file_name = preview_data
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        div()
            .w(px(250.))
            .h_full()
            .p_3()
            .bg(self.theme.bg_panel)
            .border_l_1()
            .border_color(self.theme.border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(self.theme.text_primary)
                    .child(file_name),
            )
            .child(match preview_data.kind {
                PreviewKind::Text(text) => div()
                    .flex_1()
                    .p_2()
                    .rounded_md()
                    .bg(self.theme.bg_content)
                    .text_color(self.theme.text_secondary)
                    .text_size(px(11.))
                    .overflow_hidden()
                    .child(text),
                PreviewKind::Image(path) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(img(path).max_w_full().max_h_full()),
                PreviewKind::Binary { name: _, size } => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(self.theme.text_muted)
                    .child("Binary File")
                    .child(format!("Size: {} bytes", size)),
                PreviewKind::Loading => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(self.theme.text_muted)
                    .child("Loading preview..."),
            })
    }
}
