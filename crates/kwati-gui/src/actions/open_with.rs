//! Open with actions and application launcher (owned by inspect-worker for #68).
//!
//! Queries system and desktop applications associated with files, manages application
//! selection dialog, and launches files with default, chosen, or custom executables.

use std::fs;
use std::path::{Path, PathBuf};

use gpui::actions;

actions!(open_with, [OpenWithDefault, OpenWithPicker]);

/// Metadata for an application that can open a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub executable: String,
    pub is_default: bool,
    pub icon_name: Option<String>,
    pub no_display: bool,
    pub terminal: bool,
}

impl AppInfo {
    pub fn new(name: impl Into<String>, executable: impl Into<String>, is_default: bool) -> Self {
        Self {
            id: String::new(),
            name: name.into(),
            executable: executable.into(),
            is_default,
            icon_name: None,
            no_display: false,
            terminal: false,
        }
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon_name = Some(icon.into());
        self
    }

    fn to_core(&self) -> kwati_core::AppInfo {
        kwati_core::AppInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            executable: self.executable.clone(),
            icon: self.icon_name.clone(),
            is_default: self.is_default,
            no_display: self.no_display,
            terminal: self.terminal,
        }
    }
}

impl From<kwati_core::AppInfo> for AppInfo {
    fn from(app: kwati_core::AppInfo) -> Self {
        Self {
            id: app.id,
            name: app.name,
            executable: app.executable,
            is_default: app.is_default,
            icon_name: app.icon,
            no_display: app.no_display,
            terminal: app.terminal,
        }
    }
}

/// Scans standard XDG application directories.
#[cfg(target_os = "linux")]
fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut search_dirs = Vec::new();

    if let Some(xdg_data_home) = std::env::var_os("XDG_DATA_HOME") {
        search_dirs.push(PathBuf::from(xdg_data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        search_dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }

    if let Some(xdg_data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&xdg_data_dirs) {
            search_dirs.push(dir.join("applications"));
        }
    } else {
        search_dirs.push(PathBuf::from("/usr/local/share/applications"));
        search_dirs.push(PathBuf::from("/usr/share/applications"));
    }

    search_dirs
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedDesktopEntry {
    name: String,
    exec: String,
    icon: Option<String>,
}

fn read_desktop_entry(content: &str) -> Option<ParsedDesktopEntry> {
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_desktop_entry = trimmed == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        if let Some((key, val)) = trimmed.split_once('=') {
            match key.trim() {
                "Name" if name.is_none() => name = Some(val.trim().to_string()),
                "Exec" if exec.is_none() => exec = Some(val.trim().to_string()),
                "Icon" if icon.is_none() => icon = Some(val.trim().to_string()),
                "NoDisplay" if val.trim().eq_ignore_ascii_case("true") => return None,
                "Hidden" if val.trim().eq_ignore_ascii_case("true") => return None,
                _ => {}
            }
        }
    }

    Some(ParsedDesktopEntry {
        name: name?,
        exec: exec?,
        icon,
    })
}

