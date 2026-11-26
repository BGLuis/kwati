//! Context menu view for file list entries and empty space (#55).

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnyElement, Context, CursorStyle, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, Pixels,
    Point, SharedString, Size, Window,
};
use kwati_core::{
    all_local_volumes, volume_kind, Location, SortColumn, SortDirection, ViewMode, VolumeKind,
};

use crate::actions::open_with::{menu_applications_for, AppInfo};
use crate::actions::operations::has_location_clipboard;
use crate::components::{animated_icon, popup_entrance_metrics, Ellipsize, IconMotion, IconName};
use crate::motion::{
    color_lerp, is_reduced_motion, request_frame_if_active, Curve, MotionKind, MotionTracker,
    INSTANT, MENU_CASCADE_STEP, QUICK,
};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space, Fonts, TextRole};

/// Actions offered by the file list or sidebar Places context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Open,
    OpenWithApp(usize),
    OpenWithOther,
    Rename,
    Bookmark,
    Copy,
    Cut,
    Paste,
    Remove,
    NewFolder,
    Undo,
    Refresh,
    SidebarOpen(usize),
    SidebarRename(usize),
    SidebarRemove(usize),
    SetViewMode(ViewMode),
    SetSortColumn(SortColumn),
    SetSortDirection(SortDirection),
    ToggleShowHidden,
    ToggleFoldersFirst,
    ToggleHiddenLast,
    ToggleSidebar,
    TogglePreview,
    ApplyFolderSettingsToAll,
}

/// Active listing options for the current directory shown in the empty-space context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderMenuState {
    pub view_mode: ViewMode,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub show_hidden: bool,
    pub folders_first: bool,
    pub hidden_last: bool,
    pub show_sidebar: bool,
    pub show_preview: bool,
}

impl Default for FolderMenuState {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::default(),
            sort_column: SortColumn::default(),
            sort_direction: SortDirection::default(),
            show_hidden: false,
            folders_first: true,
            hidden_last: false,
            show_sidebar: true,
            show_preview: false,
        }
    }
}

/// An item, submenu, or separator inside the context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuEntry {
    Item {
        action: ContextMenuAction,
        label: Cow<'static, str>,
        shortcut: Option<&'static str>,
        enabled: bool,
        destructive: bool,
        checked: bool,
    },
    Submenu {
        label: Cow<'static, str>,
        items: Vec<ContextMenuEntry>,
    },
    Separator,
}

/// Events emitted by [`ContextMenuView`] to [`super::MainView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuEvent {
    Execute(ContextMenuAction),
    Closed,
}

fn menu_item(
    action: ContextMenuAction,
    label: impl Into<Cow<'static, str>>,
    shortcut: Option<&'static str>,
    enabled: bool,
    destructive: bool,
) -> ContextMenuEntry {
    ContextMenuEntry::Item {
        action,
        label: label.into(),
        shortcut,
        enabled,
        destructive,
        checked: false,
    }
}

fn checked_menu_item(
    action: ContextMenuAction,
    label: impl Into<Cow<'static, str>>,
    shortcut: Option<&'static str>,
    checked: bool,
) -> ContextMenuEntry {
    ContextMenuEntry::Item {
        action,
        label: label.into(),
        shortcut,
        enabled: true,
        destructive: false,
        checked,
    }
}

fn submenu(label: impl Into<Cow<'static, str>>, items: Vec<ContextMenuEntry>) -> ContextMenuEntry {
    ContextMenuEntry::Submenu {
        label: label.into(),
        items,
    }
}

fn direction_labels_for_column(column: SortColumn) -> (&'static str, &'static str) {
    match column {
        SortColumn::Name | SortColumn::Type => ("A to Z", "Z to A"),
        SortColumn::Size => ("Smallest first", "Largest first"),
        SortColumn::Date | SortColumn::Created | SortColumn::Accessed => {
            ("Oldest first", "Newest first")
        }
    }
}

const UNDO_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Cmd+Z"
} else {
    "Ctrl+Z"
};

fn build_multi_selection_menu_entries(trash_available: bool) -> Vec<ContextMenuEntry> {
    let remove_label = if trash_available {
        "Move to Trash"
    } else {
        "Delete Permanently"
    };
    vec![
        menu_item(ContextMenuAction::Copy, "Copy", Some("Ctrl+C"), true, false),
        menu_item(ContextMenuAction::Cut, "Cut", Some("Ctrl+X"), true, false),
        ContextMenuEntry::Separator,
        menu_item(
            ContextMenuAction::Remove,
            remove_label,
            Some("Delete"),
            true,
            true,
        ),
    ]
}

