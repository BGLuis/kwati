pub mod dialogs;
pub mod header_view;
pub mod jobs_view;
pub mod list_view;
pub mod network_view;
pub mod preview_view;
pub mod remote_dialogs;
pub mod search_view;
pub mod settings_view;
pub mod sidebar_view;

pub use dialogs::{DialogKind, DialogsView};
pub use header_view::HeaderView;
pub use jobs_view::JobsView;
pub use list_view::ListView;
pub use network_view::NetworkView;
pub use preview_view::PreviewView;
pub use remote_dialogs::{RemoteDialogKind, RemoteDialogsView};
pub use search_view::SearchView;
pub use settings_view::SettingsView;
pub use sidebar_view::SidebarView;

use gpui::{
    div, prelude::*, px, App, Context, CursorStyle, DragMoveEvent, EmptyView, Entity, Pixels,
    SharedString, Window,
};
use kwati_core::Location;

use crate::actions::operations::{
    self, copy_locations_to_clipboard, cut_locations_to_clipboard, execute_undo,
    get_location_clipboard,
};
use crate::actions::remote_sync::RemotePollerHost;
use crate::actions::{
    DisconnectRemote, ManageServers, OpenSettings, ReconnectRemote, RefreshListing, TogglePreview,
};
use crate::layout::PanelLayout;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{size, space, Fonts, TextRole};
use crate::views::list_view::format_size;
use crate::views::search_view::ToggleSearch;

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
    pub settings: Entity<SettingsView>,
    pub remote_sync: Entity<RemotePollerHost>,
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
        let settings = cx.new(|cx| SettingsView::new(app_state.clone(), cx));
        let remote_sync = cx.new(|cx| RemotePollerHost::new(app_state.clone(), cx));

        cx.observe(&app_state, |_, _, cx| cx.notify()).detach();

        // The list owns the keyboard whenever no modal or search bar is open; only restore focus
        // on an actual open -> closed transition so unrelated updates do not steal focus.
        let mut dialog_was_open = false;
        cx.observe_in(&dialogs, window, move |this, dialogs, window, cx| {
            let now_open = dialogs.read(cx).current_dialog != DialogKind::None;
            if !dialog_was_open && now_open {
                dialogs.update(cx, |dialogs, cx| dialogs.focus(window, cx));
            } else if dialog_was_open && !now_open && !this.any_modal_or_search_open(cx) {
                this.list.read(cx).focus_handle.focus(window);
            }
            dialog_was_open = now_open;
            cx.notify();
        })
        .detach();

        let mut remote_was_open = false;
        cx.observe_in(&remote_dialogs, window, move |this, remote, window, cx| {
            let now_open = remote.read(cx).is_open(cx);
            if remote_was_open && !now_open && !this.any_modal_or_search_open(cx) {
                this.list.read(cx).focus_handle.focus(window);
            }
            remote_was_open = now_open;
            cx.notify();
        })
        .detach();

        let mut settings_was_open = false;
        cx.observe_in(&settings, window, move |this, settings, window, cx| {
            let now_open = settings.read(cx).is_open(cx);
            if settings_was_open && !now_open && !this.any_modal_or_search_open(cx) {
                this.list.read(cx).focus_handle.focus(window);
            }
            settings_was_open = now_open;
            cx.notify();
        })
        .detach();

        let mut search_was_active = false;
        cx.observe_in(&search, window, move |this, search, window, cx| {
            let now_active = search.read(cx).is_active();
            if search_was_active && !now_active && !this.any_modal_or_search_open(cx) {
                this.list.read(cx).focus_handle.focus(window);
            }
            search_was_active = now_active;
            cx.notify();
        })
        .detach();

        list.read(cx).focus_handle.focus(window);
        // Read the appearance from the window: asking the platform (`cx.window_appearance()`)
        // from inside this callback re-borrows gpui's Wayland client and panics.
        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::apply_appearance(window.appearance(), cx)
        })
        .detach();

        let remote_sync_for_window = remote_sync.clone();
        cx.observe_window_activation(window, move |_, window, cx| {
            let focused = window.is_window_active();
            remote_sync_for_window.update(cx, |host, cx| host.on_window_activation(focused, cx));
        })
        .detach();

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
            settings,
            remote_sync,
        }
    }
}

