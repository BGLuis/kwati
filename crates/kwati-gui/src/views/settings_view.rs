//! Grouped, data-driven Settings modal view (#29, #34, #50, #52).

use std::collections::HashMap;

use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, Subscription,
    Window,
};
use kwati_core::{Settings, SortColumn, SortDirection};

use crate::components::{icon, IconName, TextInput};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

/// Logical groups in the Settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSectionId {
    General,
    NetworkMounts,
    Remote,
}

/// Stable identifier for every setting row rendered by [`SettingsView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    ShowHidden,
    DefaultSortColumn,
    DefaultSortDirection,
    ListTimeoutSecs,
    RefreshIntervalSecs,
    RemotePollEnabled,
    RemotePollIntervalSecs,
    RemoteSearchDepth,
    RemoteSearchParallel,
}

/// Control variant rendered for a [`SettingRowSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingControlKind {
    Toggle,
    SortColumn,
    SortDirection,
    Number {
        min: u32,
        max: u32,
        unit: &'static str,
    },
}

/// Declarative definition of a settings row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingRowSpec {
    pub key: SettingKey,
    pub section: SettingsSectionId,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: SettingControlKind,
}

pub const SETTINGS_SECTIONS: &[(SettingsSectionId, &str)] = &[
    (SettingsSectionId::General, "Geral"),
    (SettingsSectionId::NetworkMounts, "Montagens de rede"),
    (SettingsSectionId::Remote, "Remoto"),
];

pub const SETTINGS_ROWS: &[SettingRowSpec] = &[
    SettingRowSpec {
        key: SettingKey::ShowHidden,
        section: SettingsSectionId::General,
        label: "Mostrar arquivos ocultos",
        description: "Exibe arquivos e pastas iniciados por ponto nas listagens.",
        kind: SettingControlKind::Toggle,
    },
    SettingRowSpec {
        key: SettingKey::DefaultSortColumn,
        section: SettingsSectionId::General,
        label: "Ordenação padrão (coluna)",
        description: "Critério principal usado ao listar pastas.",
        kind: SettingControlKind::SortColumn,
    },
    SettingRowSpec {
        key: SettingKey::DefaultSortDirection,
        section: SettingsSectionId::General,
        label: "Ordenação padrão (direção)",
        description: "Ordem crescente ou decrescente da listagem.",
        kind: SettingControlKind::SortDirection,
    },
    SettingRowSpec {
        key: SettingKey::ListTimeoutSecs,
        section: SettingsSectionId::NetworkMounts,
        label: "Tempo limite de listagem",
        description: "Segundos até abortar a leitura de uma montagem lenta (1–3600).",
        kind: SettingControlKind::Number {
            min: 1,
            max: 3600,
            unit: "s",
        },
    },
    SettingRowSpec {
        key: SettingKey::RefreshIntervalSecs,
        section: SettingsSectionId::NetworkMounts,
        label: "Intervalo de atualização",
        description: "Frequência de atualização para pastas monitoradas (1–3600).",
        kind: SettingControlKind::Number {
            min: 1,
            max: 3600,
            unit: "s",
        },
    },
    SettingRowSpec {
        key: SettingKey::RemotePollEnabled,
        section: SettingsSectionId::Remote,
        label: "Atualização automática remota",
        description: "Reconsulta a pasta remota aberta enquanto a janela estiver em foco.",
        kind: SettingControlKind::Toggle,
    },
    SettingRowSpec {
        key: SettingKey::RemotePollIntervalSecs,
        section: SettingsSectionId::Remote,
        label: "Intervalo de atualização remota",
        description: "Segundos entre atualizações automáticas remotas (mínimo 10).",
        kind: SettingControlKind::Number {
            min: 10,
            max: 3600,
            unit: "s",
        },
    },
    SettingRowSpec {
        key: SettingKey::RemoteSearchDepth,
        section: SettingsSectionId::Remote,
        label: "Profundidade da busca remota",
        description: "Níveis de subpastas percorridos na busca remota (1–64).",
        kind: SettingControlKind::Number {
            min: 1,
            max: 64,
            unit: "níveis",
        },
    },
    SettingRowSpec {
        key: SettingKey::RemoteSearchParallel,
        section: SettingsSectionId::Remote,
        label: "Requisições paralelas na busca",
        description: "Listagens simultâneas durante a busca remota (1–32).",
        kind: SettingControlKind::Number {
            min: 1,
            max: 32,
            unit: "req",
        },
    },
];

