pub mod dialogs;
pub mod header_view;
pub mod jobs_view;
pub mod list_view;
pub mod network_view;
pub mod preview_view;
pub mod remote_dialogs;
pub mod search_view;
pub mod sidebar_view;

pub use dialogs::{DialogKind, DialogsView};
pub use header_view::HeaderView;
pub use jobs_view::JobsView;
pub use list_view::ListView;
pub use network_view::NetworkView;
pub use preview_view::PreviewView;
pub use remote_dialogs::{RemoteDialogKind, RemoteDialogsView};
pub use search_view::SearchView;
pub use sidebar_view::SidebarView;

use std::path::PathBuf;

use gpui::{div, prelude::*, px, App, Context, Entity, Window};
use kwati_core::{volume_kind, ConflictPolicy, Location, VolumeKind};

use crate::actions::operations::{
    self, check_paste_conflict, copy_to_clipboard, cut_to_clipboard, execute_undo, get_clipboard,
    start_transfer, Transfer,
};

use crate::state::AppState;
use crate::theme::Theme;

/// The root shell view of kwati-gui.
pub struct MainView {
    pub app_state: Entity<AppState>,
    pub header: Entity<HeaderView>,
    pub sidebar: Entity<SidebarView>,
    pub list: Entity<ListView>,
    pub preview: Entity<PreviewView>,
    pub jobs: Entity<JobsView>,
    pub search: Entity<SearchView>,
    pub network: Entity<NetworkView>,
    pub dialogs: Entity<DialogsView>,
    pub remote_dialogs: Entity<RemoteDialogsView>,
    pub theme: Theme,
}

impl MainView {
    pub fn new(app_state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let header = cx.new(|cx| HeaderView::new(app_state.clone(), cx));
        let sidebar = cx.new(|cx| SidebarView::new(app_state.clone(), cx));
        let list = cx.new(|cx| ListView::new(app_state.clone(), cx));
        let preview = cx.new(|cx| PreviewView::new(app_state.clone(), cx));
        let jobs = cx.new(|cx| JobsView::new(app_state.clone(), cx));
        let search = cx.new(|cx| SearchView::new(app_state.clone(), cx));
        let network = cx.new(|cx| NetworkView::new(app_state.clone(), cx));
        let dialogs = cx.new(|cx| DialogsView::new(app_state.clone(), cx));
        let remote_dialogs = cx.new(|cx| RemoteDialogsView::new(app_state.clone(), cx));

        cx.observe(&app_state, |_, _, cx| cx.notify()).detach();
        // The list owns the keyboard whenever no dialog is open, so the shortcuts work from
        // the first frame and again after a dialog closes.
        cx.observe_in(&dialogs, window, |this, dialogs, window, cx| {
            if dialogs.read(cx).current_dialog == DialogKind::None {
                this.list.read(cx).focus_handle.focus(window);
            }
            cx.notify();
        })
        .detach();
        list.read(cx).focus_handle.focus(window);

        Self {
            app_state,
            header,
            sidebar,
            list,
            preview,
            jobs,
            search,
            network,
            dialogs,
            remote_dialogs,
            theme: Theme::dark(),
        }
    }
}

impl MainView {
    // File shortcuts act on the list's selection, so they are ignored while the keyboard is
    // in the address bar, the search field, or a dialog.
    fn list_has_keyboard(&self, window: &Window, cx: &App) -> bool {
        self.list.read(cx).focus_handle.is_focused(window)
    }

    fn selected_paths(&self, cx: &App) -> Vec<PathBuf> {
        let state = self.app_state.read(cx);
        state
            .selection
            .selected_in_order(&state.entries)
            .into_iter()
            .map(|entry| entry.path().to_path_buf())
            .collect()
    }

    fn current_dir(&self, cx: &App) -> Option<PathBuf> {
        match &self.app_state.read(cx).current_location {
            Location::Local(path) => Some(path.clone()),
            Location::Remote { .. } => None,
        }
    }

    fn set_status(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.app_state
            .update(cx, |state, _| state.set_status(message));
        cx.notify();
    }

