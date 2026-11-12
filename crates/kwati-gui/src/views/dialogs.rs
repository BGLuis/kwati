//! Operation dialogs view (owned by guiops-worker for #17, #19, #35).
//!
//! Provides modals and inline prompts for rename, new folder, trash confirmation,
//! and permanent delete warnings.

use gpui::{div, prelude::*, px, rgb, rgba, Context, Entity, MouseButton, Window};

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    None,
    NewFolder,
    Rename { current_name: String },
    ConfirmTrash { count: usize },
    ConfirmDelete { count: usize },
}

pub struct DialogsView {
    pub app_state: Entity<AppState>,
    pub input: Entity<TextInput>,
    pub current_dialog: DialogKind,
    pub theme: Theme,
}

impl DialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        Self {
            app_state,
            input,
            current_dialog: DialogKind::None,
            theme: Theme::dark(),
        }
    }

    pub fn show(&mut self, dialog: DialogKind, cx: &mut Context<Self>) {
        self.current_dialog = dialog;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.current_dialog = DialogKind::None;
        cx.notify();
    }
}

impl Render for DialogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.current_dialog == DialogKind::None {
            return div();
        }

        let title = match &self.current_dialog {
            DialogKind::NewFolder => "Create New Folder",
            DialogKind::Rename { .. } => "Rename Entry",
            DialogKind::ConfirmTrash { count: _ } => "Move to Trash?",
            DialogKind::ConfirmDelete { count: _ } => "Permanently Delete?",
            DialogKind::None => "",
        };

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .child(
                div()
                    .w(px(380.))
                    .p_4()
                    .rounded_lg()
                    .bg(self.theme.bg_panel)
                    .border_1()
                    .border_color(self.theme.border)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(self.theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(self.theme.bg_header)
                                    .text_color(self.theme.text_secondary)
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(self.theme.accent)
                                    .text_color(rgb(0x181825))
                                    .cursor_pointer()
                                    .child("Confirm")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close(cx)),
                                    ),
                            ),
                    ),
            )
    }
}