impl MainView {
    fn any_modal_or_search_open(&self, cx: &App) -> bool {
        self.dialogs.read(cx).current_dialog != DialogKind::None
            || self.remote_dialogs.read(cx).is_open(cx)
            || self.settings.read(cx).is_open(cx)
            || self.search.read(cx).is_active()
    }

    // File shortcuts act on the list's selection, so they are ignored while the keyboard is
    // in the address bar, the search field, or a dialog.
    fn list_has_keyboard(&self, window: &Window, cx: &App) -> bool {
        self.list.read(cx).focus_handle.is_focused(window)
    }

    fn selected_locations(&self, cx: &App) -> Vec<Location> {
        self.app_state.read(cx).selected_locations()
    }

    fn current_location(&self, cx: &App) -> Location {
        self.app_state.read(cx).current_location().clone()
    }

    fn resize_panels(&mut self, cx: &mut Context<Self>, resize: impl FnOnce(&mut PanelLayout)) {
        self.app_state.update(cx, |state, cx| {
            let before = state.panels;
            resize(&mut state.panels);
            if state.panels != before {
                cx.notify();
            }
        });
    }

    fn on_sidebar_edge_drag(
        &mut self,
        event: &DragMoveEvent<SidebarEdge>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pointer = event.event.position.x - event.bounds.left();
        let width = event.bounds.size.width;
        self.resize_panels(cx, |panels| panels.drag_sidebar(pointer, width));
    }

    fn on_preview_edge_drag(
        &mut self,
        event: &DragMoveEvent<PreviewEdge>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pointer = event.bounds.right() - event.event.position.x;
        let width = event.bounds.size.width;
        self.resize_panels(cx, |panels| panels.drag_preview(pointer, width));
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
        let locations = self.selected_locations(cx);
        if locations.is_empty() {
            return;
        }
        let count = locations.len();
        if is_cut {
            cut_locations_to_clipboard(locations);
            self.set_status(format!("Cut {count} item(s)"), cx);
        } else {
            copy_locations_to_clipboard(locations);
            self.set_status(format!("Copied {count} item(s) to the clipboard"), cx);
        }
    }

    fn on_paste(&mut self, _: &operations::Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let Some(clipboard) = get_location_clipboard() else {
            self.set_status("Nothing to paste", cx);
            return;
        };
        let dest_dir = self.current_location(cx);
        let in_place = clipboard
            .sources
            .iter()
            .all(|src| src.parent().as_ref() == Some(&dest_dir));
        if clipboard.is_cut && in_place {
            self.set_status("The items are already in this folder", cx);
            return;
        }

        self.dialogs.update(cx, |dialogs, cx| {
            dialogs.begin_paste(clipboard.sources, dest_dir, clipboard.is_cut, cx);
        });
        cx.notify();
    }

    fn on_trash(&mut self, _: &operations::Trash, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_to_remove(window, cx);
    }

    fn on_delete(&mut self, _: &operations::Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_to_remove(window, cx);
    }

