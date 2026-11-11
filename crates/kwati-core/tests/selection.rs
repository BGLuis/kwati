use kwati_core::{list_dir, Location, Selection};
use std::path::PathBuf;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("kwati_test_{}_{}_{}", name, std::process::id(), id));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn selection_range() {
    let test_dir = TestDir::new("selection_range");
    let base = test_dir.path();

    // Create files with names that sort predictably in alphabetical order
    std::fs::write(base.join("a.txt"), b"").unwrap();
    std::fs::write(base.join("b.txt"), b"").unwrap();
    std::fs::write(base.join("c.txt"), b"").unwrap();
    std::fs::write(base.join("d.txt"), b"").unwrap();
    std::fs::write(base.join("e.txt"), b"").unwrap();

    let listing = list_dir(base).unwrap();
    assert_eq!(listing.len(), 5);

    let entries = listing.entries();
    let loc_a = entries[0].location();
    let loc_b = entries[1].location();
    let loc_c = entries[2].location();
    let loc_d = entries[3].location();
    let loc_e = entries[4].location();

    // 1. Forward range: anchor at b (index 1), range to d (index 3)
    let mut selection = Selection::new();
    selection.set_anchor(Some(loc_b.clone()));

    let range = selection.select_range(&loc_d, &listing);
    // Range returned must be in list order
    assert_eq!(range, vec![loc_b.clone(), loc_c.clone(), loc_d.clone()]);

    // All entries between b and d (inclusive) must be selected
    assert!(selection.is_selected(&loc_b));
    assert!(selection.is_selected(&loc_c));
    assert!(selection.is_selected(&loc_d));
    assert!(!selection.is_selected(&loc_a));
    assert!(!selection.is_selected(&loc_e));

    // selected_entries and selected_locations_in_order must match list order
    let selected_in_order = selection.selected_locations_in_order(&listing);
    assert_eq!(
        selected_in_order,
        vec![loc_b.clone(), loc_c.clone(), loc_d.clone()]
    );

    let selected_entries = selection.selected_entries(&listing);
    assert_eq!(
        selected_entries,
        vec![&entries[1], &entries[2], &entries[3]]
    );

    // 2. Reverse range: anchor at d (index 3), range to b (index 1)
    let mut rev_selection = Selection::new();
    rev_selection.set_anchor(Some(loc_d.clone()));

    let rev_range = rev_selection.select_range(&loc_b, &listing);
    // Still in list order
    assert_eq!(rev_range, vec![loc_b.clone(), loc_c.clone(), loc_d.clone()]);
    assert!(rev_selection.is_selected(&loc_b));
    assert!(rev_selection.is_selected(&loc_c));
    assert!(rev_selection.is_selected(&loc_d));
    assert!(!rev_selection.is_selected(&loc_a));
    assert!(!rev_selection.is_selected(&loc_e));

    // 3. Range with no initial anchor sets anchor to target and selects target
    let mut fresh_selection = Selection::new();
    assert_eq!(fresh_selection.anchor(), None);
    let single_range = fresh_selection.select_range(&loc_c, &listing);
    assert_eq!(single_range, vec![loc_c.clone()]);
    assert_eq!(fresh_selection.anchor(), Some(&loc_c));
    assert!(fresh_selection.is_selected(&loc_c));
    assert_eq!(fresh_selection.len(), 1);
}

