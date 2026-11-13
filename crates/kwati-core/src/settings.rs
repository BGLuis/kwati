use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::places::Bookmark;
use crate::{SortColumn, SortDirection, SortOrder};

pub const CURRENT_VERSION: u32 = 1;
pub const DEFAULT_SHOW_HIDDEN: bool = false;
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

/// A value in the settings file that was ignored in favour of its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsWarning {
    pub key: &'static str,
    pub message: String,
}

/// User settings. Features add their own options here, each with a default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub version: u32,
    pub show_hidden: bool,
    pub default_sort: SortOrder,
    pub refresh_interval_secs: u32,
    /// Seconds a directory listing may take before it is abandoned (see `list_dir_bounded`).
    pub list_timeout_secs: u32,
    pub remote_poll_enabled: bool,
    pub remote_poll_interval_secs: u32,
    /// Folder levels a remote search lists (see `RemoteSearchOptions::max_depth`); at least 1.
    pub remote_search_depth: u32,
    /// Listing requests a remote search may have in flight at once; at least 1.
    pub remote_search_parallel: u32,
    pub bookmarks: Vec<Bookmark>,
    /// Keys this version does not know, kept so a newer version's file survives a save.
    unknown: Table,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            show_hidden: DEFAULT_SHOW_HIDDEN,
            default_sort: SortOrder::default(),
            refresh_interval_secs: DEFAULT_REFRESH_INTERVAL_SECS,
            list_timeout_secs: DEFAULT_LIST_TIMEOUT_SECS,
            remote_poll_enabled: false,
            remote_poll_interval_secs: DEFAULT_REMOTE_POLL_INTERVAL_SECS,
            remote_search_depth: DEFAULT_REMOTE_SEARCH_DEPTH,
            remote_search_parallel: DEFAULT_REMOTE_SEARCH_PARALLEL,
            bookmarks: Vec::new(),
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

fn parse_sort_column(raw: &str) -> Option<SortColumn> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "name" => Some(SortColumn::Name),
        "size" => Some(SortColumn::Size),
        "date" => Some(SortColumn::Date),
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
    }
}

fn sort_direction_str(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "ascending",
        SortDirection::Descending => "descending",
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
        let default_sort =
            take_default_sort_or_default(&mut table, defaults.default_sort, &mut warnings);
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

        let settings = Self {
            version,
            show_hidden,
            default_sort,
            refresh_interval_secs,
            list_timeout_secs,
            remote_poll_enabled,
            remote_poll_interval_secs,
            remote_search_depth,
            remote_search_parallel,
            bookmarks,
            unknown: table,
        };
        Ok((settings, warnings))
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
}
