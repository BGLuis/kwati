//! Remote authentication dialogs (owned by guinet-worker for #47).
//!
//! Provides host key verification and SSH/credential prompt modals.

use gpui::{div, prelude::*, px, rgb, rgba, Context, Entity, MouseButton, Window};

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDialogKind {
    None,
    TrustFingerprint { host: String, fingerprint: String },
    PasswordPrompt { host: String, username: String },
}

pub struct RemoteDialogsView {
    pub app_state: Entity<AppState>,
    pub password_input: Entity<TextInput>,
    pub current_dialog: RemoteDialogKind,
    pub theme: Theme,
}

impl RemoteDialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let password_input = cx.new(|cx| TextInput::with_placeholder("Password", cx));
        Self {
            app_state,
            password_input,
            current_dialog: RemoteDialogKind::None,
            theme: Theme::dark(),
        }
    }

    pub fn show(&mut self, dialog: RemoteDialogKind, cx: &mut Context<Self>) {
        self.current_dialog = dialog;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.current_dialog = RemoteDialogKind::None;
        cx.notify();
    }
}

impl Render for RemoteDialogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.current_dialog == RemoteDialogKind::None {
            return div();
        }

        let title = match &self.current_dialog {
            RemoteDialogKind::TrustFingerprint { host, .. } => {
                format!("Trust Host Key for {}", host)
            }
            RemoteDialogKind::PasswordPrompt { host, .. } => format!("Authenticate to {}", host),
            RemoteDialogKind::None => String::new(),
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
                    .w(px(400.))
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
                                    .child("Accept")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close(cx)),
                                    ),
                            ),
                    ),
            )
    }
}