/// Builds the context menu entries applicable to the current selection, clipboard, and undo stack.
/// `folder_bookmarked` is `Some` when the selection is a single local folder, telling whether
/// the sidebar already lists it. `open_with_apps` is `Some` when the selection is a single local
/// file, listing the applications to display in the "Open With" submenu.
pub fn build_context_menu_entries(
    selected: &[Location],
    has_clipboard: bool,
    folder_bookmarked: Option<bool>,
    open_with_apps: Option<&[AppInfo]>,
    folder_state: FolderMenuState,
    undo_label: Option<&'static str>,
) -> Vec<ContextMenuEntry> {
    if selected.is_empty() {
        let (asc_label, desc_label) = direction_labels_for_column(folder_state.sort_column);
        return vec![
            menu_item(
                ContextMenuAction::Paste,
                "Paste",
                Some("Ctrl+V"),
                has_clipboard,
                false,
            ),
            menu_item(
                ContextMenuAction::NewFolder,
                "New Folder",
                Some("Ctrl+Shift+N"),
                true,
                false,
            ),
            menu_item(
                ContextMenuAction::Undo,
                undo_label.unwrap_or("Undo"),
                Some(UNDO_SHORTCUT),
                undo_label.is_some(),
                false,
            ),
            ContextMenuEntry::Separator,
            submenu(
                "View",
                vec![
                    checked_menu_item(
                        ContextMenuAction::SetViewMode(ViewMode::Details),
                        "Details",
                        Some("Ctrl+1"),
                        folder_state.view_mode == ViewMode::Details,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetViewMode(ViewMode::Grid),
                        "Grid",
                        Some("Ctrl+2"),
                        folder_state.view_mode == ViewMode::Grid,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetViewMode(ViewMode::CompactList),
                        "Compact List",
                        Some("Ctrl+3"),
                        folder_state.view_mode == ViewMode::CompactList,
                    ),
                    ContextMenuEntry::Separator,
                    checked_menu_item(
                        ContextMenuAction::ToggleSidebar,
                        "Show Sidebar",
                        Some("F9"),
                        folder_state.show_sidebar,
                    ),
                    checked_menu_item(
                        ContextMenuAction::TogglePreview,
                        "Show Preview",
                        Some("Alt+P"),
                        folder_state.show_preview,
                    ),
                ],
            ),
            submenu(
                "Sort by",
                vec![
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Name),
                        "Name",
                        None,
                        folder_state.sort_column == SortColumn::Name,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Size),
                        "Size",
                        None,
                        folder_state.sort_column == SortColumn::Size,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Date),
                        "Modified",
                        None,
                        folder_state.sort_column == SortColumn::Date,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Created),
                        "Created",
                        None,
                        folder_state.sort_column == SortColumn::Created,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Accessed),
                        "Accessed",
                        None,
                        folder_state.sort_column == SortColumn::Accessed,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortColumn(SortColumn::Type),
                        "Type",
                        None,
                        folder_state.sort_column == SortColumn::Type,
                    ),
                    ContextMenuEntry::Separator,
                    checked_menu_item(
                        ContextMenuAction::SetSortDirection(SortDirection::Ascending),
                        asc_label,
                        None,
                        folder_state.sort_direction == SortDirection::Ascending,
                    ),
                    checked_menu_item(
                        ContextMenuAction::SetSortDirection(SortDirection::Descending),
                        desc_label,
                        None,
                        folder_state.sort_direction == SortDirection::Descending,
                    ),
                    ContextMenuEntry::Separator,
                    checked_menu_item(
                        ContextMenuAction::ToggleFoldersFirst,
                        "Folders First",
                        None,
                        folder_state.folders_first,
                    ),
                    checked_menu_item(
                        ContextMenuAction::ToggleHiddenLast,
                        "Hidden Files Last",
                        None,
                        folder_state.hidden_last,
                    ),
                ],
            ),
            checked_menu_item(
                ContextMenuAction::ToggleShowHidden,
                "Show Hidden Files",
                Some("Ctrl+H"),
                folder_state.show_hidden,
            ),
            menu_item(
                ContextMenuAction::ApplyFolderSettingsToAll,
                "Apply to All Folders",
                None,
                true,
                false,
            ),
            ContextMenuEntry::Separator,
            menu_item(
                ContextMenuAction::Refresh,
                "Refresh",
                Some("F5"),
                true,
                false,
            ),
        ];
    }

    let local_paths: Vec<&Path> = selected
        .iter()
        .filter_map(|loc| match loc {
            Location::Local(path) => Some(path.as_path()),
            Location::Remote { .. } => None,
        })
        .collect();
    let trash_available = local_paths.len() == selected.len() && all_local_volumes(&local_paths);
    let remove_label = if trash_available {
        "Move to Trash"
    } else {
        "Delete Permanently"
    };

    if selected.len() == 1 {
        let mut entries = vec![
            menu_item(ContextMenuAction::Open, "Open", Some("Enter"), true, false),
            menu_item(ContextMenuAction::Rename, "Rename", Some("F2"), true, false),
        ];
        if let Some(bookmarked) = folder_bookmarked {
            entries.push(menu_item(
                ContextMenuAction::Bookmark,
                "Add to Places",
                None,
                !bookmarked,
                false,
            ));
        }
        if let Some(apps) = open_with_apps {
            entries.push(submenu("Open With", build_open_with_submenu_items(apps)));
        }
        entries.extend([
            ContextMenuEntry::Separator,
            menu_item(ContextMenuAction::Copy, "Copy", Some("Ctrl+C"), true, false),
            menu_item(ContextMenuAction::Cut, "Cut", Some("Ctrl+X"), true, false),
            menu_item(
                ContextMenuAction::Paste,
                "Paste",
                Some("Ctrl+V"),
                has_clipboard,
                false,
            ),
            ContextMenuEntry::Separator,
            menu_item(
                ContextMenuAction::Remove,
                remove_label,
                Some("Delete"),
                true,
                true,
            ),
        ]);
        entries
    } else {
        build_multi_selection_menu_entries(trash_available)
    }
}