#[test]
fn selection_toggle() {
    let test_dir = TestDir::new("selection_toggle");
    let base = test_dir.path();

    std::fs::write(base.join("1.txt"), b"").unwrap();
    std::fs::write(base.join("2.txt"), b"").unwrap();
    std::fs::write(base.join("3.txt"), b"").unwrap();

    let listing = list_dir(base).unwrap();
    let entries = listing.entries();
    let loc_1 = entries[0].location();
    let loc_2 = entries[1].location();
    let loc_3 = entries[2].location();

    let mut selection = Selection::new();
    assert_eq!(selection.len(), 0);
    assert_eq!(selection.anchor(), None);

    // 1. Toggling an entry adds only it and moves the anchor
    selection.toggle(&loc_2);
    assert!(selection.is_selected(&loc_2));
    assert!(!selection.is_selected(&loc_1));
    assert!(!selection.is_selected(&loc_3));
    assert_eq!(selection.len(), 1);
    assert_eq!(selection.anchor(), Some(&loc_2));

    // 2. Toggling another entry adds only it and moves the anchor
    selection.toggle(&loc_3);
    assert!(selection.is_selected(&loc_2));
    assert!(selection.is_selected(&loc_3));
    assert!(!selection.is_selected(&loc_1));
    assert_eq!(selection.len(), 2);
    assert_eq!(selection.anchor(), Some(&loc_3));

    // 3. Toggling an already-selected entry removes only it and moves the anchor
    selection.toggle(&loc_2);
    assert!(!selection.is_selected(&loc_2));
    assert!(selection.is_selected(&loc_3));
    assert!(!selection.is_selected(&loc_1));
    assert_eq!(selection.len(), 1);
    assert_eq!(selection.anchor(), Some(&loc_2));

    // 4. Toggling the last entry removes only it and moves the anchor
    selection.toggle(&loc_3);
    assert!(!selection.is_selected(&loc_3));
    assert!(selection.is_empty());
    assert_eq!(selection.anchor(), Some(&loc_3));
}

#[test]
fn selection_survives_reload() {
    let test_dir = TestDir::new("selection_survives_reload");
    let base = test_dir.path();

    let file_1 = base.join("file_1.txt");
    let file_2 = base.join("file_2.txt");
    let file_3 = base.join("file_3.txt");

    std::fs::write(&file_1, b"").unwrap();
    std::fs::write(&file_2, b"").unwrap();
    std::fs::write(&file_3, b"").unwrap();

    let initial_listing = list_dir(base).unwrap();
    assert_eq!(initial_listing.len(), 3);
    let loc_1 = Location::from(&file_1);
    let loc_2 = Location::from(&file_2);
    let loc_3 = Location::from(&file_3);

    let mut selection = Selection::new();
    selection.toggle(&loc_1);
    selection.toggle(&loc_2);
    selection.toggle(&loc_3);

    assert_eq!(selection.len(), 3);
    assert!(selection.is_selected(&loc_1));
    assert!(selection.is_selected(&loc_2));
    assert!(selection.is_selected(&loc_3));

    // Delete file_2 from disk
    std::fs::remove_file(&file_2).unwrap();

    // Reload directory
    let reloaded_listing = list_dir(base).unwrap();
    assert_eq!(reloaded_listing.len(), 2);

    // Update selection across reload
    selection.reload(&reloaded_listing);

    // The other selected entries (file_1 and file_3) stay selected, and file_2 is dropped
    assert_eq!(selection.len(), 2);
    assert!(selection.is_selected(&loc_1));
    assert!(selection.is_selected(&loc_3));
    assert!(!selection.is_selected(&loc_2));
}

#[test]
fn selection_all_clear_invert() {
    let test_dir = TestDir::new("selection_all_clear_invert");
    let base = test_dir.path();

    std::fs::write(base.join("x.txt"), b"").unwrap();
    std::fs::write(base.join("y.txt"), b"").unwrap();
    std::fs::write(base.join("z.txt"), b"").unwrap();

    let listing = list_dir(base).unwrap();
    let entries = listing.entries();
    let loc_x = entries[0].location();
    let loc_y = entries[1].location();
    let loc_z = entries[2].location();

    let mut selection = Selection::new();

    // select_all
    selection.select_all(&listing);
    assert_eq!(selection.len(), 3);
    assert!(selection.is_selected(&loc_x));
    assert!(selection.is_selected(&loc_y));
    assert!(selection.is_selected(&loc_z));

    // clear
    selection.clear();
    assert!(selection.is_empty());
    assert_eq!(selection.anchor(), None);

    // invert when empty selects all
    selection.invert(&listing);
    assert_eq!(selection.len(), 3);
    assert!(selection.is_selected(&loc_x));
    assert!(selection.is_selected(&loc_y));
    assert!(selection.is_selected(&loc_z));

    // deselect one and invert
    selection.toggle(&loc_y); // loc_y removed
    assert_eq!(selection.len(), 2);
    assert!(!selection.is_selected(&loc_y));

    selection.invert(&listing);
    // Now only loc_y should be selected
    assert_eq!(selection.len(), 1);
    assert!(selection.is_selected(&loc_y));
    assert!(!selection.is_selected(&loc_x));
    assert!(!selection.is_selected(&loc_z));
}