const NUMERIC_KEYS: &[SettingKey] = &[
    SettingKey::ListTimeoutSecs,
    SettingKey::RefreshIntervalSecs,
    SettingKey::RemotePollIntervalSecs,
    SettingKey::RemoteSearchDepth,
    SettingKey::RemoteSearchParallel,
];

fn numeric_value(settings: &Settings, key: SettingKey) -> Option<u32> {
    match key {
        SettingKey::ListTimeoutSecs => Some(settings.list_timeout_secs),
        SettingKey::RefreshIntervalSecs => Some(settings.refresh_interval_secs),
        SettingKey::RemotePollIntervalSecs => Some(settings.remote_poll_interval_secs),
        SettingKey::RemoteSearchDepth => Some(settings.remote_search_depth),
        SettingKey::RemoteSearchParallel => Some(settings.remote_search_parallel),
        _ => None,
    }
}

fn spec_for(key: SettingKey) -> Option<&'static SettingRowSpec> {
    SETTINGS_ROWS.iter().find(|r| r.key == key)
}

pub struct SettingsView {
    pub app_state: Entity<AppState>,
    pub focus_handle: FocusHandle,
    inputs: HashMap<SettingKey, Entity<TextInput>>,
    errors: HashMap<SettingKey, String>,
    last_open: bool,
    is_syncing: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let initial = app_state.read(cx).settings.clone();
        let last_open = app_state.read(cx).settings_open;
        let mut inputs = HashMap::new();
        let mut subscriptions = Vec::new();