    fn open_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        open: impl FnOnce(&mut DialogsView, &mut Context<DialogsView>),
    ) {
        self.dialogs.update(cx, |dialogs, cx| {
            open(dialogs, cx);
            dialogs.focus(window, cx);
        });
    }

    fn on_copy(&mut self, _: &operations::Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.fill_clipboard(false, window, cx);
    }

    fn on_cut(&mut self, _: &operations::Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.fill_clipboard(true, window, cx);
    }

    fn fill_clipboard(&mut self, is_cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let paths = self.selected_paths(cx);
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        if is_cut {
            cut_to_clipboard(paths);
            self.set_status(format!("Cut {count} item(s)"), cx);
        } else {
            copy_to_clipboard(paths);
            self.set_status(format!("Copied {count} item(s) to the clipboard"), cx);
        }
    }

    fn on_paste(&mut self, _: &operations::Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let Some(clipboard) = get_clipboard() else {
            self.set_status("Nothing to paste", cx);
            return;
        };
        let Some(dest_dir) = self.current_dir(cx) else {
            return;
        };
        let in_place = clipboard
            .sources
            .iter()
            .all(|src| src.parent() == Some(dest_dir.as_path()));
        if clipboard.is_cut && in_place {
            self.set_status("The items are already in this folder", cx);
            return;
        }

        if let Some(conflict) = check_paste_conflict(&clipboard.sources, &dest_dir) {
            self.open_dialog(window, cx, |dialogs, cx| {
                dialogs.open_conflict_resolution(
                    clipboard.sources,
                    dest_dir,
                    clipboard.is_cut,
                    conflict,
                    cx,
                )
            });
            return;
        }

        let transfer = if clipboard.is_cut {
            Transfer::moving(clipboard.sources, dest_dir, ConflictPolicy::Skip)
        } else {
            Transfer::copy(clipboard.sources, dest_dir, ConflictPolicy::Skip)
        };
        self.app_state
            .update(cx, |state, _| start_transfer(state, transfer));
        cx.notify();
    }

    fn on_trash(&mut self, _: &operations::Trash, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_to_remove(window, cx);
    }

    fn on_delete(&mut self, _: &operations::Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_to_remove(window, cx);
    }

    /// Removal goes to the trash. The permanent-deletion prompt is offered only where the
    /// trash would refuse the items, which is on network and unidentified volumes.
    fn ask_to_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let paths = self.selected_paths(cx);
        if paths.is_empty() {
            return;
        }
        let trash_available = paths
            .iter()
            .all(|path| volume_kind(path) == VolumeKind::Local);
        self.open_dialog(window, cx, |dialogs, cx| {
            if trash_available {
                dialogs.open_confirm_trash(paths, cx)
            } else {
                dialogs.open_confirm_delete(paths, cx)
            }
        });
    }

    fn on_rename(&mut self, _: &operations::Rename, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let Ok([path]) = <[PathBuf; 1]>::try_from(self.selected_paths(cx)) else {
            return;
        };
        self.open_dialog(window, cx, |dialogs, cx| dialogs.open_rename(path, cx));
    }

    fn on_new_folder(
        &mut self,
        _: &operations::NewFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let Some(parent) = self.current_dir(cx) else {
            return;
        };
        self.open_dialog(window, cx, |dialogs, cx| {
            dialogs.open_new_folder(parent, cx)
        });
    }

    fn on_undo(&mut self, _: &operations::Undo, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        self.app_state.update(cx, |state, _| {
            let _ = execute_undo(state);
        });
        cx.notify();
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let entry_count = state.entries.len();
        let selected_count = state.selection.len();
        let status_msg = state.status_message.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.bg_app)
            .relative()
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_trash))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_undo))
            // Top Header (Address Bar + Nav)
            .child(self.header.clone())
            // Network banner (if active)
            .child(self.network.clone())
            // Search field (if active)
            .child(self.search.clone())
            // Middle Body: Sidebar + File List + Preview
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.sidebar.clone())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .child(self.list.clone())
                            .child(self.jobs.clone()),
                    )
                    .child(self.preview.clone()),
            )
            // Footer: Status Bar
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .h(px(24.))
                    .px_3()
                    .bg(self.theme.bg_status)
                    .border_t_1()
                    .border_color(self.theme.border)
                    .text_size(px(11.))
                    .text_color(self.theme.text_muted)
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(format!("{} items", entry_count))
                            .child(if selected_count > 0 {
                                format!("{} selected", selected_count)
                            } else {
                                "".to_string()
                            }),
                    )
                    .child(
                        div()
                            .text_color(if status_msg.is_some() {
                                self.theme.text_warning
                            } else {
                                self.theme.text_muted
                            })
                            .child(status_msg.unwrap_or_default()),
                    ),
            )
            // Dialog overlays
            .child(self.dialogs.clone())
            .child(self.remote_dialogs.clone())
    }
}
