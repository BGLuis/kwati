//! Cross-platform discovery and launching of applications that can open a local file.

use std::env;
use std::io;
use std::path::{Path, PathBuf};

/// Metadata for an application capable of opening a local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    /// Platform identifier (`.desktop` ID on Linux, handler name/AUMID on Windows, `.app` bundle path on macOS).
    pub id: String,
    /// Human-readable application name.
    pub name: String,
    /// Launch command, executable, or `.app` bundle path.
    pub executable: String,
    /// Platform icon reference (freedesktop icon name/path on Linux, `"path,index"` on Windows, `.app` bundle path on macOS).
    pub icon: Option<String>,
    /// Whether this application is the system default for the queried file.
    pub is_default: bool,
    /// Whether the application is hidden from normal app menus (`NoDisplay=true` on Linux).
    pub no_display: bool,
    /// Whether the application expects a terminal (`Terminal=true` on Linux).
    pub terminal: bool,
}

/// Lists applications registered to open local `path`, ordered with the default application first.
///
/// Returns an empty list for directories or when no registered application matches.
pub fn applications_for(path: &Path) -> Vec<AppInfo> {
    if path.as_os_str().is_empty() || path.is_dir() {
        return Vec::new();
    }

    #[cfg(target_os = "linux")]
    {
        linux::applications_for(path)
    }
    #[cfg(windows)]
    {
        windows::applications_for(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos::applications_for(path)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Launches `app` with `paths`.
///
/// Returns an error (such as [`io::ErrorKind::NotFound`]) if the application or executable no
/// longer exists or cannot be spawned, and never falls back to opening with the system default.
pub fn open_with(app: &AppInfo, paths: &[impl AsRef<Path>]) -> io::Result<()> {
    let path_refs: Vec<&Path> = paths.iter().map(AsRef::as_ref).collect();

    #[cfg(target_os = "linux")]
    {
        linux::open_with(app, &path_refs)
    }
    #[cfg(windows)]
    {
        windows::open_with(app, &path_refs)
    }
    #[cfg(target_os = "macos")]
    {
        macos::open_with(app, &path_refs)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (app, path_refs);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "open_with is not supported on this platform",
        ))
    }
}

/// Launches `paths` using a custom executable path or command line.
///
/// Returns an error if `executable` is empty or cannot be found on disk or in `PATH`.
pub fn open_with_executable(executable: &str, paths: &[impl AsRef<Path>]) -> io::Result<()> {
    let path_refs: Vec<&Path> = paths.iter().map(AsRef::as_ref).collect();

    #[cfg(target_os = "linux")]
    {
        linux::open_with_executable(executable, &path_refs)
    }
    #[cfg(windows)]
    {
        windows::open_with_executable(executable, &path_refs)
    }
    #[cfg(target_os = "macos")]
    {
        macos::open_with_executable(executable, &path_refs)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (executable, path_refs);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "open_with_executable is not supported on this platform",
        ))
    }
}

/// Checks if an executable exists either as a file path or on `PATH`.
pub fn which_in_path(bin: &str) -> bool {
    resolve_executable(bin).is_some()
}

