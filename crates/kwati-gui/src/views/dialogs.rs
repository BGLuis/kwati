//! Operation dialogs view (#17, #19, #35, #48).
//!
//! Provides modals and inline prompts for rename, new folder, trash confirmation,
//! permanent delete warnings, conflict resolution, and open-with selection.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, transparent_black, App, Context, Entity, FocusHandle, KeyDownEvent,
    MouseButton, Window,
};
use kwati_core::{volume_kind, Backend, Confirmed, ConflictPolicy, Location, VolumeKind};

use crate::actions::open_with::{
    applications_for, launch_application, launch_custom_command, AppInfo,
};
use crate::actions::operations::{
    apply_create_folder_result, apply_delete_report, apply_rename_result, apply_trash_report,
    check_paste_conflict_at, create_folder_on_backend, delete_locations_permanently_with_backends,
    location_display_name, rename_location_on_backend, start_transfer, trash_locations_on_backends,
    validate_entry_name, Transfer, TrashOutcome,
};
use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    None,
    NewFolder {
        parent: Location,
        error: Option<String>,
    },
    Rename {
        source: Location,
        current_name: String,
        error: Option<String>,
    },
    ConfirmTrash {
        locations: Vec<Location>,
    },
    ConfirmDelete {
        locations: Vec<Location>,
    },
    ConflictResolution {
        sources: Vec<Location>,
        dest_dir: Location,
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
    pending_op: bool,
    op_generation: u64,
}

fn format_item_names(locations: &[Location]) -> String {
    match locations {
        [] => "0 items".to_string(),
        [single] => format!("'{}'", location_display_name(single)),
        many => {
            let shown: Vec<String> = many.iter().take(5).map(location_display_name).collect();
            if many.len() <= 5 {
                format!("{} selected items ({})", many.len(), shown.join(", "))
            } else {
                format!(
                    "{} selected items ({}, and {} more)",
                    many.len(),
                    shown.join(", "),
                    many.len() - 5
                )
            }
        }
    }
}

/// Formats the permanent-deletion prompt naming the target items and warning it cannot be undone.
pub fn confirm_delete_message(locations: &[Location]) -> String {
    format!(
        "Permanently delete {}? This action is irreversible and cannot be undone.",
        format_item_names(locations)
    )
}

