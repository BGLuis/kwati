//! Operation dialogs view (owned by guiops-worker for #17, #19, #35).
//!
//! Provides modals and inline prompts for rename, new folder, trash confirmation,
//! and permanent delete warnings.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, transparent_black, Context, Entity, FocusHandle, KeyDownEvent,
    MouseButton, Window,
};
use kwati_core::ConflictPolicy;

use crate::actions::open_with::{
    applications_for, launch_application, launch_custom_command, AppInfo,
};
use crate::actions::operations::{
    execute_create_folder, execute_permanent_delete, execute_rename, execute_trash, start_transfer,
    Transfer,
};
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
        paths: Vec<PathBuf>,
    },
    ConflictResolution {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        is_cut: bool,
        conflict_name: String,
    },
    OpenWith {
        target_path: PathBuf,
        available_apps: Vec<AppInfo>,
        selected_index: Option<usize>,
        error: Option<String>,
    },
}

pub struct DialogsView {
    pub app_state: Entity<AppState>,
    pub input: Entity<TextInput>,
    pub current_dialog: DialogKind,
    focus_handle: FocusHandle,
}

impl DialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        Self {
            app_state,
            input,
            current_dialog: DialogKind::None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Gives the open dialog the keyboard: its text field when it has one, so the name can
    /// be typed at once, otherwise the dialog itself, so Enter and Escape reach it.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.current_dialog {
            DialogKind::None => {}
            DialogKind::NewFolder { .. } | DialogKind::Rename { .. } => {
                self.input.read(cx).focus_handle.focus(window)
            }
            _ => self.focus_handle.focus(window),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "enter" || key == "escape" {
            // Otherwise Enter goes on to the text field as a line break.
            cx.stop_propagation();
        }
        match key {
            "escape" => self.close(cx),
            // Enter never confirms a permanent deletion, and a name conflict has no default
            // policy: both need a deliberate click.
            "enter"
                if !matches!(
                    self.current_dialog,
                    DialogKind::ConfirmDelete { .. } | DialogKind::ConflictResolution { .. }
                ) =>
            {
                self.submit(cx)
            }
            _ => {}
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

    /// Opens the permanent deletion confirmation modal with irreversible warning.
    pub fn open_confirm_delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.current_dialog = DialogKind::ConfirmDelete { paths };
        cx.notify();
    }

    /// Opens the conflict resolution dialog when paste encounters an existing target.
    pub fn open_conflict_resolution(
        &mut self,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        is_cut: bool,
        conflict_name: String,
        cx: &mut Context<Self>,
    ) {
        self.current_dialog = DialogKind::ConflictResolution {
            sources,
            dest_dir,
            is_cut,
            conflict_name,
        };
        cx.notify();
    }

    /// Resolves the current conflict with the chosen ConflictPolicy.
    pub fn resolve_conflict(&mut self, policy: ConflictPolicy, cx: &mut Context<Self>) {
        if let DialogKind::ConflictResolution {
            sources,
            dest_dir,
            is_cut,
            ..
        } = &self.current_dialog
        {
            let transfer = if *is_cut {
                Transfer::moving(sources.clone(), dest_dir.clone(), policy)
            } else {
                Transfer::copy(sources.clone(), dest_dir.clone(), policy)
            };
            self.app_state
                .update(cx, |state, _cx| start_transfer(state, transfer));
        }
        self.close(cx);
    }

    /// Opens the "Open With..." application picker dialog.
    pub fn open_open_with(&mut self, target: PathBuf, cx: &mut Context<Self>) {
        let apps = applications_for(&target);
        self.input.update(cx, |input, cx| {
            input.set_text("", cx);
        });
        self.current_dialog = DialogKind::OpenWith {
            target_path: target,
            available_apps: apps,
            selected_index: Some(0),
            error: None,
        };
        cx.notify();
    }

    /// Selects an application in the Open With dialog.
    pub fn select_app(&mut self, index: usize, cx: &mut Context<Self>) {
        if let DialogKind::OpenWith {
            ref mut selected_index,
            ..
        } = self.current_dialog
        {
            *selected_index = Some(index);
            cx.notify();
        }
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
            DialogKind::ConfirmDelete { paths } => {
                let targets = paths.clone();
                self.app_state.update(cx, |state, _cx| {
                    let _ = execute_permanent_delete(state, &targets);
                });
                self.close(cx);
            }
            DialogKind::ConflictResolution { .. } => {
                self.close(cx);
            }
            DialogKind::OpenWith {
                target_path,
                available_apps,
                selected_index,
                ..
            } => {
                let custom_cmd = self.input.read(cx).text().trim().to_string();
                let target = target_path.clone();
                let res = if !custom_cmd.is_empty() {
                    launch_custom_command(&custom_cmd, &target)
                } else if let Some(idx) = *selected_index {
                    if let Some(app) = available_apps.get(idx) {
                        launch_application(app, &target)
                    } else {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "no application selected",
                        ))
                    }
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no application selected",
                    ))
                };

                match res {
                    Ok(_) => self.close(cx),
                    Err(err) => {
                        let err_msg = err.to_string();
                        self.app_state.update(cx, |state, _cx| {
                            state.set_status(err_msg.clone());
                        });
                        self.current_dialog = DialogKind::OpenWith {
                            target_path: target,
                            available_apps: available_apps.clone(),
                            selected_index: *selected_index,
                            error: Some(err_msg),
                        };
                        cx.notify();
                    }
                }
            }
            DialogKind::None => {}
        }
    }
}