pub(crate) fn resolve_executable(bin: &str) -> Option<PathBuf> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }

    let path = Path::new(bin);
    let has_separator = bin.contains('/') || (cfg!(windows) && bin.contains('\\'));
    if has_separator {
        return path.is_file().then(|| path.to_path_buf());
    }

    if path.is_file() {
        return Some(path.to_path_buf());
    }

    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        if path.extension().is_none() {
            let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
            for ext in pathext.split(';').filter(|e| !e.is_empty()) {
                let with_ext = dir.join(format!("{bin}{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }

    None
}

/// Linux XDG MIME detection, `mimeapps.list` resolution, `.desktop` parsing, and launching.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{HashMap, HashSet};
    use std::env;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::{resolve_executable, which_in_path, AppInfo};

    #[derive(Debug, Clone)]
    struct DesktopEntry {
        id: String,
        name: String,
        exec: String,
        icon: Option<String>,
        mimes: Vec<String>,
        no_display: bool,
        terminal: bool,
    }

    #[derive(Debug, Default)]
    struct MimeAppsData {
        default_apps: HashMap<String, Vec<String>>,
        added: HashMap<String, Vec<String>>,
        removed: HashMap<String, Vec<String>>,
    }

    /// Returns XDG data directories (`$XDG_DATA_HOME`, then `$XDG_DATA_DIRS`) in precedence order.
    fn xdg_data_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home_data) = env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home_data));
        } else if let Some(home) = env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".local/share"));
        }

        if let Some(data_dirs) = env::var_os("XDG_DATA_DIRS").filter(|v| !v.is_empty()) {
            dirs.extend(env::split_paths(&data_dirs));
        } else {
            dirs.push(PathBuf::from("/usr/local/share"));
            dirs.push(PathBuf::from("/usr/share"));
        }
        dirs
    }

    /// Returns XDG config directories (`$XDG_CONFIG_HOME`, then `$XDG_CONFIG_DIRS`) in precedence order.
    fn xdg_config_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(cfg_home) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(cfg_home));
        } else if let Some(home) = env::var_os("HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(home).join(".config"));
        }

        if let Some(cfg_dirs) = env::var_os("XDG_CONFIG_DIRS").filter(|v| !v.is_empty()) {
            dirs.extend(env::split_paths(&cfg_dirs));
        } else {
            dirs.push(PathBuf::from("/etc/xdg"));
        }
        dirs
    }

    /// Returns lowercased desktop names from `$XDG_CURRENT_DESKTOP`.
    fn current_desktops() -> Vec<String> {
        env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Ordered list of `mimeapps.list` paths per XDG MIME Apps Specification 1.0.1.
    fn mimeapps_list_paths() -> Vec<PathBuf> {
        let desktops = current_desktops();
        let cfg_dirs = xdg_config_dirs();
        let data_dirs = xdg_data_dirs();
        let mut out = Vec::new();

        for dir in &cfg_dirs {
            for desktop in &desktops {
                out.push(dir.join(format!("{desktop}-mimeapps.list")));
            }
            out.push(dir.join("mimeapps.list"));
        }
        for dir in &data_dirs {
            let app_dir = dir.join("applications");
            for desktop in &desktops {
                out.push(app_dir.join(format!("{desktop}-mimeapps.list")));
            }
            out.push(app_dir.join("mimeapps.list"));
        }
        out
    }

    /// Detects the canonical MIME type for `path` using XDG `mime/globs2` and `mime/aliases`,
    /// falling back to common extensions if no `globs2` match is found.
    fn detect_mime_type(path: &Path) -> Option<String> {
        let file_name = path.file_name()?.to_str()?;
        let data_dirs = xdg_data_dirs();

        let mut best: Option<(u32, usize, String)> = None;
        for dir in &data_dirs {
            let globs_path = dir.join("mime/globs2");
            let Ok(content) = fs::read_to_string(&globs_path) else {
                continue;
            };
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.split(':');
                let (Some(weight_str), Some(mime), Some(pattern)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let flags = parts.next().unwrap_or("");
                if pattern == "__NOGLOBS__" {
                    continue;
                }
                let Ok(weight) = weight_str.parse::<u32>() else {
                    continue;
                };
                let case_sensitive = flags.split(',').any(|f| f == "cs");
                if glob_matches(pattern, file_name, case_sensitive) {
                    let specificity = pattern.len();
                    let replace = match &best {
                        None => true,
                        Some((bw, bs, _)) => (weight, specificity) > (*bw, *bs),
                    };
                    if replace {
                        best = Some((weight, specificity, mime.to_ascii_lowercase()));
                    }
                }
            }
        }

        let raw_mime = best
            .map(|(_, _, m)| m)
            .or_else(|| fallback_mime_for_extension(path))?;

        Some(resolve_mime_alias(&raw_mime, &data_dirs))
    }

    fn glob_matches(pattern: &str, name: &str, case_sensitive: bool) -> bool {
        if case_sensitive {
            wildcard_match(pattern.as_bytes(), name.as_bytes())
        } else {
            let p = pattern.to_ascii_lowercase();
            let n = name.to_ascii_lowercase();
            wildcard_match(p.as_bytes(), n.as_bytes())
        }
    }

    fn wildcard_match(pat: &[u8], text: &[u8]) -> bool {
        if let Some(suffix) = pat.strip_prefix(b"*") {
            if !suffix.contains(&b'*') && !suffix.contains(&b'?') && !suffix.contains(&b'[') {
                return text.ends_with(suffix);
            }
        }
        let (mut pi, mut ti) = (0, 0);
        let (mut star, mut match_ti) = (None, 0);
        while ti < text.len() {
            if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == text[ti]) {
                pi += 1;
                ti += 1;
            } else if pi < pat.len() && pat[pi] == b'*' {
                star = Some(pi);
                pi += 1;
                match_ti = ti;
            } else if let Some(s) = star {
                pi = s + 1;
                match_ti += 1;
                ti = match_ti;
            } else {
                return false;
            }
        }
        while pi < pat.len() && pat[pi] == b'*' {
            pi += 1;
        }
        pi == pat.len()
    }

    fn resolve_mime_alias(mime: &str, data_dirs: &[PathBuf]) -> String {
        for dir in data_dirs {
            let Ok(content) = fs::read_to_string(dir.join("mime/aliases")) else {
                continue;
            };
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.split_whitespace();
                if let (Some(alias), Some(canon)) = (parts.next(), parts.next()) {
                    if alias.eq_ignore_ascii_case(mime) {
                        return canon.to_ascii_lowercase();
                    }
                }
            }
        }
        mime.to_string()
    }

    fn fallback_mime_for_extension(path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let mime = match ext.as_str() {
            "txt" | "log" | "csv" => "text/plain",
            "md" => "text/markdown",
            "rs" => "text/rust",
            "json" => "application/json",
            "html" | "htm" => "text/html",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "pdf" => "application/pdf",
            _ => return None,
        };
        Some(mime.to_string())
    }

    /// Parses a `mimeapps.list` file into its three sections.
    fn parse_mimeapps_file(content: &str) -> MimeAppsData {
        #[derive(Copy, Clone, PartialEq, Eq)]
        enum Section {
            None,
            Default,
            Added,
            Removed,
        }

        let mut data = MimeAppsData::default();
        let mut section = Section::None;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                section = match trimmed {
                    "[Default Applications]" => Section::Default,
                    "[Added Associations]" => Section::Added,
                    "[Removed Associations]" => Section::Removed,
                    _ => Section::None,
                };
                continue;
            }
            if section == Section::None {
                continue;
            }
            let Some((mime, list)) = trimmed.split_once('=') else {
                continue;
            };
            let mime = mime.trim().to_ascii_lowercase();
            let ids: Vec<String> = list
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            if ids.is_empty() {
                continue;
            }
            let map = match section {
                Section::Default => &mut data.default_apps,
                Section::Added => &mut data.added,
                Section::Removed => &mut data.removed,
                Section::None => unreachable!(),
            };
            map.entry(mime).or_default().extend(ids);
        }
        data
    }

    /// Parses a `.desktop` file `[Desktop Entry]` section.
    ///
    /// Returns `Some(None)` if `Hidden=true` (so the caller masks lower-precedence entries with the
    /// same desktop ID), `None` if invalid or `TryExec` is missing, or `Some(Some(entry))`.
    fn parse_desktop_file(id: String, content: &str) -> Option<Option<DesktopEntry>> {
        let mut in_entry = false;
        let mut entry_type: Option<String> = None;
        let mut name: Option<String> = None;
        let mut exec: Option<String> = None;
        let mut try_exec: Option<String> = None;
        let mut icon: Option<String> = None;
        let mut mimes = Vec::new();
        let mut hidden = false;
        let mut no_display = false;
        let mut terminal = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') {
                in_entry = trimmed == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            let Some((key, val)) = trimmed.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim();
            match key {
                "Type" => entry_type = Some(val.to_string()),
                "Name" if name.is_none() => name = Some(val.to_string()),
                "Exec" if exec.is_none() => exec = Some(val.to_string()),
                "TryExec" if try_exec.is_none() => try_exec = Some(val.to_string()),
                "Icon" if icon.is_none() => icon = Some(val.to_string()),
                "Hidden" => hidden = val.eq_ignore_ascii_case("true"),
                "NoDisplay" => no_display = val.eq_ignore_ascii_case("true"),
                "Terminal" => terminal = val.eq_ignore_ascii_case("true"),
                "MimeType" => {
                    for m in val.split(';') {
                        let m = m.trim().to_ascii_lowercase();
                        if !m.is_empty() {
                            mimes.push(m);
                        }
                    }
                }
                _ => {}
            }
        }

        if hidden {
            return Some(None);
        }
        if entry_type.as_deref() != Some("Application") {
            return None;
        }
        if let Some(ref te) = try_exec {
            if !which_in_path(te) {
                return None;
            }
        }

        let name = name?;
        let exec = exec?;
        Some(Some(DesktopEntry {
            id,
            name,
            exec,
            icon,
            mimes,
            no_display,
            terminal,
        }))
    }

    /// Collects all `.desktop` entries across `$XDG_DATA_HOME/applications` and `$XDG_DATA_DIRS/applications`,
    /// respecting XDG desktop-ID shadowing (including `Hidden=true` masking lower-precedence directories).
    fn load_desktop_entries() -> HashMap<String, DesktopEntry> {
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut entries: HashMap<String, DesktopEntry> = HashMap::new();

        for data_dir in xdg_data_dirs() {
            let app_dir = data_dir.join("applications");
            if !app_dir.is_dir() {
                continue;
            }
            collect_desktop_dir(&app_dir, &app_dir, &mut seen_ids, &mut entries);
        }
        entries
    }

    fn collect_desktop_dir(
        root: &Path,
        dir: &Path,
        seen_ids: &mut HashSet<String>,
        entries: &mut HashMap<String, DesktopEntry>,
    ) {
        let Ok(read_dir) = fs::read_dir(dir) else {
            return;
        };
        let mut dir_entries: Vec<PathBuf> = read_dir.flatten().map(|e| e.path()).collect();
        dir_entries.sort();

        for path in dir_entries {
            if path.is_dir() {
                collect_desktop_dir(root, &path, seen_ids, entries);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let desktop_id = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("-");

            if seen_ids.contains(&desktop_id) {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            // A higher-precedence desktop file with this ID shadows lower-precedence directories
            // even when it fails TryExec or sets Hidden=true.
            seen_ids.insert(desktop_id.clone());
            if let Some(Some(entry)) = parse_desktop_file(desktop_id.clone(), &content) {
                entries.insert(desktop_id, entry);
            }
        }
    }

    /// Reads `[MIME Cache]` entries for `mime` across all `mimeinfo.cache` files.
    fn read_mimeinfo_cache(mime: &str) -> Vec<String> {
        let mut ids = Vec::new();
        for data_dir in xdg_data_dirs() {
            let cache_path = data_dir.join("applications/mimeinfo.cache");
            let Ok(content) = fs::read_to_string(&cache_path) else {
                continue;
            };
            let mut in_cache = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('[') {
                    in_cache = trimmed == "[MIME Cache]";
                    continue;
                }
                if !in_cache {
                    continue;
                }
                if let Some((m, val)) = trimmed.split_once('=') {
                    if m.trim().eq_ignore_ascii_case(mime) {
                        for id in val.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                            ids.push(id.to_string());
                        }
                    }
                }
            }
        }
        ids
    }

    /// Lists all applications capable of opening `path` per XDG MIME Apps Spec 1.0.1.
    pub(super) fn applications_for(path: &Path) -> Vec<AppInfo> {
        let Some(mime) = detect_mime_type(path) else {
            return Vec::new();
        };
        let entries = load_desktop_entries();

        let mut default_candidate: Option<String> = None;
        let mut ordered_ids: Vec<String> = Vec::new();
        let mut removed: HashSet<String> = HashSet::new();

        for ma_path in mimeapps_list_paths() {
            let Ok(content) = fs::read_to_string(&ma_path) else {
                continue;
            };
            let parsed = parse_mimeapps_file(&content);

            if default_candidate.is_none() {
                if let Some(defs) = parsed.default_apps.get(&mime) {
                    for id in defs {
                        if !removed.contains(id) && entries.contains_key(id) {
                            default_candidate = Some(id.clone());
                            break;
                        }
                    }
                }
            }

            if let Some(added) = parsed.added.get(&mime) {
                for id in added {
                    if !removed.contains(id)
                        && entries.contains_key(id)
                        && !ordered_ids.contains(id)
                    {
                        ordered_ids.push(id.clone());
                    }
                }
            }

            if let Some(rems) = parsed.removed.get(&mime) {
                for id in rems {
                    removed.insert(id.clone());
                }
            }
        }

        for id in read_mimeinfo_cache(&mime) {
            if !removed.contains(&id) && entries.contains_key(&id) && !ordered_ids.contains(&id) {
                ordered_ids.push(id);
            }
        }

        // Supplement from .desktop MimeType= lines so newly added .desktop files work even if
        // mimeinfo.cache has not been regenerated.
        let mut scanned: Vec<&DesktopEntry> = entries
            .values()
            .filter(|e| e.mimes.iter().any(|m| m == &mime))
            .collect();
        scanned.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for entry in scanned {
            if !removed.contains(&entry.id) && !ordered_ids.contains(&entry.id) {
                ordered_ids.push(entry.id.clone());
            }
        }

        if let Some(ref def_id) = default_candidate {
            ordered_ids.retain(|id| id != def_id);
            ordered_ids.insert(0, def_id.clone());
        }

        ordered_ids
            .into_iter()
            .filter_map(|id| {
                let entry = entries.get(&id)?;
                let is_default = default_candidate.as_deref() == Some(id.as_str());
                Some(AppInfo {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    executable: entry.exec.clone(),
                    icon: entry.icon.clone(),
                    is_default,
                    no_display: entry.no_display,
                    terminal: entry.terminal,
                })
            })
            .collect()
    }

    /// Tokenizes a `.desktop` `Exec` line according to FreeDesktop Desktop Entry Spec quoting rules.
    fn split_exec_tokens(exec: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut chars = exec.chars();

        while let Some(ch) = chars.next() {
            if in_quotes {
                match ch {
                    '"' => in_quotes = false,
                    '\\' => {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    }
                    c => current.push(c),
                }
            } else {
                match ch {
                    '"' => in_quotes = true,
                    c if c.is_whitespace() => {
                        if !current.is_empty() {
                            tokens.push(std::mem::take(&mut current));
                        }
                    }
                    c => current.push(c),
                }
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens
    }

    fn path_to_file_uri(path: &Path) -> String {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let mut uri = String::from("file://");
        for byte in abs.as_os_str().as_encoded_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
                uri.push(*byte as char);
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
        uri
    }

    /// Launches `app` with `paths`, expanding `%f/%F/%u/%U/%i/%c/%k/%%` field codes.
    pub(super) fn open_with(app: &AppInfo, paths: &[&Path]) -> io::Result<()> {
        let trimmed = app.executable.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "application executable not specified",
            ));
        }

        let tokens = if Path::new(trimmed).is_file() {
            vec![trimmed.to_string()]
        } else {
            split_exec_tokens(trimmed)
        };
        let Some(bin) = tokens.first() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "application executable not specified",
            ));
        };
        if !which_in_path(bin) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("application executable '{bin}' not found"),
            ));
        }

        let wants_single = tokens.iter().any(|t| t == "%f" || t == "%u");
        if wants_single && paths.len() > 1 {
            for &p in paths {
                spawn_exec_tokens(&tokens, app, &[p])?;
            }
            return Ok(());
        }
        spawn_exec_tokens(&tokens, app, paths)
    }

    fn spawn_exec_tokens(tokens: &[String], app: &AppInfo, paths: &[&Path]) -> io::Result<()> {
        let bin = &tokens[0];
        let mut cmd = Command::new(bin);
        let mut added_paths = false;

        for token in &tokens[1..] {
            match token.as_str() {
                "%f" => {
                    if let Some(p) = paths.first() {
                        cmd.arg(p);
                    }
                    added_paths = true;
                }
                "%F" => {
                    for p in paths {
                        cmd.arg(p);
                    }
                    added_paths = true;
                }
                "%u" => {
                    if let Some(p) = paths.first() {
                        cmd.arg(path_to_file_uri(p));
                    }
                    added_paths = true;
                }
                "%U" => {
                    for p in paths {
                        cmd.arg(path_to_file_uri(p));
                    }
                    added_paths = true;
                }
                "%i" => {
                    if let Some(ref icon) = app.icon {
                        cmd.arg("--icon").arg(icon);
                    }
                }
                "%c" => {
                    cmd.arg(&app.name);
                }
                "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
                other => {
                    if other.contains("%%") {
                        cmd.arg(other.replace("%%", "%"));
                    } else {
                        cmd.arg(other);
                    }
                }
            }
        }

        if !added_paths {
            for p in paths {
                cmd.arg(p);
            }
        }

        spawn_detached(&mut cmd)
    }

    /// Launches a custom command line or executable path with `paths`.
    pub(super) fn open_with_executable(command_line: &str, paths: &[&Path]) -> io::Result<()> {
        let trimmed = command_line.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "executable command cannot be empty",
            ));
        }

        let tokens = if Path::new(trimmed).is_file() {
            vec![trimmed.to_string()]
        } else {
            split_exec_tokens(trimmed)
        };
        let Some(bin) = tokens.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "executable command cannot be empty",
            ));
        };
        if resolve_executable(bin).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("executable '{bin}' not found"),
            ));
        }

        let app = AppInfo {
            id: bin.clone(),
            name: bin.clone(),
            executable: command_line.to_string(),
            icon: None,
            is_default: false,
            no_display: false,
            terminal: false,
        };
        let wants_single = tokens.iter().any(|t| t == "%f" || t == "%u");
        if wants_single && paths.len() > 1 {
            for &p in paths {
                spawn_exec_tokens(&tokens, &app, &[p])?;
            }
            return Ok(());
        }
        spawn_exec_tokens(&tokens, &app, paths)
    }

    fn spawn_detached(cmd: &mut Command) -> io::Result<()> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_desktop_entry_ignoring_actions() {
            let content = "\
    [Desktop Entry]
    Type=Application
    Name=Firefox
    Exec=/usr/lib/firefox/firefox %u
    Icon=firefox
    MimeType=text/html;image/png;

    [Desktop Action NewWindow]
    Name=New Window
    Exec=/usr/lib/firefox/firefox --new-window %u
    ";
            let parsed = parse_desktop_file("firefox.desktop".into(), content)
                .unwrap()
                .unwrap();
            assert_eq!(parsed.name, "Firefox");
            assert_eq!(parsed.exec, "/usr/lib/firefox/firefox %u");
            assert_eq!(parsed.mimes, vec!["text/html", "image/png"]);
        }

        #[test]
        fn hidden_true_masks_entry() {
            let content = "[Desktop Entry]\nType=Application\nName=Gone\nExec=true\nHidden=true\n";
            let parsed = parse_desktop_file("gone.desktop".into(), content);
            assert!(matches!(parsed, Some(None)));
        }

        #[test]
        fn mimeapps_parses_sections() {
            let data = parse_mimeapps_file(
                "\
    [Default Applications]
    text/plain=user-nodisplay.desktop;

    [Added Associations]
    text/plain=extra.desktop;

    [Removed Associations]
    text/plain=unwanted.desktop;
    ",
            );
            assert_eq!(
                data.default_apps.get("text/plain").unwrap(),
                &vec!["user-nodisplay.desktop".to_string()]
            );
            assert_eq!(
                data.added.get("text/plain").unwrap(),
                &vec!["extra.desktop".to_string()]
            );
            assert_eq!(
                data.removed.get("text/plain").unwrap(),
                &vec!["unwanted.desktop".to_string()]
            );
        }

        #[test]
        fn exec_tokenizer_handles_quotes_and_field_codes() {
            let tokens = split_exec_tokens("\"/opt/My App/bin\" --flag %F \"arg two\"");
            assert_eq!(tokens, vec!["/opt/My App/bin", "--flag", "%F", "arg two"]);
        }
    }
}