/// Finds the default application capable of opening an HTTP stream URL directly for `path`,
/// if one is configured on the system.
pub fn stream_launcher_for(path: &Path) -> Option<AppInfo> {
    #[cfg(target_os = "linux")]
    {
        let mime = kwati_core::media_mime_type(path);
        if mime == "application/octet-stream" {
            return None;
        }
        let desktop_id = query_default_desktop_id(mime)?;
        stream_launcher_from_desktop_id(path, &desktop_id, &xdg_application_dirs())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

#[cfg(target_os = "linux")]
fn query_default_desktop_id(mime: &str) -> Option<String> {
    let output = std::process::Command::new("xdg-mime")
        .args(["query", "default", mime])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Resolves a `.desktop` file id in `search_dirs` as a URL-capable stream launcher for `path`.
pub fn stream_launcher_from_desktop_id(
    path: &Path,
    desktop_id: &str,
    search_dirs: &[PathBuf],
) -> Option<AppInfo> {
    let relative = desktop_id.replace('-', "/");
    let content = search_dirs.iter().find_map(|dir| {
        fs::read_to_string(dir.join(desktop_id))
            .ok()
            .or_else(|| fs::read_to_string(dir.join(&relative)).ok())
    })?;
    let entry = read_desktop_entry(&content)?;
    let tokens = split_exec_tokens(&entry.exec);
    let bin = tokens.first()?;
    if !tokens.iter().any(|t| t == "%u" || t == "%U") {
        return None;
    }
    if !kwati_core::is_browser_native_media(path) && is_browser_executable(bin, desktop_id) {
        return None;
    }
    if !which_in_path(bin) {
        return None;
    }
    Some(AppInfo {
        id: desktop_id.to_string(),
        name: entry.name,
        executable: entry.exec,
        is_default: true,
        icon_name: entry.icon,
        no_display: false,
        terminal: false,
    })
}

fn is_browser_executable(bin: &str, desktop_id: &str) -> bool {
    let bin_name = Path::new(bin)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(bin)
        .to_ascii_lowercase();
    let id_lower = desktop_id.to_ascii_lowercase();
    const BROWSERS: &[&str] = &[
        "firefox",
        "chromium",
        "google-chrome",
        "chrome",
        "brave",
        "vivaldi",
        "opera",
        "epiphany",
        "microsoft-edge",
    ];
    BROWSERS
        .iter()
        .any(|b| bin_name.contains(b) || id_lower.contains(b))
}

fn split_exec_tokens(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in exec.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Lists all applications capable of opening the given file, ordered with the default application first.
pub fn applications_for(path: &Path) -> Vec<AppInfo> {
    kwati_core::applications_for(path)
        .into_iter()
        .map(AppInfo::from)
        .collect()
}

/// Filters discovered applications for UI menus, keeping hidden (`NoDisplay=true`) entries
/// only when they are the configured default, and ensuring the default application comes first.
pub fn filter_menu_applications(apps: impl IntoIterator<Item = AppInfo>) -> Vec<AppInfo> {
    let mut filtered: Vec<AppInfo> = apps
        .into_iter()
        .filter(|app| app.is_default || !app.no_display)
        .collect();
    if let Some(default_ix) = filtered.iter().position(|app| app.is_default) {
        if default_ix != 0 {
            let default_app = filtered.remove(default_ix);
            filtered.insert(0, default_app);
        }
    }
    filtered
}

/// Lists applications suitable for display in UI "Open With" menus and dialogs for `path`.
pub fn menu_applications_for(path: &Path) -> Vec<AppInfo> {
    filter_menu_applications(applications_for(path))
}

/// Checks if an executable exists either as a path or in PATH.
pub fn which_in_path(bin: &str) -> bool {
    kwati_core::which_in_path(bin)
}

/// Launches a file or HTTP stream URL with the selected application.
///
/// Returns an error if the executable does not exist or fails to spawn.
pub fn launch_application(app: &AppInfo, path: &Path) -> std::io::Result<()> {
    // kwati_core::open_with expands %u/%U to file:// URIs for local paths, so HTTP stream URLs
    // must pass through without file:// rewriting.
    if path
        .to_str()
        .is_some_and(|s| s.starts_with("http://") || s.starts_with("https://"))
    {
        return launch_stream_url(app, path);
    }
    kwati_core::open_with(&app.to_core(), &[path])
}

fn launch_stream_url(app: &AppInfo, url: &Path) -> std::io::Result<()> {
    let exec = app.executable.trim();
    if exec.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executable not specified",
        ));
    }

    let parts = split_exec_tokens(exec);
    let Some(bin_name) = parts.first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executable not specified",
        ));
    };
    if !which_in_path(bin_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Application executable '{bin_name}' not found"),
        ));
    }

    let mut cmd = std::process::Command::new(bin_name);
    let mut url_added = false;
    for arg in parts.iter().skip(1) {
        if arg == "%f" || arg == "%F" || arg == "%u" || arg == "%U" {
            cmd.arg(url);
            url_added = true;
        } else if !arg.starts_with('%') {
            cmd.arg(arg);
        }
    }
    if !url_added {
        cmd.arg(url);
    }

    crate::actions::spawn_detached(&mut cmd)
}

/// Launches a file with a custom command line or executable path.
pub fn launch_custom_command(command_line: &str, path: &Path) -> std::io::Result<()> {
    kwati_core::open_with_executable(command_line, &[path])
}
