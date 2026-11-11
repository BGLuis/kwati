use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

pub const CURRENT_VERSION: u32 = 1;
pub const DEFAULT_REFRESH_INTERVAL_SECS: u32 = 5;

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
    pub refresh_interval_secs: u32,
    /// Keys this version does not know, kept so a newer version's file survives a save.
    unknown: Table,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            refresh_interval_secs: DEFAULT_REFRESH_INTERVAL_SECS,
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

impl Settings {
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
        let refresh_interval_secs = take_u32_or_default(
            &mut table,
            "refresh_interval_secs",
            defaults.refresh_interval_secs,
            &mut warnings,
        );
        let settings = Self {
            version,
            refresh_interval_secs,
            unknown: table,
        };
        Ok((settings, warnings))
    }

    /// Atomically replaces the file at `path`: writes a temporary file in the same folder, then
    /// renames it over the target.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        self.save_with_hook(path, || Ok(()))
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
        table.insert(
            "refresh_interval_secs".into(),
            Value::Integer(self.refresh_interval_secs.into()),
        );
        let text = toml::to_string(&table)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

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
}