/// Windows Shell association backend (`SHAssocEnumHandlers` / `IAssocHandler` + `AssocQueryStringW`).
#[cfg(windows)]
mod windows {
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{S_FALSE, S_OK};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, IDataObject, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, BHID_DataObject, IAssocHandler, IEnumAssocHandlers, ILCreateFromPathW,
        ILFree, IShellItem, SHAssocEnumHandlers, SHCreateItemFromParsingName,
        SHCreateShellItemArrayFromIDLists, ShellExecuteExW, ASSOCF_INIT_IGNOREUNKNOWN,
        ASSOCSTR_EXECUTABLE, ASSOC_FILTER_NONE, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::{resolve_executable, AppInfo};

    struct ComGuard {
        initialized: bool,
    }

    impl ComGuard {
        fn enter() -> io::Result<Self> {
            // S_OK or S_FALSE means COM is initialized on this thread and must be balanced.
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr == S_OK || hr == S_FALSE {
                Ok(Self { initialized: true })
            } else if hr.0 == -2147417850i32 {
                // RPC_E_CHANGED_MODE (0x80010106) means the thread already initialized COM in MTA.
                Ok(Self { initialized: false })
            } else {
                Err(io::Error::from_raw_os_error(hr.0))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.initialized {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn to_wide_null(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    unsafe fn take_pwstr(pwstr: PWSTR) -> Option<String> {
        if pwstr.is_null() {
            return None;
        }
        let s = unsafe { pwstr.to_string().ok() };
        unsafe { CoTaskMemFree(Some(pwstr.as_ptr().cast())) };
        s
    }

    fn default_executable_for_ext(ext_with_dot: &[u16]) -> Option<String> {
        let mut len = 0u32;
        let hr = unsafe {
            AssocQueryStringW(
                ASSOCF_INIT_IGNOREUNKNOWN,
                ASSOCSTR_EXECUTABLE,
                PCWSTR::from_raw(ext_with_dot.as_ptr()),
                None,
                None,
                &mut len,
            )
        };
        if len == 0 && hr.is_err() {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let hr = unsafe {
            AssocQueryStringW(
                ASSOCF_INIT_IGNOREUNKNOWN,
                ASSOCSTR_EXECUTABLE,
                PCWSTR::from_raw(ext_with_dot.as_ptr()),
                None,
                Some(PWSTR::from_raw(buf.as_mut_ptr())),
                &mut len,
            )
        };
        if hr.is_err() {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Lists handlers registered for `path`'s extension via `SHAssocEnumHandlers`.
    pub(super) fn applications_for(path: &Path) -> Vec<AppInfo> {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return Vec::new();
        };
        let ext_dot = format!(".{ext}");
        let ext_wide = to_wide_null(OsStr::new(&ext_dot));

        let Ok(_com) = ComGuard::enter() else {
            return Vec::new();
        };

        let default_exe = default_executable_for_ext(&ext_wide).map(|s| s.to_ascii_lowercase());

        let Ok(enumerator): Result<IEnumAssocHandlers, _> = (unsafe {
            SHAssocEnumHandlers(PCWSTR::from_raw(ext_wide.as_ptr()), ASSOC_FILTER_NONE)
        }) else {
            return Vec::new();
        };

        let mut apps = Vec::new();
        let mut seen_ids = HashSet::new();
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let hr = unsafe { enumerator.Next(&mut slot, Some(&mut fetched)) };
            if hr.is_err() || fetched == 0 {
                break;
            }
            let Some(handler) = slot[0].take() else {
                break;
            };

            let Some(raw_name) = (unsafe { handler.GetName().ok().and_then(|p| take_pwstr(p)) })
            else {
                continue;
            };
            let key = raw_name.to_ascii_lowercase();
            if !seen_ids.insert(key.clone()) {
                continue;
            }

            let ui_name = unsafe { handler.GetUIName().ok().and_then(|p| take_pwstr(p)) }
                .unwrap_or_else(|| raw_name.clone());

            let mut icon_pwstr = PWSTR::null();
            let mut icon_idx = 0i32;
            let icon = unsafe {
                if handler
                    .GetIconLocation(&mut icon_pwstr, &mut icon_idx)
                    .is_ok()
                {
                    take_pwstr(icon_pwstr).map(|p| format!("{p},{icon_idx}"))
                } else {
                    None
                }
            };

            let is_default = default_exe.as_ref().is_some_and(|d| d == &key);

            apps.push(AppInfo {
                id: raw_name.clone(),
                name: ui_name,
                executable: raw_name,
                icon,
                is_default,
                no_display: false,
                terminal: false,
            });
        }

        if let Some(idx) = apps.iter().position(|a| a.is_default) {
            if idx != 0 {
                let def = apps.remove(idx);
                apps.insert(0, def);
            }
        }

        apps
    }

    /// Launches `paths` via `IAssocHandler::CreateInvoker` / `Invoke`.
    pub(super) fn open_with(app: &AppInfo, paths: &[&Path]) -> io::Result<()> {
        let Some(first) = paths.first().copied() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one path is required",
            ));
        };
        let Some(ext) = first.extension().and_then(|e| e.to_str()) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("application handler '{}' not found", app.id),
            ));
        };

