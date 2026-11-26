use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use kwati_core::{applications_for, open_with, open_with_executable, which_in_path, AppInfo};

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "kwati_test_open_with_{}_{}_{}",
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

#[cfg(target_os = "linux")]
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(target_os = "linux")]
struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

#[cfg(target_os = "linux")]
impl EnvGuard {
    fn set(vars: &[(&'static str, &Path)]) -> Self {
        let mut saved = Vec::with_capacity(vars.len() + 1);
        for &(key, val) in vars {
            saved.push((key, std::env::var_os(key)));
            std::env::set_var(key, val);
        }
        saved.push((
            "XDG_CURRENT_DESKTOP",
            std::env::var_os("XDG_CURRENT_DESKTOP"),
        ));
        std::env::set_var("XDG_CURRENT_DESKTOP", "");
        Self { saved }
    }
}

#[cfg(target_os = "linux")]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in self.saved.drain(..).rev() {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write test file");
}

#[cfg(all(test, target_os = "linux"))]
fn write_executable_script(path: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", marker.display());
    write_file(path, &script);
    let mut perms = fs::metadata(path).expect("script metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod script");
}

#[cfg(target_os = "linux")]
fn wait_for_marker(marker: &Path) -> String {
    for _ in 0..200 {
        if let Ok(contents) = fs::read_to_string(marker) {
            if !contents.is_empty() {
                return contents;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for marker file {}", marker.display());
}

#[test]
#[cfg(target_os = "linux")]
fn open_with_lists_default() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = TestDir::new("lists_default");
    let data_home = root.path.join("data");
    let config_home = root.path.join("config");
    let sys_data = root.path.join("sys_data");
    let sys_config = root.path.join("sys_config");
    let _env = EnvGuard::set(&[
        ("XDG_DATA_HOME", &data_home),
        ("XDG_CONFIG_HOME", &config_home),
        ("XDG_DATA_DIRS", &sys_data),
        ("XDG_CONFIG_DIRS", &sys_config),
    ]);

    write_file(
        &data_home.join("mime/globs2"),
        "50:application/x-kwatitest:*.kwatitest\n",
    );
    write_file(
        &data_home.join("applications/alpha.desktop"),
        "[Desktop Entry]\nType=Application\nName=Alpha Viewer\nExec=/bin/true %f\nIcon=alpha\nMimeType=application/x-kwatitest;\n",
    );
    write_file(
        &data_home.join("applications/zulu-default.desktop"),
        "[Desktop Entry]\nType=Application\nName=Zulu Default\nExec=/bin/true %f\nIcon=zulu\nMimeType=application/x-kwatitest;\n",
    );
    write_file(
        &config_home.join("mimeapps.list"),
        "[Default Applications]\napplication/x-kwatitest=zulu-default.desktop;\n",
    );

    let target = root.path.join("sample.kwatitest");
    write_file(&target, "hello");

    let apps = applications_for(&target);
    assert_eq!(apps.len(), 2, "expected 2 apps, got {apps:?}");
    assert_eq!(apps[0].id, "zulu-default.desktop");
    assert_eq!(apps[0].name, "Zulu Default");
    assert!(apps[0].is_default);
    assert_eq!(apps[0].icon.as_deref(), Some("zulu"));

    assert_eq!(apps[1].id, "alpha.desktop");
    assert!(!apps[1].is_default);
}

#[test]
fn open_with_missing_app() {
    let root = TestDir::new("missing_app");
    let target = root.path.join("sample.txt");
    fs::write(&target, "hello").expect("write target");

    let missing = AppInfo {
        id: "nonexistent.desktop".to_string(),
        name: "Nonexistent App".to_string(),
        executable: root
            .path
            .join("definitely-does-not-exist-kwati-bin")
            .display()
            .to_string(),
        icon: None,
        is_default: false,
        no_display: false,
        terminal: false,
    };

    let err = open_with(&missing, &[target.as_path()]).expect_err("missing app must fail");
    assert_eq!(err.kind(), ErrorKind::NotFound);

    let empty_exec = AppInfo {
        executable: "   ".to_string(),
        ..missing
    };
    let err = open_with(&empty_exec, &[target.as_path()]).expect_err("empty exec must fail");
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
#[cfg(target_os = "linux")]
fn open_with_launches_non_default_script_with_target_path() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = TestDir::new("launch_non_default");
    let data_home = root.path.join("data");
    let config_home = root.path.join("config");
    let sys_data = root.path.join("sys_data");
    let sys_config = root.path.join("sys_config");
    let _env = EnvGuard::set(&[
        ("XDG_DATA_HOME", &data_home),
        ("XDG_CONFIG_HOME", &config_home),
        ("XDG_DATA_DIRS", &sys_data),
        ("XDG_CONFIG_DIRS", &sys_config),
    ]);

    let default_marker = root.path.join("default_marker.txt");
    let secondary_marker = root.path.join("secondary_marker.txt");
    let default_script = root.path.join("default_runner.sh");
    let secondary_script = root.path.join("secondary_runner.sh");
    write_executable_script(&default_script, &default_marker);
    write_executable_script(&secondary_script, &secondary_marker);

    write_file(
        &data_home.join("mime/globs2"),
        "50:application/x-kwatilaunch:*.kwatilaunch\n",
    );
    write_file(
        &data_home.join("applications/def.desktop"),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Default Runner\nExec={} %f\nMimeType=application/x-kwatilaunch;\n",
            default_script.display()
        ),
    );
    write_file(
        &data_home.join("applications/alt.desktop"),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Alt Runner\nExec={} %f\nMimeType=application/x-kwatilaunch;\n",
            secondary_script.display()
        ),
    );
    write_file(
        &config_home.join("mimeapps.list"),
        "[Default Applications]\napplication/x-kwatilaunch=def.desktop;\n",
    );

    let target = root.path.join("document.kwatilaunch");
    write_file(&target, "payload");

    let apps = applications_for(&target);
    let alt = apps
        .iter()
        .find(|a| a.id == "alt.desktop")
        .expect("alt.desktop should be listed");
    assert!(!alt.is_default);

    open_with(alt, &[target.as_path()]).expect("launch alt.desktop");
    let recorded = wait_for_marker(&secondary_marker);
    assert_eq!(recorded, target.display().to_string());
    assert!(
        !default_marker.exists(),
        "default app must not be invoked when launching non-default app"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn open_with_respects_nodisplay_hidden_tryexec_and_removed_associations() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = TestDir::new("xdg_filters");
    let data_home = root.path.join("data");
    let config_home = root.path.join("config");
    let sys_data = root.path.join("sys_data");
    let sys_config = root.path.join("sys_config");
    let _env = EnvGuard::set(&[
        ("XDG_DATA_HOME", &data_home),
        ("XDG_CONFIG_HOME", &config_home),
        ("XDG_DATA_DIRS", &sys_data),
        ("XDG_CONFIG_DIRS", &sys_config),
    ]);

    write_file(
        &data_home.join("mime/globs2"),
        "50:application/x-kwatifilter-alias:*.kwatifilter\n",
    );
    write_file(
        &data_home.join("mime/aliases"),
        "application/x-kwatifilter-alias application/x-kwatifilter\n",
    );

    // NoDisplay=true stays in the list with no_display == true.
    write_file(
        &data_home.join("applications/nodisp.desktop"),
        "[Desktop Entry]\nType=Application\nName=NoDisplay App\nExec=/bin/true %f\nNoDisplay=true\nTerminal=true\nMimeType=application/x-kwatifilter;\n",
    );
    // Hidden=true in XDG_DATA_HOME shadows the same ID in XDG_DATA_DIRS.
    write_file(
        &data_home.join("applications/masked.desktop"),
        "[Desktop Entry]\nHidden=true\n",
    );
    write_file(
        &sys_data.join("applications/masked.desktop"),
        "[Desktop Entry]\nType=Application\nName=Masked App\nExec=/bin/true %f\nMimeType=application/x-kwatifilter;\n",
    );
    // Missing TryExec excludes the entry.
    write_file(
        &data_home.join("applications/missing-tryexec.desktop"),
        "[Desktop Entry]\nType=Application\nName=Missing TryExec\nExec=/bin/true %f\nTryExec=/nonexistent/kwati/binary\nMimeType=application/x-kwatifilter;\n",
    );
    // Removed association is excluded even when listed in mimeinfo.cache and MimeType=.
    write_file(
        &data_home.join("applications/removed.desktop"),
        "[Desktop Entry]\nType=Application\nName=Removed App\nExec=/bin/true %f\nMimeType=application/x-kwatifilter;\n",
    );
    write_file(
        &data_home.join("applications/mimeinfo.cache"),
        "[MIME Cache]\napplication/x-kwatifilter=removed.desktop;nodisp.desktop;\n",
    );
    write_file(
        &config_home.join("mimeapps.list"),
        "[Removed Associations]\napplication/x-kwatifilter=removed.desktop;\n",
    );

    let target = root.path.join("item.kwatifilter");
    write_file(&target, "data");

    let apps = applications_for(&target);
    assert_eq!(apps.len(), 1, "expected only nodisp.desktop, got {apps:?}");
    assert_eq!(apps[0].id, "nodisp.desktop");
    assert!(apps[0].no_display);
    assert!(apps[0].terminal);
    assert!(!apps[0].is_default);
}

#[test]
fn open_with_executable_validates_and_launches() {
    let root = TestDir::new("custom_exec");
    let target = root.path.join("file.txt");
    fs::write(&target, "hello").expect("write target");

    let missing_err = open_with_executable(
        &root.path.join("missing-binary-kwati").display().to_string(),
        &[target.as_path()],
    )
    .expect_err("missing custom executable must fail");
    assert_eq!(missing_err.kind(), ErrorKind::NotFound);

    let empty_err =
        open_with_executable("   ", &[target.as_path()]).expect_err("empty executable must fail");
    assert_eq!(empty_err.kind(), ErrorKind::InvalidInput);

    assert!(!which_in_path("definitely-missing-kwati-binary-xyz"));

    #[cfg(target_os = "linux")]
    {
        let marker = root.path.join("custom_marker.txt");
        let script = root.path.join("custom runner.sh");
        write_executable_script(&script, &marker);

        open_with_executable(&script.display().to_string(), &[target.as_path()])
            .expect("launch custom executable with space in path");
        let recorded = wait_for_marker(&marker);
        assert_eq!(recorded, target.display().to_string());
    }
}

#[test]
#[cfg(target_os = "linux")]
fn applications_for_returns_empty_for_directory_or_unknown_extension() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = TestDir::new("empty_cases");
    let data_home = root.path.join("data");
    let config_home = root.path.join("config");
    let sys_data = root.path.join("sys_data");
    let sys_config = root.path.join("sys_config");
    let _env = EnvGuard::set(&[
        ("XDG_DATA_HOME", &data_home),
        ("XDG_CONFIG_HOME", &config_home),
        ("XDG_DATA_DIRS", &sys_data),
        ("XDG_CONFIG_DIRS", &sys_config),
    ]);

    assert!(applications_for(&root.path).is_empty());

    let unknown = root.path.join("file.unknown_kwati_ext_999");
    write_file(&unknown, "x");
    assert!(applications_for(&unknown).is_empty());
}

#[test]
#[cfg(target_os = "linux")]
fn applications_for_uses_mimeinfo_cache_and_respects_hidden_shadowing() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = TestDir::new("mimeinfo_cache");
    let data_home = root.path.join("data");
    let config_home = root.path.join("config");
    let sys_data = root.path.join("sys_data");
    let sys_config = root.path.join("sys_config");
    let _env = EnvGuard::set(&[
        ("XDG_DATA_HOME", &data_home),
        ("XDG_CONFIG_HOME", &config_home),
        ("XDG_DATA_DIRS", &sys_data),
        ("XDG_CONFIG_DIRS", &sys_config),
    ]);

