//! Remote authentication dialogs and connection profile manager (#45, #47).
//!
//! Renders connection prompts driven by [`AppState::remote_prompt`](crate::state::AppState::remote_prompt)
//! (password/passphrase, unknown host key trust, changed host key refusal, FTP TLS fallback,
//! and invalid TLS certificate) as well as the saved server profile manager.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, transparent_black, AnyElement, App, Context, Entity, FocusHandle,
    KeyDownEvent, MouseButton, Window,
};
use kwati_core::{AuthMethod, ConnectionProfile, Protocol};

use crate::components::{icon, IconName, TextInput};
use crate::remote::{default_port, new_profile_id, protocol_label};
use crate::state::{AppState, RemotePrompt};
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

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

/// Identifies which `RemotePrompt` is currently initialized in view state so unrelated
/// `AppState` updates do not wipe what the user is typing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptKey {
    None,
    Password {
        profile_id: String,
        error: Option<String>,
    },
    TrustHostKey {
        profile_id: String,
        fingerprint: String,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        presented: String,
    },
    TlsUnavailable {
        profile_id: String,
        host: String,
        port: u16,
    },
    CertificateInvalid {
        host: String,
        port: u16,
        reason: String,
    },
}

impl PromptKey {
    fn from_prompt(prompt: Option<&RemotePrompt>) -> Self {
        match prompt {
            None => Self::None,
            Some(RemotePrompt::Password { profile_id, error }) => Self::Password {
                profile_id: profile_id.clone(),
                error: error.clone(),
            },
            Some(RemotePrompt::TrustHostKey {
                profile_id,
                unknown,
                ..
            }) => Self::TrustHostKey {
                profile_id: profile_id.clone(),
                fingerprint: unknown.fingerprint.clone(),
            },
            Some(RemotePrompt::HostKeyChanged {
                host,
                port,
                presented,
                ..
            }) => Self::HostKeyChanged {
                host: host.clone(),
                port: *port,
                presented: presented.clone(),
            },
            Some(RemotePrompt::TlsUnavailable {
                profile_id,
                host,
                port,
                ..
            }) => Self::TlsUnavailable {
                profile_id: profile_id.clone(),
                host: host.clone(),
                port: *port,
            },
            Some(RemotePrompt::CertificateInvalid { host, port, reason }) => {
                Self::CertificateInvalid {
                    host: host.clone(),
                    port: *port,
                    reason: reason.clone(),
                }
            }
        }
    }
}

/// Editable fields for adding or modifying a [`ConnectionProfile`].
pub struct ProfileFormState {
    pub editing_id: Option<String>,
    pub protocol: Protocol,
    pub auth: AuthMethod,
    pub allow_insecure: bool,
    pub name_input: Entity<TextInput>,
    pub host_input: Entity<TextInput>,
    pub port_input: Entity<TextInput>,
    pub user_input: Entity<TextInput>,
    pub start_path_input: Entity<TextInput>,
    pub private_key_path_input: Entity<TextInput>,
    /// Always masked via [`TextInput::password`]; persisted only through `Secrets`.
    pub secret_input: Entity<TextInput>,
    pub error: Option<String>,
}

impl ProfileFormState {
    fn new(existing: Option<&ConnectionProfile>, cx: &mut Context<RemoteDialogsView>) -> Self {
        let protocol = existing.map_or(Protocol::Sftp, |p| p.protocol);
        let auth = existing.map_or(AuthMethod::Password, |p| p.auth);
        let allow_insecure = existing.is_some_and(|p| p.allow_insecure);
        let port = existing.map_or_else(|| default_port(protocol), |p| p.port);
        let start_path = existing.map_or_else(|| "/".to_owned(), |p| p.start_path.clone());
        let key_path = existing
            .and_then(|p| p.private_key_path.as_ref())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let name_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("My Server", cx);
            if let Some(p) = existing {
                input.set_text(p.name.clone(), cx);
            }
            input
        });
        let host_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("example.com", cx);
            if let Some(p) = existing {
                input.set_text(p.host.clone(), cx);
            }
            input
        });
        let port_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("22", cx);
            input.set_text(port.to_string(), cx);
            input
        });
        let user_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("username", cx);
            if let Some(p) = existing {
                input.set_text(p.user.clone(), cx);
            }
            input
        });
        let start_path_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("/", cx);
            input.set_text(start_path, cx);
            input
        });
        let private_key_path_input = cx.new(|cx| {
            let mut input = TextInput::with_placeholder("/path/to/id_ed25519", cx);
            if !key_path.is_empty() {
                input.set_text(key_path, cx);
            }
            input
        });
        let secret_placeholder = secret_placeholder_for(auth, existing.is_some());
        let secret_input = cx.new(|cx| TextInput::password(secret_placeholder, cx));

        Self {
            editing_id: existing.map(|p| p.id.clone()),
            protocol,
            auth,
            allow_insecure,
            name_input,
            host_input,
            port_input,
            user_input,
            start_path_input,
            private_key_path_input,
            secret_input,
            error: None,
        }
    }

    fn focus_handles(&self, cx: &App) -> Vec<FocusHandle> {
        let mut handles = vec![
            self.name_input.read(cx).focus_handle.clone(),
            self.host_input.read(cx).focus_handle.clone(),
            self.port_input.read(cx).focus_handle.clone(),
            self.user_input.read(cx).focus_handle.clone(),
            self.start_path_input.read(cx).focus_handle.clone(),
        ];
        if self.auth == AuthMethod::PrivateKey {
            handles.push(self.private_key_path_input.read(cx).focus_handle.clone());
        }
        handles.push(self.secret_input.read(cx).focus_handle.clone());
        handles
    }
}

fn secret_placeholder_for(auth: AuthMethod, is_editing: bool) -> &'static str {
    match (auth, is_editing) {
        (AuthMethod::Password, true) => "Password (leave blank to keep existing)",
        (AuthMethod::Password, false) => "Password (optional; prompts on connect if blank)",
        (AuthMethod::PrivateKey, true) => "Key passphrase (leave blank to keep existing)",
        (AuthMethod::PrivateKey, false) => "Key passphrase (optional)",
    }
}

pub struct RemoteDialogsView {
    pub app_state: Entity<AppState>,
    pub password_input: Entity<TextInput>,
    pub current_dialog: RemoteDialogKind,
    pub remember_password: bool,
    pub profile_manager_open: bool,
    pub selected_profile_id: Option<String>,
    pub profile_form: Option<ProfileFormState>,
    pub manager_error: Option<String>,
    pub focus_handle: FocusHandle,
    active_prompt_key: PromptKey,
    needs_focus: bool,
}