        let _com = ComGuard::enter()?;
        let ext_wide = to_wide_null(OsStr::new(&format!(".{ext}")));
        let enumerator =
            unsafe { SHAssocEnumHandlers(PCWSTR::from_raw(ext_wide.as_ptr()), ASSOC_FILTER_NONE) }
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        let mut matched: Option<IAssocHandler> = None;
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            let hr = unsafe { enumerator.Next(&mut slot, Some(&mut fetched)) };
            if hr.is_err() || fetched == 0 {
                break;
            }
            if let Some(handler) = slot[0].take() {
                let name = unsafe { handler.GetName().ok().and_then(|p| take_pwstr(p)) };
                if name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&app.id))
                {
                    matched = Some(handler);
                    break;
                }
            }
        }

        let Some(handler) = matched else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("application '{}' is no longer registered", app.name),
            ));
        };

        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
        for p in paths {
            let wide = to_wide_null(p.as_os_str());
            let pidl = unsafe { ILCreateFromPathW(PCWSTR::from_raw(wide.as_ptr())) };
            if pidl.is_null() {
                for prev in pidls {
                    unsafe { ILFree(Some(prev)) };
                }
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("path '{}' not found", p.display()),
                ));
            }
            pidls.push(pidl);
        }

        let const_pidls: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p.cast_const()).collect();
        let item_array = unsafe { SHCreateShellItemArrayFromIDLists(&const_pidls) };
        for pidl in pidls {
            unsafe { ILFree(Some(pidl)) };
        }
        let item_array = item_array.map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let data_obj: IDataObject = unsafe { item_array.BindToHandler(None, &BHID_DataObject) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;

        if let Ok(invoker) = unsafe { handler.CreateInvoker(&data_obj) } {
            if unsafe { invoker.SupportsSelection() }.is_ok() {
                return unsafe { invoker.Invoke() }
                    .map_err(|e| io::Error::from_raw_os_error(e.code().0));
            }
        }

        let first_wide = to_wide_null(first.as_os_str());
        let shell_item: IShellItem =
            unsafe { SHCreateItemFromParsingName(PCWSTR::from_raw(first_wide.as_ptr()), None) }
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let single_obj: IDataObject = unsafe { shell_item.BindToHandler(None, &BHID_DataObject) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        unsafe { handler.Invoke(&single_obj) }.map_err(|e| io::Error::from_raw_os_error(e.code().0))
    }

    /// Launches `paths` using a custom executable path via `ShellExecuteExW`.
    pub(super) fn open_with_executable(executable: &str, paths: &[&Path]) -> io::Result<()> {
        let trimmed = executable.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "executable command cannot be empty",
            ));
        }
        let Some(exe_path) = resolve_executable(trimmed) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("executable '{executable}' not found"),
            ));
        };

        let exe_wide = to_wide_null(exe_path.as_os_str());
        let params_str = paths
            .iter()
            .map(|p| format!("\"{}\"", p.display()))
            .collect::<Vec<_>>()
            .join(" ");
        let params_wide = to_wide_null(OsStr::new(&params_str));
        let verb_wide = to_wide_null(OsStr::new("open"));

        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            lpVerb: PCWSTR::from_raw(verb_wide.as_ptr()),
            lpFile: PCWSTR::from_raw(exe_wide.as_ptr()),
            lpParameters: PCWSTR::from_raw(params_wide.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };

        unsafe { ShellExecuteExW(&mut info) }.map_err(|e| io::Error::from_raw_os_error(e.code().0))
    }
}