impl DialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        Self {
            app_state,
            input,
            current_dialog: DialogKind::None,
            focus_handle: cx.focus_handle(),
            pending_op: false,
            op_generation: 0,
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
        self.pending_op = false;
        self.op_generation = self.op_generation.wrapping_add(1);
        self.current_dialog = dialog;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.pending_op = false;
        self.op_generation = self.op_generation.wrapping_add(1);
        self.current_dialog = DialogKind::None;
        cx.notify();
    }

    /// Opens the New Folder dialog under `parent` (local or remote).
    pub fn open_new_folder(&mut self, parent: impl Into<Location>, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("New Folder", cx);
        });
        self.show(
            DialogKind::NewFolder {
                parent: parent.into(),
                error: None,
            },
            cx,
        );
    }

    /// Opens the Rename dialog for a local or remote `source` location.
    pub fn open_rename(&mut self, source: impl Into<Location>, cx: &mut Context<Self>) {
        let source = source.into();
        let current_name = location_display_name(&source);
        let name_clone = current_name.clone();
        self.input.update(cx, |input, cx| {
            input.set_text(name_clone, cx);
        });
        self.show(
            DialogKind::Rename {
                source,
                current_name,
                error: None,
            },
            cx,
        );
    }

    /// Opens the Move to Trash confirmation prompt.
    pub fn open_confirm_trash(
        &mut self,
        targets: impl IntoIterator<Item = impl Into<Location>>,
        cx: &mut Context<Self>,
    ) {
        let locations: Vec<Location> = targets.into_iter().map(Into::into).collect();
        self.show(DialogKind::ConfirmTrash { locations }, cx);
    }

    /// Opens the permanent deletion confirmation modal with irreversible warning.
    pub fn open_confirm_delete(
        &mut self,
        targets: impl IntoIterator<Item = impl Into<Location>>,
        cx: &mut Context<Self>,
    ) {
        let locations: Vec<Location> = targets.into_iter().map(Into::into).collect();
        self.show(DialogKind::ConfirmDelete { locations }, cx);
    }

    /// Chooses the appropriate deletion confirmation prompt: trash for local-disk entries,
    /// or permanent-delete confirmation when any target is remote or on a non-local volume.
    pub fn open_remove_confirmation(
        &mut self,
        targets: impl IntoIterator<Item = impl Into<Location>>,
        cx: &mut Context<Self>,
    ) {
        let locations: Vec<Location> = targets.into_iter().map(Into::into).collect();
        if locations.is_empty() {
            return;
        }
        let trash_available = locations.iter().all(
            |loc| matches!(loc, Location::Local(path) if volume_kind(path) == VolumeKind::Local),
        );
        if trash_available {
            self.open_confirm_trash(locations, cx);
        } else {
            self.open_confirm_delete(locations, cx);
        }
    }

    /// Opens the conflict resolution dialog when paste encounters an existing target.
    pub fn open_conflict_resolution(
        &mut self,
        sources: impl IntoIterator<Item = impl Into<Location>>,
        dest_dir: impl Into<Location>,
        is_cut: bool,
        conflict_name: String,
        cx: &mut Context<Self>,
    ) {
        self.show(
            DialogKind::ConflictResolution {
                sources: sources.into_iter().map(Into::into).collect(),
                dest_dir: dest_dir.into(),
                is_cut,
                conflict_name,
            },
            cx,
        );
    }

    /// Checks for paste conflicts on the background executor and either opens
    /// `ConflictResolution` or starts the transfer immediately.
    pub fn begin_paste(
        &mut self,
        sources: impl IntoIterator<Item = impl Into<Location>>,
        dest_dir: impl Into<Location>,
        is_cut: bool,
        cx: &mut Context<Self>,
    ) {
        let sources: Vec<Location> = sources.into_iter().map(Into::into).collect();
        let dest_dir = dest_dir.into();
        if sources.is_empty() {
            return;
        }

        let Some(dest_backend) = self.app_state.read(cx).backend_for(&dest_dir) else {
            self.app_state.update(cx, |state, _| {
                state.set_status("Destination server is not connected");
            });
            return;
        };

        let sources_check = sources.clone();
        let dest_check = dest_dir.clone();
        cx.spawn(async move |this, cx| {
            let conflict_res = cx
                .background_executor()
                .spawn(async move {
                    check_paste_conflict_at(&sources_check, &dest_check, &*dest_backend)
                })
                .await;

            let _ = this.update(cx, |this, cx| match conflict_res {
                Ok(Some(conflict)) => {
                    this.open_conflict_resolution(sources, dest_dir, is_cut, conflict, cx);
                }
                Ok(None) => {
                    let transfer = if is_cut {
                        Transfer::moving(sources, dest_dir, ConflictPolicy::Skip)
                    } else {
                        Transfer::copy(sources, dest_dir, ConflictPolicy::Skip)
                    };
                    this.app_state
                        .update(cx, |state, _| start_transfer(state, transfer));
                }
                Err(err) => {
                    this.app_state.update(cx, |state, _| {
                        state.set_status(format!("Paste failed: {err}"));
                    });
                }
            });
        })
        .detach();
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
        self.show(
            DialogKind::OpenWith {
                target_path: target,
                available_apps: apps,
                selected_index: Some(0),
                error: None,
            },
            cx,
        );
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

    fn resolve_backend_items(
        &self,
        targets: &[Location],
        cx: &App,
    ) -> Option<Vec<(Location, Arc<dyn Backend>)>> {
        let state = self.app_state.read(cx);
        targets
            .iter()
            .map(|loc| state.backend_for(loc).map(|b| (loc.clone(), b)))
            .collect()
    }

    /// Runs permanent deletion after the user explicitly clicks the confirmation button.
    /// This is the only dialog handler that constructs [`Confirmed::user_confirmed`].
    pub fn confirm_permanent_delete(&mut self, cx: &mut Context<Self>) {
        if self.pending_op {
            return;
        }
        let DialogKind::ConfirmDelete { locations } = &self.current_dialog else {
            return;
        };
        let targets = locations.clone();
        if targets.is_empty() {
            self.close(cx);
            return;
        }

        let confirmed = Confirmed::user_confirmed();
        let Some(items) = self.resolve_backend_items(&targets, cx) else {
            self.app_state.update(cx, |state, _| {
                state.set_status("Failed to delete permanently: server is not connected");
            });
            self.close(cx);
            return;
        };

        self.pending_op = true;
        self.op_generation = self.op_generation.wrapping_add(1);
        let gen = self.op_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move { delete_locations_permanently_with_backends(&items, confirmed) })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.op_generation != gen {
                    return;
                }
                this.pending_op = false;
                this.app_state.update(cx, |state, _| {
                    let _ = apply_delete_report(state, report);
                });
                this.close(cx);
            });
        })
        .detach();
    }

    /// Submits the current active dialog (NewFolder, Rename, ConfirmTrash, or OpenWith).
    /// `ConfirmDelete` is intentionally ignored here and requires [`Self::confirm_permanent_delete`].
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.pending_op {
            return;
        }
        let input_text = self.input.read(cx).text().to_string();

        match &self.current_dialog {
            DialogKind::NewFolder { parent, .. } => {
                let parent = parent.clone();
                if let Err(err) = validate_entry_name(&input_text) {
                    self.app_state
                        .update(cx, |state, _| state.set_status(err.clone()));
                    self.current_dialog = DialogKind::NewFolder {
                        parent,
                        error: Some(err),
                    };
                    cx.notify();
                    return;
                }

                let Some(backend) = self.app_state.read(cx).backend_for(&parent) else {
                    let err = "Server is not connected".to_string();
                    self.app_state
                        .update(cx, |state, _| state.set_status(err.clone()));
                    self.current_dialog = DialogKind::NewFolder {
                        parent,
                        error: Some(err),
                    };
                    cx.notify();
                    return;
                };

                self.pending_op = true;
                self.op_generation = self.op_generation.wrapping_add(1);
                let gen = self.op_generation;
                let parent_bg = parent.clone();
                cx.notify();

                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            create_folder_on_backend(&*backend, &parent_bg, &input_text)
                        })
                        .await;

                    let _ = this.update(cx, |this, cx| {
                        if this.op_generation != gen {
                            return;
                        }
                        this.pending_op = false;
                        match result {
                            Ok(target) => {
                                this.app_state.update(cx, |state, _| {
                                    apply_create_folder_result(state, &parent, &target);
                                });
                                this.close(cx);
                            }
                            Err(err) => {
                                this.app_state
                                    .update(cx, |state, _| state.set_status(err.clone()));
                                this.current_dialog = DialogKind::NewFolder {
                                    parent,
                                    error: Some(err),
                                };
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            DialogKind::Rename {
                source,
                current_name,
                ..
            } => {
                let source = source.clone();
                let original_name = current_name.clone();
                if let Err(err) = validate_entry_name(&input_text) {
                    self.app_state
                        .update(cx, |state, _| state.set_status(err.clone()));
                    self.current_dialog = DialogKind::Rename {
                        source,
                        current_name: original_name,
                        error: Some(err),
                    };
                    cx.notify();
                    return;
                }

                let Some(backend) = self.app_state.read(cx).backend_for(&source) else {
                    let err = "Server is not connected".to_string();
                    self.app_state
                        .update(cx, |state, _| state.set_status(err.clone()));
                    self.current_dialog = DialogKind::Rename {
                        source,
                        current_name: original_name,
                        error: Some(err),
                    };
                    cx.notify();
                    return;
                };

                self.pending_op = true;
                self.op_generation = self.op_generation.wrapping_add(1);
                let gen = self.op_generation;
                let source_bg = source.clone();
                cx.notify();

                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            rename_location_on_backend(&*backend, &source_bg, &input_text)
                        })
                        .await;

                    let _ = this.update(cx, |this, cx| {
                        if this.op_generation != gen {
                            return;
                        }
                        this.pending_op = false;
                        match result {
                            Ok(target) => {
                                this.app_state.update(cx, |state, _| {
                                    apply_rename_result(state, &source, &target);
                                });
                                this.close(cx);
                            }
                            Err(err) => {
                                this.app_state
                                    .update(cx, |state, _| state.set_status(err.clone()));
                                this.current_dialog = DialogKind::Rename {
                                    source,
                                    current_name: original_name,
                                    error: Some(err),
                                };
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            DialogKind::ConfirmTrash { locations } => {
                let targets = locations.clone();
                if targets.is_empty() {
                    self.close(cx);
                    return;
                }

                let Some(items) = self.resolve_backend_items(&targets, cx) else {
                    self.app_state.update(cx, |state, _| {
                        state.set_status("Failed to move to trash: server is not connected");
                    });
                    self.close(cx);
                    return;
                };

                self.pending_op = true;
                self.op_generation = self.op_generation.wrapping_add(1);
                let gen = self.op_generation;
                cx.notify();

                cx.spawn(async move |this, cx| {
                    let report = cx
                        .background_executor()
                        .spawn(async move { trash_locations_on_backends(&items) })
                        .await;

                    let _ = this.update(cx, |this, cx| {
                        if this.op_generation != gen {
                            return;
                        }
                        this.pending_op = false;
                        let outcome = this
                            .app_state
                            .update(cx, |state, _| apply_trash_report(state, report));
                        match outcome {
                            TrashOutcome::Unavailable(unavailable) => {
                                // Only TrashUnavailable opens the second ConfirmDelete modal;
                                // generic trash failures close without offering permanent delete.
                                this.open_confirm_delete(unavailable, cx);
                            }
                            TrashOutcome::Trashed(_) | TrashOutcome::Failed(_) => {
                                this.close(cx);
                            }
                        }
                    });
                })
                .detach();
            }
            DialogKind::ConfirmDelete { .. } => {
                // Never confirm permanent deletion via generic submit; requires clicking the button.
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
            DialogKind::ConfirmTrash { locations } => {
                let msg = format!("Move {} to the system trash?", format_item_names(locations));
                ("Move to Trash?", None, false, Some(msg))
            }
            DialogKind::ConfirmDelete { locations } => (
                "Permanently Delete?",
                None,
                false,
                Some(confirm_delete_message(locations)),
            ),
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
                                            cx.listener(move |this, _, _, cx| {
                                                if is_delete_dialog {
                                                    this.confirm_permanent_delete(cx);
                                                } else {
                                                    this.submit(cx);
                                                }
                                            }),
                                        ),
                                ),
                        )
                    }),
            )
    }
}
