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
        PageUp,
        PageDown,
        Refresh,
        ToggleHidden,
    ]
);

/// Opens a file with the system default application.
///
/// On Linux/macOS/Windows, uses the standard system handler to open the given path
/// with its associated default program.
pub fn open_with_system(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
        Ok(())
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