/// macOS Launch Services / `NSWorkspace` backend (`objc2-app-kit` + `/usr/bin/open -a`).
#[cfg(target_os = "macos")]
mod macos {
    use std::collections::HashSet;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    use super::AppInfo;

    /// Lists applications registered to open `path` via `NSWorkspace::URLsForApplicationsToOpenURL:`.
    pub(super) fn applications_for(path: &Path) -> Vec<AppInfo> {
        let Some(path_str) = path.to_str() else {
            return Vec::new();
        };
        let ns_path = NSString::from_str(path_str);
        let file_url = NSURL::fileURLWithPath(&ns_path);
        let workspace = NSWorkspace::sharedWorkspace();

        let default_path = workspace
            .URLForApplicationToOpenURL(&file_url)
            .and_then(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()));

        let app_urls = workspace.URLsForApplicationsToOpenURL(&file_url);
        let mut apps = Vec::with_capacity(app_urls.count());
        let mut seen_ids = HashSet::new();

        for i in 0..app_urls.count() {
            let app_url = app_urls.objectAtIndex(i);
            let Some(bundle_ns) = app_url.path() else {
                continue;
            };
            let bundle_path = PathBuf::from(bundle_ns.to_string());
            let id = bundle_path.to_string_lossy().into_owned();
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            let name = bundle_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Application")
                .to_string();
            let is_default = default_path.as_ref().is_some_and(|d| d == &bundle_path);

            apps.push(AppInfo {
                id: id.clone(),
                name,
                executable: id.clone(),
                icon: Some(id),
                is_default,
                no_display: false,
                terminal: false,
            });
        }

        if let Some(idx) = apps.iter().position(|a| a.is_default) {
            if idx != 0 {
                let def = apps.remove(idx);
                apps.insert(0, def);
            }
        }

        apps
    }

    /// Launches `app` with `paths` via `/usr/bin/open -a <app_bundle>`.
    /// Fails with `NotFound` if the `.app` bundle or executable no longer exists.
    pub(super) fn open_with(app: &AppInfo, paths: &[&Path]) -> io::Result<()> {
        let app_path = Path::new(&app.executable);
        if !app_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("application '{}' not found", app.executable),
            ));
        }
        open_with_executable(&app.executable, paths)
    }

    /// Launches `paths` using an explicit `.app` bundle path or executable via `/usr/bin/open -a`.
    pub(super) fn open_with_executable(app_or_exec: &str, paths: &[&Path]) -> io::Result<()> {
        let target = app_or_exec.trim();
        if target.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "application path cannot be empty",
            ));
        }
        if target.contains('/') && !Path::new(target).exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("application '{target}' not found"),
            ));
        }

        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-a").arg(target);
        if !paths.is_empty() {
            cmd.arg("--").args(paths);
        }
        let status = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()?;

        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("open -a '{target}' exited with {status}"),
            ));
        }
        Ok(())
    }
}
