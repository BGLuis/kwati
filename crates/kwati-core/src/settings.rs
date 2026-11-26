use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::places::{default_place_entries, ensure_trash_present, Bookmark, PlaceEntry};
use crate::thumbnail::{
    default_thumbnail_extensions, parse_thumbnail_extensions, split_thumbnail_extensions,
};
use crate::{Location, SortColumn, SortDirection, SortOrder};

pub const CURRENT_VERSION: u32 = 1;
pub const DEFAULT_SHOW_HIDDEN: bool = false;
pub const DEFAULT_SHOW_SIDEBAR: bool = true;
pub const DEFAULT_FOLDERS_FIRST: bool = true;
pub const DEFAULT_HIDDEN_LAST: bool = false;
pub const DEFAULT_REFRESH_INTERVAL_SECS: u32 = 5;
pub const MIN_REFRESH_INTERVAL_SECS: u32 = 1;
pub const MAX_REFRESH_INTERVAL_SECS: u32 = 3600;
pub const DEFAULT_LIST_TIMEOUT_SECS: u32 = 10;
pub const MIN_LIST_TIMEOUT_SECS: u32 = 1;
pub const MAX_LIST_TIMEOUT_SECS: u32 = 3600;
pub const DEFAULT_REMOTE_POLL_INTERVAL_SECS: u32 = 30;
pub const MIN_REMOTE_POLL_INTERVAL_SECS: u32 = 10;
pub const MAX_REMOTE_POLL_INTERVAL_SECS: u32 = 3600;
pub const DEFAULT_REMOTE_SEARCH_DEPTH: u32 = 3;
pub const MIN_REMOTE_SEARCH_DEPTH: u32 = 1;
pub const MAX_REMOTE_SEARCH_DEPTH: u32 = 64;
pub const DEFAULT_REMOTE_SEARCH_PARALLEL: u32 = 2;
pub const MIN_REMOTE_SEARCH_PARALLEL: u32 = 1;
pub const MAX_REMOTE_SEARCH_PARALLEL: u32 = 32;
pub const DEFAULT_THUMBNAILS_ENABLED: bool = true;
pub const DEFAULT_THUMBNAIL_MAX_FILE_SIZE_MB: u32 = 20;
pub const MIN_THUMBNAIL_MAX_FILE_SIZE_MB: u32 = 1;
pub const MAX_THUMBNAIL_MAX_FILE_SIZE_MB: u32 = 1024;

/// A value in the settings file that was ignored in favour of its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsWarning {
    pub key: &'static str,
    pub message: String,
}

/// Reduced-motion preference: follow OS, force full motion, or force reduced motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionPreference {
    #[default]
    System,
    Full,
    Reduced,
}

/// Listing layout mode for directory entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Details,
    CompactList,
    Grid,
}

/// Whether view, sort, and hidden-file changes from the file list or context menu apply only
/// to the active folder or globally to all folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderSettingsScope {
    #[default]
    PerFolder,
    AllFolders,
}

/// Per-folder overrides for listing display and sorting. `None` inherits the global default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FolderPreferences {
    pub view_mode: Option<ViewMode>,
    pub sort_column: Option<SortColumn>,
    pub sort_direction: Option<SortDirection>,
    pub show_hidden: Option<bool>,
    pub folders_first: Option<bool>,
    pub hidden_last: Option<bool>,
}

impl FolderPreferences {
    /// Returns `true` when every field inherits the global default.
    pub fn is_empty(&self) -> bool {
        self.view_mode.is_none()
            && self.sort_column.is_none()
            && self.sort_direction.is_none()
            && self.show_hidden.is_none()
            && self.folders_first.is_none()
            && self.hidden_last.is_none()
    }
}

/// Effective listing settings for a specific folder after applying any per-folder override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedFolderSettings {
    pub view_mode: ViewMode,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub show_hidden: bool,
    pub folders_first: bool,
    pub hidden_last: bool,
}

/// User settings. Features add their own options here, each with a default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub version: u32,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub default_sort: SortOrder,
    pub folders_first: bool,
    pub hidden_last: bool,
    pub view_mode: ViewMode,
    pub folder_settings_scope: FolderSettingsScope,
    pub folder_preferences: BTreeMap<Location, FolderPreferences>,
    pub refresh_interval_secs: u32,
    /// Seconds a directory listing may take before it is abandoned (see `list_dir_bounded`).
    pub list_timeout_secs: u32,
    pub remote_poll_enabled: bool,
    pub remote_poll_interval_secs: u32,
    /// Folder levels a remote search lists (see `RemoteSearchOptions::max_depth`); at least 1.
    pub remote_search_depth: u32,
    /// Listing requests a remote search may have in flight at once; at least 1.
    pub remote_search_parallel: u32,
    pub motion_preference: MotionPreference,
    /// Grid tiles show a picture of the file instead of its icon (see `thumbnail::wants_thumbnail`).
    pub thumbnails_enabled: bool,
    /// Lower-case extensions, without the dot, that get a thumbnail; each one is supported.
    pub thumbnail_extensions: Vec<String>,
    /// Files larger than this are never decoded or downloaded for a thumbnail.
    pub thumbnail_max_file_size_mb: u32,
    /// Remote files are downloaded to the preview cache to build their thumbnail.
    pub thumbnail_remote_enabled: bool,
    pub bookmarks: Vec<Bookmark>,
    pub places: Option<Vec<PlaceEntry>>,
    /// Keys this version does not know, kept so a newer version's file survives a save.
    unknown: Table,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            show_hidden: DEFAULT_SHOW_HIDDEN,
            show_sidebar: DEFAULT_SHOW_SIDEBAR,
            default_sort: SortOrder::default(),
            folders_first: DEFAULT_FOLDERS_FIRST,
            hidden_last: DEFAULT_HIDDEN_LAST,
            view_mode: ViewMode::default(),
            folder_settings_scope: FolderSettingsScope::default(),
            folder_preferences: BTreeMap::new(),
            refresh_interval_secs: DEFAULT_REFRESH_INTERVAL_SECS,
            list_timeout_secs: DEFAULT_LIST_TIMEOUT_SECS,
            remote_poll_enabled: false,
            remote_poll_interval_secs: DEFAULT_REMOTE_POLL_INTERVAL_SECS,
            remote_search_depth: DEFAULT_REMOTE_SEARCH_DEPTH,
            remote_search_parallel: DEFAULT_REMOTE_SEARCH_PARALLEL,
            motion_preference: MotionPreference::default(),
            thumbnails_enabled: DEFAULT_THUMBNAILS_ENABLED,
            thumbnail_extensions: default_thumbnail_extensions(),
            thumbnail_max_file_size_mb: DEFAULT_THUMBNAIL_MAX_FILE_SIZE_MB,
            thumbnail_remote_enabled: false,
            bookmarks: Vec::new(),
            places: None,
            unknown: Table::new(),
        }
    }
}

/// `<config dir>/kwati/settings.toml`, or `None` when the platform has no config dir.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kwati").join("settings.toml"))
}

fn take_u32_or_default(
    table: &mut Table,
    key: &'static str,
    default: u32,
    warnings: &mut Vec<SettingsWarning>,
) -> u32 {
    let Some(value) = table.remove(key) else {
        return default;
    };
    match value.try_into() {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(SettingsWarning {
                key,
                message: err.to_string(),
            });
            default
        }
    }
}

fn take_bool_or_default(
    table: &mut Table,
    key: &'static str,
    default: bool,
    warnings: &mut Vec<SettingsWarning>,
) -> bool {
    let Some(value) = table.remove(key) else {
        return default;
    };
    match value.try_into() {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(SettingsWarning {
                key,
                message: err.to_string(),
            });
            default
        }
    }
}