fn build_open_with_submenu_items(apps: &[AppInfo]) -> Vec<ContextMenuEntry> {
    let mut items = Vec::with_capacity(apps.len() + 2);
    if !apps.is_empty() {
        for (ix, app) in apps.iter().enumerate() {
            let label: Cow<'static, str> = if app.is_default {
                Cow::Owned(format!("{} (Default)", app.name))
            } else {
                Cow::Owned(app.name.clone())
            };
            items.push(menu_item(
                ContextMenuAction::OpenWithApp(ix),
                label,
                None,
                true,
                false,
            ));
        }
        items.push(ContextMenuEntry::Separator);
    }
    items.push(menu_item(
        ContextMenuAction::OpenWithOther,
        "Other application...",
        None,
        true,
        false,
    ));
    items
}

/// Builds the context menu entries for a sidebar Places item at `index`.
/// Trash can be opened, while folders can be opened, renamed, or removed from Places.
pub fn build_sidebar_place_menu_entries(index: usize, is_trash: bool) -> Vec<ContextMenuEntry> {
    if is_trash {
        return vec![menu_item(
            ContextMenuAction::SidebarOpen(index),
            "Open",
            None,
            true,
            false,
        )];
    }
    vec![
        menu_item(
            ContextMenuAction::SidebarOpen(index),
            "Open",
            None,
            true,
            false,
        ),
        menu_item(
            ContextMenuAction::SidebarRename(index),
            "Rename",
            None,
            true,
            false,
        ),
        ContextMenuEntry::Separator,
        menu_item(
            ContextMenuAction::SidebarRemove(index),
            "Remove from Places",
            None,
            true,
            false,
        ),
    ]
}

const CARD_W: f32 = 224.0;
const SUBMENU_W: f32 = 196.0;

fn is_entry_navigable(entry: &ContextMenuEntry) -> bool {
    match entry {
        ContextMenuEntry::Item { enabled, .. } => *enabled,
        ContextMenuEntry::Submenu { items, .. } => items.iter().any(is_entry_navigable),
        ContextMenuEntry::Separator => false,
    }
}

fn first_enabled_index_in(entries: &[ContextMenuEntry]) -> Option<usize> {
    entries.iter().position(is_entry_navigable)
}

fn next_enabled_index_in(
    entries: &[ContextMenuEntry],
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let len = entries.len();
    if len == 0 {
        return None;
    }
    let start = current.unwrap_or(0);
    for step in 1..=len {
        let candidate = if forward {
            (start + step) % len
        } else {
            (start + len - (step % len)) % len
        };
        if entries.get(candidate).is_some_and(is_entry_navigable) {
            return Some(candidate);
        }
    }
    None
}

fn estimated_menu_height(entries: &[ContextMenuEntry]) -> Pixels {
    let pad = space::S4 * 2.0 + px(2.);
    let gaps = if entries.len() > 1 {
        space::S2 * ((entries.len() - 1) as f32)
    } else {
        px(0.)
    };
    let rows: Pixels = entries.iter().fold(px(0.), |acc, entry| {
        acc + match entry {
            ContextMenuEntry::Separator => px(1.) + space::S2 * 2.0,
            ContextMenuEntry::Item { .. } | ContextMenuEntry::Submenu { .. } => size::ROW,
        }
    });
    pad + rows + gaps
}

fn row_y_offset(entries: &[ContextMenuEntry], target_ix: usize) -> Pixels {
    let mut y = space::S4 + px(1.);
    for entry in entries.iter().take(target_ix) {
        y += match entry {
            ContextMenuEntry::Separator => px(1.) + space::S2 * 2.0,
            ContextMenuEntry::Item { .. } | ContextMenuEntry::Submenu { .. } => size::ROW,
        };
        y += space::S2;
    }
    y
}