    /// Removal goes to the trash on local disks; permanent deletion is offered directly for
    /// remote servers and non-local volumes where trash is unavailable.
    fn ask_to_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let locations = self.selected_locations(cx);
        if locations.is_empty() {
            return;
        }
        self.open_dialog(window, cx, |dialogs, cx| {
            dialogs.open_remove_confirmation(locations, cx)
        });
    }

    fn on_rename(&mut self, _: &operations::Rename, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_has_keyboard(window, cx) {
            return;
        }
        let Ok([location]) = <[Location; 1]>::try_from(self.selected_locations(cx)) else {
            return;
        };
        self.open_dialog(window, cx, |dialogs, cx| dialogs.open_rename(location, cx));
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
        let parent = self.current_location(cx);
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

    fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let is_open = self.settings.read(cx).is_open(cx);
        self.settings.update(cx, |settings, cx| {
            if is_open {
                settings.close(cx);
            } else {
                settings.open(cx);
                settings.focus(window, cx);
            }
        });
    }

    fn on_manage_servers(
        &mut self,
        _: &ManageServers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remote_dialogs.update(cx, |dialogs, cx| {
            dialogs.open_profile_manager(cx);
            dialogs.focus(window, cx);
        });
    }

    fn on_toggle_search(&mut self, _: &ToggleSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| {
            search.toggle(window, cx);
        });
    }

    fn on_refresh_listing(&mut self, _: &RefreshListing, _: &mut Window, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.refresh_current();
            cx.notify();
        });
    }

    fn on_reconnect_remote(&mut self, _: &ReconnectRemote, _: &mut Window, cx: &mut Context<Self>) {
        self.app_state.update(cx, |state, cx| {
            state.reconnect_current();
            cx.notify();
        });
    }

    fn on_disconnect_remote(
        &mut self,
        _: &DisconnectRemote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app_state.update(cx, |state, cx| {
            state.disconnect_current();
            cx.notify();
        });
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let state = self.app_state.read(cx);
        let panels = state.panels.resolve(window.viewport_size().width);
        let entry_count = state.entries.len();
        let selected_count = state.selection.len();
        let selected_bytes = state.selected_total_size();
        let status_msg = state.status_message.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            .font_family(fonts.sans.clone())
            .text_body()
            .text_color(theme.text)
            .relative()
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_trash))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_manage_servers))
            .on_action(cx.listener(Self::on_toggle_search))
            .on_action(cx.listener(Self::on_refresh_listing))
            .on_action(cx.listener(Self::on_reconnect_remote))
            .on_action(cx.listener(Self::on_disconnect_remote))
            .on_action(cx.listener(|this, _: &TogglePreview, _, cx| {
                this.resize_panels(cx, PanelLayout::toggle_preview)
            }))
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
                    .on_drag_move(cx.listener(Self::on_sidebar_edge_drag))
                    .on_drag_move(cx.listener(Self::on_preview_edge_drag))
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .w(panels.sidebar)
                            .h_full()
                            .child(self.sidebar.clone())
                            .child(resize_handle(
                                "sidebar-edge",
                                SidebarEdge,
                                Edge::Right,
                                &theme,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.list.clone())
                            .child(self.jobs.clone()),
                    )
                    .when_some(panels.preview, |row, width| {
                        row.child(
                            div()
                                .relative()
                                .flex_shrink_0()
                                .w(width)
                                .h_full()
                                .child(self.preview.clone())
                                .child(resize_handle(
                                    "preview-edge",
                                    PreviewEdge,
                                    Edge::Left,
                                    &theme,
                                )),
                        )
                    }),
            )
            // Footer: Status Bar
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .h(size::STATUS_BAR)
                    .px(space::S8)
                    .bg(theme.bg)
                    .border_t_1()
                    .border_color(theme.border)
                    .text_data(&fonts)
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(format!("{} items", entry_count))
                            .child(format_selection_summary(selected_count, selected_bytes)),
                    )
                    .child(
                        div()
                            .text_color(if status_msg.is_some() {
                                theme.warning
                            } else {
                                theme.text_muted
                            })
                            .child(status_msg.unwrap_or_default()),
                    ),
            )
            // Dialog overlays
            .child(self.dialogs.clone())
            .child(self.remote_dialogs.clone())
            .child(self.settings.clone())
    }
}

/// Formats the selection count and total byte size shown in the status bar.
pub fn format_selection_summary(count: usize, total_bytes: u64) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(
            "{count} selected ({})",
            format_size(Some(total_bytes), false)
        )
    }
}

/// Drag payload of the sidebar's right edge.
struct SidebarEdge;

/// Drag payload of the preview's left edge.
struct PreviewEdge;

enum Edge {
    Left,
    Right,
}

const RESIZE_HANDLE_WIDTH: Pixels = px(4.);

/// A strip along one edge of a pane that resizes it when dragged. It lies inside the pane so
/// it paints over the pane's own content rather than under the neighbouring pane.
fn resize_handle<T: 'static>(
    id: impl Into<SharedString>,
    payload: T,
    edge: Edge,
    theme: &Theme,
) -> impl IntoElement {
    let hover = theme.border_strong;
    div()
        .id(id.into())
        .absolute()
        .top_0()
        .bottom_0()
        .map(|handle| match edge {
            Edge::Left => handle.left_0(),
            Edge::Right => handle.right_0(),
        })
        .w(RESIZE_HANDLE_WIDTH)
        .cursor(CursorStyle::ResizeLeftRight)
        .hover(move |handle| handle.bg(hover))
        .on_drag(payload, |_, _, _, cx| cx.new(|_| EmptyView))
}