fn take_bounded_u32_or_default(
    table: &mut Table,
    key: &'static str,
    min: u32,
    max: u32,
    default: u32,
    warnings: &mut Vec<SettingsWarning>,
) -> u32 {
    let Some(value) = table.remove(key) else {
        return default;
    };
    let parsed: u32 = match value.try_into() {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(SettingsWarning {
                key,
                message: err.to_string(),
            });
            return default;
        }
    };
    if parsed < min || parsed > max {
        warnings.push(SettingsWarning {
            key,
            message: format!("value ({parsed}) must be between {min} and {max}"),
        });
        default
    } else {
        parsed
    }
}

/// Keeps the supported extensions of a list and reports the rest, so one unknown entry does not
/// discard the user's whole choice.
fn take_thumbnail_extensions_or_default(
    table: &mut Table,
    default: Vec<String>,
    warnings: &mut Vec<SettingsWarning>,
) -> Vec<String> {
    const KEY: &str = "thumbnail_extensions";
    let Some(value) = table.remove(KEY) else {
        return default;
    };
    let raw: Vec<String> = match value.try_into() {
        Ok(raw) => raw,
        Err(err) => {
            warnings.push(SettingsWarning {
                key: KEY,
                message: err.to_string(),
            });
            return default;
        }
    };
    let (supported, unsupported) = split_thumbnail_extensions(raw.iter().map(String::as_str));
    if !unsupported.is_empty() {
        warnings.push(SettingsWarning {
            key: KEY,
            message: format!("unsupported extensions ignored: {}", unsupported.join(", ")),
        });
    }
    supported
}

fn parse_sort_column(raw: &str) -> Option<SortColumn> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "name" => Some(SortColumn::Name),
        "size" => Some(SortColumn::Size),
        "date" | "modified" => Some(SortColumn::Date),
        "created" => Some(SortColumn::Created),
        "accessed" => Some(SortColumn::Accessed),
        "type" => Some(SortColumn::Type),
        _ => None,
    }
}

fn parse_sort_direction(raw: &str) -> Option<SortDirection> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ascending" | "asc" => Some(SortDirection::Ascending),
        "descending" | "desc" => Some(SortDirection::Descending),
        _ => None,
    }
}

fn sort_column_str(column: SortColumn) -> &'static str {
    match column {
        SortColumn::Name => "name",
        SortColumn::Size => "size",
        SortColumn::Date => "date",
        SortColumn::Created => "created",
        SortColumn::Accessed => "accessed",
        SortColumn::Type => "type",
    }
}

fn sort_direction_str(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "ascending",
        SortDirection::Descending => "descending",
    }
}

fn parse_motion_preference(raw: &str) -> Option<MotionPreference> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "system" | "auto" => Some(MotionPreference::System),
        "full" | "enabled" => Some(MotionPreference::Full),
        "reduced" | "off" => Some(MotionPreference::Reduced),
        _ => None,
    }
}

fn motion_preference_str(pref: MotionPreference) -> &'static str {
    match pref {
        MotionPreference::System => "system",
        MotionPreference::Full => "full",
        MotionPreference::Reduced => "reduced",
    }
}

fn parse_view_mode(raw: &str) -> Option<ViewMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "details" | "list" => Some(ViewMode::Details),
        "compact_list" | "compact-list" | "compact" => Some(ViewMode::CompactList),
        "grid" => Some(ViewMode::Grid),
        _ => None,
    }
}

fn view_mode_str(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Details => "details",
        ViewMode::CompactList => "compact_list",
        ViewMode::Grid => "grid",
    }
}

fn parse_folder_settings_scope(raw: &str) -> Option<FolderSettingsScope> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "per_folder" | "per-folder" | "folder" | "current" => Some(FolderSettingsScope::PerFolder),
        "all_folders" | "all-folders" | "all" | "global" => Some(FolderSettingsScope::AllFolders),
        _ => None,
    }
}

fn folder_settings_scope_str(scope: FolderSettingsScope) -> &'static str {
    match scope {
        FolderSettingsScope::PerFolder => "per_folder",
        FolderSettingsScope::AllFolders => "all_folders",
    }
}

fn take_folder_settings_scope_or_default(
    table: &mut Table,
    default: FolderSettingsScope,
    warnings: &mut Vec<SettingsWarning>,
) -> FolderSettingsScope {
    let Some(value) = table.remove("folder_settings_scope") else {
        return default;
    };
    match value {
        Value::String(raw) => match parse_folder_settings_scope(&raw) {
            Some(scope) => scope,
            None => {
                warnings.push(SettingsWarning {
                    key: "folder_settings_scope",
                    message: format!("unknown folder_settings_scope '{raw}'"),
                });
                default
            }
        },
        _ => {
            warnings.push(SettingsWarning {
                key: "folder_settings_scope",
                message: "folder_settings_scope must be a string".into(),
            });
            default
        }
    }
}

fn take_folder_preferences(
    table: &mut Table,
    warnings: &mut Vec<SettingsWarning>,
) -> BTreeMap<Location, FolderPreferences> {
    let mut map = BTreeMap::new();
    let Some(val) = table.remove("folder_preferences") else {
        return map;
    };
    let Value::Array(items) = val else {
        warnings.push(SettingsWarning {
            key: "folder_preferences",
            message: "folder_preferences must be an array of tables".into(),
        });
        return map;
    };

    for (idx, item) in items.into_iter().enumerate() {
        let Value::Table(mut t) = item else {
            warnings.push(SettingsWarning {
                key: "folder_preferences",
                message: format!("folder preference entry at index {idx} is not a table"),
            });
            continue;
        };

        let kind = t.remove("kind").and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        });
        let path = t.remove("path").and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(PathBuf::from(s)),
            _ => None,
        });

        let location = match (kind.as_deref(), path) {
            (Some("local") | None, Some(path)) => Location::Local(path),
            (Some("remote"), Some(path)) => {
                let profile_id = t.remove("profile_id").and_then(|v| match v {
                    Value::String(s) if !s.is_empty() => Some(s),
                    _ => None,
                });
                match profile_id {
                    Some(profile_id) => Location::Remote { profile_id, path },
                    None => {
                        warnings.push(SettingsWarning {
                            key: "folder_preferences",
                            message: format!(
                                "remote folder preference at index {idx} is missing profile_id"
                            ),
                        });
                        continue;
                    }
                }
            }
            (Some(other), _) if other != "local" && other != "remote" => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("unknown folder preference kind '{other}' at index {idx}"),
                });
                continue;
            }
            _ => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("invalid folder preference entry at index {idx}"),
                });
                continue;
            }
        };

        let view_mode = match t.remove("view_mode") {
            None => None,
            Some(Value::String(raw)) => match parse_view_mode(&raw) {
                Some(mode) => Some(mode),
                None => {
                    warnings.push(SettingsWarning {
                        key: "folder_preferences",
                        message: format!("unknown view_mode '{raw}' at index {idx}"),
                    });
                    None
                }
            },
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("view_mode at index {idx} must be a string"),
                });
                None
            }
        };

        let sort_column = match t.remove("sort_column") {
            None => None,
            Some(Value::String(raw)) => match parse_sort_column(&raw) {
                Some(col) => Some(col),
                None => {
                    warnings.push(SettingsWarning {
                        key: "folder_preferences",
                        message: format!("unknown sort_column '{raw}' at index {idx}"),
                    });
                    None
                }
            },
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("sort_column at index {idx} must be a string"),
                });
                None
            }
        };

        let sort_direction = match t.remove("sort_direction") {
            None => None,
            Some(Value::String(raw)) => match parse_sort_direction(&raw) {
                Some(dir) => Some(dir),
                None => {
                    warnings.push(SettingsWarning {
                        key: "folder_preferences",
                        message: format!("unknown sort_direction '{raw}' at index {idx}"),
                    });
                    None
                }
            },
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("sort_direction at index {idx} must be a string"),
                });
                None
            }
        };

        let show_hidden = match t.remove("show_hidden") {
            None => None,
            Some(Value::Boolean(b)) => Some(b),
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("show_hidden at index {idx} must be a boolean"),
                });
                None
            }
        };

        let folders_first = match t.remove("folders_first") {
            None => None,
            Some(Value::Boolean(b)) => Some(b),
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("folders_first at index {idx} must be a boolean"),
                });
                None
            }
        };

        let hidden_last = match t.remove("hidden_last") {
            None => None,
            Some(Value::Boolean(b)) => Some(b),
            Some(_) => {
                warnings.push(SettingsWarning {
                    key: "folder_preferences",
                    message: format!("hidden_last at index {idx} must be a boolean"),
                });
                None
            }
        };

        let prefs = FolderPreferences {
            view_mode,
            sort_column,
            sort_direction,
            show_hidden,
            folders_first,
            hidden_last,
        };
        if !prefs.is_empty() {
            map.insert(location, prefs);
        }
    }

    map
}

