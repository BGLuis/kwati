//! Operation dialogs view (owned by guiops-worker for #17, #19, #35).
//!
//! Provides modals and inline prompts for rename, new folder, trash confirmation,
//! and permanent delete warnings.

use std::path::PathBuf;

use gpui::{div, prelude::*, px, rgb, rgba, Context, Entity, MouseButton, Window};

use crate::actions::operations::{execute_create_folder, execute_rename, execute_trash};
use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    None,
    NewFolder {
        parent_path: PathBuf,
        error: Option<String>,
    },
    Rename {
        source_path: PathBuf,
        current_name: String,
        error: Option<String>,
    },
    ConfirmTrash {
        paths: Vec<PathBuf>,
    },
    ConfirmDelete {
        count: usize,
    },
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

    /// Opens the New Folder dialog with a default or cleared name.
    pub fn open_new_folder(&mut self, parent: PathBuf, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("New Folder", cx);
        });
        self.current_dialog = DialogKind::NewFolder {
            parent_path: parent,
            error: None,
        };
        cx.notify();
    }

    /// Opens the Rename dialog for a given source entry.
    pub fn open_rename(&mut self, source: PathBuf, cx: &mut Context<Self>) {
        let current_name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let name_clone = current_name.clone();
        self.input.update(cx, |input, cx| {
            input.set_text(name_clone, cx);
        });
        self.current_dialog = DialogKind::Rename {
            source_path: source,
            current_name,
            error: None,
        };
        cx.notify();
    }

    /// Opens the Move to Trash confirmation prompt.
    pub fn open_confirm_trash(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.current_dialog = DialogKind::ConfirmTrash { paths };
        cx.notify();
    }

    /// Submits the current active dialog (NewFolder or Rename).
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let input_text = self.input.read(cx).text().to_string();

        match &self.current_dialog {
            DialogKind::NewFolder { parent_path, .. } => {
                let parent = parent_path.clone();
                let mut error_opt = None;
                self.app_state.update(cx, |state, _cx| {
                    match execute_create_folder(state, &parent, &input_text) {
                        Ok(_) => {}
                        Err(err) => {
                            state.set_status(err.clone());
                            error_opt = Some(err);
                        }
                    }
                });

                if let Some(err) = error_opt {
                    self.current_dialog = DialogKind::NewFolder {
                        parent_path: parent,
                        error: Some(err),
                    };
                    cx.notify();
                } else {
                    self.close(cx);
                }
            }
            DialogKind::Rename {
                source_path,
                current_name,
                ..
            } => {
                let source = source_path.clone();
                let original_name = current_name.clone();
                let mut error_opt = None;

                self.app_state.update(cx, |state, _cx| {
                    match execute_rename(state, &source, &input_text) {
                        Ok(_) => {}
                        Err(err) => {
                            state.set_status(err.clone());
                            error_opt = Some(err);
                        }
                    }
                });

                if let Some(err) = error_opt {
                    self.current_dialog = DialogKind::Rename {
                        source_path: source,
                        current_name: original_name,
                        error: Some(err),
                    };
                    cx.notify();
                } else {
                    self.close(cx);
                }
            }
            DialogKind::ConfirmTrash { paths } => {
                let targets = paths.clone();
                self.app_state.update(cx, |state, _cx| {
                    let _ = execute_trash(state, &targets);
                });
                self.close(cx);
            }
            DialogKind::ConfirmDelete { .. } => {
                self.close(cx);
            }
            DialogKind::None => {}
        }
    }
}

impl Render for DialogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.current_dialog == DialogKind::None {
            return div();
        }

        let (title, error_msg, is_input_dialog, prompt_text) = match &self.current_dialog {
            DialogKind::NewFolder { error, .. } => {
                ("Create New Folder", error.as_deref(), true, None)
            }
            DialogKind::Rename { error, .. } => ("Rename Entry", error.as_deref(), true, None),
            DialogKind::ConfirmTrash { paths } => {
                let msg = if paths.len() == 1 {
                    format!(
                        "Move '{}' to the system trash?",
                        paths[0].file_name().unwrap_or_default().to_string_lossy()
                    )
                } else {
                    format!("Move {} selected items to the system trash?", paths.len())
                };
                ("Move to Trash?", None, false, Some(msg))
            }
            DialogKind::ConfirmDelete { count } => {
                let _ = count;
                ("Permanently Delete?", None, false, None)
            }
            DialogKind::None => ("", None, false, None),
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
                    .children(prompt_text.map(|prompt| {
                        div()
                            .text_size(px(12.))
                            .text_color(self.theme.text_secondary)
                            .child(prompt)
                    }))
                    // Input element if rename or new folder
                    .children(if is_input_dialog {
                        Some(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(self.input.clone())
                                .children(error_msg.map(|err| {
                                    div()
                                        .text_size(px(11.))
                                        .text_color(self.theme.text_error)
                                        .child(err.to_string())
                                })),
                        )
                    } else {
                        None
                    })
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
                                        cx.listener(|this, _, _, cx| this.submit(cx)),
                                    ),
                            ),
                    ),
            )
    }
}
