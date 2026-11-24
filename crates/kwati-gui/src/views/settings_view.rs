//! Grouped, data-driven Settings modal view (#29, #34, #50, #52).

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FocusHandle, KeyDownEvent, MouseButton,
    PathPromptOptions, Subscription, Task, Window,
};
use kwati_core::{
    list_installed_themes, IconGroup, IconSource, MotionPreference, Settings, SortColumn,
    SortDirection, ViewMode, ICON_TYPE_SPECS,
};

use crate::components::{
    animated_icon, animated_toggle_switch, icon, modal_scrim_color, resolve_icon_color,
    resolved_icon, wrap_modal_card_entrance, IconMotion, IconName, TextInput,
};
use crate::motion::{
    is_reduced_motion, request_frame_if_active, Curve, MotionKind, MotionTracker, INSTANT, QUICK,
    SETTLE,
};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

/// Logical groups in the Settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSectionId {
    General,
    Icons,
    NetworkMounts,
    Remote,
}

/// Stable identifier for every setting row rendered by [`SettingsView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    ShowHidden,
    ShowSidebar,
    DefaultSortColumn,
    DefaultSortDirection,
    ViewMode,
    MotionPreference,
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
    ViewMode,
    MotionPreference,
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
    (SettingsSectionId::Icons, "Ícones"),
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
        key: SettingKey::ShowSidebar,
        section: SettingsSectionId::General,
        label: "Mostrar barra lateral",
        description: "Exibe locais, favoritos e drives à esquerda da lista (F9).",
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
        key: SettingKey::ViewMode,
        section: SettingsSectionId::General,
        label: "Modo de visualização",
        description: "Exibe arquivos como detalhes, grade de ícones ou lista compacta.",
        kind: SettingControlKind::ViewMode,
    },
    SettingRowSpec {
        key: SettingKey::MotionPreference,
        section: SettingsSectionId::General,
        label: "Animações e movimento",
        description: "Controla movimento e escalas; o modo reduzido mantém apenas cor e opacidade.",
        kind: SettingControlKind::MotionPreference,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SettingsMotionKey {
    ModalOpen,
    Toggle(SettingKey),
    HeaderSettingsIcon,
    Button(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum IconFocusKey {
    HeaderClose,
    BaseBuiltIn,
    BaseTheme(usize),
    BaseThemeDir,
    GroupInherit(usize),
    GroupBuiltIn(usize),
    GroupTheme(usize),
    GroupImage(usize),
    TypeInherit(usize),
    TypeBuiltIn(usize),
    TypeTheme(usize),
    TypeImage(usize),
    PresetSave,
    PresetApply(usize),
    PresetRename(usize),
    PresetRenameConfirm(usize),
    PresetRenameCancel(usize),
    PresetDelete(usize),
    ResetDefaults,
    FooterClose,
}

#[derive(Debug, Clone)]
enum IconPickerTarget {
    Group(IconGroup),
    Type(String),
    ThemeDir,
}

const SAMPLE_PREVIEW_ICONS: &[(IconName, &str)] = &[
    (IconName::Folder, "Pasta"),
    (IconName::FolderDownloads, "Downloads"),
    (IconName::FileImage, "Imagem"),
    (IconName::FilePdf, "PDF"),
    (IconName::FileCode, "Código"),
    (IconName::FileArchive, "Arquivo"),
    (IconName::FileAudio, "Áudio"),
    (IconName::File, "Outro"),
];

pub struct SettingsView {
    pub app_state: Entity<AppState>,
    pub focus_handle: FocusHandle,
    inputs: HashMap<SettingKey, Entity<TextInput>>,
    last_input_texts: HashMap<SettingKey, String>,
    errors: HashMap<SettingKey, String>,
    icon_error: Option<String>,
    installed_themes: Vec<String>,
    preset_name_input: Entity<TextInput>,
    preset_rename_input: Entity<TextInput>,
    renaming_preset: Option<String>,
    focus_handles: HashMap<IconFocusKey, FocusHandle>,
    focus_order: Vec<FocusHandle>,
    picker_task: Option<Task<()>>,
    motion: MotionTracker<SettingsMotionKey>,
    last_open: bool,
    is_syncing: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let initial = app_state.read(cx).settings.clone();
        let last_open = app_state.read(cx).settings_open;
        let mut inputs = HashMap::new();
        let mut last_input_texts = HashMap::new();
        let mut subscriptions = Vec::new();

        for &key in NUMERIC_KEYS {
            let val = numeric_value(&initial, key).unwrap_or(1).to_string();
            last_input_texts.insert(key, val.clone());
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
                if this.last_input_texts.get(&key) == Some(&text) {
                    return;
                }
                this.last_input_texts.insert(key, text.clone());
                let _ = this.apply_numeric_text(key, &text, cx);
            });
            inputs.insert(key, input);
            subscriptions.push(sub);
        }

        let preset_name_input = cx.new(|cx| TextInput::with_placeholder("Nome do preset", cx));
        let preset_rename_input = cx.new(|cx| TextInput::with_placeholder("Novo nome", cx));

        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            let open = app_state.read(cx).settings_open;
            if open && !this.last_open {
                let now = Instant::now();
                this.errors.clear();
                this.icon_error = None;
                this.renaming_preset = None;
                this.installed_themes = list_installed_themes();
                this.sync_inputs_from_state(cx);
                this.motion.set_immediate(
                    SettingsMotionKey::ModalOpen,
                    0.0,
                    MotionKind::MovementOrScale,
                );
                this.motion.trigger_pulse(
                    SettingsMotionKey::HeaderSettingsIcon,
                    QUICK,
                    MotionKind::MovementOrScale,
                    now,
                );
            }
            this.last_open = open;
            cx.notify();
        });
        subscriptions.push(state_sub);

        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            inputs,
            last_input_texts,
            errors: HashMap::new(),
            icon_error: None,
            installed_themes: list_installed_themes(),
            preset_name_input,
            preset_rename_input,
            renaming_preset: None,
            focus_handles: HashMap::new(),
            focus_order: Vec::new(),
            picker_task: None,
            motion: MotionTracker::new(),
            last_open,
            is_syncing: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self, cx: &gpui::App) -> bool {
        self.app_state.read(cx).settings_open
    }

    pub fn open(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.errors.clear();
        self.icon_error = None;
        self.renaming_preset = None;
        self.installed_themes = list_installed_themes();
        self.app_state.update(cx, |state, _| state.open_settings());
        self.sync_inputs_from_state(cx);
        self.motion.set_immediate(
            SettingsMotionKey::ModalOpen,
            0.0,
            MotionKind::MovementOrScale,
        );
        self.motion.trigger_pulse(
            SettingsMotionKey::HeaderSettingsIcon,
            QUICK,
            MotionKind::MovementOrScale,
            now,
        );
        self.last_open = true;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.errors.clear();
        self.icon_error = None;
        self.renaming_preset = None;
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

    pub fn icon_error(&self) -> Option<&str> {
        self.icon_error.as_deref()
    }

    pub fn installed_themes(&self) -> &[String] {
        &self.installed_themes
    }

    /// Overrides the detected system themes list (for platform-independent view tests).
    pub fn set_installed_themes_for_test(&mut self, themes: Vec<String>, cx: &mut Context<Self>) {
        self.installed_themes = themes;
        cx.notify();
    }

    pub fn preset_name_input(&self) -> &Entity<TextInput> {
        &self.preset_name_input
    }

    pub fn preset_rename_input(&self) -> &Entity<TextInput> {
        &self.preset_rename_input
    }

    pub fn renaming_preset(&self) -> Option<&str> {
        self.renaming_preset.as_deref()
    }

    pub fn focus_handles_count(&self) -> usize {
        self.focus_order.len()
    }

    pub fn is_valid_icon_image_path(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png") || ext.eq_ignore_ascii_case("svg"))
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
                self.last_input_texts.insert(key, text.clone());
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
        self.last_input_texts.insert(key, raw.to_string());
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
                if let Some(input) = self.inputs.get(&key).cloned() {
                    input.update(cx, |field, cx| field.trigger_shake(cx));
                }
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
                if let Some(input) = self.inputs.get(&key).cloned() {
                    input.update(cx, |field, cx| field.set_invalid(false, cx));
                }
                cx.notify();
                Ok(())
            }
            Err(err) => {
                self.errors.insert(key, err.clone());
                if let Some(input) = self.inputs.get(&key).cloned() {
                    input.update(cx, |field, cx| field.trigger_shake(cx));
                }
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
            SettingKey::ShowSidebar => {
                let next = !state.settings.show_sidebar;
                state.update_settings(|s| {
                    s.show_sidebar = next;
                    Ok(())
                })?;
                state.set_sidebar_open(next);
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

    pub fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| {
            state.set_view_mode(mode);
            Ok(())
        });
        cx.notify();
        res
    }

    pub fn set_motion_preference(
        &mut self,
        pref: MotionPreference,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, _| {
            state.update_settings(|s| {
                s.motion_preference = pref;
                Ok(())
            })
        });
        cx.notify();
        res
    }

    pub fn reset_to_defaults(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.reset_settings()?;
            state.reset_icon_preferences()?;
            let defaults = state.settings.clone();
            state.set_show_hidden(defaults.show_hidden);
            state.set_sort(
                defaults.default_sort.column,
                defaults.default_sort.direction,
            );
            state.set_view_mode(defaults.view_mode);
            cx.notify();
            Ok(())
        });
        if res.is_ok() {
            self.errors.clear();
            self.icon_error = None;
            self.renaming_preset = None;
            self.sync_inputs_from_state(cx);
        }
        cx.notify();
        res
    }

    pub fn set_icon_base(
        &mut self,
        source: IconSource,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.set_icon_base(source)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        cx.notify();
        res
    }

    pub fn set_icon_group_override(
        &mut self,
        group: IconGroup,
        source: Option<IconSource>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.set_icon_group_override(group, source)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        cx.notify();
        res
    }

    pub fn set_icon_type_override(
        &mut self,
        type_key: &str,
        source: Option<IconSource>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.set_icon_type_override(type_key, source)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        cx.notify();
        res
    }

    pub fn save_icon_preset(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.save_icon_preset(name)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        if res.is_ok() {
            self.preset_name_input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        cx.notify();
        res
    }

    pub fn apply_icon_preset(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.apply_icon_preset(name)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        cx.notify();
        res
    }

    pub fn rename_icon_preset(
        &mut self,
        old_name: &str,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let res = self.app_state.update(cx, |state, cx| {
            state.rename_icon_preset(old_name, new_name)?;
            cx.notify();
            Ok(())
        });
        self.record_icon_result(&res);
        if res.is_ok() {
            self.renaming_preset = None;
        }
        cx.notify();
        res
    }

    pub fn delete_icon_preset(
        &mut self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let res: Result<bool, String> = self.app_state.update(cx, |state, cx| {
            let deleted = state.delete_icon_preset(name)?;
            cx.notify();
            Ok(deleted)
        });
        match &res {
            Ok(_) => {
                self.icon_error = None;
                if self.renaming_preset.as_deref() == Some(name) {
                    self.renaming_preset = None;
                }
            }
            Err(err) => self.icon_error = Some(err.clone()),
        }
        cx.notify();
        res
    }

    fn begin_rename_preset(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_preset = Some(name.to_string());
        let text = name.to_string();
        self.preset_rename_input.update(cx, |input, cx| {
            input.set_text(text, cx);
        });
        self.preset_rename_input.read(cx).focus_handle.focus(window);
        cx.notify();
    }

    fn commit_rename_preset(&mut self, cx: &mut Context<Self>) {
        let Some(old_name) = self.renaming_preset.clone() else {
            return;
        };
        let new_name = self.preset_rename_input.read(cx).text().to_string();
        let _ = self.rename_icon_preset(&old_name, &new_name, cx);
    }

    fn cancel_rename_preset(&mut self, cx: &mut Context<Self>) {
        self.renaming_preset = None;
        self.icon_error = None;
        cx.notify();
    }

    fn record_icon_result(&mut self, res: &Result<(), String>) {
        match res {
            Ok(()) => self.icon_error = None,
            Err(err) => self.icon_error = Some(err.clone()),
        }
    }

    fn next_theme_choice(&self, current: Option<&IconSource>, base: &IconSource) -> Option<String> {
        if self.installed_themes.is_empty() {
            return None;
        }
        if let Some(IconSource::Theme(cur)) = current {
            if let Some(idx) = self.installed_themes.iter().position(|t| t == cur) {
                let next = (idx + 1) % self.installed_themes.len();
                return Some(self.installed_themes[next].clone());
            }
        }
        if let IconSource::Theme(base_theme) = base {
            if self.installed_themes.iter().any(|t| t == base_theme) {
                return Some(base_theme.clone());
            }
        }
        Some(self.installed_themes[0].clone())
    }

    fn open_icon_picker(&mut self, target: IconPickerTarget, cx: &mut Context<Self>) {
        let options = match target {
            IconPickerTarget::ThemeDir => PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Selecionar pasta do tema de ícones".into()),
            },
            IconPickerTarget::Group(_) | IconPickerTarget::Type(_) => PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Selecionar ícone (PNG ou SVG)".into()),
            },
        };
        let rx = cx.prompt_for_paths(options);
        self.picker_task = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(chosen) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |view, cx| match target {
                IconPickerTarget::ThemeDir => {
                    if chosen.join("index.theme").is_file() {
                        let _ = view.set_icon_base(
                            IconSource::Theme(chosen.to_string_lossy().into_owned()),
                            cx,
                        );
                    } else {
                        view.icon_error =
                            Some("A pasta selecionada não contém index.theme.".into());
                        cx.notify();
                    }
                }
                IconPickerTarget::Group(group) => {
                    if Self::is_valid_icon_image_path(&chosen) {
                        let _ = view.set_icon_group_override(
                            group,
                            Some(IconSource::Image(chosen)),
                            cx,
                        );
                    } else {
                        view.icon_error = Some("Selecione uma imagem PNG ou SVG.".into());
                        cx.notify();
                    }
                }
                IconPickerTarget::Type(key) => {
                    if Self::is_valid_icon_image_path(&chosen) {
                        let _ =
                            view.set_icon_type_override(&key, Some(IconSource::Image(chosen)), cx);
                    } else {
                        view.icon_error = Some("Selecione uma imagem PNG ou SVG.".into());
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn ensure_focus_handle(&mut self, key: IconFocusKey, cx: &mut Context<Self>) -> FocusHandle {
        self.focus_handles
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn register_focus(&mut self, handle: &FocusHandle) {
        self.focus_order.push(handle.clone());
    }

    fn cycle_focus(&self, reverse: bool, window: &mut Window) {
        if self.focus_order.is_empty() {
            return;
        }
        let current = self.focus_order.iter().position(|h| h.is_focused(window));
        let next = match (current, reverse) {
            (Some(idx), false) => (idx + 1) % self.focus_order.len(),
            (Some(idx), true) => (idx + self.focus_order.len() - 1) % self.focus_order.len(),
            (None, false) => 0,
            (None, true) => self.focus_order.len() - 1,
        };
        self.focus_order[next].focus(window);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "tab" => {
                cx.stop_propagation();
                self.cycle_focus(event.keystroke.modifiers.shift, window);
                cx.notify();
            }
            "escape" => {
                cx.stop_propagation();
                if self.renaming_preset.is_some() {
                    self.cancel_rename_preset(cx);
                } else {
                    self.close(cx);
                }
            }
            _ => {}
        }
    }

    fn render_row(
        &mut self,
        spec: &SettingRowSpec,
        now: Instant,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let settings = self.app_state.read(cx).settings.clone();
        let error = self.errors.get(&spec.key).cloned();

        let control = match spec.kind {
            SettingControlKind::Toggle => {
                let active = match spec.key {
                    SettingKey::ShowHidden => settings.show_hidden,
                    SettingKey::ShowSidebar => settings.show_sidebar,
                    SettingKey::RemotePollEnabled => settings.remote_poll_enabled,
                    _ => false,
                };
                let key = spec.key;
                let slide_t = self.motion.step_scalar(
                    SettingsMotionKey::Toggle(key),
                    if active { 1.0 } else { 0.0 },
                    if active { 1.0 } else { 0.0 },
                    QUICK,
                    Curve::Spring,
                    MotionKind::OpacityOrColor,
                    now,
                    reduced,
                );
                let press_t = self
                    .motion
                    .pulse_progress(&SettingsMotionKey::Button(key as usize), now, reduced)
                    .map_or(0.0, |p| (p * std::f32::consts::PI).sin());

                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(
                        div()
                            .text_label()
                            .text_color(if active { theme.text } else { theme.text_muted })
                            .child(if active { "Ativado" } else { "Desativado" }),
                    )
                    .child(
                        animated_toggle_switch(
                            ("setting-toggle", key as usize),
                            slide_t,
                            press_t,
                            &theme,
                            reduced,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.motion.trigger_pulse(
                                    SettingsMotionKey::Button(key as usize),
                                    INSTANT,
                                    MotionKind::MovementOrScale,
                                    Instant::now(),
                                );
                                let _ = this.toggle_bool_setting(key, cx);
                            }),
                        ),
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
            SettingControlKind::ViewMode => {
                let current = settings.view_mode;
                let choices = [
                    (ViewMode::Details, "Detalhes"),
                    (ViewMode::Grid, "Grade"),
                    (ViewMode::CompactList, "Compacto"),
                ];
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .children(choices.into_iter().enumerate().map(|(ix, (mode, label))| {
                        let selected = current == mode;
                        div()
                            .id(("view-mode-opt", ix))
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
                                    let _ = this.set_view_mode(mode, cx);
                                }),
                            )
                    }))
                    .into_any_element()
            }
            SettingControlKind::MotionPreference => {
                let current = settings.motion_preference;
                let choices = [
                    (MotionPreference::System, "Automático"),
                    (MotionPreference::Full, "Completo"),
                    (MotionPreference::Reduced, "Reduzido"),
                ];
                div()
                    .flex()
                    .items_center()
                    .gap(space::S4)
                    .children(choices.into_iter().enumerate().map(|(ix, (pref, label))| {
                        let selected = current == pref;
                        div()
                            .id(("motion-pref-opt", ix))
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
                                    let _ = this.set_motion_preference(pref, cx);
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
                    .children(input.map(|field| div().w(px(76.)).text_data(&fonts).child(field)))
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

    #[allow(clippy::too_many_arguments)]
    fn render_action_pill(
        &mut self,
        id: impl Into<gpui::ElementId>,
        focus_key: IconFocusKey,
        label: impl Into<gpui::SharedString>,
        active: bool,
        primary: bool,
        window: &Window,
        cx: &mut Context<Self>,
        on_activate: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let theme = Theme::get(cx);
        let handle = self.ensure_focus_handle(focus_key, cx);
        self.register_focus(&handle);
        let focused = handle.is_focused(window);

        let bg = if primary {
            theme.accent
        } else if active {
            theme.selection
        } else {
            theme.surface_raised
        };
        let fg = if primary {
            theme.on_accent
        } else if active {
            theme.text
        } else {
            theme.text_muted
        };
        let border = if focused || active {
            theme.accent
        } else {
            theme.border
        };

        let handler = std::rc::Rc::new(on_activate);
        let handler_key = handler.clone();

        div()
            .id(id)
            .track_focus(&handle)
            .px(space::S8)
            .py(space::S2)
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_label()
            .text_color(fg)
            .cursor_pointer()
            .hover(|s| s.border_color(theme.accent))
            .child(label.into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    handler(this, window, cx);
                }),
            )
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                if matches!(ev.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    handler_key(this, window, cx);
                }
            }))
            .into_any_element()
    }

    fn render_icons_section(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx);
        let (prefs, presets, resolver) = {
            let state = self.app_state.read(cx);
            (
                state.icon_config.active.clone(),
                state.icon_config.presets.clone(),
                state.icon_resolver.clone(),
            )
        };
        let installed_themes = self.installed_themes.clone();
        let has_themes = !installed_themes.is_empty();

        // 1. Base source row
        let mut base_buttons = vec![self.render_action_pill(
            "icon-base-builtin",
            IconFocusKey::BaseBuiltIn,
            "Integrado",
            prefs.base == IconSource::BuiltIn,
            false,
            window,
            cx,
            |this, _, cx| {
                let _ = this.set_icon_base(IconSource::BuiltIn, cx);
            },
        )];
        for (ix, theme_name) in installed_themes.iter().enumerate() {
            let t_name = theme_name.clone();
            let selected = matches!(&prefs.base, IconSource::Theme(cur) if cur == theme_name);
            base_buttons.push(self.render_action_pill(
                ("icon-base-theme", ix),
                IconFocusKey::BaseTheme(ix),
                theme_name.clone(),
                selected,
                false,
                window,
                cx,
                move |this, _, cx| {
                    let _ = this.set_icon_base(IconSource::Theme(t_name.clone()), cx);
                },
            ));
        }
        let custom_theme_active = matches!(
            &prefs.base,
            IconSource::Theme(cur) if !installed_themes.iter().any(|t| t == cur)
        );
        base_buttons.push(self.render_action_pill(
            "icon-base-theme-dir",
            IconFocusKey::BaseThemeDir,
            "Pasta de tema…",
            custom_theme_active,
            false,
            window,
            cx,
            |this, _, cx| {
                this.open_icon_picker(IconPickerTarget::ThemeDir, cx);
            },
        ));

        let base_row = div()
            .flex()
            .flex_col()
            .gap(space::S8)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S8)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space::S2)
                            .child(div().text_body().text_color(theme.text).child("Origem base"))
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(if has_themes {
                                        format!("Atual: {}", source_summary(Some(&prefs.base)))
                                    } else {
                                        "Nenhum tema do sistema encontrado; use Integrado, Pasta de tema… ou imagens.".to_string()
                                    }),
                            ),
                    ),
            )
            .child(div().flex().flex_wrap().gap(space::S4).children(base_buttons));

        // 2. Live preview row
        let preview_items = SAMPLE_PREVIEW_ICONS.iter().map(|&(builtin, label)| {
            let color = resolve_icon_color(builtin.key_name(), &prefs, &theme);
            div()
                .flex()
                .items_center()
                .gap(space::S4)
                .px(space::S8)
                .py(space::S4)
                .rounded(radius::CONTROL)
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .child(resolved_icon(
                    builtin,
                    IconMotion::Rest,
                    size::ICON,
                    color,
                    &prefs,
                    &resolver,
                ))
                .child(div().text_label().text_color(theme.text_muted).child(label))
        });

        let preview_row = div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_label()
                    .text_color(theme.text_muted)
                    .child("Pré-visualização"),
            )
            .child(
                div()
                    .p(space::S8)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .flex()
                    .flex_wrap()
                    .gap(space::S8)
                    .children(preview_items),
            );

        // 3. Group overrides list
        let mut group_rows = Vec::with_capacity(IconGroup::ALL.len());
        for (ix, &group) in IconGroup::ALL.iter().enumerate() {
            let override_src = prefs.groups.get(&group);
            let rep = representative_icon_for_group(group);
            let color = prefs
                .group_colors
                .get(&group)
                .copied()
                .or(prefs.base_color)
                .map(|c| theme.resolve_icon_color(c).into())
                .unwrap_or_else(|| resolve_icon_color(rep.key_name(), &prefs, &theme));
            let mut controls = vec![
                self.render_action_pill(
                    ("icon-grp-inherit", ix),
                    IconFocusKey::GroupInherit(ix),
                    "Herdar",
                    override_src.is_none(),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ = this.set_icon_group_override(group, None, cx);
                    },
                ),
                self.render_action_pill(
                    ("icon-grp-builtin", ix),
                    IconFocusKey::GroupBuiltIn(ix),
                    "Integrado",
                    matches!(override_src, Some(IconSource::BuiltIn)),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ = this.set_icon_group_override(group, Some(IconSource::BuiltIn), cx);
                    },
                ),
            ];
            if has_themes {
                let next_theme = self.next_theme_choice(override_src, &prefs.base);
                controls.push(self.render_action_pill(
                    ("icon-grp-theme", ix),
                    IconFocusKey::GroupTheme(ix),
                    "Tema",
                    matches!(override_src, Some(IconSource::Theme(_))),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        if let Some(name) = next_theme.clone() {
                            let _ = this.set_icon_group_override(
                                group,
                                Some(IconSource::Theme(name)),
                                cx,
                            );
                        }
                    },
                ));
            }
            controls.push(self.render_action_pill(
                ("icon-grp-image", ix),
                IconFocusKey::GroupImage(ix),
                "Imagem…",
                matches!(override_src, Some(IconSource::Image(_))),
                false,
                window,
                cx,
                move |this, _, cx| {
                    this.open_icon_picker(IconPickerTarget::Group(group), cx);
                },
            ));

            group_rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S8)
                    .py(space::S4)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S8)
                            .flex_1()
                            .min_w_0()
                            .child(resolved_icon(
                                rep,
                                IconMotion::Rest,
                                size::ICON,
                                color,
                                &prefs,
                                &resolver,
                            ))
                            .child(
                                div()
                                    .text_body()
                                    .text_color(theme.text)
                                    .child(group.label()),
                            )
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(source_summary(override_src)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S4)
                            .children(controls),
                    ),
            );
        }

        let groups_block = div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_label()
                    .text_color(theme.text_muted)
                    .child("Substituições por grupo"),
            )
            .children(group_rows);

        // 4. Per-type overrides list
        let mut type_rows = Vec::with_capacity(ICON_TYPE_SPECS.len());
        for (ix, spec) in ICON_TYPE_SPECS.iter().enumerate() {
            let builtin = IconName::from_key_name(spec.key).unwrap_or(IconName::File);
            let color = resolve_icon_color(spec.key, &prefs, &theme);
            let override_src = prefs.types.get(spec.key);
            let type_key = spec.key;

            let mut controls = vec![
                self.render_action_pill(
                    ("icon-type-inherit", ix),
                    IconFocusKey::TypeInherit(ix),
                    "Herdar",
                    override_src.is_none(),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ = this.set_icon_type_override(type_key, None, cx);
                    },
                ),
                self.render_action_pill(
                    ("icon-type-builtin", ix),
                    IconFocusKey::TypeBuiltIn(ix),
                    "Integrado",
                    matches!(override_src, Some(IconSource::BuiltIn)),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ =
                            this.set_icon_type_override(type_key, Some(IconSource::BuiltIn), cx);
                    },
                ),
            ];
            if has_themes {
                let next_theme = self.next_theme_choice(override_src, &prefs.base);
                controls.push(self.render_action_pill(
                    ("icon-type-theme", ix),
                    IconFocusKey::TypeTheme(ix),
                    "Tema",
                    matches!(override_src, Some(IconSource::Theme(_))),
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        if let Some(name) = next_theme.clone() {
                            let _ = this.set_icon_type_override(
                                type_key,
                                Some(IconSource::Theme(name)),
                                cx,
                            );
                        }
                    },
                ));
            }
            controls.push(self.render_action_pill(
                ("icon-type-image", ix),
                IconFocusKey::TypeImage(ix),
                "Imagem…",
                matches!(override_src, Some(IconSource::Image(_))),
                false,
                window,
                cx,
                move |this, _, cx| {
                    this.open_icon_picker(IconPickerTarget::Type(type_key.to_string()), cx);
                },
            ));

            type_rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space::S8)
                    .py(space::S2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S8)
                            .flex_1()
                            .min_w_0()
                            .child(resolved_icon(
                                builtin,
                                IconMotion::Rest,
                                size::ICON,
                                color,
                                &prefs,
                                &resolver,
                            ))
                            .child(div().text_body().text_color(theme.text).child(spec.label))
                            .child(
                                div()
                                    .px(space::S4)
                                    .py(px(1.))
                                    .rounded(radius::CONTROL)
                                    .bg(theme.surface_raised)
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(spec.group.label()),
                            )
                            .child(
                                div()
                                    .text_label()
                                    .text_color(theme.text_muted)
                                    .child(source_summary(override_src)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S4)
                            .children(controls),
                    ),
            );
        }

        let types_block = div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_label()
                    .text_color(theme.text_muted)
                    .child("Substituições por tipo"),
            )
            .children(type_rows);

        // 5. Presets block
        let preset_input_handle = self.preset_name_input.read(cx).focus_handle.clone();
        self.register_focus(&preset_input_handle);
        let save_btn = self.render_action_pill(
            "icon-preset-save",
            IconFocusKey::PresetSave,
            "Salvar preset",
            false,
            true,
            window,
            cx,
            |this, _, cx| {
                let name = this.preset_name_input.read(cx).text().to_string();
                let _ = this.save_icon_preset(&name, cx);
            },
        );

        let mut preset_rows = Vec::new();
        let renaming = self.renaming_preset.clone();
        for (ix, name) in presets.keys().enumerate() {
            let preset_name = name.clone();
            if renaming.as_deref() == Some(name.as_str()) {
                let rename_handle = self.preset_rename_input.read(cx).focus_handle.clone();
                self.register_focus(&rename_handle);
                let confirm_btn = self.render_action_pill(
                    ("icon-preset-rename-confirm", ix),
                    IconFocusKey::PresetRenameConfirm(ix),
                    "Confirmar",
                    false,
                    true,
                    window,
                    cx,
                    |this, _, cx| {
                        this.commit_rename_preset(cx);
                    },
                );
                let cancel_btn = self.render_action_pill(
                    ("icon-preset-rename-cancel", ix),
                    IconFocusKey::PresetRenameCancel(ix),
                    "Cancelar",
                    false,
                    false,
                    window,
                    cx,
                    |this, _, cx| {
                        this.cancel_rename_preset(cx);
                    },
                );
                preset_rows.push(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(space::S8)
                        .py(space::S4)
                        .child(
                            div()
                                .flex_1()
                                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                    if ev.keystroke.key == "enter" {
                                        cx.stop_propagation();
                                        this.commit_rename_preset(cx);
                                    } else if ev.keystroke.key == "escape" {
                                        cx.stop_propagation();
                                        this.cancel_rename_preset(cx);
                                    }
                                }))
                                .child(self.preset_rename_input.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S4)
                                .child(confirm_btn)
                                .child(cancel_btn),
                        )
                        .into_any_element(),
                );
            } else {
                let apply_name = preset_name.clone();
                let rename_name = preset_name.clone();
                let delete_name = preset_name.clone();
                let apply_btn = self.render_action_pill(
                    ("icon-preset-apply", ix),
                    IconFocusKey::PresetApply(ix),
                    "Aplicar",
                    false,
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ = this.apply_icon_preset(&apply_name, cx);
                    },
                );
                let rename_btn = self.render_action_pill(
                    ("icon-preset-rename", ix),
                    IconFocusKey::PresetRename(ix),
                    "Renomear",
                    false,
                    false,
                    window,
                    cx,
                    move |this, window, cx| {
                        this.begin_rename_preset(&rename_name, window, cx);
                    },
                );
                let delete_btn = self.render_action_pill(
                    ("icon-preset-delete", ix),
                    IconFocusKey::PresetDelete(ix),
                    "Excluir",
                    false,
                    false,
                    window,
                    cx,
                    move |this, _, cx| {
                        let _ = this.delete_icon_preset(&delete_name, cx);
                    },
                );
                preset_rows.push(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(space::S8)
                        .py(space::S4)
                        .child(div().text_body().text_color(theme.text).child(preset_name))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S4)
                                .child(apply_btn)
                                .child(rename_btn)
                                .child(delete_btn),
                        )
                        .into_any_element(),
                );
            }
        }

        let presets_block = div()
            .flex()
            .flex_col()
            .gap(space::S8)
            .py(space::S8)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_label()
                    .text_color(theme.text_muted)
                    .child("Presets salvos"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S8)
                    .child(
                        div()
                            .flex_1()
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                if ev.keystroke.key == "enter" {
                                    cx.stop_propagation();
                                    let name = this.preset_name_input.read(cx).text().to_string();
                                    let _ = this.save_icon_preset(&name, cx);
                                }
                            }))
                            .child(self.preset_name_input.clone()),
                    )
                    .child(save_btn),
            )
            .when(preset_rows.is_empty(), |el| {
                el.child(
                    div()
                        .text_label()
                        .text_color(theme.text_muted)
                        .child("Nenhum preset salvo."),
                )
            })
            .children(preset_rows);

        div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .child(base_row)
            .child(preview_row)
            .child(groups_block)
            .child(types_block)
            .child(presets_block)
            .when_some(self.icon_error.clone(), |el, msg| {
                el.child(div().text_label().text_color(theme.danger).child(msg))
            })
            .into_any_element()
    }
}