fn take_view_mode_or_default(
    table: &mut Table,
    default: ViewMode,
    warnings: &mut Vec<SettingsWarning>,
) -> ViewMode {
    let Some(value) = table.remove("view_mode") else {
        return default;
    };
    match value {
        Value::String(raw) => match parse_view_mode(&raw) {
            Some(mode) => mode,
            None => {
                warnings.push(SettingsWarning {
                    key: "view_mode",
                    message: format!("unknown view_mode '{raw}'"),
                });
                default
            }
        },
        _ => {
            warnings.push(SettingsWarning {
                key: "view_mode",
                message: "view_mode must be a string".into(),
            });
            default
        }
    }
}

fn take_motion_preference_or_default(
    table: &mut Table,
    default: MotionPreference,
    warnings: &mut Vec<SettingsWarning>,
) -> MotionPreference {
    let Some(value) = table.remove("motion_preference") else {
        return default;
    };
    match value {
        Value::String(raw) => match parse_motion_preference(&raw) {
            Some(pref) => pref,
            None => {
                warnings.push(SettingsWarning {
                    key: "motion_preference",
                    message: format!("unknown motion_preference '{raw}'"),
                });
                default
            }
        },
        _ => {
            warnings.push(SettingsWarning {
                key: "motion_preference",
                message: "motion_preference must be a string".into(),
            });
            default
        }
    }
}

fn take_default_sort_or_default(
    table: &mut Table,
    default: SortOrder,
    warnings: &mut Vec<SettingsWarning>,
) -> SortOrder {
    let Some(value) = table.remove("default_sort") else {
        return default;
    };
    let Value::Table(mut sort_table) = value else {
        warnings.push(SettingsWarning {
            key: "default_sort",
            message: "default_sort must be a table".into(),
        });
        return default;
    };

    let column = match sort_table.remove("column") {
        None => default.column,
        Some(Value::String(raw)) => match parse_sort_column(&raw) {
            Some(col) => col,
            None => {
                warnings.push(SettingsWarning {
                    key: "default_sort",
                    message: format!("unknown sort column '{raw}'"),
                });
                default.column
            }
        },
        Some(_) => {
            warnings.push(SettingsWarning {
                key: "default_sort",
                message: "default_sort.column must be a string".into(),
            });
            default.column
        }
    };

    let direction = match sort_table.remove("direction") {
        None => default.direction,
        Some(Value::String(raw)) => match parse_sort_direction(&raw) {
            Some(dir) => dir,
            None => {
                warnings.push(SettingsWarning {
                    key: "default_sort",
                    message: format!("unknown sort direction '{raw}'"),
                });
                default.direction
            }
        },
        Some(_) => {
            warnings.push(SettingsWarning {
                key: "default_sort",
                message: "default_sort.direction must be a string".into(),
            });
            default.direction
        }
    };

    SortOrder::new(column, direction)
}