        for &key in NUMERIC_KEYS {
            let val = numeric_value(&initial, key).unwrap_or(1).to_string();
            let input = cx.new(|cx| {
                let mut field = TextInput::with_placeholder("", cx);
                field.set_text(val, cx);
                field
            });
            let sub = cx.observe(&input, move |this, input, cx| {
                if this.is_syncing {
                    return;
                }
                let text = input.read(cx).text().to_string();
                let _ = this.apply_numeric_text(key, &text, cx);
            });
            inputs.insert(key, input);
            subscriptions.push(sub);
        }

        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            let open = app_state.read(cx).settings_open;
            if open && !this.last_open {
                this.errors.clear();
                this.sync_inputs_from_state(cx);
            }
            this.last_open = open;
            cx.notify();
        });
        subscriptions.push(state_sub);

        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            inputs,
            errors: HashMap::new(),
            last_open,
            is_syncing: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self, cx: &gpui::App) -> bool {
        self.app_state.read(cx).settings_open
    }

    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.errors.clear();
        self.app_state.update(cx, |state, _| state.open_settings());
        self.sync_inputs_from_state(cx);
        self.last_open = true;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.errors.clear();
        self.sync_inputs_from_state(cx);
        self.app_state.update(cx, |state, _| state.close_settings());
        self.last_open = false;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, _cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
    }

    pub fn field_error(&self, key: SettingKey) -> Option<&str> {
        self.errors.get(&key).map(String::as_str)
    }

    pub fn input_entity(&self, key: SettingKey) -> Option<&Entity<TextInput>> {
        self.inputs.get(&key)
    }

    /// Synchronizes all text inputs with the currently saved `AppState::settings`.
    pub fn sync_inputs_from_state(&mut self, cx: &mut Context<Self>) {
        let settings = self.app_state.read(cx).settings.clone();
        self.is_syncing = true;
        for &key in NUMERIC_KEYS {
            if let (Some(input), Some(val)) = (self.inputs.get(&key), numeric_value(&settings, key))
            {
                let text = val.to_string();
                input.update(cx, |field, cx| {
                    if field.text() != text {
                        field.set_text(text, cx);
                    }
                });
            }
        }
        self.is_syncing = false;
    }

    /// Updates a numeric text input and validates/persists it immediately.
    pub fn set_numeric_input(
        &mut self,
        key: SettingKey,
        raw: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(input) = self.inputs.get(&key) {
            self.is_syncing = true;
            input.update(cx, |field, cx| field.set_text(raw.to_string(), cx));
            self.is_syncing = false;
        }
        self.apply_numeric_text(key, raw, cx)
    }

    fn apply_numeric_text(
        &mut self,
        key: SettingKey,
        raw: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(spec) = spec_for(key) else {
            return Err("Configuração desconhecida.".into());
        };
        let SettingControlKind::Number { min, max, .. } = spec.kind else {
            return Err("Campo não numérico.".into());
        };

        let trimmed = raw.trim();
        let parsed = match trimmed.parse::<u32>() {
            Ok(v) if (min..=max).contains(&v) => v,
            Ok(_) | Err(_) => {
                let msg = format!("Informe um número inteiro entre {min} e {max}.");
                self.errors.insert(key, msg.clone());
                cx.notify();
                return Err(msg);
            }
        };

        let result = self.app_state.update(cx, |state, _| match key {
            SettingKey::ListTimeoutSecs => state.set_list_timeout_secs(parsed),
            SettingKey::RefreshIntervalSecs => state
                .update_settings(|s| s.set_refresh_interval_secs(parsed).map_err(str::to_owned)),
            SettingKey::RemotePollIntervalSecs => {
                let enabled = state.settings.remote_poll_enabled;
                state.set_remote_poll(enabled, parsed)
            }
            SettingKey::RemoteSearchDepth => {
                let parallel = state.settings.remote_search_parallel;
                state.set_remote_search_limits(parsed, parallel)
            }
            SettingKey::RemoteSearchParallel => {
                let depth = state.settings.remote_search_depth;
                state.set_remote_search_limits(depth, parsed)
            }
            _ => Ok(()),
        });

        match result {
            Ok(()) => {
                self.errors.remove(&key);
                cx.notify();
                Ok(())
            }
            Err(err) => {
                self.errors.insert(key, err.clone());
                cx.notify();
                Err(err)
            }
        }
    }

    pub fn toggle_bool_setting(
        &mut self,
        key: SettingKey,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| match key {
            SettingKey::ShowHidden => {
                let next = !state.settings.show_hidden;
                state.update_settings(|s| {
                    s.show_hidden = next;
                    Ok(())
                })?;
                state.set_show_hidden(next);
                Ok(())
            }
            SettingKey::RemotePollEnabled => {
                let next = !state.settings.remote_poll_enabled;
                let interval = state.settings.remote_poll_interval_secs;
                state.set_remote_poll(next, interval)
            }
            _ => Err("Configuração booleana inválida.".into()),
        });
        match &res {
            Ok(()) => {
                self.errors.remove(&key);
            }
            Err(err) => {
                self.errors.insert(key, err.clone());
            }
        }
        cx.notify();
        res
    }

    pub fn set_sort_column(
        &mut self,
        column: SortColumn,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| {
            let dir = state.settings.default_sort.direction;
            state.update_settings(|s| {
                s.default_sort.column = column;
                Ok(())
            })?;
            state.set_sort(column, dir);
            Ok(())
        });
        cx.notify();
        res
    }

    pub fn set_sort_direction(
        &mut self,
        direction: SortDirection,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| {
            let col = state.settings.default_sort.column;
            state.update_settings(|s| {
                s.default_sort.direction = direction;
                Ok(())
            })?;
            state.set_sort(col, direction);
            Ok(())
        });
        cx.notify();
        res
    }

    pub fn reset_to_defaults(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| {
            state.reset_settings()?;
            let defaults = state.settings.clone();
            state.set_show_hidden(defaults.show_hidden);
            state.set_sort(
                defaults.default_sort.column,
                defaults.default_sort.direction,
            );
            Ok(())
        });
        if res.is_ok() {
            self.errors.clear();
            self.sync_inputs_from_state(cx);
        }
        cx.notify();
        res
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            cx.stop_propagation();
            self.close(cx);
        }
    }

    fn render_row(&self, spec: &SettingRowSpec, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let settings = self.app_state.read(cx).settings.clone();
        let error = self.errors.get(&spec.key).cloned();

        let control = match spec.kind {
            SettingControlKind::Toggle => {
                let active = match spec.key {
                    SettingKey::ShowHidden => settings.show_hidden,
                    SettingKey::RemotePollEnabled => settings.remote_poll_enabled,
                    _ => false,
                };
                let key = spec.key;
                div()
                    .id(("setting-toggle", key as usize))
                    .px(space::S12)
                    .py(space::S4)
                    .rounded(radius::CONTROL)
                    .border_1()
                    .border_color(if active { theme.accent } else { theme.border })
                    .bg(if active {
                        theme.accent
                    } else {
                        theme.surface_raised
                    })
                    .text_label()
                    .text_color(if active {
                        theme.on_accent
                    } else {
                        theme.text_muted
                    })
                    .cursor_pointer()
                    .child(if active { "Ativado" } else { "Desativado" })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            let _ = this.toggle_bool_setting(key, cx);
                        }),
                    )
                    .into_any_element()
            }
            SettingControlKind::SortColumn => {
                let current = settings.default_sort.column;
                let choices = [
                    (SortColumn::Name, "Nome"),
                    (SortColumn::Size, "Tamanho"),
                    (SortColumn::Date, "Data"),
                ];
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .children(choices.into_iter().enumerate().map(|(ix, (col, label))| {
                        let selected = current == col;
                        div()
                            .id(("sort-col-opt", ix))
                            .px(space::S8)
                            .py(space::S4)
                            .rounded(radius::CONTROL)
                            .border_1()
                            .border_color(if selected { theme.accent } else { theme.border })
                            .bg(if selected {
                                theme.selection
                            } else {
                                theme.surface_raised
                            })
                            .text_label()
                            .text_color(if selected {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .cursor_pointer()
                            .child(label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let _ = this.set_sort_column(col, cx);
                                }),
                            )
                    }))
                    .into_any_element()
            }
            SettingControlKind::SortDirection => {
                let current = settings.default_sort.direction;
                let choices = [
                    (SortDirection::Ascending, "Crescente"),
                    (SortDirection::Descending, "Decrescente"),
                ];
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .children(choices.into_iter().enumerate().map(|(ix, (dir, label))| {
                        let selected = current == dir;
                        div()
                            .id(("sort-dir-opt", ix))
                            .px(space::S8)
                            .py(space::S4)
                            .rounded(radius::CONTROL)
                            .border_1()
                            .border_color(if selected { theme.accent } else { theme.border })
                            .bg(if selected {
                                theme.selection
                            } else {
                                theme.surface_raised
                            })
                            .text_label()
                            .text_color(if selected {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .cursor_pointer()
                            .child(label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let _ = this.set_sort_direction(dir, cx);
                                }),
                            )
                    }))
                    .into_any_element()
            }
            SettingControlKind::Number { unit, .. } => {
                let input = self.inputs.get(&spec.key).cloned();
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .children(input.map(|field| {
                        div()
                            .w(px(76.))
                            .text_data(&fonts)
                            .border_1()
                            .border_color(if error.is_some() {
                                theme.danger
                            } else {
                                theme.border
                            })
                            .rounded(radius::CONTROL)
                            .child(field)
                    }))
                    .child(div().text_label().text_color(theme.text_muted).child(unit))
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S12)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(space::S2)
                            .child(div().text_body().text_color(theme.text).child(spec.label))
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(spec.description),
                            ),
                    )
                    .child(control),
            )
            .when_some(error, |el, msg| {
                el.child(div().text_label().text_color(theme.danger).child(msg))
            })
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open(cx) {
            return div().into_any_element();
        }

        let theme = Theme::get(cx);

        div()
            .id("settings-modal-scrim")
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
                    .id("settings-modal-card")
                    .w(px(580.))
                    .max_h(px(640.))
                    .rounded(radius::DIALOG)
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Header
                    .child(
                        div()
                            .px(space::S16)
                            .py(space::S12)
                            .border_b_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(space::S8)
                                    .child(icon(IconName::Settings).text_color(theme.accent))
                                    .child(
                                        div()
                                            .text_title()
                                            .text_color(theme.text)
                                            .child("Configurações"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-header-close")
                                    .p(space::S4)
                                    .rounded(radius::CONTROL)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.surface_raised))
                                    .child(icon(IconName::Close).text_color(theme.text_muted))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close(cx)),
                                    ),
                            ),
                    )
                    // Grouped sections
                    .child(
                        div()
                            .id("settings-scroll-body")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(space::S16)
                            .py(space::S12)
                            .flex()
                            .flex_col()
                            .gap(space::S16)
                            .children(SETTINGS_SECTIONS.iter().map(|&(section_id, title)| {
                                let rows: Vec<&SettingRowSpec> = SETTINGS_ROWS
                                    .iter()
                                    .filter(|r| r.section == section_id)
                                    .collect();
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(space::S4)
                                    .child(div().text_label().text_color(theme.accent).child(title))
                                    .children(
                                        rows.into_iter().map(|spec| self.render_row(spec, cx)),
                                    )
                            })),
                    )
                    // Footer
                    .child(
                        div()
                            .px(space::S16)
                            .py(space::S12)
                            .border_t_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .id("settings-reset")
                                    .h(size::CONTROL)
                                    .px(space::S12)
                                    .rounded(radius::CONTROL)
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.surface_raised)
                                    .flex()
                                    .items_center()
                                    .text_body()
                                    .text_color(theme.text)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.border))
                                    .child("Restaurar padrões")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            let _ = this.reset_to_defaults(cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-close")
                                    .h(size::CONTROL)
                                    .px(space::S12)
                                    .rounded(radius::CONTROL)
                                    .bg(theme.accent)
                                    .flex()
                                    .items_center()
                                    .text_body()
                                    .text_color(theme.on_accent)
                                    .cursor_pointer()
                                    .child("Fechar")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
