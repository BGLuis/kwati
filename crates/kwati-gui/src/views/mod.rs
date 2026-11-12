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

use gpui::{div, prelude::*, px, Context, Entity, Window};

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
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let header = cx.new(|cx| HeaderView::new(app_state.clone(), cx));
        let sidebar = cx.new(|cx| SidebarView::new(app_state.clone(), cx));
        let list = cx.new(|cx| ListView::new(app_state.clone(), cx));
        let preview = cx.new(|cx| PreviewView::new(app_state.clone(), cx));
        let jobs = cx.new(|cx| JobsView::new(app_state.clone(), cx));
        let search = cx.new(|cx| SearchView::new(app_state.clone(), cx));
        let network = cx.new(|cx| NetworkView::new(app_state.clone(), cx));
        let dialogs = cx.new(|cx| DialogsView::new(app_state.clone(), cx));
        let remote_dialogs = cx.new(|cx| RemoteDialogsView::new(app_state.clone(), cx));

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
