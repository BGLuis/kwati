use std::ffi::OsString;
use std::path::PathBuf;

use kwati_core::{Entry, Location};
use kwati_gui::components::ScrollbarState;
use kwati_gui::state::AppState;

fn make_dummy_entries(count: usize) -> Vec<Entry> {
    (0..count)
        .map(|i| {
            let is_dir = i % 5 == 0;
            let name = format!("item_{:04}.txt", i);
            let path = PathBuf::from(format!("/tmp/{}", name));
            Entry::new(OsString::from(name), path, is_dir).with_size((i as u64) * 1024)
        })
        .collect()
}

#[test]
fn virtualized_list_selection_and_scrollbar_metrics() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let entries = make_dummy_entries(1000);
    state.set_entries(entries);

    assert_eq!(state.entries.len(), 1000);
    assert_eq!(state.selected_index(), None);

    // Select index 42
    state.select_index(42);
    assert_eq!(state.selected_index(), Some(42));
    assert!(state.selection.contains(&state.entries[42].location()));

    // Scrollbar state for 1000 items
    let mut scrollbar = ScrollbarState::new(1000, 25);
    assert!(scrollbar.can_scroll());
    assert_eq!(scrollbar.scroll_ratio(), 0.0);
    assert_eq!(scrollbar.thumb_ratio(), 0.05); // clamped minimum thumb ratio

    // Scroll offset midway
    scrollbar.set_metrics(1000, 25, 487);
    assert_eq!(scrollbar.current_offset, 487);
    assert!((scrollbar.scroll_ratio() - 0.499).abs() < 0.01);

    // Dragging / scrolling to the end reaches max offset (1000 - 25 = 975)
    scrollbar.set_metrics(1000, 25, 1000);
    assert_eq!(scrollbar.current_offset, 975);
    assert_eq!(scrollbar.scroll_ratio(), 1.0);
}
