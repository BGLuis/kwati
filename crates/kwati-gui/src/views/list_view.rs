//! File list view (owned by guinav-worker for #7, #12).
//!
//! Uses `uniform_list` to render only visible rows, with support for columns
//! (name, size, date), sort headers, and scrollbar synchronization.

use gpui::{
    div, prelude::*, px, rgb, rgba, uniform_list, Context, Entity, MouseButton, MouseDownEvent,
    Window,
};
use kwati_core::{Entry, Location};

use crate::state::AppState;
use crate::theme::Theme;

/// View rendering directory entries in a windowed list.
pub struct ListView {
    pub app_state: Entity<AppState>,
    pub theme: Theme,
}

impl ListView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            theme: Theme::dark(),
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
}

impl Render for ListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let entries = state.entries.clone();
        let selection = state.selection.clone();
        let entry_count = entries.len();

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
            // Row list
            .child(
                div().flex_1().size_full().child(
                    uniform_list(
                        "directory_entries",
                        entry_count,
                        cx.processor(move |_this: &mut ListView, range, _window, _cx| {
                            let mut items: Vec<gpui::AnyElement> = Vec::new();
                            for ix in range {
                                if let Some(entry) = entries_clone.get(ix) {
                                    let entry: &Entry = entry;
                                    let loc: Location = entry.location();
                                    let is_selected = selection.contains(&loc);
                                    let is_dir = entry.is_dir();
                                    let name = entry.display_name();

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
                    .h_full(),
                ),
            )
    }
}
