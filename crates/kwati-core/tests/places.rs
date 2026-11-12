use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use kwati_core::{
    list_places, resolve_bookmarks, standard_places, standard_places_from, Bookmark, PlaceKind,
    Settings,
};

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "kwati_test_places_{}_{}_{}",
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
fn places_skip_missing() {
    // 1. All standard places returned by standard_places() must exist on disk.
    let system_places = standard_places();
    for place in &system_places {
        assert!(
            place.path.exists(),
            "standard place {:?} returned but does not exist",
            place.path
        );
    }

    // 2. Controlled test with candidate standard directories:
    // A standard folder that does not exist on disk is left out of the list.
    let dir = TestDir::new("places_skip_missing");
    let existing_folder = dir.path.join("documents");
    fs::create_dir_all(&existing_folder).expect("failed to create existing test folder");

    let missing_folder = dir.path.join("music_does_not_exist");
    // Explicitly do not create missing_folder

    let candidates = [
        (
            PlaceKind::Documents,
            "Documents",
            Some(existing_folder.clone()),
        ),
        (PlaceKind::Music, "Music", Some(missing_folder.clone())),
        (PlaceKind::Videos, "Videos", None),
    ];

    let filtered = standard_places_from(candidates);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].kind, PlaceKind::Documents);
    assert_eq!(filtered[0].name, "Documents");
    assert_eq!(filtered[0].path, existing_folder);
}

#[test]
fn bookmarks_round_trip() {
    let dir = TestDir::new("bookmarks_round_trip");
    let settings_path = dir.path.join("settings.toml");

    let mut settings = Settings::default();
    assert!(settings.bookmarks.is_empty());

    // Add bookmarks
    let path_a = dir.path.join("folder_a");
    let path_b = dir.path.join("folder_b");
    let path_c = dir.path.join("folder_c");
    fs::create_dir_all(&path_a).unwrap();
    fs::create_dir_all(&path_b).unwrap();
    fs::create_dir_all(&path_c).unwrap();

    settings.add_bookmark(Bookmark::new("Folder A", &path_a));
    settings.add_bookmark(Bookmark::new("Folder B", &path_b));
    settings.add_bookmark(Bookmark::new("Folder C", &path_c));

    // Save and reload
    settings.save_to(&settings_path).unwrap();
    let (mut loaded, warnings) = Settings::load_from(&settings_path).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(loaded.bookmarks.len(), 3);
    assert_eq!(loaded.bookmarks[0].name, "Folder A");
    assert_eq!(loaded.bookmarks[1].name, "Folder B");
    assert_eq!(loaded.bookmarks[2].name, "Folder C");

    // Reorder: move Folder C from index 2 to index 0 -> [C, A, B]
    assert!(loaded.reorder_bookmarks(2, 0));
    assert_eq!(loaded.bookmarks[0].name, "Folder C");
    assert_eq!(loaded.bookmarks[1].name, "Folder A");
    assert_eq!(loaded.bookmarks[2].name, "Folder B");

    // Rename: rename Folder A to "Renamed A"
    assert!(loaded.rename_bookmark(1, "Renamed A"));
    assert_eq!(loaded.bookmarks[1].name, "Renamed A");

    // Remove: remove Folder B at index 2
    let removed = loaded.remove_bookmark(2);
    assert_eq!(removed.map(|b| b.name), Some("Folder B".to_string()));
    assert_eq!(loaded.bookmarks.len(), 2);

    // Save and reload again to verify persistence of reordering, rename, and removal
    loaded.save_to(&settings_path).unwrap();
    let (reloaded, warnings) = Settings::load_from(&settings_path).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(reloaded.bookmarks.len(), 2);
    assert_eq!(reloaded.bookmarks[0].name, "Folder C");
    assert_eq!(reloaded.bookmarks[0].path, path_c);
    assert_eq!(reloaded.bookmarks[1].name, "Renamed A");
    assert_eq!(reloaded.bookmarks[1].path, path_a);
}

#[test]
fn bookmarks_missing_target() {
    let dir = TestDir::new("bookmarks_missing_target");
    let settings_path = dir.path.join("settings.toml");

    let target_folder = dir.path.join("to_be_deleted");
    fs::create_dir_all(&target_folder).unwrap();

    let mut settings = Settings::default();
    let bookmark = Bookmark::new("Temporary Target", &target_folder);
    assert!(!bookmark.is_missing());
    assert!(bookmark.exists());
    settings.add_bookmark(bookmark);

    settings.save_to(&settings_path).unwrap();

    // Now delete the bookmarked folder on disk
    fs::remove_dir_all(&target_folder).unwrap();
    assert!(!target_folder.exists());

    // Reload settings from disk
    let (loaded, warnings) = Settings::load_from(&settings_path).unwrap();
    assert!(warnings.is_empty());

    // The bookmark is kept, not removed
    assert_eq!(loaded.bookmarks.len(), 1);
    assert_eq!(loaded.bookmarks[0].name, "Temporary Target");
    assert_eq!(loaded.bookmarks[0].path, target_folder);

    // And reported as missing
    assert!(loaded.bookmarks[0].is_missing());
    assert!(!loaded.bookmarks[0].exists());

    let resolved = resolve_bookmarks(&loaded);
    assert_eq!(resolved.len(), 1);
    assert!(resolved[0].is_missing);
    assert!(!resolved[0].exists());

    // list_places also reflects this
    let sidebar = list_places(&loaded);
    assert_eq!(sidebar.bookmarks.len(), 1);
    assert!(sidebar.bookmarks[0].is_missing);

    // Saving settings again preserves the bookmark
    loaded.save_to(&settings_path).unwrap();
    let (persisted, _) = Settings::load_from(&settings_path).unwrap();
    assert_eq!(persisted.bookmarks.len(), 1);
    assert!(persisted.bookmarks[0].is_missing());
}