/// Writes `text` to a temporary file beside `path`, then renames it over `path`.
/// `between_write_and_rename` runs in between, so tests can simulate a crash.
pub(crate) fn write_atomic(
    path: &Path,
    text: &str,
    between_write_and_rename: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = path.with_file_name(tmp_name);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let result = fs::write(&tmp_path, text)
        .and_then(|()| between_write_and_rename())
        .and_then(|()| fs::rename(&tmp_path, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

impl Settings {
    /// Sets the local/mount refresh interval in seconds (`1..=3600`).
    pub fn set_refresh_interval_secs(&mut self, secs: u32) -> Result<(), &'static str> {
        if !(MIN_REFRESH_INTERVAL_SECS..=MAX_REFRESH_INTERVAL_SECS).contains(&secs) {
            Err("refresh interval must be between 1 and 3600 seconds")
        } else {
            self.refresh_interval_secs = secs;
            Ok(())
        }
    }

    /// Sets the bounded listing timeout in seconds (`1..=3600`).
    pub fn set_list_timeout_secs(&mut self, secs: u32) -> Result<(), &'static str> {
        if !(MIN_LIST_TIMEOUT_SECS..=MAX_LIST_TIMEOUT_SECS).contains(&secs) {
            Err("list timeout must be between 1 and 3600 seconds")
        } else {
            self.list_timeout_secs = secs;
            Ok(())
        }
    }

    /// Sets the remote polling interval in seconds (`10..=3600`).
    pub fn set_remote_poll_interval_secs(&mut self, secs: u32) -> Result<(), &'static str> {
        if !(MIN_REMOTE_POLL_INTERVAL_SECS..=MAX_REMOTE_POLL_INTERVAL_SECS).contains(&secs) {
            Err("remote poll interval must be between 10 and 3600 seconds")
        } else {
            self.remote_poll_interval_secs = secs;
            Ok(())
        }
    }

    /// Sets the maximum remote search depth (`1..=64`).
    pub fn set_remote_search_depth(&mut self, depth: u32) -> Result<(), &'static str> {
        if !(MIN_REMOTE_SEARCH_DEPTH..=MAX_REMOTE_SEARCH_DEPTH).contains(&depth) {
            Err("remote search depth must be between 1 and 64")
        } else {
            self.remote_search_depth = depth;
            Ok(())
        }
    }

    /// Sets the remote search parallelism (`1..=32`).
    pub fn set_remote_search_parallel(&mut self, parallel: u32) -> Result<(), &'static str> {
        if !(MIN_REMOTE_SEARCH_PARALLEL..=MAX_REMOTE_SEARCH_PARALLEL).contains(&parallel) {
            Err("remote search parallel requests must be between 1 and 32")
        } else {
            self.remote_search_parallel = parallel;
            Ok(())
        }
    }

    /// Sets the largest file size, in MiB, that gets a thumbnail (`1..=1024`).
    pub fn set_thumbnail_max_file_size_mb(&mut self, mb: u32) -> Result<(), &'static str> {
        if !(MIN_THUMBNAIL_MAX_FILE_SIZE_MB..=MAX_THUMBNAIL_MAX_FILE_SIZE_MB).contains(&mb) {
            Err("thumbnail size limit must be between 1 and 1024 MB")
        } else {
            self.thumbnail_max_file_size_mb = mb;
            Ok(())
        }
    }

    /// Replaces the thumbnail extensions with a comma- or space-separated list such as
    /// `"png, .JPG pdf"`. Fails without changing anything, naming the unsupported entries.
    pub fn set_thumbnail_extensions(&mut self, raw: &str) -> Result<(), Vec<String>> {
        self.thumbnail_extensions = parse_thumbnail_extensions(raw)?;
        Ok(())
    }

    /// Validates all numeric bounds in `self`.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(MIN_REFRESH_INTERVAL_SECS..=MAX_REFRESH_INTERVAL_SECS)
            .contains(&self.refresh_interval_secs)
        {
            return Err("refresh interval must be between 1 and 3600 seconds");
        }
        if !(MIN_LIST_TIMEOUT_SECS..=MAX_LIST_TIMEOUT_SECS).contains(&self.list_timeout_secs) {
            return Err("list timeout must be between 1 and 3600 seconds");
        }
        if !(MIN_REMOTE_POLL_INTERVAL_SECS..=MAX_REMOTE_POLL_INTERVAL_SECS)
            .contains(&self.remote_poll_interval_secs)
        {
            return Err("remote poll interval must be between 10 and 3600 seconds");
        }
        if !(MIN_REMOTE_SEARCH_DEPTH..=MAX_REMOTE_SEARCH_DEPTH).contains(&self.remote_search_depth)
        {
            return Err("remote search depth must be between 1 and 64");
        }
        if !(MIN_REMOTE_SEARCH_PARALLEL..=MAX_REMOTE_SEARCH_PARALLEL)
            .contains(&self.remote_search_parallel)
        {
            return Err("remote search parallel requests must be between 1 and 32");
        }
        if !(MIN_THUMBNAIL_MAX_FILE_SIZE_MB..=MAX_THUMBNAIL_MAX_FILE_SIZE_MB)
            .contains(&self.thumbnail_max_file_size_mb)
        {
            return Err("thumbnail size limit must be between 1 and 1024 MB");
        }
        Ok(())
    }

    /// Reads settings from `path`. A missing file yields the defaults; an invalid value falls
    /// back to its default and is reported. A file that is not valid TOML is an error, so the
    /// caller never overwrites it by accident.
    pub fn load_from(path: &Path) -> io::Result<(Self, Vec<SettingsWarning>)> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), Vec::new()));
            }
            Err(err) => return Err(err),
        };
        let mut table: Table = text
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let defaults = Self::default();
        let mut warnings = Vec::new();
        let version = take_u32_or_default(&mut table, "version", defaults.version, &mut warnings);
        let show_hidden = take_bool_or_default(
            &mut table,
            "show_hidden",
            defaults.show_hidden,
            &mut warnings,
        );
        let show_sidebar = take_bool_or_default(
            &mut table,
            "show_sidebar",
            defaults.show_sidebar,
            &mut warnings,
        );
        let default_sort =
            take_default_sort_or_default(&mut table, defaults.default_sort, &mut warnings);
        let folders_first = take_bool_or_default(
            &mut table,
            "folders_first",
            defaults.folders_first,
            &mut warnings,
        );
        let hidden_last = take_bool_or_default(
            &mut table,
            "hidden_last",
            defaults.hidden_last,
            &mut warnings,
        );
        let view_mode = take_view_mode_or_default(&mut table, defaults.view_mode, &mut warnings);
        let folder_settings_scope = take_folder_settings_scope_or_default(
            &mut table,
            defaults.folder_settings_scope,
            &mut warnings,
        );
        let folder_preferences = take_folder_preferences(&mut table, &mut warnings);
        let refresh_interval_secs = take_bounded_u32_or_default(
            &mut table,
            "refresh_interval_secs",
            MIN_REFRESH_INTERVAL_SECS,
            MAX_REFRESH_INTERVAL_SECS,
            defaults.refresh_interval_secs,
            &mut warnings,
        );
        let list_timeout_secs = take_bounded_u32_or_default(
            &mut table,
            "list_timeout_secs",
            MIN_LIST_TIMEOUT_SECS,
            MAX_LIST_TIMEOUT_SECS,
            defaults.list_timeout_secs,
            &mut warnings,
        );
        let remote_poll_enabled = take_bool_or_default(
            &mut table,
            "remote_poll_enabled",
            defaults.remote_poll_enabled,
            &mut warnings,
        );
        let remote_poll_interval_secs = take_bounded_u32_or_default(
            &mut table,
            "remote_poll_interval_secs",
            MIN_REMOTE_POLL_INTERVAL_SECS,
            MAX_REMOTE_POLL_INTERVAL_SECS,
            defaults.remote_poll_interval_secs,
            &mut warnings,
        );

        let remote_search_depth = take_bounded_u32_or_default(
            &mut table,
            "remote_search_depth",
            MIN_REMOTE_SEARCH_DEPTH,
            MAX_REMOTE_SEARCH_DEPTH,
            defaults.remote_search_depth,
            &mut warnings,
        );
        let remote_search_parallel = take_bounded_u32_or_default(
            &mut table,
            "remote_search_parallel",
            MIN_REMOTE_SEARCH_PARALLEL,
            MAX_REMOTE_SEARCH_PARALLEL,
            defaults.remote_search_parallel,
            &mut warnings,
        );
        let motion_preference = take_motion_preference_or_default(
            &mut table,
            defaults.motion_preference,
            &mut warnings,
        );
        let thumbnails_enabled = take_bool_or_default(
            &mut table,
            "thumbnails_enabled",
            defaults.thumbnails_enabled,
            &mut warnings,
        );
        let thumbnail_extensions = take_thumbnail_extensions_or_default(
            &mut table,
            defaults.thumbnail_extensions.clone(),
            &mut warnings,
        );
        let thumbnail_max_file_size_mb = take_bounded_u32_or_default(
            &mut table,
            "thumbnail_max_file_size_mb",
            MIN_THUMBNAIL_MAX_FILE_SIZE_MB,
            MAX_THUMBNAIL_MAX_FILE_SIZE_MB,
            defaults.thumbnail_max_file_size_mb,
            &mut warnings,
        );
        let thumbnail_remote_enabled = take_bool_or_default(
            &mut table,
            "thumbnail_remote_enabled",
            defaults.thumbnail_remote_enabled,
            &mut warnings,
        );

        let mut bookmarks = Vec::new();
        if let Some(val) = table.remove("bookmarks") {
            match val {
                Value::Array(items) => {
                    for (idx, item) in items.into_iter().enumerate() {
                        if let Value::Table(mut t) = item {
                            let name = t.remove("name").and_then(|v| match v {
                                Value::String(s) => Some(s),
                                _ => None,
                            });
                            let path = t.remove("path").and_then(|v| match v {
                                Value::String(s) => Some(PathBuf::from(s)),
                                _ => None,
                            });
                            match (name, path) {
                                (Some(name), Some(path)) => {
                                    bookmarks.push(Bookmark::new(name, path));
                                }
                                _ => {
                                    warnings.push(SettingsWarning {
                                        key: "bookmarks",
                                        message: format!("invalid bookmark entry at index {}", idx),
                                    });
                                }
                            }
                        } else {
                            warnings.push(SettingsWarning {
                                key: "bookmarks",
                                message: format!("bookmark entry at index {} is not a table", idx),
                            });
                        }
                    }
                }
                _ => {
                    warnings.push(SettingsWarning {
                        key: "bookmarks",
                        message: "bookmarks must be an array of tables".into(),
                    });
                }
            }
        }

        let mut places = None;
        if let Some(val) = table.remove("places") {
            match val {
                Value::Array(items) => {
                    let mut parsed = Vec::with_capacity(items.len());
                    for (idx, item) in items.into_iter().enumerate() {
                        if let Value::Table(mut t) = item {
                            let kind = t.remove("kind").and_then(|v| match v {
                                Value::String(s) => Some(s),
                                _ => None,
                            });
                            match kind.as_deref() {
                                Some("trash") => parsed.push(PlaceEntry::Trash),
                                Some("folder") | None => {
                                    let name = t.remove("name").and_then(|v| match v {
                                        Value::String(s) => Some(s),
                                        _ => None,
                                    });
                                    let path = t.remove("path").and_then(|v| match v {
                                        Value::String(s) => Some(PathBuf::from(s)),
                                        _ => None,
                                    });
                                    match (name, path) {
                                        (Some(name), Some(path)) => {
                                            parsed.push(PlaceEntry::folder(name, path));
                                        }
                                        _ => {
                                            warnings.push(SettingsWarning {
                                                key: "places",
                                                message: format!(
                                                    "invalid place entry at index {}",
                                                    idx
                                                ),
                                            });
                                        }
                                    }
                                }
                                Some(other) => {
                                    warnings.push(SettingsWarning {
                                        key: "places",
                                        message: format!(
                                            "unknown place kind '{}' at index {}",
                                            other, idx
                                        ),
                                    });
                                }
                            }
                        } else {
                            warnings.push(SettingsWarning {
                                key: "places",
                                message: format!("place entry at index {} is not a table", idx),
                            });
                        }
                    }
                    ensure_trash_present(&mut parsed);
                    places = Some(parsed);
                }
                _ => {
                    warnings.push(SettingsWarning {
                        key: "places",
                        message: "places must be an array of tables".into(),
                    });
                }
            }
        }

        let settings = Self {
            version,
            show_hidden,
            show_sidebar,
            default_sort,
            folders_first,
            hidden_last,
            view_mode,
            folder_settings_scope,
            folder_preferences,
            refresh_interval_secs,
            list_timeout_secs,
            remote_poll_enabled,
            remote_poll_interval_secs,
            remote_search_depth,
            remote_search_parallel,
            motion_preference,
            thumbnails_enabled,
            thumbnail_extensions,
            thumbnail_max_file_size_mb,
            thumbnail_remote_enabled,
            bookmarks,
            places,
            unknown: table,
        };
        Ok((settings, warnings))
    }

    /// Resolves the effective display and sorting settings for `location`, applying any per-folder
    /// overrides on top of the global defaults.
    pub fn resolve_folder_settings(&self, location: &Location) -> ResolvedFolderSettings {
        let prefs = self.folder_preferences.get(location);
        ResolvedFolderSettings {
            view_mode: prefs.and_then(|p| p.view_mode).unwrap_or(self.view_mode),
            sort_column: prefs
                .and_then(|p| p.sort_column)
                .unwrap_or(self.default_sort.column),
            sort_direction: prefs
                .and_then(|p| p.sort_direction)
                .unwrap_or(self.default_sort.direction),
            show_hidden: prefs
                .and_then(|p| p.show_hidden)
                .unwrap_or(self.show_hidden),
            folders_first: prefs
                .and_then(|p| p.folders_first)
                .unwrap_or(self.folders_first),
            hidden_last: prefs
                .and_then(|p| p.hidden_last)
                .unwrap_or(self.hidden_last),
        }
    }

    /// Returns `true` if `location` has at least one per-folder override stored.
    pub fn has_folder_override(&self, location: &Location) -> bool {
        self.folder_preferences
            .get(location)
            .is_some_and(|p| !p.is_empty())
    }

    fn update_folder_preference(
        &mut self,
        location: &Location,
        update: impl FnOnce(&mut FolderPreferences),
    ) {
        let mut prefs = self
            .folder_preferences
            .get(location)
            .copied()
            .unwrap_or_default();
        update(&mut prefs);
        if prefs.is_empty() {
            self.folder_preferences.remove(location);
        } else {
            self.folder_preferences.insert(location.clone(), prefs);
        }
    }

    /// Stores a per-folder view mode override for `location`.
    pub fn set_folder_view_mode(&mut self, location: &Location, mode: ViewMode) {
        self.update_folder_preference(location, |p| p.view_mode = Some(mode));
    }

    /// Stores a per-folder sort column and direction override for `location`.
    pub fn set_folder_sort(
        &mut self,
        location: &Location,
        column: SortColumn,
        direction: SortDirection,
    ) {
        self.update_folder_preference(location, |p| {
            p.sort_column = Some(column);
            p.sort_direction = Some(direction);
        });
    }

    /// Stores a per-folder hidden-files override for `location`.
    pub fn set_folder_show_hidden(&mut self, location: &Location, show: bool) {
        self.update_folder_preference(location, |p| p.show_hidden = Some(show));
    }

    /// Stores a per-folder folders-first override for `location`.
    pub fn set_folder_folders_first(&mut self, location: &Location, folders_first: bool) {
        self.update_folder_preference(location, |p| p.folders_first = Some(folders_first));
    }

    /// Stores a per-folder hidden-last override for `location`.
    pub fn set_folder_hidden_last(&mut self, location: &Location, hidden_last: bool) {
        self.update_folder_preference(location, |p| p.hidden_last = Some(hidden_last));
    }

    /// Clears all per-folder `view_mode` overrides (used when applying a global change).
    pub fn clear_all_folder_view_mode_overrides(&mut self) {
        self.folder_preferences.retain(|_, p| {
            p.view_mode = None;
            !p.is_empty()
        });
    }

    /// Clears all per-folder sort overrides (used when applying a global change).
    pub fn clear_all_folder_sort_overrides(&mut self) {
        self.folder_preferences.retain(|_, p| {
            p.sort_column = None;
            p.sort_direction = None;
            !p.is_empty()
        });
    }

    /// Clears all per-folder `show_hidden` overrides (used when applying a global change).
    pub fn clear_all_folder_show_hidden_overrides(&mut self) {
        self.folder_preferences.retain(|_, p| {
            p.show_hidden = None;
            !p.is_empty()
        });
    }

    /// Clears all per-folder `folders_first` overrides (used when applying a global change).
    pub fn clear_all_folder_folders_first_overrides(&mut self) {
        self.folder_preferences.retain(|_, p| {
            p.folders_first = None;
            !p.is_empty()
        });
    }

    /// Clears all per-folder `hidden_last` overrides (used when applying a global change).
    pub fn clear_all_folder_hidden_last_overrides(&mut self) {
        self.folder_preferences.retain(|_, p| {
            p.hidden_last = None;
            !p.is_empty()
        });
    }

    /// Applies `resolved` as the global defaults and clears all per-folder overrides.
    pub fn apply_folder_settings_to_all(&mut self, resolved: ResolvedFolderSettings) {
        self.view_mode = resolved.view_mode;
        self.default_sort = SortOrder::new(resolved.sort_column, resolved.sort_direction);
        self.show_hidden = resolved.show_hidden;
        self.folders_first = resolved.folders_first;
        self.hidden_last = resolved.hidden_last;
        self.folder_preferences.clear();
    }

    /// Atomically replaces the file at `path`: writes a temporary file in the same folder, then
    /// renames it over the target.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        self.save_with_hook(path, || Ok(()))
    }

    /// Appends a bookmark.
    pub fn add_bookmark(&mut self, bookmark: impl Into<Bookmark>) {
        self.bookmarks.push(bookmark.into());
    }

    /// Renames the bookmark at `index` to `new_name`. Returns `true` if index was valid.
    pub fn rename_bookmark(&mut self, index: usize, new_name: impl Into<String>) -> bool {
        if let Some(b) = self.bookmarks.get_mut(index) {
            b.name = new_name.into();
            true
        } else {
            false
        }
    }

    /// Renames the first bookmark with matching `path` to `new_name`. Returns `true` if found.
    pub fn rename_bookmark_by_path(&mut self, path: &Path, new_name: impl Into<String>) -> bool {
        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.path == path) {
            b.name = new_name.into();
            true
        } else {
            false
        }
    }

    /// Removes the bookmark at `index`. Returns the removed bookmark if index was valid.
    pub fn remove_bookmark(&mut self, index: usize) -> Option<Bookmark> {
        if index < self.bookmarks.len() {
            Some(self.bookmarks.remove(index))
        } else {
            None
        }
    }

    /// Removes all bookmarks matching `path`. Returns `true` if any was removed.
    pub fn remove_bookmark_by_path(&mut self, path: &Path) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.path != path);
        self.bookmarks.len() < before
    }

    /// Reorders a bookmark from index `from` to index `to`. Returns `true` if successful.
    pub fn reorder_bookmarks(&mut self, from: usize, to: usize) -> bool {
        if from >= self.bookmarks.len() || to >= self.bookmarks.len() {
            return false;
        }
        let item = self.bookmarks.remove(from);
        self.bookmarks.insert(to, item);
        true
    }

    /// Returns the unified sidebar Places list, migrating from standard places + legacy
    /// bookmarks when `places` has not yet been customized, and guaranteeing Trash is present.
    pub fn resolved_places(&self) -> Vec<PlaceEntry> {
        match &self.places {
            Some(entries) => {
                let mut list = entries.clone();
                ensure_trash_present(&mut list);
                list
            }
            None => default_place_entries(&self.bookmarks),
        }
    }

    fn ensure_places_mut(&mut self) -> &mut Vec<PlaceEntry> {
        if self.places.is_none() {
            self.places = Some(default_place_entries(&self.bookmarks));
        }
        let list = self.places.as_mut().expect("initialized above");
        ensure_trash_present(list);
        list
    }

    /// Adds a folder to the unified Places list unless `path` is already pinned.
    pub fn add_place(&mut self, name: impl Into<String>, path: PathBuf) -> bool {
        let list = self.ensure_places_mut();
        if list.iter().any(|e| e.path().is_some_and(|p| p == path)) {
            return false;
        }
        list.push(PlaceEntry::folder(name, path));
        true
    }

    /// Renames the folder place at `index`. Returns `false` if `index` is out of range or Trash.
    pub fn rename_place(&mut self, index: usize, new_name: impl Into<String>) -> bool {
        let list = self.ensure_places_mut();
        match list.get_mut(index) {
            Some(PlaceEntry::Folder { name, .. }) => {
                *name = new_name.into();
                true
            }
            _ => false,
        }
    }

    /// Removes the folder place at `index`. Trash cannot be removed.
    pub fn remove_place(&mut self, index: usize) -> Result<PlaceEntry, &'static str> {
        let list = self.ensure_places_mut();
        let Some(entry) = list.get(index) else {
            return Err("Place index out of range.");
        };
        if entry.is_trash() {
            return Err("Trash cannot be removed.");
        }
        Ok(list.remove(index))
    }

    /// Moves any place (including Trash) from `from` to `to`.
    pub fn reorder_places(&mut self, from: usize, to: usize) -> bool {
        let list = self.ensure_places_mut();
        if from >= list.len() || to >= list.len() {
            return false;
        }
        let item = list.remove(from);
        list.insert(to, item);
        true
    }

    /// `between_write_and_rename` runs after the temporary file is written, letting tests
    /// simulate a crash at the point where the old file must still be intact.
    fn save_with_hook(
        &self,
        path: &Path,
        between_write_and_rename: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let mut table = self.unknown.clone();
        table.insert("version".into(), Value::Integer(self.version.into()));
        table.insert("show_hidden".into(), Value::Boolean(self.show_hidden));
        table.insert("show_sidebar".into(), Value::Boolean(self.show_sidebar));
        let mut sort_table = Table::new();
        sort_table.insert(
            "column".into(),
            Value::String(sort_column_str(self.default_sort.column).into()),
        );
        sort_table.insert(
            "direction".into(),
            Value::String(sort_direction_str(self.default_sort.direction).into()),
        );
        table.insert("default_sort".into(), Value::Table(sort_table));
        table.insert("folders_first".into(), Value::Boolean(self.folders_first));
        table.insert("hidden_last".into(), Value::Boolean(self.hidden_last));
        table.insert(
            "view_mode".into(),
            Value::String(view_mode_str(self.view_mode).into()),
        );
        table.insert(
            "folder_settings_scope".into(),
            Value::String(folder_settings_scope_str(self.folder_settings_scope).into()),
        );
        if !self.folder_preferences.is_empty() {
            let folder_tables: Vec<Value> = self
                .folder_preferences
                .iter()
                .filter(|(_, p)| !p.is_empty())
                .map(|(loc, prefs)| {
                    let mut t = Table::new();
                    match loc {
                        Location::Local(path) => {
                            t.insert("kind".into(), Value::String("local".into()));
                            t.insert(
                                "path".into(),
                                Value::String(path.to_string_lossy().to_string()),
                            );
                        }
                        Location::Remote { profile_id, path } => {
                            t.insert("kind".into(), Value::String("remote".into()));
                            t.insert("profile_id".into(), Value::String(profile_id.clone()));
                            t.insert(
                                "path".into(),
                                Value::String(path.to_string_lossy().to_string()),
                            );
                        }
                    }
                    if let Some(mode) = prefs.view_mode {
                        t.insert(
                            "view_mode".into(),
                            Value::String(view_mode_str(mode).into()),
                        );
                    }
                    if let Some(col) = prefs.sort_column {
                        t.insert(
                            "sort_column".into(),
                            Value::String(sort_column_str(col).into()),
                        );
                    }
                    if let Some(dir) = prefs.sort_direction {
                        t.insert(
                            "sort_direction".into(),
                            Value::String(sort_direction_str(dir).into()),
                        );
                    }
                    if let Some(show) = prefs.show_hidden {
                        t.insert("show_hidden".into(), Value::Boolean(show));
                    }
                    if let Some(folders_first) = prefs.folders_first {
                        t.insert("folders_first".into(), Value::Boolean(folders_first));
                    }
                    if let Some(hidden_last) = prefs.hidden_last {
                        t.insert("hidden_last".into(), Value::Boolean(hidden_last));
                    }
                    Value::Table(t)
                })
                .collect();
            if !folder_tables.is_empty() {
                table.insert("folder_preferences".into(), Value::Array(folder_tables));
            }
        }
        table.insert(
            "refresh_interval_secs".into(),
            Value::Integer(self.refresh_interval_secs.into()),
        );
        table.insert(
            "list_timeout_secs".into(),
            Value::Integer(self.list_timeout_secs.into()),
        );
        table.insert(
            "remote_poll_enabled".into(),
            Value::Boolean(self.remote_poll_enabled),
        );
        table.insert(
            "remote_poll_interval_secs".into(),
            Value::Integer(self.remote_poll_interval_secs.into()),
        );
        table.insert(
            "remote_search_depth".into(),
            Value::Integer(self.remote_search_depth.into()),
        );
        table.insert(
            "remote_search_parallel".into(),
            Value::Integer(self.remote_search_parallel.into()),
        );
        table.insert(
            "motion_preference".into(),
            Value::String(motion_preference_str(self.motion_preference).into()),
        );
        table.insert(
            "thumbnails_enabled".into(),
            Value::Boolean(self.thumbnails_enabled),
        );
        table.insert(
            "thumbnail_extensions".into(),
            Value::Array(
                self.thumbnail_extensions
                    .iter()
                    .map(|ext| Value::String(ext.clone()))
                    .collect(),
            ),
        );
        table.insert(
            "thumbnail_max_file_size_mb".into(),
            Value::Integer(self.thumbnail_max_file_size_mb.into()),
        );
        table.insert(
            "thumbnail_remote_enabled".into(),
            Value::Boolean(self.thumbnail_remote_enabled),
        );
        let bookmark_tables: Vec<Value> = self
            .bookmarks
            .iter()
            .map(|b| {
                let mut t = Table::new();
                t.insert("name".into(), Value::String(b.name.clone()));
                t.insert(
                    "path".into(),
                    Value::String(b.path.to_string_lossy().to_string()),
                );
                Value::Table(t)
            })
            .collect();
        table.insert("bookmarks".into(), Value::Array(bookmark_tables));
        if let Some(places) = &self.places {
            let place_tables: Vec<Value> = places
                .iter()
                .map(|entry| {
                    let mut t = Table::new();
                    match entry {
                        PlaceEntry::Trash => {
                            t.insert("kind".into(), Value::String("trash".into()));
                        }
                        PlaceEntry::Folder { name, path } => {
                            t.insert("kind".into(), Value::String("folder".into()));
                            t.insert("name".into(), Value::String(name.clone()));
                            t.insert(
                                "path".into(),
                                Value::String(path.to_string_lossy().to_string()),
                            );
                        }
                    }
                    Value::Table(t)
                })
                .collect();
            table.insert("places".into(), Value::Array(place_tables));
        }
        let text = toml::to_string(&table)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        write_atomic(path, &text, between_write_and_rename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "kwati_test_{}_{}_{}",
                name,
                std::process::id(),
                id
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("failed to create test dir");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn settings_missing_file() {
        let dir = TestDir::new("settings_missing_file");
        let (settings, warnings) = Settings::load_from(&dir.path.join("settings.toml")).unwrap();
        assert_eq!(settings, Settings::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn settings_invalid_value() {
        let dir = TestDir::new("settings_invalid_value");
        let path = dir.path.join("settings.toml");
        fs::write(&path, "version = 7\nrefresh_interval_secs = \"soon\"\n").unwrap();

        let (settings, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(
            settings.refresh_interval_secs,
            DEFAULT_REFRESH_INTERVAL_SECS
        );
        assert_eq!(settings.version, 7);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "refresh_interval_secs");
    }

    #[test]
    fn settings_remote_search_limits() {
        let dir = TestDir::new("settings_remote_search_limits");
        let path = dir.path.join("settings.toml");
        let defaults = Settings::default();
        assert_eq!(
            (
                defaults.remote_search_depth,
                defaults.remote_search_parallel
            ),
            (3, 2)
        );

        let mut settings = defaults;
        settings.remote_search_depth = 5;
        settings.remote_search_parallel = 4;
        settings.save_to(&path).unwrap();
        let (saved, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            (saved.remote_search_depth, saved.remote_search_parallel),
            (5, 4)
        );

        fs::write(
            &path,
            "remote_search_depth = 0\nremote_search_parallel = 0\n",
        )
        .unwrap();
        let (settings, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(
            (
                settings.remote_search_depth,
                settings.remote_search_parallel
            ),
            (DEFAULT_REMOTE_SEARCH_DEPTH, DEFAULT_REMOTE_SEARCH_PARALLEL),
            "0 would search nothing or never send a request"
        );
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn settings_thumbnails_round_trip_and_fallbacks() {
        let dir = TestDir::new("settings_thumbnails");
        let path = dir.path.join("settings.toml");
        let defaults = Settings::default();
        assert!(defaults.thumbnails_enabled);
        assert!(
            !defaults.thumbnail_remote_enabled,
            "remote thumbnails download whole files, so they are opt-in"
        );
        assert!(defaults.thumbnail_extensions.iter().any(|e| e == "pdf"));

        let mut settings = defaults;
        settings.thumbnails_enabled = false;
        settings.thumbnail_remote_enabled = true;
        settings.set_thumbnail_extensions("PNG, .jpg png").unwrap();
        settings.set_thumbnail_max_file_size_mb(64).unwrap();
        settings.save_to(&path).unwrap();
        let (saved, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(saved, settings);
        assert_eq!(saved.thumbnail_extensions, ["png", "jpg"]);

        fs::write(
            &path,
            "thumbnail_extensions = [\"png\", \"heic\", \"pdf\"]\nthumbnail_max_file_size_mb = 0\n",
        )
        .unwrap();
        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(
            loaded.thumbnail_extensions,
            ["png", "pdf"],
            "an unsupported entry is dropped without discarding the rest of the list"
        );
        assert_eq!(
            loaded.thumbnail_max_file_size_mb,
            DEFAULT_THUMBNAIL_MAX_FILE_SIZE_MB
        );
        let keys: Vec<_> = warnings.iter().map(|w| w.key).collect();
        assert_eq!(keys, ["thumbnail_extensions", "thumbnail_max_file_size_mb"]);
    }

    #[test]
    fn settings_thumbnail_setters_reject_invalid_input_unchanged() {
        let mut s = Settings::default();
        assert_eq!(
            s.set_thumbnail_extensions("png, heic, docx"),
            Err(vec!["heic".to_string(), "docx".to_string()])
        );
        assert_eq!(
            s.thumbnail_extensions,
            Settings::default().thumbnail_extensions
        );
        assert!(s.set_thumbnail_max_file_size_mb(0).is_err());
        assert!(s.set_thumbnail_max_file_size_mb(1025).is_err());
        assert_eq!(
            s.thumbnail_max_file_size_mb,
            DEFAULT_THUMBNAIL_MAX_FILE_SIZE_MB
        );
    }

    #[test]
    fn settings_list_timeout() {
        let dir = TestDir::new("settings_list_timeout");
        let path = dir.path.join("settings.toml");
        assert_eq!(Settings::default().list_timeout_secs, 10);

        fs::write(&path, "list_timeout_secs = 3\n").unwrap();
        let (mut settings, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(settings.list_timeout_secs, 3);

        settings.list_timeout_secs = 20;
        settings.save_to(&path).unwrap();
        let (saved, _) = Settings::load_from(&path).unwrap();
        assert_eq!(saved.list_timeout_secs, 20);

        fs::write(&path, "list_timeout_secs = -1\n").unwrap();
        let (settings, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(settings.list_timeout_secs, DEFAULT_LIST_TIMEOUT_SECS);
        assert_eq!(warnings[0].key, "list_timeout_secs");
    }

    #[test]
    fn settings_atomic_save() {
        let dir = TestDir::new("settings_atomic_save");
        let path = dir.path.join("settings.toml");
        let original = "version = 1\nrefresh_interval_secs = 9\n";
        fs::write(&path, original).unwrap();

        let settings = Settings {
            refresh_interval_secs: 42,
            ..Settings::default()
        };
        let err = settings
            .save_with_hook(&path, || Err(io::Error::other("crash before rename")))
            .unwrap_err();
        assert_eq!(err.to_string(), "crash before rename");
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&dir.path).unwrap().count(), 1);

        settings.save_to(&path).unwrap();
        let (saved, _) = Settings::load_from(&path).unwrap();
        assert_eq!(saved.refresh_interval_secs, 42);
    }

    #[test]
    fn settings_keep_unknown() {
        let dir = TestDir::new("settings_keep_unknown");
        let path = dir.path.join("settings.toml");
        fs::write(
            &path,
            "version = 1\nfuture_flag = true\n[future_section]\nname = \"x\"\n",
        )
        .unwrap();

        let (mut settings, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        settings.refresh_interval_secs = 11;
        settings.save_to(&path).unwrap();

        let saved: Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(saved["future_flag"], Value::Boolean(true));
        assert_eq!(saved["future_section"]["name"].as_str(), Some("x"));
        assert_eq!(saved["refresh_interval_secs"].as_integer(), Some(11));
    }

    #[test]
    fn settings_show_hidden_and_default_sort_round_trip() {
        let dir = TestDir::new("settings_general_round_trip");
        let path = dir.path.join("settings.toml");

        let mut settings = Settings::default();
        assert!(!settings.show_hidden);
        assert_eq!(
            settings.default_sort,
            SortOrder::new(SortColumn::Name, SortDirection::Ascending)
        );

        settings.show_hidden = true;
        settings.default_sort = SortOrder::new(SortColumn::Date, SortDirection::Descending);
        settings.save_to(&path).unwrap();

        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert!(loaded.show_hidden);
        assert_eq!(
            loaded.default_sort,
            SortOrder::new(SortColumn::Date, SortDirection::Descending)
        );
    }

    #[test]
    fn hidden_sidebar_survives_a_restart_and_older_files_show_it() {
        let dir = TestDir::new("settings_show_sidebar");
        let path = dir.path.join("settings.toml");

        fs::write(&path, "version = 1\n").unwrap();
        let (mut settings, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert!(settings.show_sidebar);

        settings.show_sidebar = false;
        settings.save_to(&path).unwrap();
        let (loaded, _) = Settings::load_from(&path).unwrap();
        assert!(!loaded.show_sidebar);

        fs::write(&path, "show_sidebar = \"no\"\n").unwrap();
        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(loaded.show_sidebar);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "show_sidebar");
    }

    #[test]
    fn settings_backward_compatible_without_general_keys() {
        let dir = TestDir::new("settings_backward_compat");
        let path = dir.path.join("settings.toml");
        fs::write(&path, "version = 1\nlist_timeout_secs = 15\n").unwrap();

        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert!(!loaded.show_hidden);
        assert_eq!(
            loaded.default_sort,
            SortOrder::new(SortColumn::Name, SortDirection::Ascending)
        );
        assert_eq!(loaded.list_timeout_secs, 15);
    }

    #[test]
    fn settings_invalid_general_values_warn_and_fallback() {
        let dir = TestDir::new("settings_invalid_general");
        let path = dir.path.join("settings.toml");
        fs::write(
            &path,
            "show_hidden = \"yes\"\n[default_sort]\ncolumn = \"owner\"\ndirection = \"sideways\"\n",
        )
        .unwrap();

        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(!loaded.show_hidden);
        assert_eq!(loaded.default_sort, SortOrder::default());
        assert_eq!(warnings.len(), 3);
        assert_eq!(warnings[0].key, "show_hidden");
        assert_eq!(warnings[1].key, "default_sort");
        assert_eq!(warnings[2].key, "default_sort");

        // Partial default_sort fills missing field with default and emits no warning.
        fs::write(&path, "[default_sort]\ncolumn = \"size\"\n").unwrap();
        let (partial, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            partial.default_sort,
            SortOrder::new(SortColumn::Size, SortDirection::Ascending)
        );
    }

    #[test]
    fn settings_validation_rejects_out_of_range() {
        let mut s = Settings::default();
        assert!(s.validate().is_ok());

        assert!(s.set_refresh_interval_secs(0).is_err());
        assert!(s.set_refresh_interval_secs(3601).is_err());
        assert_eq!(s.refresh_interval_secs, DEFAULT_REFRESH_INTERVAL_SECS);
        assert!(s.set_refresh_interval_secs(12).is_ok());

        assert!(s.set_list_timeout_secs(0).is_err());
        assert!(s.set_list_timeout_secs(3601).is_err());
        assert_eq!(s.list_timeout_secs, DEFAULT_LIST_TIMEOUT_SECS);
        assert!(s.set_list_timeout_secs(25).is_ok());

        assert!(s.set_remote_poll_interval_secs(9).is_err());
        assert!(s.set_remote_poll_interval_secs(3601).is_err());
        assert_eq!(
            s.remote_poll_interval_secs,
            DEFAULT_REMOTE_POLL_INTERVAL_SECS
        );
        assert!(s.set_remote_poll_interval_secs(15).is_ok());

        assert!(s.set_remote_search_depth(0).is_err());
        assert!(s.set_remote_search_depth(65).is_err());
        assert_eq!(s.remote_search_depth, DEFAULT_REMOTE_SEARCH_DEPTH);
        assert!(s.set_remote_search_depth(5).is_ok());

        assert!(s.set_remote_search_parallel(0).is_err());
        assert!(s.set_remote_search_parallel(33).is_err());
        assert_eq!(s.remote_search_parallel, DEFAULT_REMOTE_SEARCH_PARALLEL);
        assert!(s.set_remote_search_parallel(4).is_ok());

        assert!(s.validate().is_ok());
    }

    #[test]
    fn settings_motion_preference_roundtrip_and_invalid_warning() {
        let dir = TestDir::new("settings_motion_preference");
        let path = dir.path.join("settings.toml");

        let mut s = Settings::default();
        assert_eq!(s.motion_preference, MotionPreference::System);
        s.motion_preference = MotionPreference::Reduced;
        s.save_to(&path).unwrap();

        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(loaded.motion_preference, MotionPreference::Reduced);

        fs::write(&path, "motion_preference = \"bogus\"\n").unwrap();
        let (fallback, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(fallback.motion_preference, MotionPreference::System);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "motion_preference");
    }

    #[test]
    fn settings_view_mode_roundtrip_and_invalid_warning() {
        let dir = TestDir::new("settings_view_mode");
        let path = dir.path.join("settings.toml");

        let mut s = Settings::default();
        assert_eq!(s.view_mode, ViewMode::Details);

        for mode in [ViewMode::Details, ViewMode::CompactList, ViewMode::Grid] {
            s.view_mode = mode;
            s.save_to(&path).unwrap();
            let (loaded, warnings) = Settings::load_from(&path).unwrap();
            assert!(warnings.is_empty());
            assert_eq!(loaded.view_mode, mode);
        }

        // Backward compatibility: missing view_mode yields Details with no warning.
        fs::write(&path, "version = 1\n").unwrap();
        let (compat, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(compat.view_mode, ViewMode::Details);

        // Aliases and case-insensitivity.
        fs::write(&path, "view_mode = \"compact\"\n").unwrap();
        let (alias, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(alias.view_mode, ViewMode::CompactList);

        // Invalid string and non-string values warn and fall back to Details.
        fs::write(&path, "view_mode = \"carousel\"\n").unwrap();
        let (fallback, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(fallback.view_mode, ViewMode::Details);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "view_mode");

        fs::write(&path, "view_mode = 42\n").unwrap();
        let (fallback, warnings) = Settings::load_from(&path).unwrap();
        assert_eq!(fallback.view_mode, ViewMode::Details);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "view_mode");
    }

    #[test]
    fn settings_folder_scope_and_per_folder_preferences_roundtrip() {
        let dir = TestDir::new("settings_folder_prefs");
        let path = dir.path.join("settings.toml");
        let folder_a = Location::Local(dir.path.join("a"));
        let folder_b = Location::Local(dir.path.join("b"));
        let remote_c = Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/data"),
        };

        let mut s = Settings::default();
        assert_eq!(s.folder_settings_scope, FolderSettingsScope::PerFolder);
        s.folder_settings_scope = FolderSettingsScope::AllFolders;
        s.set_folder_view_mode(&folder_a, ViewMode::Grid);
        s.set_folder_sort(&folder_a, SortColumn::Created, SortDirection::Descending);
        s.set_folder_folders_first(&folder_a, false);
        s.set_folder_hidden_last(&folder_a, true);
        s.set_folder_show_hidden(&remote_c, true);
        s.save_to(&path).unwrap();

        let (loaded, warnings) = Settings::load_from(&path).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            loaded.folder_settings_scope,
            FolderSettingsScope::AllFolders
        );
        assert!(loaded.has_folder_override(&folder_a));
        assert!(!loaded.has_folder_override(&folder_b));
        assert!(loaded.has_folder_override(&remote_c));

        let resolved_a = loaded.resolve_folder_settings(&folder_a);
        assert_eq!(resolved_a.view_mode, ViewMode::Grid);
        assert_eq!(resolved_a.sort_column, SortColumn::Created);
        assert_eq!(resolved_a.sort_direction, SortDirection::Descending);
        assert!(!resolved_a.show_hidden);
        assert!(!resolved_a.folders_first);
        assert!(resolved_a.hidden_last);

        let resolved_b = loaded.resolve_folder_settings(&folder_b);
        assert_eq!(resolved_b.view_mode, ViewMode::Details);
        assert_eq!(resolved_b.sort_column, SortColumn::Name);
        assert_eq!(resolved_b.sort_direction, SortDirection::Ascending);
        assert!(!resolved_b.show_hidden);
        assert!(resolved_b.folders_first);
        assert!(!resolved_b.hidden_last);

        let resolved_c = loaded.resolve_folder_settings(&remote_c);
        assert_eq!(resolved_c.view_mode, ViewMode::Details);
        assert!(resolved_c.show_hidden);

        let mut applied = loaded;
        applied.apply_folder_settings_to_all(resolved_a);
        assert_eq!(applied.view_mode, ViewMode::Grid);
        assert_eq!(
            applied.default_sort,
            SortOrder::new(SortColumn::Created, SortDirection::Descending)
        );
        assert!(!applied.show_hidden);
        assert!(!applied.folders_first);
        assert!(applied.hidden_last);
        assert!(applied.folder_preferences.is_empty());
    }
}
