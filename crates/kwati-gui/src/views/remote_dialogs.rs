//! Remote authentication dialogs (owned by guinet-worker for #47).
//!
//! Provides host key verification modals, changed key alerts, password/passphrase prompts,
//! and saved connection profile management.

use std::collections::{HashMap, HashSet};

use gpui::{
    div, prelude::*, px, transparent_black, Context, Entity, FontWeight, MouseButton, Window,
};
use kwati_core::ConnectionProfile;

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDialogKind {
    None,
    TrustFingerprint {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        known_fingerprint: String,
        presented_fingerprint: String,
    },
    PasswordPrompt {
        host: String,
        username: String,
        remember: bool,
        error_message: Option<String>,
    },
    ProfileManager {
        profiles: Vec<ConnectionProfile>,
        selected_id: Option<String>,
    },
}

pub struct RemoteDialogsView {
    pub app_state: Entity<AppState>,
    pub password_input: Entity<TextInput>,
    pub current_dialog: RemoteDialogKind,
    pub remember_password: bool,
    pub trusted_fingerprints: HashSet<String>,
    pub stored_passwords: HashMap<String, String>,
    pub theme: Theme,
}

impl RemoteDialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let password_input = cx.new(|cx| TextInput::with_placeholder("Password", cx));
        Self {
            app_state,
            password_input,
            current_dialog: RemoteDialogKind::None,
            remember_password: false,
            trusted_fingerprints: HashSet::new(),
            stored_passwords: HashMap::new(),
            theme: Theme::dark(),
        }
    }

    pub fn show(&mut self, dialog: RemoteDialogKind, cx: &mut Context<Self>) {
        if let RemoteDialogKind::PasswordPrompt { remember, .. } = &dialog {
            self.remember_password = *remember;
        }
        self.current_dialog = dialog;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.current_dialog = RemoteDialogKind::None;
        cx.notify();
    }

    /// Returns whether the currently active modal displays an action to connect/proceed.
    /// Crucially returns `false` for `HostKeyChanged` (where no connect button must be offered).
    pub fn has_connect_button(&self) -> bool {
        match &self.current_dialog {
            RemoteDialogKind::TrustFingerprint { .. } => true,
            RemoteDialogKind::PasswordPrompt { .. } => true,
            RemoteDialogKind::ProfileManager { .. } => true,
            RemoteDialogKind::HostKeyChanged { .. } => false,
            RemoteDialogKind::None => false,
        }
    }

    /// Trusts the currently presented host key fingerprint and closes the dialog.
    pub fn trust_current_fingerprint(&mut self, cx: &mut Context<Self>) -> bool {
        if let RemoteDialogKind::TrustFingerprint { fingerprint, .. } = &self.current_dialog {
            self.trusted_fingerprints.insert(fingerprint.clone());
            self.close(cx);
            true
        } else {
            false
        }
    }

    /// Declines to trust the host key fingerprint and aborts connection.
    pub fn decline_current_fingerprint(&mut self, cx: &mut Context<Self>) -> bool {
        if matches!(
            self.current_dialog,
            RemoteDialogKind::TrustFingerprint { .. }
        ) {
            self.close(cx);
            self.app_state.update(cx, |state, _cx| {
                state.set_status("Connection cancelled: host key rejected");
            });
            true
        } else {
            false
        }
    }

    /// Toggles the remember password checkbox.
    pub fn toggle_remember_password(&mut self) {
        self.remember_password = !self.remember_password;
    }

    /// Submits the password. If `remember` is true, stores it; otherwise does not store it.
    pub fn submit_password(
        &mut self,
        password: &str,
        remember: bool,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if let RemoteDialogKind::PasswordPrompt { host, .. } = &self.current_dialog {
            let host_key = host.clone();
            if remember {
                self.stored_passwords.insert(host_key, password.to_string());
            } else {
                self.stored_passwords.remove(&host_key);
            }
            let pass = password.to_string();
            self.close(cx);
            Some(pass)
        } else {
            None
        }
    }

    /// Retrieves any remembered password for the host.
    pub fn get_stored_password(&self, host: &str) -> Option<&String> {
        self.stored_passwords.get(host)
    }

    /// Clears any stored password for the host.
    pub fn clear_stored_password(&mut self, host: &str) {
        self.stored_passwords.remove(host);
    }
}

