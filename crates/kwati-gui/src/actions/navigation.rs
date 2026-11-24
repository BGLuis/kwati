//! Navigation actions (owned by guinav-worker for #9, #13, #25).

use std::io;
use std::path::Path;
use std::time::Duration;

use gpui::actions;
use kwati_core::DirWatcher;

actions!(
    navigation,
    [
        GoUp,
        GoBack,
        GoForward,
        OpenSelected,
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        SelectAll,
        ClearSelection,
        PageUp,
        PageDown,
        Refresh,
        ToggleHidden,
        TogglePreview,
        ToggleSidebar,
        OpenContextMenu,
        SetViewDetails,
        SetViewGrid,
        SetViewCompact,
    ]
);

/// Opens a file with the system default application.
///
/// On Linux/macOS/Windows, uses the standard system handler to open the given path
/// with its associated default program.
pub fn open_with_system(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(path);
        crate::actions::spawn_detached(&mut cmd)
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &path.to_string_lossy()]);
        crate::actions::spawn_detached(&mut cmd)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(path);
        crate::actions::spawn_detached(&mut cmd)
    }
}

/// Opens the operating system's native trash / recycle bin window.
pub fn open_system_trash() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("osascript");
        cmd.args([
            "-e",
            "tell application \"Finder\" to open trash",
            "-e",
            "tell application \"Finder\" to activate",
        ]);
        crate::actions::spawn_detached(&mut cmd)
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.arg("shell:RecycleBinFolder");
        crate::actions::spawn_detached(&mut cmd)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg("trash:///");
        crate::actions::spawn_detached(&mut cmd)
    }
}

/// Watches a directory for changes, coalescing events within the given debounce window.
///
/// Dropping the returned `DirWatcher` stops background watching immediately, ensuring
/// navigating away prevents stale reload callbacks.
pub fn watch_directory(
    dir: &Path,
    window: Duration,
    on_change: impl FnMut() + Send + 'static,
) -> io::Result<DirWatcher> {
    DirWatcher::new(dir, window, on_change).map_err(|e| io::Error::other(e.to_string()))
}