fn submenu_anchor_position(
    menu_pos: Point<Pixels>,
    main_entries: &[ContextMenuEntry],
    parent_ix: usize,
    submenu_entries: &[ContextMenuEntry],
    viewport: Size<Pixels>,
) -> Point<Pixels> {
    let margin = px(8.);
    let main_h = estimated_menu_height(main_entries);
    let sub_h = estimated_menu_height(submenu_entries);
    let main_x = menu_pos
        .x
        .min((viewport.width - px(CARD_W) - margin).max(margin))
        .max(margin);
    let main_y = menu_pos
        .y
        .min((viewport.height - main_h - margin).max(margin))
        .max(margin);

    let sub_x = if main_x + px(CARD_W) + px(SUBMENU_W) + margin <= viewport.width {
        main_x + px(CARD_W) - px(2.)
    } else {
        (main_x - px(SUBMENU_W) + px(2.)).max(margin)
    };
    let raw_sub_y = main_y + row_y_offset(main_entries, parent_ix) - space::S4;
    let sub_y = raw_sub_y
        .min((viewport.height - sub_h - margin).max(margin))
        .max(margin);
    point(sub_x, sub_y)
}

/// Floating context menu anchored at the pointer or focused list row.
pub struct ContextMenuView {
    app_state: Entity<AppState>,
    focus_handle: FocusHandle,
    is_open: bool,
    position: Point<Pixels>,
    entries: Vec<ContextMenuEntry>,
    open_with_target: Option<PathBuf>,
    open_with_apps: Vec<AppInfo>,
    focused_index: Option<usize>,
    hovered_index: Option<usize>,
    open_submenu: Option<usize>,
    submenu_focused_index: Option<usize>,
    submenu_hovered_index: Option<usize>,
    opened_at: Option<Instant>,
    motion: MotionTracker<usize>,
}

impl EventEmitter<ContextMenuEvent> for ContextMenuView {}