impl Render for RemoteDialogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.current_dialog == RemoteDialogKind::None {
            return div();
        }

        match &self.current_dialog {
            RemoteDialogKind::None => div(),

            RemoteDialogKind::TrustFingerprint {
                host,
                port,
                algorithm,
                fingerprint,
            } => {
                let host_str = format!("{}:{}", host, port);
                let algo_str = algorithm.clone();
                let fp_str = fingerprint.clone();

                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(self.theme.scrim)
                    .child(
                        div()
                            .w(px(460.))
                            .p_5()
                            .rounded_lg()
                            .bg(self.theme.surface)
                            .border_1()
                            .border_color(self.theme.border)
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text)
                                    .child("🔒 Authenticate Host Key"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(self.theme.text_muted)
                                    .child(format!(
                                        "The authenticity of host '{}' cannot be established.",
                                        host_str
                                    )),
                            )
                            .child(
                                div()
                                    .p_3()
                                    .rounded_md()
                                    .bg(self.theme.surface_raised)
                                    .border_1()
                                    .border_color(self.theme.border)
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(self.theme.text_muted)
                                            .child(format!("Key type: {}", algo_str)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family(".SystemUIFont")
                                            .text_color(self.theme.text)
                                            .child(format!("Fingerprint: {}", fp_str)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(self.theme.text_muted)
                                    .child("Are you sure you want to trust this key and continue connecting?"),
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
                                            .bg(self.theme.surface_raised)
                                            .text_color(self.theme.text_muted)
                                            .cursor_pointer()
                                            .child("Decline")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.decline_current_fingerprint(cx);
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .bg(self.theme.accent)
                                            .text_color(self.theme.on_accent)
                                            .font_weight(FontWeight::MEDIUM)
                                            .cursor_pointer()
                                            .child("Trust & Connect")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.trust_current_fingerprint(cx);
                                                }),
                                            ),
                                    ),
                            ),
                    )
            }

            RemoteDialogKind::HostKeyChanged {
                host,
                port,
                known_fingerprint,
                presented_fingerprint,
            } => {
                let host_str = format!("{}:{}", host, port);
                let known_str = known_fingerprint.clone();
                let presented_str = presented_fingerprint.clone();

                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(self.theme.scrim)
                    .child(
                        div()
                            .w(px(500.))
                            .p_5()
                            .rounded_lg()
                            .bg(self.theme.surface)
                            .border_2()
                            .border_color(self.theme.danger)
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(self.theme.danger)
                                    .child("🚨 WARNING: HOST IDENTIFICATION HAS CHANGED!"),
                            )
                            .child(div().text_size(px(13.)).text_color(self.theme.text).child(
                                format!(
                                    "The host key for '{}' does not match the key saved locally.",
                                    host_str
                                ),
                            ))
                            .child(
                                div()
                                    .p_3()
                                    .rounded_md()
                                    .bg(self.theme.surface_raised)
                                    .border_1()
                                    .border_color(self.theme.danger)
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(self.theme.text_muted)
                                            .child(format!(
                                                "Stored key fingerprint:\n{}",
                                                known_str
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(self.theme.danger)
                                            .child(format!(
                                                "Server offered fingerprint:\n{}",
                                                presented_str
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(self.theme.text_muted)
                                    .child(
                                    "Someone could be eavesdropping or spoofing this connection. \
                                         For security, connection is strictly refused.",
                                ),
                            )
                            .child(
                                div().flex().flex_row().justify_end().child(
                                    div()
                                        .px_4()
                                        .py_1()
                                        .rounded_md()
                                        .bg(self.theme.surface_raised)
                                        .text_color(self.theme.text)
                                        .cursor_pointer()
                                        .child("Close (Refuse Connection)")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.close(cx)),
                                        ),
                                ),
                            ),
                    )
            }

            RemoteDialogKind::PasswordPrompt {
                host,
                username,
                remember: _,
                error_message,
            } => {
                let host_str = host.clone();
                let user_str = username.clone();
                let err_opt = error_message.clone();
                let is_remembered = self.remember_password;

                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(self.theme.scrim)
                    .child(
                        div()
                            .w(px(400.))
                            .p_5()
                            .rounded_lg()
                            .bg(self.theme.surface)
                            .border_1()
                            .border_color(self.theme.border)
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text)
                                    .child(format!("Authenticate to {}", host_str)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(self.theme.text_muted)
                                    .child(format!("User: {}", user_str)),
                            )
                            .child(self.password_input.clone())
                            .when_some(err_opt, |parent, err| {
                                parent.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(self.theme.danger)
                                        .child(err),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_remember_password();
                                            cx.notify();
                                        }),
                                    )
                                    .child(if is_remembered { "☑" } else { "☐" })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(self.theme.text_muted)
                                            .child("Remember password in keyring"),
                                    ),
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
                                            .bg(self.theme.surface_raised)
                                            .text_color(self.theme.text_muted)
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
                                            .text_color(self.theme.on_accent)
                                            .font_weight(FontWeight::MEDIUM)
                                            .cursor_pointer()
                                            .child("Connect")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    let pass = this
                                                        .password_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string();
                                                    let remember = this.remember_password;
                                                    this.submit_password(&pass, remember, cx);
                                                }),
                                            ),
                                    ),
                            ),
                    )
            }

            RemoteDialogKind::ProfileManager {
                profiles,
                selected_id,
            } => {
                let profiles_list = profiles.clone();
                let sel_id = selected_id.clone();

                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(self.theme.scrim)
                    .child(
                        div()
                            .w(px(480.))
                            .max_h(px(400.))
                            .p_5()
                            .rounded_lg()
                            .bg(self.theme.surface)
                            .border_1()
                            .border_color(self.theme.border)
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(self.theme.text)
                                    .child("Saved Connection Profiles"),
                            )
                            .child(div().flex().flex_col().gap_1().children(
                                profiles_list.into_iter().map(|p| {
                                    let is_selected = sel_id.as_deref() == Some(&p.id);
                                    div()
                                        .px_3()
                                        .py_2()
                                        .rounded_md()
                                        .bg(if is_selected {
                                            self.theme.selection
                                        } else {
                                            transparent_black().into()
                                        })
                                        .border_1()
                                        .border_color(if is_selected {
                                            self.theme.accent
                                        } else {
                                            self.theme.border
                                        })
                                        .flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(self.theme.text)
                                                        .child(p.name),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(self.theme.text_muted)
                                                        .child(format!(
                                                            "{}@{}:{}",
                                                            p.user, p.host, p.port
                                                        )),
                                                ),
                                        )
                                }),
                            ))
                            .child(
                                div().flex().flex_row().justify_end().gap_2().child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .bg(self.theme.surface_raised)
                                        .text_color(self.theme.text_muted)
                                        .cursor_pointer()
                                        .child("Close")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.close(cx)),
                                        ),
                                ),
                            ),
                    )
            }
        }
    }
}