impl RemoteDialogsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let password_input = cx.new(|cx| TextInput::password("Password", cx));
        cx.observe(&app_state, |this, app_state, cx| {
            this.sync_from_state(&app_state, cx);
        })
        .detach();

        let mut view = Self {
            app_state: app_state.clone(),
            password_input,
            current_dialog: RemoteDialogKind::None,
            remember_password: false,
            profile_manager_open: false,
            selected_profile_id: None,
            profile_form: None,
            manager_error: None,
            focus_handle: cx.focus_handle(),
            active_prompt_key: PromptKey::None,
            needs_focus: false,
        };
        view.sync_from_state(&app_state, cx);
        view
    }

    /// Synchronizes view state with `AppState.remote_prompt` and profile list.
    pub fn sync_from_state(&mut self, app_state: &Entity<AppState>, cx: &mut Context<Self>) {
        let (prompt, profiles) = {
            let state = app_state.read(cx);
            (
                state.remote_prompt.clone(),
                state.remote.profiles().to_vec(),
            )
        };
        let next_key = PromptKey::from_prompt(prompt.as_ref());

        if next_key != self.active_prompt_key {
            let prev_profile = match &self.active_prompt_key {
                PromptKey::Password { profile_id, .. } => Some(profile_id.clone()),
                _ => None,
            };
            self.active_prompt_key = next_key;

            match &prompt {
                Some(RemotePrompt::Password { profile_id, .. }) => {
                    if prev_profile.as_deref() != Some(profile_id.as_str()) {
                        self.remember_password = false;
                    }
                    self.password_input.update(cx, |input, cx| input.clear(cx));
                    self.needs_focus = true;
                }
                Some(_) => {
                    self.needs_focus = true;
                }
                None => {}
            }
        }

        if let Some(selected) = &self.selected_profile_id {
            if !profiles.iter().any(|p| &p.id == selected) {
                self.selected_profile_id = profiles.first().map(|p| p.id.clone());
            }
        } else if self.profile_manager_open {
            self.selected_profile_id = profiles.first().map(|p| p.id.clone());
        }

        self.current_dialog = match prompt.as_ref() {
            Some(RemotePrompt::Password { profile_id, error }) => {
                let (host, username) = app_state.read(cx).remote.profile(profile_id).map_or_else(
                    || (profile_id.clone(), String::new()),
                    |p| (p.host.clone(), p.user.clone()),
                );
                RemoteDialogKind::PasswordPrompt {
                    host,
                    username,
                    remember: self.remember_password,
                    error_message: error.clone(),
                }
            }
            Some(RemotePrompt::TrustHostKey { unknown, .. }) => {
                RemoteDialogKind::TrustFingerprint {
                    host: unknown.host.clone(),
                    port: unknown.port,
                    algorithm: "SSH".into(),
                    fingerprint: unknown.fingerprint.clone(),
                }
            }
            Some(RemotePrompt::HostKeyChanged {
                host,
                port,
                presented,
                stored,
            }) => RemoteDialogKind::HostKeyChanged {
                host: host.clone(),
                port: *port,
                known_fingerprint: stored.join("\n"),
                presented_fingerprint: presented.clone(),
            },
            Some(RemotePrompt::TlsUnavailable { .. })
            | Some(RemotePrompt::CertificateInvalid { .. }) => RemoteDialogKind::None,
            None if self.profile_manager_open => RemoteDialogKind::ProfileManager {
                profiles,
                selected_id: self.selected_profile_id.clone(),
            },
            None => RemoteDialogKind::None,
        };

        cx.notify();
    }

    pub fn is_open(&self, cx: &App) -> bool {
        self.app_state.read(cx).remote_prompt.is_some()
            || self.profile_manager_open
            || self.current_dialog != RemoteDialogKind::None
    }

    /// Focuses the primary control of whichever remote dialog is currently active.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.app_state.read(cx).remote_prompt.clone();
        match prompt {
            Some(RemotePrompt::Password { .. }) => {
                self.password_input.read(cx).focus_handle.focus(window);
                return;
            }
            Some(_) => {
                self.focus_handle.focus(window);
                return;
            }
            None => {}
        }

        if let Some(form) = &self.profile_form {
            form.name_input.read(cx).focus_handle.focus(window);
        } else if self.profile_manager_open {
            self.focus_handle.focus(window);
        } else if matches!(self.current_dialog, RemoteDialogKind::PasswordPrompt { .. }) {
            self.password_input.read(cx).focus_handle.focus(window);
        } else if self.current_dialog != RemoteDialogKind::None {
            self.focus_handle.focus(window);
        }
    }

    pub fn show(&mut self, dialog: RemoteDialogKind, cx: &mut Context<Self>) {
        match &dialog {
            RemoteDialogKind::PasswordPrompt { remember, .. } => {
                self.remember_password = *remember;
                self.password_input.update(cx, |input, cx| input.clear(cx));
            }
            RemoteDialogKind::ProfileManager { selected_id, .. } => {
                self.profile_manager_open = true;
                self.profile_form = None;
                self.manager_error = None;
                self.selected_profile_id = selected_id.clone().or_else(|| {
                    self.app_state
                        .read(cx)
                        .remote
                        .profiles()
                        .first()
                        .map(|p| p.id.clone())
                });
            }
            RemoteDialogKind::None => {
                self.profile_manager_open = false;
                self.profile_form = None;
            }
            _ => {}
        }
        self.current_dialog = dialog;
        self.needs_focus = true;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.app_state.read(cx).remote_prompt.is_some() {
            self.app_state.update(cx, |state, cx| {
                state.dismiss_remote_prompt();
                cx.notify();
            });
        }
        self.profile_manager_open = false;
        self.profile_form = None;
        self.manager_error = None;
        self.current_dialog = RemoteDialogKind::None;
        cx.notify();
    }

    /// Returns whether the currently active modal displays an action to connect or proceed.
    /// Returns `false` for `HostKeyChanged` and `CertificateInvalid`, where no connect/bypass
    /// action may ever be offered.
    pub fn has_connect_button(&self, cx: &App) -> bool {
        if let Some(prompt) = &self.app_state.read(cx).remote_prompt {
            return match prompt {
                RemotePrompt::Password { .. } => true,
                RemotePrompt::TrustHostKey { .. } => true,
                RemotePrompt::TlsUnavailable { .. } => true,
                RemotePrompt::HostKeyChanged { .. } => false,
                RemotePrompt::CertificateInvalid { .. } => false,
            };
        }
        match &self.current_dialog {
            RemoteDialogKind::TrustFingerprint { .. } => true,
            RemoteDialogKind::PasswordPrompt { .. } => true,
            RemoteDialogKind::ProfileManager { .. } => true,
            RemoteDialogKind::HostKeyChanged { .. } => false,
            RemoteDialogKind::None => false,
        }
    }

    pub fn toggle_remember_password(&mut self) {
        self.remember_password = !self.remember_password;
    }

    /// Submits the active password prompt to [`AppState::submit_password`].
    pub fn submit_active_password(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(RemotePrompt::Password { profile_id, .. }) =
            self.app_state.read(cx).remote_prompt.clone()
        else {
            return false;
        };
        let password = self.password_input.read(cx).text().to_string();
        let remember = self.remember_password;
        self.password_input.update(cx, |input, cx| input.clear(cx));
        self.app_state.update(cx, |state, cx| {
            state.submit_password(&profile_id, password, remember);
            cx.notify();
        });
        true
    }

    /// Trusts the presented unknown host key via [`AppState::trust_and_connect`].
    pub fn trust_active_host_key(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(RemotePrompt::TrustHostKey {
            profile_id,
            unknown,
            password,
            remember,
        }) = self.app_state.read(cx).remote_prompt.clone()
        else {
            return false;
        };
        self.app_state.update(cx, |state, cx| {
            state.trust_and_connect(&profile_id, &unknown, password, remember);
            cx.notify();
        });
        true
    }

    /// Proceeds with unencrypted FTP via [`AppState::allow_insecure_and_connect`].
    pub fn allow_insecure_active(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(RemotePrompt::TlsUnavailable {
            profile_id,
            password,
            remember,
            ..
        }) = self.app_state.read(cx).remote_prompt.clone()
        else {
            return false;
        };
        self.app_state.update(cx, |state, cx| {
            state.allow_insecure_and_connect(&profile_id, password, remember);
            cx.notify();
        });
        true
    }

    /// Dismisses the active [`AppState::remote_prompt`] via [`AppState::dismiss_remote_prompt`].
    pub fn dismiss_prompt(&mut self, cx: &mut Context<Self>) {
        self.password_input.update(cx, |input, cx| input.clear(cx));
        self.app_state.update(cx, |state, cx| {
            state.dismiss_remote_prompt();
            cx.notify();
        });
        if self.current_dialog != RemoteDialogKind::None && !self.profile_manager_open {
            self.current_dialog = RemoteDialogKind::None;
            cx.notify();
        }
    }

    /// Opens the connection profile manager modal.
    pub fn open_profile_manager(&mut self, cx: &mut Context<Self>) {
        let profiles = self.app_state.read(cx).remote.profiles().to_vec();
        self.profile_manager_open = true;
        self.profile_form = None;
        self.manager_error = None;
        if self
            .selected_profile_id
            .as_ref()
            .is_none_or(|id| !profiles.iter().any(|p| &p.id == id))
        {
            self.selected_profile_id = profiles.first().map(|p| p.id.clone());
        }
        self.current_dialog = RemoteDialogKind::ProfileManager {
            profiles,
            selected_id: self.selected_profile_id.clone(),
        };
        self.needs_focus = true;
        cx.notify();
    }

    pub fn select_profile(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        self.selected_profile_id = Some(profile_id.to_owned());
        if let RemoteDialogKind::ProfileManager {
            ref mut selected_id,
            ..
        } = self.current_dialog
        {
            *selected_id = self.selected_profile_id.clone();
        }
        cx.notify();
    }

    /// Opens the profile form for creating a new profile.
    pub fn open_add_profile(&mut self, cx: &mut Context<Self>) {
        self.profile_manager_open = true;
        self.manager_error = None;
        self.profile_form = Some(ProfileFormState::new(None, cx));
        self.needs_focus = true;
        cx.notify();
    }

    /// Opens the profile form pre-populated from an existing profile.
    pub fn open_edit_profile(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        let existing = self.app_state.read(cx).remote.profile(profile_id).cloned();
        let Some(profile) = existing else {
            return;
        };
        self.profile_manager_open = true;
        self.manager_error = None;
        self.selected_profile_id = Some(profile.id.clone());
        self.profile_form = Some(ProfileFormState::new(Some(&profile), cx));
        self.needs_focus = true;
        cx.notify();
    }

    pub fn cancel_profile_form(&mut self, cx: &mut Context<Self>) {
        self.profile_form = None;
        self.needs_focus = true;
        cx.notify();
    }

    /// Switches the form's protocol and updates `port` to the new protocol's default when the
    /// current port is empty or still equal to the previous protocol's default.
    pub fn set_form_protocol(&mut self, protocol: Protocol, cx: &mut Context<Self>) {
        let Some(form) = &mut self.profile_form else {
            return;
        };
        let old_default = default_port(form.protocol).to_string();
        let current_port = form.port_input.read(cx).text().trim().to_owned();
        if current_port.is_empty() || current_port == old_default {
            let new_default = default_port(protocol).to_string();
            form.port_input
                .update(cx, |input, cx| input.set_text(new_default, cx));
        }
        form.protocol = protocol;
        if protocol != Protocol::Ftp {
            form.allow_insecure = false;
        }
        if protocol != Protocol::Sftp && form.auth == AuthMethod::PrivateKey {
            form.auth = AuthMethod::Password;
            let placeholder = secret_placeholder_for(form.auth, form.editing_id.is_some());
            form.secret_input
                .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        }
        form.error = None;
        cx.notify();
    }

    pub fn set_form_auth(&mut self, auth: AuthMethod, cx: &mut Context<Self>) {
        let Some(form) = &mut self.profile_form else {
            return;
        };
        form.auth = auth;
        let placeholder = secret_placeholder_for(auth, form.editing_id.is_some());
        form.secret_input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        form.error = None;
        cx.notify();
    }

    pub fn toggle_form_allow_insecure(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.profile_form {
            form.allow_insecure = !form.allow_insecure;
            cx.notify();
        }
    }

    /// Validates and saves the current profile form via [`RemoteSessions::save_profile`].
    /// Returns `true` when saved.
    pub fn save_profile_form(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(form) = &self.profile_form else {
            return false;
        };
        let name = form.name_input.read(cx).text().trim().to_owned();
        let host = form.host_input.read(cx).text().trim().to_owned();
        let port_text = form.port_input.read(cx).text().trim().to_owned();
        let user = form.user_input.read(cx).text().trim().to_owned();
        let raw_start_path = form.start_path_input.read(cx).text().trim().to_owned();
        let key_path_text = form
            .private_key_path_input
            .read(cx)
            .text()
            .trim()
            .to_owned();
        let secret_text = form.secret_input.read(cx).text().to_owned();

        let validation_err = if name.is_empty() {
            Some("Profile name is required.".to_owned())
        } else if host.is_empty() {
            Some("Host is required.".to_owned())
        } else if port_text.parse::<u16>().ok().filter(|&p| p > 0).is_none() {
            Some("Port must be a number between 1 and 65535.".to_owned())
        } else if user.is_empty() {
            Some("Username is required.".to_owned())
        } else if form.auth == AuthMethod::PrivateKey && key_path_text.is_empty() {
            Some("Private key path is required for key authentication.".to_owned())
        } else if form.protocol == Protocol::Smb
            && (raw_start_path.is_empty() || raw_start_path == "/")
        {
            Some("SMB start path must begin with a share name (e.g. /share).".to_owned())
        } else {
            None
        };

        if let Some(err) = validation_err {
            if let Some(form) = &mut self.profile_form {
                form.error = Some(err);
            }
            cx.notify();
            return false;
        }

        let port = port_text.parse::<u16>().expect("validated above");
        let start_path = if raw_start_path.is_empty() {
            "/".to_owned()
        } else {
            raw_start_path
        };
        let private_key_path = if form.auth == AuthMethod::PrivateKey {
            Some(PathBuf::from(key_path_text))
        } else {
            None
        };
        let id = form.editing_id.clone().unwrap_or_else(new_profile_id);
        let profile = ConnectionProfile {
            id: id.clone(),
            name,
            protocol: form.protocol,
            host,
            port,
            user,
            start_path,
            auth: form.auth,
            private_key_path,
            allow_insecure: form.protocol == Protocol::Ftp && form.allow_insecure,
        };
        let secret = (!secret_text.is_empty()).then_some(secret_text);

        let save_res = self.app_state.update(cx, |state, cx| {
            let res = state.remote.save_profile(profile, secret);
            if res.is_ok() {
                state.clear_status();
            }
            cx.notify();
            res
        });

        match save_res {
            Ok(()) => {
                self.selected_profile_id = Some(id);
                self.profile_form = None;
                self.manager_error = None;
                self.sync_manager_dialog(cx);
                self.needs_focus = true;
                cx.notify();
                true
            }
            Err(err) => {
                if let Some(form) = &mut self.profile_form {
                    form.error = Some(err);
                }
                cx.notify();
                false
            }
        }
    }

    /// Removes `profile_id` and its saved secret via [`RemoteSessions::remove_profile`].
    pub fn remove_profile(&mut self, profile_id: &str, cx: &mut Context<Self>) -> bool {
        let res = self.app_state.update(cx, |state, cx| {
            let res = state.remote.remove_profile(profile_id);
            if let Err(ref err) = res {
                state.set_status(err.clone());
            }
            cx.notify();
            res
        });
        match res {
            Ok(()) => {
                self.manager_error = None;
                if self.selected_profile_id.as_deref() == Some(profile_id) {
                    self.selected_profile_id = self
                        .app_state
                        .read(cx)
                        .remote
                        .profiles()
                        .first()
                        .map(|p| p.id.clone());
                }
                self.sync_manager_dialog(cx);
                cx.notify();
                true
            }
            Err(err) => {
                self.manager_error = Some(err);
                cx.notify();
                false
            }
        }
    }

    /// Closes the profile manager and initiates connection to `profile_id` via [`AppState::connect`].
    pub fn connect_profile(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        self.profile_manager_open = false;
        self.profile_form = None;
        self.manager_error = None;
        self.current_dialog = RemoteDialogKind::None;
        self.app_state.update(cx, |state, cx| {
            state.connect(profile_id, None, false);
            cx.notify();
        });
        cx.notify();
    }

    fn sync_manager_dialog(&mut self, cx: &App) {
        if self.profile_manager_open {
            let profiles = self.app_state.read(cx).remote.profiles().to_vec();
            self.current_dialog = RemoteDialogKind::ProfileManager {
                profiles,
                selected_id: self.selected_profile_id.clone(),
            };
        }
    }

    fn cycle_focus(&self, reverse: bool, window: &mut Window, cx: &App) {
        let prompt = self.app_state.read(cx).remote_prompt.clone();
        let handles: Vec<FocusHandle> = if matches!(prompt, Some(RemotePrompt::Password { .. })) {
            vec![
                self.password_input.read(cx).focus_handle.clone(),
                self.focus_handle.clone(),
            ]
        } else if let Some(form) = &self.profile_form {
            form.focus_handles(cx)
        } else {
            return;
        };

        if handles.is_empty() {
            return;
        }
        let current = handles.iter().position(|h| h.is_focused(window));
        let next = match (current, reverse) {
            (Some(idx), false) => (idx + 1) % handles.len(),
            (Some(idx), true) => (idx + handles.len() - 1) % handles.len(),
            (None, false) => 0,
            (None, true) => handles.len() - 1,
        };
        handles[next].focus(window);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        match key {
            "tab" => {
                cx.stop_propagation();
                self.cycle_focus(event.keystroke.modifiers.shift, window, cx);
            }
            "escape" => {
                cx.stop_propagation();
                if self.app_state.read(cx).remote_prompt.is_some() {
                    self.dismiss_prompt(cx);
                } else if self.profile_form.is_some() {
                    self.cancel_profile_form(cx);
                } else {
                    self.close(cx);
                }
            }
            "enter" => {
                cx.stop_propagation();
                let prompt = self.app_state.read(cx).remote_prompt.clone();
                match prompt {
                    Some(RemotePrompt::Password { .. }) => {
                        self.submit_active_password(cx);
                    }
                    Some(RemotePrompt::TrustHostKey { .. }) => {
                        self.trust_active_host_key(cx);
                    }
                    // HostKeyChanged, CertificateInvalid, and TlsUnavailable must never
                    // proceed or auto-accept on an accidental Enter keypress.
                    Some(_) => {}
                    None if self.profile_form.is_some() => {
                        self.save_profile_form(cx);
                    }
                    None if self.profile_manager_open => {
                        if let Some(id) = self.selected_profile_id.clone() {
                            self.connect_profile(&id, cx);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
}

impl Render for RemoteDialogsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prompt = self.app_state.read(cx).remote_prompt.clone();
        if prompt.is_none()
            && !self.profile_manager_open
            && self.current_dialog == RemoteDialogKind::None
        {
            return div();
        }

        if self.needs_focus {
            self.needs_focus = false;
            self.focus(window, cx);
        }

        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);

        let content: AnyElement = if let Some(prompt) = prompt {
            self.render_prompt(prompt, &theme, &fonts, cx)
        } else if self.profile_manager_open {
            self.render_profile_manager(&theme, &fonts, cx)
        } else {
            self.render_legacy_dialog(&theme, &fonts, cx)
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
            .child(content)
    }
}

impl RemoteDialogsView {
    fn render_prompt(
        &self,
        prompt: RemotePrompt,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match prompt {
            RemotePrompt::Password { profile_id, error } => {
                self.render_password_prompt(&profile_id, error, theme, cx)
            }
            RemotePrompt::TrustHostKey { unknown, .. } => self.render_trust_host_key(
                &unknown.host,
                unknown.port,
                &unknown.fingerprint,
                theme,
                fonts,
                cx,
            ),
            RemotePrompt::HostKeyChanged {
                host,
                port,
                presented,
                stored,
            } => self.render_host_key_changed(&host, port, &presented, &stored, theme, fonts, cx),
            RemotePrompt::TlsUnavailable {
                host, port, reply, ..
            } => self.render_tls_unavailable(&host, port, &reply, theme, fonts, cx),
            RemotePrompt::CertificateInvalid { host, port, reason } => {
                self.render_certificate_invalid(&host, port, &reason, theme, cx)
            }
        }
    }

    fn render_legacy_dialog(
        &self,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.current_dialog {
            RemoteDialogKind::TrustFingerprint {
                host,
                port,
                fingerprint,
                ..
            } => self.render_trust_host_key(host, *port, fingerprint, theme, fonts, cx),
            RemoteDialogKind::HostKeyChanged {
                host,
                port,
                known_fingerprint,
                presented_fingerprint,
            } => {
                let stored: Vec<String> = known_fingerprint
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.render_host_key_changed(
                    host,
                    *port,
                    presented_fingerprint,
                    &stored,
                    theme,
                    fonts,
                    cx,
                )
            }
            RemoteDialogKind::PasswordPrompt {
                host,
                error_message,
                ..
            } => self.render_password_prompt(host, error_message.clone(), theme, cx),
            RemoteDialogKind::ProfileManager { .. } => {
                self.render_profile_manager(theme, fonts, cx)
            }
            RemoteDialogKind::None => div().into_any_element(),
        }
    }

    fn render_password_prompt(
        &self,
        profile_id: &str,
        error: Option<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let profile = self.app_state.read(cx).remote.profile(profile_id).cloned();
        let title = match &profile {
            Some(p) => format!("Authenticate to {}", p.name),
            None => format!("Authenticate to {}", profile_id),
        };
        let subtitle = match &profile {
            Some(p) => format!(
                "{}: {}@{}:{}",
                protocol_label(p.protocol),
                p.user,
                p.host,
                p.port
            ),
            None => "Enter credentials to continue connecting.".to_owned(),
        };

        dialog_card(px(420.), theme.border, theme)
            .child(div().text_title().text_color(theme.text).child(title))
            .child(
                div()
                    .text_body()
                    .text_color(theme.text_muted)
                    .child(subtitle),
            )
            .child(self.password_input.clone())
            .when_some(error, |parent, err| {
                parent.child(div().text_label().text_color(theme.danger).child(err))
            })
            .child(
                checkbox_row(
                    "remember-password-checkbox",
                    self.remember_password,
                    "Save password",
                    theme,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_remember_password();
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(space::S8)
                    .child(
                        action_button("Cancel", ButtonTone::Secondary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)),
                        ),
                    )
                    .child(
                        action_button("Connect", ButtonTone::Primary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.submit_active_password(cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host_label = format!("{host}:{port}");
        dialog_card(px(480.), theme.border, theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(icon(IconName::Alert).text_color(theme.warning))
                    .child(
                        div()
                            .text_title()
                            .text_color(theme.text)
                            .child("Unknown Host Key"),
                    ),
            )
            .child(
                div()
                    .text_body()
                    .text_color(theme.text_muted)
                    .child(format!(
                        "The authenticity of host '{host_label}' cannot be established."
                    )),
            )
            .child(
                div()
                    .p(space::S12)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .gap(space::S4)
                    .child(
                        div()
                            .text_label()
                            .text_color(theme.text_muted)
                            .child("SHA256 Fingerprint"),
                    )
                    .child(
                        div()
                            .text_data(fonts)
                            .text_color(theme.text)
                            .child(fingerprint.to_owned()),
                    ),
            )
            .child(
                div()
                    .text_body()
                    .text_color(theme.text_muted)
                    .child("Do you want to trust this host key and continue connecting?"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(space::S8)
                    .child(
                        action_button("Cancel", ButtonTone::Secondary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)),
                        ),
                    )
                    .child(
                        action_button("Trust & Connect", ButtonTone::Primary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.trust_active_host_key(cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_host_key_changed(
        &self,
        host: &str,
        port: u16,
        presented: &str,
        stored: &[String],
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host_label = format!("{host}:{port}");
        let stored_rows: Vec<AnyElement> = if stored.is_empty() {
            vec![div()
                .text_data(fonts)
                .text_color(theme.text_muted)
                .child("(none recorded)")
                .into_any_element()]
        } else {
            stored
                .iter()
                .map(|fp| {
                    div()
                        .text_data(fonts)
                        .text_color(theme.text)
                        .child(fp.clone())
                        .into_any_element()
                })
                .collect()
        };

        dialog_card(px(520.), theme.danger, theme)
            .border_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(icon(IconName::Alert).text_color(theme.danger))
                    .child(
                        div()
                            .text_title()
                            .text_color(theme.danger)
                            .child("Host Key Has Changed — Connection Refused"),
                    ),
            )
            .child(div().text_body().text_color(theme.text).child(format!(
                "The host key presented by '{host_label}' does not match the stored key(s)."
            )))
            .child(
                div()
                    .p(space::S12)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.danger)
                    .flex()
                    .flex_col()
                    .gap(space::S8)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S2)
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.danger)
                                    .child("Server presented fingerprint"),
                            )
                            .child(
                                div()
                                    .text_data(fonts)
                                    .text_color(theme.danger)
                                    .child(presented.to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S2)
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child("Stored fingerprint(s)"),
                            )
                            .children(stored_rows),
                    ),
            )
            .child(
                div().text_body().text_color(theme.text_muted).child(
                    "WARNING: Someone could be eavesdropping or spoofing this server. \
                     For your security, connection is strictly refused and cannot be bypassed here.",
                ),
            )
            .child(
                div().flex().flex_row().justify_end().child(
                    action_button("Close", ButtonTone::Secondary, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)),
                    ),
                ),
            )
            .into_any_element()
    }

    fn render_tls_unavailable(
        &self,
        host: &str,
        port: u16,
        reply: &str,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        dialog_card(px(480.), theme.warning, theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(icon(IconName::Alert).text_color(theme.warning))
                    .child(
                        div()
                            .text_title()
                            .text_color(theme.warning)
                            .child("TLS Unavailable on FTP Server"),
                    ),
            )
            .child(div().text_body().text_color(theme.text).child(format!(
                "Server {host}:{port} does not offer TLS ({reply})."
            )))
            .child(
                div()
                    .p(space::S12)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.warning)
                    .text_data(fonts)
                    .text_color(theme.warning)
                    .child(
                        "Warning: Continuing with plain FTP sends your username, password, and \
                         file contents unencrypted over the network.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(space::S8)
                    .child(
                        action_button("Cancel", ButtonTone::Secondary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)),
                        ),
                    )
                    .child(
                        action_button("Allow Plain FTP & Connect", ButtonTone::Danger, theme)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.allow_insecure_active(cx);
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_certificate_invalid(
        &self,
        host: &str,
        port: u16,
        reason: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        dialog_card(px(480.), theme.danger, theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(icon(IconName::Alert).text_color(theme.danger))
                    .child(
                        div()
                            .text_title()
                            .text_color(theme.danger)
                            .child("Invalid TLS Certificate — Connection Refused"),
                    ),
            )
            .child(div().text_body().text_color(theme.text).child(format!(
                "The TLS certificate presented by {host}:{port} was rejected:"
            )))
            .child(
                div()
                    .p(space::S12)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.danger)
                    .text_body()
                    .text_color(theme.danger)
                    .child(reason.to_owned()),
            )
            .child(
                div()
                    .text_body()
                    .text_color(theme.text_muted)
                    .child("For your security, invalid certificates cannot be bypassed."),
            )
            .child(div().flex().flex_row().justify_end().child(
                action_button("Close", ButtonTone::Secondary, theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.dismiss_prompt(cx)),
                ),
            ))
            .into_any_element()
    }

    fn render_profile_manager(
        &self,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(form) = &self.profile_form {
            self.render_profile_form(form, theme, fonts, cx)
        } else {
            self.render_profile_list(theme, fonts, cx)
        }
    }

    fn render_profile_list(
        &self,
        theme: &Theme,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (profiles, connected_ids): (Vec<ConnectionProfile>, Vec<String>) = {
            let state = self.app_state.read(cx);
            let profiles = state.remote.profiles().to_vec();
            let connected = profiles
                .iter()
                .filter(|p| state.remote.is_connected(&p.id))
                .map(|p| p.id.clone())
                .collect();
            (profiles, connected)
        };
        let selected_id = self.selected_profile_id.clone();
        let has_selection = selected_id
            .as_ref()
            .is_some_and(|id| profiles.iter().any(|p| &p.id == id));

        let rows: Vec<AnyElement> = if profiles.is_empty() {
            vec![div()
                .p(space::S16)
                .rounded(radius::CONTROL)
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border)
                .text_body()
                .text_color(theme.text_muted)
                .child("No saved connection profiles yet. Click Add to create one.")
                .into_any_element()]
        } else {
            profiles
                .into_iter()
                .enumerate()
                .map(|(ix, p)| {
                    let is_selected = selected_id.as_deref() == Some(p.id.as_str());
                    let is_connected = connected_ids.iter().any(|id| id == &p.id);
                    let id_select = p.id.clone();
                    div()
                        .id(("profile-row", ix))
                        .px(space::S12)
                        .py(space::S8)
                        .rounded(radius::CONTROL)
                        .cursor_pointer()
                        .bg(if is_selected {
                            theme.selection
                        } else {
                            transparent_black().into()
                        })
                        .border_1()
                        .border_color(if is_selected {
                            theme.accent
                        } else {
                            theme.border
                        })
                        .flex()
                        .justify_between()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.select_profile(&id_select, cx);
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(space::S2)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(space::S8)
                                        .child(
                                            div()
                                                .text_body()
                                                .text_color(theme.text)
                                                .child(p.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .px(space::S4)
                                                .py(space::S2)
                                                .rounded(radius::CONTROL)
                                                .bg(theme.surface_raised)
                                                .text_label()
                                                .text_color(theme.text_muted)
                                                .child(protocol_label(p.protocol)),
                                        )
                                        .when(is_connected, |row| {
                                            row.child(
                                                div()
                                                    .px(space::S4)
                                                    .py(space::S2)
                                                    .rounded(radius::CONTROL)
                                                    .bg(theme.surface_raised)
                                                    .text_label()
                                                    .text_color(theme.secondary)
                                                    .child("Connected"),
                                            )
                                        }),
                                )
                                .child(div().text_data(fonts).text_color(theme.text_muted).child(
                                    format!("{}@{}:{}{}", p.user, p.host, p.port, p.start_path),
                                )),
                        )
                        .into_any_element()
                })
                .collect()
        };

        let sel_for_edit = selected_id.clone();
        let sel_for_remove = selected_id.clone();
        let sel_for_connect = selected_id;

        dialog_card(px(560.), theme.border, theme)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_title()
                            .text_color(theme.text)
                            .child("Connection Profiles"),
                    )
                    .child(
                        action_button("+ Add", ButtonTone::Secondary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.open_add_profile(cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("profile-list-scroll")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(space::S4)
                    .children(rows),
            )
            .when_some(self.manager_error.clone(), |parent, err| {
                parent.child(div().text_label().text_color(theme.danger).child(err))
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().flex().gap(space::S8).when(has_selection, |row| {
                        let edit_id = sel_for_edit.clone().unwrap_or_default();
                        let remove_id = sel_for_remove.clone().unwrap_or_default();
                        row.child(
                            action_button("Edit", ButtonTone::Secondary, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_edit_profile(&edit_id, cx);
                                }),
                            ),
                        )
                        .child(
                            action_button("Remove", ButtonTone::Danger, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.remove_profile(&remove_id, cx);
                                }),
                            ),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(space::S8)
                            .child(
                                action_button("Close", ButtonTone::Secondary, theme).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.close(cx)),
                                ),
                            )
                            .when(has_selection, |row| {
                                let conn_id = sel_for_connect.clone().unwrap_or_default();
                                row.child(
                                    action_button("Connect", ButtonTone::Primary, theme)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.connect_profile(&conn_id, cx);
                                            }),
                                        ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_profile_form(
        &self,
        form: &ProfileFormState,
        theme: &Theme,
        _fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = if form.editing_id.is_some() {
            "Edit Connection Profile"
        } else {
            "New Connection Profile"
        };
        let is_plain_ftp = form.protocol == Protocol::Ftp;
        let is_sftp = form.protocol == Protocol::Sftp;
        let is_key_auth = form.auth == AuthMethod::PrivateKey;
        let secret_label = if is_key_auth {
            "Passphrase"
        } else {
            "Password"
        };

        let protocols = [Protocol::Sftp, Protocol::Ftp, Protocol::Ftps, Protocol::Smb];

        dialog_card(px(560.), theme.border, theme)
            .child(div().text_title().text_color(theme.text).child(title))
            .child(field_group("Profile Name", theme).child(form.name_input.clone()))
            .child(
                field_group("Protocol", theme).child(
                    div().flex().gap(space::S8).children(
                        protocols.into_iter().map(|proto| {
                            selector_pill(
                                protocol_label(proto),
                                form.protocol == proto,
                                theme,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_form_protocol(proto, cx);
                                }),
                            )
                        }),
                    ),
                ),
            )
            .when(is_plain_ftp, |card| {
                card.child(
                    div()
                        .p(space::S8)
                        .rounded(radius::CONTROL)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.warning)
                        .flex()
                        .items_center()
                        .gap(space::S8)
                        .child(icon(IconName::Alert).text_color(theme.warning))
                        .child(
                            div()
                                .text_label()
                                .text_color(theme.warning)
                                .child(
                                    "Unencrypted credentials warning: plain FTP sends username, \
                                     password, and files without encryption unless TLS is negotiated.",
                                ),
                        ),
                )
                .child(
                    checkbox_row(
                        "allow-insecure-ftp-checkbox",
                        form.allow_insecure,
                        "Allow unencrypted plain FTP fallback when TLS is unavailable",
                        theme,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.toggle_form_allow_insecure(cx)),
                    ),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(space::S8)
                    .child(
                        div()
                            .flex_1()
                            .child(field_group("Host", theme).child(form.host_input.clone())),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .child(field_group("Port", theme).child(form.port_input.clone())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(space::S8)
                    .child(
                        div()
                            .flex_1()
                            .child(field_group("User", theme).child(form.user_input.clone())),
                    )
                    .child(
                        div().flex_1().child(
                            field_group("Start Path", theme).child(form.start_path_input.clone()),
                        ),
                    ),
            )
            .when(is_sftp, |card| {
                card.child(
                    field_group("Authentication Method", theme).child(
                        div()
                            .flex()
                            .gap(space::S8)
                            .child(
                                selector_pill(
                                    "Password",
                                    form.auth == AuthMethod::Password,
                                    theme,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_form_auth(AuthMethod::Password, cx);
                                    }),
                                ),
                            )
                            .child(
                                selector_pill(
                                    "Private Key",
                                    form.auth == AuthMethod::PrivateKey,
                                    theme,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.set_form_auth(AuthMethod::PrivateKey, cx);
                                    }),
                                ),
                            ),
                    ),
                )
            })
            .when(is_key_auth, |card| {
                card.child(
                    field_group("Private Key Path", theme)
                        .child(form.private_key_path_input.clone()),
                )
            })
            .child(field_group(secret_label, theme).child(form.secret_input.clone()))
            .when_some(form.error.clone(), |card, err| {
                card.child(div().text_label().text_color(theme.danger).child(err))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(space::S8)
                    .child(
                        action_button("Cancel", ButtonTone::Secondary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.cancel_profile_form(cx)),
                        ),
                    )
                    .child(
                        action_button("Save", ButtonTone::Primary, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_profile_form(cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum ButtonTone {
    Primary,
    Secondary,
    Danger,
}

fn dialog_card(width: gpui::Pixels, border: gpui::Rgba, theme: &Theme) -> gpui::Div {
    div()
        .w(width)
        .p(space::S16)
        .rounded(radius::DIALOG)
        .bg(theme.surface)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(space::S12)
}

fn field_group(label: &str, theme: &Theme) -> gpui::Div {
    div().flex().flex_col().gap(space::S4).child(
        div()
            .text_label()
            .text_color(theme.text_muted)
            .child(label.to_owned()),
    )
}

fn selector_pill(label: &str, active: bool, theme: &Theme) -> gpui::Div {
    div()
        .px(space::S12)
        .py(space::S4)
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .border_1()
        .border_color(if active { theme.accent } else { theme.border })
        .bg(if active {
            theme.selection
        } else {
            theme.surface_raised
        })
        .text_label()
        .text_color(if active { theme.text } else { theme.text_muted })
        .child(label.to_owned())
}

fn checkbox_row(
    id: &'static str,
    checked: bool,
    label: &str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(space::S8)
        .cursor_pointer()
        .child(
            div()
                .size(size::ICON)
                .rounded(radius::CONTROL)
                .border_1()
                .border_color(if checked {
                    theme.accent
                } else {
                    theme.border_strong
                })
                .bg(if checked {
                    theme.accent
                } else {
                    theme.surface_raised
                })
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |box_div| {
                    box_div.child(icon(IconName::Check).text_color(theme.on_accent))
                }),
        )
        .child(
            div()
                .text_body()
                .text_color(theme.text_muted)
                .child(label.to_owned()),
        )
}

fn action_button(label: &str, tone: ButtonTone, theme: &Theme) -> gpui::Div {
    let (bg, fg) = match tone {
        ButtonTone::Primary => (theme.accent, theme.on_accent),
        ButtonTone::Secondary => (theme.surface_raised, theme.text),
        ButtonTone::Danger => (theme.danger, theme.on_accent),
    };
    div()
        .px(space::S12)
        .py(space::S4)
        .rounded(radius::CONTROL)
        .bg(bg)
        .text_body()
        .text_color(fg)
        .cursor_pointer()
        .child(label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use gpui::{ClipboardItem, TestAppContext, VisualTestContext};
    use kwati_core::{ConnectionProfiles, KnownHosts, Location};

    use crate::components::register_text_input_keybindings;
    use crate::remote::{RemoteSessions, SessionSecrets};

    fn test_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kwati_remote_dialogs_{name}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_dialog_window<'a>(
        cx: &'a mut TestAppContext,
        state: AppState,
    ) -> (
        Entity<AppState>,
        Entity<RemoteDialogsView>,
        &'a mut VisualTestContext,
    ) {
        cx.update(register_text_input_keybindings);
        let state_entity = cx.new(|_| state);
        let state_for_view = state_entity.clone();
        let (view, cx) = cx.add_window_view(|_, cx| RemoteDialogsView::new(state_for_view, cx));
        cx.run_until_parked();
        (state_entity, view, cx)
    }

    #[gpui::test]
    fn host_key_changed_offers_no_connect_button_and_dismisses(cx: &mut TestAppContext) {
        let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
        state.remote_prompt = Some(RemotePrompt::HostKeyChanged {
            host: "sftp.example.com".into(),
            port: 22,
            presented: "SHA256:presented-key-999".into(),
            stored: vec![
                "SHA256:stored-key-111".into(),
                "SHA256:stored-key-222".into(),
            ],
        });

        let (state_entity, view, cx) = open_dialog_window(cx, state);

        view.read_with(cx, |view, cx| {
            assert!(view.is_open(cx));
            assert!(!view.has_connect_button(cx));
            assert!(matches!(
                view.current_dialog,
                RemoteDialogKind::HostKeyChanged {
                    ref known_fingerprint,
                    ref presented_fingerprint,
                    ..
                } if presented_fingerprint == "SHA256:presented-key-999"
                    && known_fingerprint.contains("SHA256:stored-key-111")
                    && known_fingerprint.contains("SHA256:stored-key-222")
            ));
        });

        // Pressing Enter must NOT connect or bypass HostKeyChanged.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        state_entity.read_with(cx, |s, _| {
            assert!(matches!(
                s.remote_prompt,
                Some(RemotePrompt::HostKeyChanged { .. })
            ));
        });

        // Escape dismisses the prompt.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        state_entity.read_with(cx, |s, _| assert!(s.remote_prompt.is_none()));
        view.read_with(cx, |view, cx| assert!(!view.is_open(cx)));
    }

    #[gpui::test]
    fn password_prompt_masks_blocks_clipboard_and_unsaved_password_reprompts(
        cx: &mut TestAppContext,
    ) {
        let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
        let profile = ConnectionProfile {
            id: "sftp-1".into(),
            name: "Prod SFTP".into(),
            protocol: Protocol::Sftp,
            host: "127.0.0.1".into(),
            port: 9,
            user: "deploy".into(),
            start_path: "/".into(),
            auth: AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        };
        state.remote.save_profile(profile, None).unwrap();
        state.connect("sftp-1", None, false);
        assert!(matches!(
            state.remote_prompt,
            Some(RemotePrompt::Password { .. })
        ));

        let (state_entity, view, cx) = open_dialog_window(cx, state);

        view.read_with(cx, |view, cx| {
            assert!(view.password_input.read(cx).masked);
            assert!(view.has_connect_button(cx));
        });

        cx.write_to_clipboard(ClipboardItem::new_string("unrelated".into()));
        cx.simulate_input("hunter2");
        cx.simulate_keystrokes("ctrl-a ctrl-c ctrl-x");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("unrelated".into()),
            "password input must block copy and cut"
        );

        // Submit without "Save password" checked: connection fails on closed port 9,
        // and the unsaved password must NOT be persisted in RemoteSessions.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        state_entity.update(cx, |state, cx| {
            assert!(!state.remote.has_secret("sftp-1"));
            assert!(state.remote.needs_password("sftp-1"));
            // Connecting again must re-prompt for a password.
            state.connect("sftp-1", None, false);
            assert!(matches!(
                state.remote_prompt,
                Some(RemotePrompt::Password { .. })
            ));
            cx.notify();
        });
        cx.run_until_parked();

        view.read_with(cx, |view, cx| {
            assert_eq!(view.password_input.read(cx).text(), "");
        });
    }

    #[gpui::test]
    fn profile_manager_crud_ftp_warning_port_defaults_and_secret_isolation(
        cx: &mut TestAppContext,
    ) {
        let dir = test_temp_dir("crud");
        let profiles_path = dir.join("connections.toml");
        let secrets = Arc::new(SessionSecrets::default());
        let remote = RemoteSessions::new(
            Some(profiles_path.clone()),
            ConnectionProfiles::default(),
            secrets.clone(),
            KnownHosts::new(dir.join("known_hosts")),
        );
        let mut state = AppState::new(Location::Local(dir.clone()));
        state.remote = remote;

        let (state_entity, view, cx) = open_dialog_window(cx, state);

        // Open ProfileManager -> Add Profile.
        view.update(cx, |view, cx| {
            view.open_profile_manager(cx);
            view.open_add_profile(cx);
        });
        cx.run_until_parked();

        // Default protocol is SFTP on port 22.
        view.read_with(cx, |view, cx| {
            let form = view.profile_form.as_ref().unwrap();
            assert_eq!(form.protocol, Protocol::Sftp);
            assert_eq!(form.port_input.read(cx).text(), "22");
            assert!(form.secret_input.read(cx).masked);
        });

        // Switching to FTP auto-updates unchanged port to 21; switching to SMB updates to 445.
        view.update(cx, |view, cx| view.set_form_protocol(Protocol::Ftp, cx));
        view.read_with(cx, |view, cx| {
            let form = view.profile_form.as_ref().unwrap();
            assert_eq!(form.port_input.read(cx).text(), "21");
        });

        // Custom port is preserved when switching protocol.
        view.update(cx, |view, cx| {
            let form = view.profile_form.as_ref().unwrap();
            form.port_input
                .update(cx, |input, cx| input.set_text("2121", cx));
            view.set_form_protocol(Protocol::Smb, cx);
        });
        view.read_with(cx, |view, cx| {
            let form = view.profile_form.as_ref().unwrap();
            assert_eq!(form.port_input.read(cx).text(), "2121");
        });

        // Switch back to FTP with default port 21 and fill out profile fields.
        view.update(cx, |view, cx| {
            let form = view.profile_form.as_ref().unwrap();
            form.port_input
                .update(cx, |input, cx| input.set_text("21", cx));
            view.set_form_protocol(Protocol::Ftp, cx);
            let form = view.profile_form.as_ref().unwrap();
            form.name_input
                .update(cx, |input, cx| input.set_text("Legacy FTP", cx));
            form.host_input
                .update(cx, |input, cx| input.set_text("ftp.example.com", cx));
            form.user_input
                .update(cx, |input, cx| input.set_text("alice", cx));
            form.start_path_input
                .update(cx, |input, cx| input.set_text("/pub", cx));
            form.secret_input
                .update(cx, |input, cx| input.set_text("super-secret-pw", cx));
            view.toggle_form_allow_insecure(cx);
            assert!(view.save_profile_form(cx));
        });
        cx.run_until_parked();

        // Verify profile persisted to connections.toml WITHOUT the secret,
        // and the secret is stored ONLY in Secrets.
        let saved_toml = fs::read_to_string(&profiles_path).unwrap();
        assert!(saved_toml.contains("Legacy FTP"));
        assert!(!saved_toml.contains("super-secret-pw"));

        let created_id = state_entity.read_with(cx, |state, _| {
            assert_eq!(state.remote.profiles().len(), 1);
            let p = &state.remote.profiles()[0];
            assert_eq!(p.protocol, Protocol::Ftp);
            assert_eq!(p.port, 21);
            assert!(p.allow_insecure);
            assert!(state.remote.has_secret(&p.id));
            p.id.clone()
        });

        // Edit profile to SFTP PrivateKey without typing a new secret -> keeps existing secret.
        view.update(cx, |view, cx| {
            view.open_edit_profile(&created_id, cx);
            view.set_form_protocol(Protocol::Sftp, cx);
            view.set_form_auth(AuthMethod::PrivateKey, cx);
            let form = view.profile_form.as_ref().unwrap();
            form.private_key_path_input.update(cx, |input, cx| {
                input.set_text("/home/alice/.ssh/id_ed25519", cx)
            });
            assert!(view.save_profile_form(cx));
        });

        state_entity.read_with(cx, |state, _| {
            let p = state.remote.profile(&created_id).unwrap();
            assert_eq!(p.protocol, Protocol::Sftp);
            assert_eq!(p.port, 22);
            assert_eq!(p.auth, AuthMethod::PrivateKey);
            assert!(!p.allow_insecure);
            assert_eq!(
                p.private_key_path.as_deref(),
                Some(PathBuf::from("/home/alice/.ssh/id_ed25519").as_path())
            );
            assert!(state.remote.has_secret(&created_id));
        });

        // Remove profile -> deletes BOTH profile and stored secret.
        view.update(cx, |view, cx| {
            assert!(view.remove_profile(&created_id, cx));
        });
        state_entity.read_with(cx, |state, _| {
            assert!(state.remote.profiles().is_empty());
            assert!(!state.remote.has_secret(&created_id));
        });

        let _ = fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn tab_cycles_focus_and_tls_unavailable_allows_explicit_insecure_retry(
        cx: &mut TestAppContext,
    ) {
        let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
        let profile = ConnectionProfile {
            id: "ftp-1".into(),
            name: "FTP Server".into(),
            protocol: Protocol::Ftp,
            host: "127.0.0.1".into(),
            port: 9,
            user: "bob".into(),
            start_path: "/".into(),
            auth: AuthMethod::Password,
            private_key_path: None,
            allow_insecure: false,
        };
        state
            .remote
            .save_profile(profile, Some("pw".into()))
            .unwrap();
        state.remote_prompt = Some(RemotePrompt::TlsUnavailable {
            profile_id: "ftp-1".into(),
            host: "127.0.0.1".into(),
            port: 9,
            reply: "500 AUTH not understood".into(),
            password: None,
            remember: false,
        });

        let (state_entity, view, cx) = open_dialog_window(cx, state);

        // Enter must NOT silently accept plain FTP fallback.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        state_entity.read_with(cx, |s, _| {
            assert!(matches!(
                s.remote_prompt,
                Some(RemotePrompt::TlsUnavailable { .. })
            ));
            assert!(!s.remote.profile("ftp-1").unwrap().allow_insecure);
        });

        // Explicit action sets allow_insecure = true and clears the prompt.
        view.update(cx, |view, cx| {
            assert!(view.allow_insecure_active(cx));
        });
        cx.run_until_parked();
        state_entity.read_with(cx, |s, _| {
            assert!(s.remote.profile("ftp-1").unwrap().allow_insecure);
            assert!(s.remote_prompt.is_none());
        });

        // Verify Tab cycles focus across ProfileManager form inputs.
        view.update(cx, |view, cx| view.open_add_profile(cx));
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let form = view.profile_form.as_ref().unwrap();
            assert!(form.name_input.read(cx).focus_handle.is_focused(window));
        });
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let form = view.profile_form.as_ref().unwrap();
            assert!(form.host_input.read(cx).focus_handle.is_focused(window));
        });
    }
}