impl Focusable for ContextMenuView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ContextMenuView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            focus_handle: cx.focus_handle(),
            is_open: false,
            position: point(px(0.), px(0.)),
            entries: Vec::new(),
            open_with_target: None,
            open_with_apps: Vec::new(),
            focused_index: None,
            hovered_index: None,
            open_submenu: None,
            submenu_focused_index: None,
            submenu_hovered_index: None,
            opened_at: None,
            motion: MotionTracker::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn entries(&self) -> &[ContextMenuEntry] {
        &self.entries
    }

    pub fn open_with_target(&self) -> Option<&PathBuf> {
        self.open_with_target.as_ref()
    }

    pub fn open_with_apps(&self) -> &[AppInfo] {
        &self.open_with_apps
    }

    pub fn position(&self) -> Point<Pixels> {
        self.position
    }

    pub fn open_submenu(&self) -> Option<usize> {
        self.open_submenu
    }

    pub fn open_submenu_entries(&self) -> Option<&[ContextMenuEntry]> {
        let ix = self.open_submenu?;
        match self.entries.get(ix) {
            Some(ContextMenuEntry::Submenu { items, .. }) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Opens the menu at `position` with items matching the current selection and clipboard.
    pub fn open_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        enum MenuSelectionKind {
            Empty,
            Single {
                location: Location,
                folder_bookmarked: Option<bool>,
                open_with_target: Option<PathBuf>,
            },
            Multi {
                trash_available: bool,
            },
        }

        let (selection_kind, folder_state, undo_label) = {
            let state = self.app_state.read(cx);
            let selection_kind = match state.selection.len() {
                0 => MenuSelectionKind::Empty,
                1 => match state.selected_entry() {
                    Some(entry) => {
                        let location = state.location_of(entry);
                        let (folder_bookmarked, open_with_target) = match &location {
                            Location::Local(path) => {
                                if entry.is_dir() || path.is_dir() {
                                    (Some(state.is_place_pinned(path)), None)
                                } else {
                                    (None, Some(path.clone()))
                                }
                            }
                            Location::Remote { .. } => (None, None),
                        };
                        MenuSelectionKind::Single {
                            location,
                            folder_bookmarked,
                            open_with_target,
                        }
                    }
                    None => MenuSelectionKind::Empty,
                },
                _ => {
                    let trash_available = match &state.current_location {
                        Location::Local(dir) => volume_kind(dir) == VolumeKind::Local,
                        Location::Remote { .. } => false,
                    };
                    MenuSelectionKind::Multi { trash_available }
                }
            };
            let folder_state = FolderMenuState {
                view_mode: state.view_mode,
                sort_column: state.sort_column,
                sort_direction: state.sort_direction,
                show_hidden: state.show_hidden,
                folders_first: state.folders_first,
                hidden_last: state.hidden_last,
                show_sidebar: state.panels.sidebar_open,
                show_preview: state.panels.preview_open,
            };
            (selection_kind, folder_state, state.undo_stack.undo_label())
        };
        let has_clipboard = has_location_clipboard();
        match selection_kind {
            MenuSelectionKind::Empty => {
                self.open_with_target = None;
                self.open_with_apps.clear();
                self.entries = build_context_menu_entries(
                    &[],
                    has_clipboard,
                    None,
                    None,
                    folder_state,
                    undo_label,
                );
            }
            MenuSelectionKind::Single {
                location,
                folder_bookmarked,
                open_with_target,
            } => {
                let open_with_apps = open_with_target
                    .as_deref()
                    .map(menu_applications_for)
                    .unwrap_or_default();
                self.open_with_target = open_with_target;
                self.open_with_apps = open_with_apps;
                self.entries = build_context_menu_entries(
                    std::slice::from_ref(&location),
                    has_clipboard,
                    folder_bookmarked,
                    self.open_with_target
                        .as_ref()
                        .map(|_| self.open_with_apps.as_slice()),
                    folder_state,
                    undo_label,
                );
            }
            MenuSelectionKind::Multi { trash_available } => {
                self.open_with_target = None;
                self.open_with_apps.clear();
                self.entries = build_multi_selection_menu_entries(trash_available);
            }
        }
        self.position = position;
        self.focused_index = first_enabled_index_in(&self.entries);
        self.hovered_index = None;
        self.open_submenu = None;
        self.submenu_focused_index = None;
        self.submenu_hovered_index = None;
        self.opened_at = Some(Instant::now());
        self.motion.clear();
        self.is_open = true;
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Opens the menu at `position` for a sidebar Places item.
    pub fn open_for_sidebar_place_at(
        &mut self,
        index: usize,
        is_trash: bool,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_with_target = None;
        self.open_with_apps.clear();
        self.entries = build_sidebar_place_menu_entries(index, is_trash);
        self.position = position;
        self.focused_index = first_enabled_index_in(&self.entries);
        self.hovered_index = None;
        self.open_submenu = None;
        self.submenu_focused_index = None;
        self.submenu_hovered_index = None;
        self.opened_at = Some(Instant::now());
        self.motion.clear();
        self.is_open = true;
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Closes the menu and notifies listeners to restore focus.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        self.is_open = false;
        self.entries.clear();
        self.open_with_target = None;
        self.open_with_apps.clear();
        self.focused_index = None;
        self.hovered_index = None;
        self.open_submenu = None;
        self.submenu_focused_index = None;
        self.submenu_hovered_index = None;
        self.opened_at = None;
        self.motion.clear();
        cx.emit(ContextMenuEvent::Closed);
        cx.notify();
    }

    fn trigger(&mut self, action: ContextMenuAction, cx: &mut Context<Self>) {
        self.is_open = false;
        self.entries.clear();
        self.focused_index = None;
        self.hovered_index = None;
        self.open_submenu = None;
        self.submenu_focused_index = None;
        self.submenu_hovered_index = None;
        cx.emit(ContextMenuEvent::Execute(action));
        cx.notify();
    }

    fn open_submenu_at(&mut self, ix: usize, focus_first: bool, cx: &mut Context<Self>) {
        let Some(ContextMenuEntry::Submenu { items, .. }) = self.entries.get(ix) else {
            return;
        };
        self.open_submenu = Some(ix);
        self.submenu_hovered_index = None;
        self.submenu_focused_index = if focus_first {
            first_enabled_index_in(items)
        } else {
            None
        };
        cx.notify();
    }

    fn move_focus(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let (Some(parent_ix), Some(_)) = (self.open_submenu, self.submenu_focused_index) {
            if let Some(ContextMenuEntry::Submenu { items, .. }) = self.entries.get(parent_ix) {
                if let Some(next) =
                    next_enabled_index_in(items, self.submenu_focused_index, forward)
                {
                    self.submenu_focused_index = Some(next);
                    cx.notify();
                }
            }
            return;
        }

        if let Some(next) = next_enabled_index_in(&self.entries, self.focused_index, forward) {
            self.focused_index = Some(next);
            if self.open_submenu != Some(next) {
                self.open_submenu = None;
                self.submenu_focused_index = None;
                self.submenu_hovered_index = None;
            }
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.close(cx);
            }
            "down" => {
                cx.stop_propagation();
                self.move_focus(true, cx);
            }
            "up" => {
                cx.stop_propagation();
                self.move_focus(false, cx);
            }
            "right" => {
                if self.submenu_focused_index.is_none() {
                    if let Some(ix) = self.focused_index {
                        if matches!(self.entries.get(ix), Some(ContextMenuEntry::Submenu { .. })) {
                            cx.stop_propagation();
                            self.open_submenu_at(ix, true, cx);
                        }
                    }
                }
            }
            "left" => {
                if self.open_submenu.is_some() && self.submenu_focused_index.is_some() {
                    cx.stop_propagation();
                    self.open_submenu = None;
                    self.submenu_focused_index = None;
                    self.submenu_hovered_index = None;
                    cx.notify();
                }
            }
            "enter" | "space" => {
                cx.stop_propagation();
                if let (Some(parent_ix), Some(sub_ix)) =
                    (self.open_submenu, self.submenu_focused_index)
                {
                    if let Some(ContextMenuEntry::Submenu { items, .. }) =
                        self.entries.get(parent_ix)
                    {
                        if let Some(ContextMenuEntry::Item {
                            action,
                            enabled: true,
                            ..
                        }) = items.get(sub_ix)
                        {
                            self.trigger(*action, cx);
                            return;
                        }
                    }
                }
                if let Some(ix) = self.focused_index {
                    match self.entries.get(ix) {
                        Some(ContextMenuEntry::Item {
                            action,
                            enabled: true,
                            ..
                        }) => {
                            let action = *action;
                            self.trigger(action, cx);
                        }
                        Some(ContextMenuEntry::Submenu { .. }) => {
                            self.open_submenu_at(ix, true, cx);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn menu_action_icon(action: ContextMenuAction) -> Option<IconName> {
    match action {
        ContextMenuAction::Open => Some(IconName::Folder),
        ContextMenuAction::Copy => Some(IconName::Copy),
        ContextMenuAction::Remove => Some(IconName::Trash),
        ContextMenuAction::NewFolder => Some(IconName::Folder),
        ContextMenuAction::Refresh => Some(IconName::Refresh),
        ContextMenuAction::SidebarRemove(_) => Some(IconName::Close),
        ContextMenuAction::OpenWithApp(_)
        | ContextMenuAction::OpenWithOther
        | ContextMenuAction::Rename
        | ContextMenuAction::Bookmark
        | ContextMenuAction::Cut
        | ContextMenuAction::Paste
        | ContextMenuAction::Undo
        | ContextMenuAction::SidebarOpen(_)
        | ContextMenuAction::SidebarRename(_)
        | ContextMenuAction::SetViewMode(_)
        | ContextMenuAction::SetSortColumn(_)
        | ContextMenuAction::SetSortDirection(_)
        | ContextMenuAction::ToggleShowHidden
        | ContextMenuAction::ToggleFoldersFirst
        | ContextMenuAction::ToggleHiddenLast
        | ContextMenuAction::ToggleSidebar
        | ContextMenuAction::TogglePreview
        | ContextMenuAction::ApplyFolderSettingsToAll => None,
    }
}

fn render_leading_icon(
    action: ContextMenuAction,
    checked: bool,
    reserve_check_slot: bool,
    icon_t: f32,
    label_color: Hsla,
) -> Option<AnyElement> {
    if checked {
        return Some(
            animated_icon(IconName::Check, IconMotion::Rest, label_color).into_any_element(),
        );
    }
    if let Some(icon_name) = menu_action_icon(action) {
        let motion = if matches!(icon_name, IconName::Refresh) {
            if icon_t > 0.01 && icon_t < 0.99 {
                IconMotion::Pulse(icon_t)
            } else {
                IconMotion::Rest
            }
        } else {
            IconMotion::State(icon_t)
        };
        return Some(animated_icon(icon_name, motion, label_color).into_any_element());
    }
    if reserve_check_slot {
        return Some(
            div()
                .w(size::ICON)
                .h(size::ICON)
                .flex_none()
                .into_any_element(),
        );
    }
    None
}

impl Render for ContextMenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            return div().into_any_element();
        }

        let now = Instant::now();
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);
        let reduced = is_reduced_motion(self.app_state.read(cx).settings.motion_preference);
        let focused = self.focused_index;

        let open_elapsed = self
            .opened_at
            .map_or(QUICK, |t| now.saturating_duration_since(t));
        let open_dur = if reduced { INSTANT } else { QUICK };
        let open_t = (open_elapsed.as_secs_f32() / open_dur.as_secs_f32()).clamp(0.0, 1.0);
        let (card_scale, card_opacity) = popup_entrance_metrics(open_t, reduced);

        let visible_w = px(CARD_W * card_scale);

        let mut card = div()
            .id("context-menu-card")
            .w(px(CARD_W))
            .py(space::S4)
            .px(space::S4)
            .rounded(radius::CONTROL)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .flex()
            .flex_col()
            .gap(space::S2)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

        let mut cascade_animating = open_t < 1.0;
        let mut item_slot: usize = 0;

        for (ix, entry) in self.entries.iter().enumerate() {
            match entry {
                ContextMenuEntry::Separator => {
                    card = card.child(div().h(px(1.)).my(space::S2).bg(theme.border));
                }
                ContextMenuEntry::Submenu { label, items } => {
                    let has_enabled = items.iter().any(is_entry_navigable);
                    let is_open_sub = self.open_submenu == Some(ix);
                    let is_focused = focused == Some(ix);
                    let is_hovered = self.hovered_index == Some(ix);
                    let active_t = self.motion.step_scalar(
                        ix,
                        0.0,
                        if has_enabled && (is_focused || is_hovered || is_open_sub) {
                            1.0
                        } else {
                            0.0
                        },
                        INSTANT,
                        Curve::EaseOut,
                        MotionKind::OpacityOrColor,
                        now,
                        reduced,
                    );

                    let (item_opacity, item_dx) = if reduced {
                        (card_opacity, 0.0)
                    } else {
                        let delay = MENU_CASCADE_STEP.saturating_mul(item_slot as u32);
                        let since_delay = open_elapsed.saturating_sub(delay);
                        let p = (since_delay.as_secs_f32() / QUICK.as_secs_f32()).clamp(0.0, 1.0);
                        if p < 1.0 {
                            cascade_animating = true;
                        }
                        let spring = Curve::Spring.sample(p);
                        (Curve::EaseOut.sample(p), -6.0 * (1.0 - spring))
                    };
                    item_slot += 1;

                    let label_color = if has_enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    };
                    let row_bg = color_lerp(theme.surface_raised, theme.selection, active_t);

                    let row = div()
                        .id(("context-menu-item", ix))
                        .debug_selector(move || format!("context-menu-item-{ix}"))
                        .relative()
                        .left(px(item_dx))
                        .opacity(item_opacity)
                        .h(size::ROW)
                        .px(space::S8)
                        .rounded(radius::CONTROL)
                        .bg(row_bg)
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_body()
                        .text_color(label_color)
                        .when(has_enabled, |el| {
                            el.cursor(CursorStyle::PointingHand)
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.hovered_index = Some(ix);
                                        this.open_submenu_at(ix, false, cx);
                                    } else if this.hovered_index == Some(ix) {
                                        this.hovered_index = None;
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.focused_index = Some(ix);
                                        this.open_submenu_at(ix, true, cx);
                                    }),
                                )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .child(SharedString::from(label.clone())),
                        )
                        .child(animated_icon(
                            IconName::ChevronRight,
                            IconMotion::Rest,
                            theme.text_muted,
                        ));

                    card = card.child(row);
                }
                ContextMenuEntry::Item {
                    action,
                    label,
                    shortcut,
                    enabled,
                    destructive,
                    checked,
                } => {
                    let action = *action;
                    let enabled = *enabled;
                    let destructive = *destructive;
                    let checked = *checked;
                    let is_focused = focused == Some(ix);
                    let is_hovered = self.hovered_index == Some(ix);
                    let active_t = self.motion.step_scalar(
                        ix,
                        0.0,
                        if enabled && (is_focused || is_hovered) {
                            1.0
                        } else {
                            0.0
                        },
                        INSTANT,
                        Curve::EaseOut,
                        MotionKind::OpacityOrColor,
                        now,
                        reduced,
                    );
                    let icon_t = self.motion.step_scalar(
                        10_000 + ix,
                        0.0,
                        if enabled && is_hovered { 1.0 } else { 0.0 },
                        QUICK,
                        Curve::Spring,
                        MotionKind::MovementOrScale,
                        now,
                        reduced,
                    );

                    let (item_opacity, item_dx) = if reduced {
                        (card_opacity, 0.0)
                    } else {
                        let delay = MENU_CASCADE_STEP.saturating_mul(item_slot as u32);
                        let since_delay = open_elapsed.saturating_sub(delay);
                        let p = (since_delay.as_secs_f32() / QUICK.as_secs_f32()).clamp(0.0, 1.0);
                        if p < 1.0 {
                            cascade_animating = true;
                        }
                        let spring = Curve::Spring.sample(p);
                        (Curve::EaseOut.sample(p), -6.0 * (1.0 - spring))
                    };
                    item_slot += 1;

                    let label_color = if !enabled {
                        theme.text_muted
                    } else if destructive {
                        theme.danger
                    } else {
                        theme.text
                    };
                    let row_bg = color_lerp(theme.surface_raised, theme.selection, active_t);
                    let icon_el =
                        render_leading_icon(action, checked, false, icon_t, label_color.into());

                    let row = div()
                        .id(("context-menu-item", ix))
                        .debug_selector(move || format!("context-menu-item-{ix}"))
                        .relative()
                        .left(px(item_dx))
                        .opacity(item_opacity)
                        .h(size::ROW)
                        .px(space::S8)
                        .rounded(radius::CONTROL)
                        .bg(row_bg)
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_body()
                        .text_color(label_color)
                        .when(enabled, |el| {
                            el.cursor(CursorStyle::PointingHand)
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    let changed = if *hovered {
                                        let prev_hover = this.hovered_index;
                                        let had_sub = this.open_submenu.is_some();
                                        this.hovered_index = Some(ix);
                                        if had_sub {
                                            this.open_submenu = None;
                                            this.submenu_focused_index = None;
                                            this.submenu_hovered_index = None;
                                        }
                                        prev_hover != Some(ix) || had_sub
                                    } else if this.hovered_index == Some(ix) {
                                        this.hovered_index = None;
                                        true
                                    } else {
                                        false
                                    };
                                    if changed {
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.trigger(action, cx);
                                    }),
                                )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(space::S8)
                                .children(icon_el)
                                .child(SharedString::from(label.clone())),
                        )
                        .when_some(*shortcut, |el, key| {
                            el.child(
                                div()
                                    .text_data(&fonts)
                                    .text_color(theme.text_muted)
                                    .child(key),
                            )
                        });

                    card = card.child(row);
                }
            }
        }

        let submenu_element = if let Some(parent_ix) = self.open_submenu {
            if let Some(ContextMenuEntry::Submenu { items, .. }) = self.entries.get(parent_ix) {
                let submenu_pos = submenu_anchor_position(
                    self.position,
                    &self.entries,
                    parent_ix,
                    items,
                    window.viewport_size(),
                );
                let mut sub_card = div()
                    .id("context-submenu-card")
                    .w(px(SUBMENU_W))
                    .py(space::S4)
                    .px(space::S4)
                    .rounded(radius::CONTROL)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_strong)
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());

                for (sub_ix, sub_entry) in items.iter().enumerate() {
                    match sub_entry {
                        ContextMenuEntry::Separator => {
                            sub_card =
                                sub_card.child(div().h(px(1.)).my(space::S2).bg(theme.border));
                        }
                        ContextMenuEntry::Item {
                            action,
                            label,
                            shortcut,
                            enabled,
                            destructive,
                            checked,
                        } => {
                            let action = *action;
                            let enabled = *enabled;
                            let destructive = *destructive;
                            let checked = *checked;
                            let is_focused = self.submenu_focused_index == Some(sub_ix);
                            let is_hovered = self.submenu_hovered_index == Some(sub_ix);
                            let active_t = self.motion.step_scalar(
                                20_000 + sub_ix,
                                0.0,
                                if enabled && (is_focused || is_hovered) {
                                    1.0
                                } else {
                                    0.0
                                },
                                INSTANT,
                                Curve::EaseOut,
                                MotionKind::OpacityOrColor,
                                now,
                                reduced,
                            );
                            let label_color = if !enabled {
                                theme.text_muted
                            } else if destructive {
                                theme.danger
                            } else {
                                theme.text
                            };
                            let row_bg =
                                color_lerp(theme.surface_raised, theme.selection, active_t);
                            let icon_el =
                                render_leading_icon(action, checked, true, 0.0, label_color.into());

                            let row = div()
                                .id(("context-menu-subitem", sub_ix))
                                .debug_selector(move || format!("context-menu-subitem-{sub_ix}"))
                                .h(size::ROW)
                                .px(space::S8)
                                .rounded(radius::CONTROL)
                                .bg(row_bg)
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .text_body()
                                .text_color(label_color)
                                .when(enabled, |el| {
                                    el.cursor(CursorStyle::PointingHand)
                                        .on_hover(cx.listener(
                                            move |this, hovered: &bool, _, cx| {
                                                let changed = if *hovered {
                                                    let prev = this.submenu_hovered_index;
                                                    this.submenu_hovered_index = Some(sub_ix);
                                                    prev != Some(sub_ix)
                                                } else if this.submenu_hovered_index == Some(sub_ix)
                                                {
                                                    this.submenu_hovered_index = None;
                                                    true
                                                } else {
                                                    false
                                                };
                                                if changed {
                                                    cx.notify();
                                                }
                                            },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation();
                                                this.trigger(action, cx);
                                            }),
                                        )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .gap(space::S8)
                                        .children(icon_el)
                                        .child(
                                            div()
                                                .flex_1()
                                                .ellipsize()
                                                .child(SharedString::from(label.clone())),
                                        ),
                                )
                                .when_some(*shortcut, |el, key| {
                                    el.child(
                                        div()
                                            .text_data(&fonts)
                                            .text_color(theme.text_muted)
                                            .child(key),
                                    )
                                });

                            sub_card = sub_card.child(row);
                        }
                        ContextMenuEntry::Submenu { .. } => {}
                    }
                }

                Some(
                    anchored()
                        .position(submenu_pos)
                        .snap_to_window_with_margin(px(8.))
                        .child(sub_card),
                )
            } else {
                None
            }
        } else {
            None
        };

        self.motion.prune_settled(now, reduced);
        request_frame_if_active(
            cascade_animating || self.motion.has_active(now, reduced),
            window,
        );

        let animated_card = div()
            .w(visible_w)
            .overflow_hidden()
            .opacity(card_opacity)
            .child(card);

        deferred(
            div()
                .id("context-menu-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::on_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                    }),
                )
                .child(
                    anchored()
                        .position(self.position)
                        .snap_to_window_with_margin(px(8.))
                        .child(animated_card),
                )
                .children(submenu_element),
        )
        .with_priority(1)
        .into_any_element()
    }
}