    let target = root.path.join("note.txt");
    write_file(&target, "hello");

    write_file(
        &sys_data.join("applications/mimeinfo.cache"),
        "[MIME Cache]\ntext/plain=cached-app.desktop;shadowed.desktop;missing-tryexec.desktop;\n",
    );
    write_file(
        &sys_data.join("applications/cached-app.desktop"),
        "[Desktop Entry]\nType=Application\nName=Cached App\nExec=sh %f\nMimeType=text/plain;\n",
    );
    write_file(
        &sys_data.join("applications/shadowed.desktop"),
        "[Desktop Entry]\nType=Application\nName=Shadowed App\nExec=sh %f\nMimeType=text/plain;\n",
    );
    write_file(
        &sys_data.join("applications/missing-tryexec.desktop"),
        "[Desktop Entry]\nType=Application\nName=Missing TryExec\nExec=sh %f\nTryExec=definitely-missing-kwati-tryexec-999\nMimeType=text/plain;\n",
    );
    // Higher-precedence XDG_DATA_HOME masks shadowed.desktop with Hidden=true.
    write_file(
        &data_home.join("applications/shadowed.desktop"),
        "[Desktop Entry]\nType=Application\nName=Shadowed Hidden\nHidden=true\n",
    );
    // Unrelated cached entry not listed in mimeinfo.cache for text/plain.
    write_file(
        &sys_data.join("applications/other-app.desktop"),
        "[Desktop Entry]\nType=Application\nName=Other\nExec=sh %f\nMimeType=image/png;\n",
    );

    let apps = applications_for(&target);
    assert_eq!(
        apps.len(),
        1,
        "expected only cached-app.desktop, got {apps:?}"
    );
    assert_eq!(apps[0].id, "cached-app.desktop");
}
