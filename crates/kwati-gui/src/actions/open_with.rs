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
    pub name: String,
    pub executable: String,
    pub is_default: bool,
    pub icon_name: Option<String>,
}

impl AppInfo {
    pub fn new(name: impl Into<String>, executable: impl Into<String>, is_default: bool) -> Self {
        Self {
            name: name.into(),
            executable: executable.into(),
            is_default,
            icon_name: None,
        }
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon_name = Some(icon.into());
        self
    }
}

/// Scans standard XDG application directories and parsed `.desktop` files.
fn scan_desktop_applications(ext: &str) -> Vec<AppInfo> {
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

    let mut found_apps = Vec::new();

    for dir in search_dirs {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Some(app) = parse_desktop_file(&content, ext) {
                        if !found_apps.iter().any(|a: &AppInfo| a.name == app.name) {
                            found_apps.push(app);
                        }
                    }
                }
            }
        }
    }

    found_apps
}

/// Parses a `.desktop` file to determine if it handles the given file extension.
fn parse_desktop_file(content: &str, target_ext: &str) -> Option<AppInfo> {
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut mimes = Vec::new();

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
                "MimeType" => {
                    for m in val.split(';') {
                        let m_trim = m.trim();
                        if !m_trim.is_empty() {
                            mimes.push(m_trim.to_string());
                        }
                    }
                }
                "NoDisplay" if val.trim().eq_ignore_ascii_case("true") => return None,
                "Hidden" if val.trim().eq_ignore_ascii_case("true") => return None,
                _ => {}
            }
        }
    }

    let name = name?;
    let exec = exec?;

    // Check MIME association or extension relevance
    let is_match = match target_ext {
        "png" => mimes.iter().any(|m| m.contains("png") || m == "image/*"),
        "jpg" | "jpeg" => mimes.iter().any(|m| m.contains("jpeg") || m == "image/*"),
        "svg" => mimes.iter().any(|m| m.contains("svg") || m == "image/*"),
        "txt" => mimes.iter().any(|m| m.contains("plain") || m == "text/*"),
        "pdf" => mimes.iter().any(|m| m.contains("pdf")),
        _ => false,
    };

    if is_match {
        Some(AppInfo {
            name,
            executable: exec,
            is_default: false,
            icon_name: icon,
        })
    } else {
        None
    }
}

/// Lists all applications capable of opening the given file, ordered with the default application first.
pub fn applications_for(path: &Path) -> Vec<AppInfo> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let mut apps = scan_desktop_applications(&ext);

    // Fallbacks based on file extension
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => {
            if !apps.iter().any(|a| a.name.contains("Image Viewer")) {
                apps.push(AppInfo::new("Image Viewer", "eog", true).with_icon("image-viewer"));
            }
            if !apps.iter().any(|a| a.name.contains("GIMP")) {
                apps.push(
                    AppInfo::new("GNU Image Manipulation Program", "gimp", false).with_icon("gimp"),
                );
            }
            if !apps.iter().any(|a| a.name.contains("Firefox")) {
                apps.push(
                    AppInfo::new("Firefox Web Browser", "firefox", false).with_icon("firefox"),
                );
            }
            if !apps.iter().any(|a| a.name.contains("Feh")) {
                apps.push(AppInfo::new("Feh Image Viewer", "feh", false).with_icon("feh"));
            }
        }
        "txt" | "rs" | "md" | "toml" | "json" | "js" | "py" | "c" | "cpp" | "sh" => {
            if !apps.iter().any(|a| a.name.contains("Text Editor")) {
                apps.push(AppInfo::new("Text Editor", "gedit", true).with_icon("text-editor"));
            }
            if !apps.iter().any(|a| a.name.contains("Visual Studio Code")) {
                apps.push(AppInfo::new("Visual Studio Code", "code", false).with_icon("vscode"));
            }
            if !apps.iter().any(|a| a.name.contains("Nano")) {
                apps.push(AppInfo::new("GNU Nano", "nano", false).with_icon("terminal"));
            }
        }
        "pdf" => {
            if !apps.iter().any(|a| a.name.contains("Document Viewer")) {
                apps.push(
                    AppInfo::new("Document Viewer", "evince", true).with_icon("document-viewer"),
                );
            }
            if !apps.iter().any(|a| a.name.contains("Firefox")) {
                apps.push(
                    AppInfo::new("Firefox Web Browser", "firefox", false).with_icon("firefox"),
                );
            }
        }
        _ => {
            if apps.is_empty() {
                apps.push(AppInfo::new("Default Application", "xdg-open", true));
            }
        }
    }

    // Ensure default application is first in the list
    if let Some(default_idx) = apps.iter().position(|a| a.is_default) {
        if default_idx != 0 {
            let default_app = apps.remove(default_idx);
            apps.insert(0, default_app);
        }
    } else if let Some(first) = apps.first_mut() {
        first.is_default = true;
    }

    apps
}

/// Checks if an executable exists either as a path or in PATH.
pub fn which_in_path(bin: &str) -> bool {
    let path = Path::new(bin);
    if path.is_file() {
        return true;
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return true;
            }
            #[cfg(windows)]
            if candidate.with_extension("exe").is_file() {
                return true;
            }
        }
    }
    false
}

/// Launches a file with the selected application.
///
/// Returns an error if the executable does not exist or fails to spawn.
pub fn launch_application(app: &AppInfo, path: &Path) -> std::io::Result<()> {
    let exec = app.executable.trim();
    if exec.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executable not specified",
        ));
    }

    let bin_name = exec.split_whitespace().next().unwrap_or(exec);
    if !which_in_path(bin_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Application executable '{}' not found", bin_name),
        ));
    }

    let mut cmd = std::process::Command::new(bin_name);
    let args: Vec<&str> = exec.split_whitespace().skip(1).collect();
    let mut path_added = false;
    for arg in args {
        if arg == "%f" || arg == "%F" || arg == "%u" || arg == "%U" {
            cmd.arg(path);
            path_added = true;
        } else if !arg.starts_with('%') {
            cmd.arg(arg);
        }
    }
    if !path_added {
        cmd.arg(path);
    }

    crate::actions::spawn_detached(&mut cmd)
}

/// Launches a file with a custom command line or executable path.
pub fn launch_custom_command(command_line: &str, path: &Path) -> std::io::Result<()> {
    let trimmed = command_line.trim();
    if trimmed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "custom command cannot be empty",
        ));
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let bin = parts[0];
    if !which_in_path(bin) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Executable '{}' not found", bin),
        ));
    }

    let mut cmd = std::process::Command::new(bin);
    let mut path_added = false;
    for &part in &parts[1..] {
        if part == "%f" || part == "%F" || part == "%u" || part == "%U" {
            cmd.arg(path);
            path_added = true;
        } else if !part.starts_with('%') {
            cmd.arg(part);
        }
    }
    if !path_added {
        cmd.arg(path);
    }

    crate::actions::spawn_detached(&mut cmd)
}