impl Render for DialogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if self.current_dialog == DialogKind::None {
            return div();
        }

        let is_conflict_dialog =
            matches!(self.current_dialog, DialogKind::ConflictResolution { .. });
        let is_delete_dialog = matches!(self.current_dialog, DialogKind::ConfirmDelete { .. });
        let is_open_with = matches!(self.current_dialog, DialogKind::OpenWith { .. });

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
            DialogKind::ConfirmDelete { paths } => {
                let msg = if paths.len() == 1 {
                    format!(
                        "Permanently delete '{}'? This action is irreversible and cannot be undone.",
                        paths[0].file_name().unwrap_or_default().to_string_lossy()
                    )
                } else {
                    format!(
                        "Permanently delete {} selected items? This action is irreversible and cannot be undone.",
                        paths.len()
                    )
                };
                ("Permanently Delete?", None, false, Some(msg))
            }
            DialogKind::ConflictResolution { conflict_name, .. } => {
                let msg = format!(
                    "Target '{}' already exists in the destination folder. Choose conflict policy:",
                    conflict_name
                );
                ("Name Conflict", None, false, Some(msg))
            }
            DialogKind::OpenWith { target_path, .. } => {
                let file_name = target_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| target_path.to_string_lossy().to_string());
                let msg = format!("Choose an application to open '{}':", file_name);
                ("Open with...", None, false, Some(msg))
            }
            DialogKind::None => ("", None, false, None),
        };

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.scrim)
            .occlude()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .w(px(420.))
                    .p_4()
                    .rounded_lg()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_size(px(14.)).text_color(theme.text).child(title))
                    .children(prompt_text.map(|prompt| {
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
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
                                        .text_color(theme.danger)
                                        .child(err.to_string())
                                })),
                        )
                    } else {
                        None
                    })
                    .children(
                        if let DialogKind::OpenWith {
                            ref available_apps,
                            selected_index,
                            ref error,
                            ..
                        } = self.current_dialog
                        {
                            let mut app_nodes = Vec::new();
                            for (ix, app) in available_apps.iter().enumerate() {
                                let is_sel = selected_index == Some(ix);
                                let app_name = app.name.clone();
                                let is_default = app.is_default;
                                app_nodes.push(
                                    div()
                                        .id(ix)
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .bg(if is_sel {
                                            theme.selection
                                        } else {
                                            transparent_black().into()
                                        })
                                        .hover(|s| s.bg(theme.surface_raised))
                                        .text_size(px(12.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(if is_sel { "🔘" } else { "⚪" })
                                                .child(
                                                    div()
                                                        .text_color(if is_sel {
                                                            theme.accent
                                                        } else {
                                                            theme.text
                                                        })
                                                        .child(app_name),
                                                ),
                                        )
                                        .children(if is_default {
                                            Some(
                                                div()
                                                    .text_size(px(10.))
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_sm()
                                                    .bg(theme.surface)
                                                    .text_color(theme.text_muted)
                                                    .child("Default"),
                                            )
                                        } else {
                                            None
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.select_app(ix, cx);
                                            }),
                                        ),
                                );
                            }

                            Some(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .max_h(px(160.))
                                            .overflow_hidden()
                                            .children(app_nodes),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.text_muted)
                                            .child("Or specify custom command / executable:"),
                                    )
                                    .child(self.input.clone())
                                    .children(error.as_ref().map(|err| {
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.danger)
                                            .child(err.clone())
                                    })),
                            )
                        } else {
                            None
                        },
                    )
                    .children(if is_conflict_dialog {
                        Some(
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
                                        .bg(theme.surface_raised)
                                        .text_color(theme.text_muted)
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
                                        .bg(theme.surface_raised)
                                        .text_color(theme.text)
                                        .cursor_pointer()
                                        .child("Skip")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(ConflictPolicy::Skip, cx)
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme.surface_raised)
                                        .text_color(theme.text)
                                        .cursor_pointer()
                                        .child("Keep Both")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(ConflictPolicy::KeepBoth, cx)
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme.accent)
                                        .text_color(theme.on_accent)
                                        .cursor_pointer()
                                        .child("Replace")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.resolve_conflict(ConflictPolicy::Replace, cx)
                                            }),
                                        ),
                                ),
                        )
                    } else {
                        Some(
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
                                        .bg(theme.surface_raised)
                                        .text_color(theme.text_muted)
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
                                        .bg(if is_delete_dialog {
                                            theme.danger
                                        } else {
                                            theme.accent
                                        })
                                        .text_color(theme.on_accent)
                                        .cursor_pointer()
                                        .child(if is_delete_dialog {
                                            "Permanently Delete"
                                        } else if is_open_with {
                                            "Open"
                                        } else {
                                            "Confirm"
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.submit(cx)),
                                        ),
                                ),
                        )
                    }),
            )
    }
}
