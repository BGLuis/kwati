pub mod navigation;
pub mod open_with;
pub mod operations;
pub mod remote_sync;

pub use navigation::*;
pub use open_with::*;
pub use operations::*;
pub use remote_sync::*;

use gpui::{actions, App, KeyBinding};

actions!(
    kwati,
    [
        OpenSettings,
        ReconnectRemote,
        DisconnectRemote,
        RefreshListing,
        ManageServers,
    ]
);

/// Registers key bindings for global explorer navigation and operations.
pub fn register_keybindings(cx: &mut App) {
    cx.bind_keys([
        // Navigation keybindings
        KeyBinding::new("backspace", navigation::GoUp, None),
        KeyBinding::new("alt-left", navigation::GoBack, None),
        KeyBinding::new("alt-right", navigation::GoForward, None),
        KeyBinding::new("enter", navigation::OpenSelected, None),
        KeyBinding::new("down", navigation::SelectNext, None),
        KeyBinding::new("up", navigation::SelectPrevious, None),
        KeyBinding::new("ctrl-a", navigation::SelectAll, Some("ListView")),
        KeyBinding::new("cmd-a", navigation::SelectAll, Some("ListView")),
        KeyBinding::new("escape", navigation::ClearSelection, Some("ListView")),
        KeyBinding::new("shift-f10", navigation::OpenContextMenu, Some("ListView")),
        KeyBinding::new("menu", navigation::OpenContextMenu, Some("ListView")),
        KeyBinding::new("f5", navigation::Refresh, None),
        KeyBinding::new("alt-p", navigation::TogglePreview, None),
        // File operations
        KeyBinding::new("ctrl-c", operations::Copy, None),
        KeyBinding::new("cmd-c", operations::Copy, None),
        KeyBinding::new("ctrl-x", operations::Cut, None),
        KeyBinding::new("cmd-x", operations::Cut, None),
        KeyBinding::new("ctrl-v", operations::Paste, None),
        KeyBinding::new("cmd-v", operations::Paste, None),
        KeyBinding::new("ctrl-z", operations::Undo, None),
        KeyBinding::new("cmd-z", operations::Undo, None),
        KeyBinding::new("delete", operations::Trash, None),
        KeyBinding::new("shift-delete", operations::Delete, None),
        KeyBinding::new("f2", operations::Rename, None),
        KeyBinding::new("ctrl-shift-n", operations::NewFolder, None),
        KeyBinding::new("cmd-shift-n", operations::NewFolder, None),
        // Settings & remote
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
    ]);
    crate::views::search_view::register_search_keybindings(cx);
}