fn representative_icon_for_group(group: IconGroup) -> IconName {
    match group {
        IconGroup::Folders => IconName::Folder,
        IconGroup::SpecialFolders => IconName::FolderDownloads,
        IconGroup::Images => IconName::FileImage,
        IconGroup::Documents => IconName::FilePdf,
        IconGroup::Code => IconName::FileCode,
        IconGroup::Archives => IconName::FileArchive,
        IconGroup::AudioVideo => IconName::FileAudio,
        IconGroup::Other => IconName::File,
    }
}

fn source_summary(source: Option<&IconSource>) -> String {
    match source {
        None => "Herdando".to_string(),
        Some(IconSource::BuiltIn) => "Integrado".to_string(),
        Some(IconSource::Theme(name)) => format!("Tema: {name}"),
        Some(IconSource::Image(path)) => {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            format!("Imagem: {file_name}")
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open(cx) {
            return div().into_any_element();
        }

        let now = Instant::now();
        let theme = Theme::get(cx);
        let reduced = is_reduced_motion(self.app_state.read(cx).settings.motion_preference);

        let open_t = self.motion.step_scalar(
            SettingsMotionKey::ModalOpen,
            0.0,
            1.0,
            SETTLE,
            Curve::Linear,
            MotionKind::OpacityOrColor,
            now,
            reduced,
        );
        let icon_motion = self
            .motion
            .pulse_progress(&SettingsMotionKey::HeaderSettingsIcon, now, reduced)
            .map_or(IconMotion::Rest, IconMotion::Pulse);

        self.focus_order.clear();
        let header_close_handle = self.ensure_focus_handle(IconFocusKey::HeaderClose, cx);
        self.register_focus(&header_close_handle);
        let header_close_focused = header_close_handle.is_focused(window);

        let sections = SETTINGS_SECTIONS
            .iter()
            .map(|&(section_id, title)| {
                if section_id == SettingsSectionId::Icons {
                    let icons_body = self.render_icons_section(window, cx);
                    return div()
                        .flex()
                        .flex_col()
                        .gap(space::S4)
                        .child(div().text_label().text_color(theme.accent).child(title))
                        .child(icons_body);
                }
                let rows: Vec<&SettingRowSpec> = SETTINGS_ROWS
                    .iter()
                    .filter(|r| r.section == section_id)
                    .collect();
                for spec in &rows {
                    if let Some(input) = self.inputs.get(&spec.key) {
                        self.register_focus(&input.read(cx).focus_handle);
                    }
                }
                let rendered_rows: Vec<_> = rows
                    .into_iter()
                    .map(|spec| self.render_row(spec, now, reduced, cx))
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(space::S4)
                    .child(div().text_label().text_color(theme.accent).child(title))
                    .children(rendered_rows)
            })
            .collect::<Vec<_>>();

        let reset_handle = self.ensure_focus_handle(IconFocusKey::ResetDefaults, cx);
        self.register_focus(&reset_handle);
        let reset_focused = reset_handle.is_focused(window);

        let footer_close_handle = self.ensure_focus_handle(IconFocusKey::FooterClose, cx);
        self.register_focus(&footer_close_handle);
        let footer_close_focused = footer_close_handle.is_focused(window);

        let card = div()
            .id("settings-modal-card")
            .w(px(660.))
            .max_h(px(660.))
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
                            .child(animated_icon(IconName::Settings, icon_motion, theme.accent))
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
                            .track_focus(&header_close_handle)
                            .p(space::S4)
                            .rounded(radius::CONTROL)
                            .border_1()
                            .border_color(if header_close_focused {
                                theme.accent
                            } else {
                                gpui::transparent_black().into()
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_raised))
                            .child(icon(IconName::Close).text_color(theme.text_muted))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.close(cx)),
                            )
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                if matches!(ev.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    this.close(cx);
                                }
                            })),
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
                    .children(sections),
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
                            .track_focus(&reset_handle)
                            .h(size::CONTROL)
                            .px(space::S12)
                            .rounded(radius::CONTROL)
                            .border_1()
                            .border_color(if reset_focused {
                                theme.accent
                            } else {
                                theme.border
                            })
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
                            )
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                if matches!(ev.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    let _ = this.reset_to_defaults(cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("settings-close")
                            .track_focus(&footer_close_handle)
                            .h(size::CONTROL)
                            .px(space::S12)
                            .rounded(radius::CONTROL)
                            .border_1()
                            .border_color(if footer_close_focused {
                                theme.text
                            } else {
                                theme.accent
                            })
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
                            )
                            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                if matches!(ev.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    this.close(cx);
                                }
                            })),
                    ),
            )
            .into_any_element();

        request_frame_if_active(self.motion.has_active(now, reduced), window);

        div()
            .id("settings-modal-scrim")
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(modal_scrim_color(&theme, open_t))
            .occlude()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(wrap_modal_card_entrance(card, px(660.), open_t, reduced))
            .into_any_element()
    }
}
