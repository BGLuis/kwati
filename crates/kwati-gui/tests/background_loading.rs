use std::path::PathBuf;

use gpui::{AppContext as _, TestAppContext};
use kwati_core::Location;
use kwati_gui::state::AppState;

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("kwati_test_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("first")).unwrap();
    std::fs::create_dir_all(root.join("second")).unwrap();
    std::fs::write(root.join("second").join("inside.txt"), "x").unwrap();
    root
}

#[gpui::test]
fn attached_state_reads_off_the_main_thread(cx: &mut TestAppContext) {
    let root = fixture("background_read");
    let state = cx.new(|cx| {
        let mut state = AppState::new(Location::Local(root.clone()));
        state.attach(cx);
        state
    });

    state.update(cx, |state, _| {
        state.navigate_to(Location::Local(root.join("second")));
        // The window stays on the old directory until the read comes back.
        assert_eq!(state.current_location, Location::Local(root.clone()));
        assert!(state.entries.is_empty());
    });

    cx.run_until_parked();

    state.read_with(cx, |state, _| {
        assert_eq!(state.current_location, Location::Local(root.join("second")));
        assert_eq!(state.entries.len(), 1);
        assert!(state.history.can_go_back());
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui::test]
fn a_newer_navigation_cancels_the_one_still_loading(cx: &mut TestAppContext) {
    let root = fixture("superseded_read");
    let state = cx.new(|cx| {
        let mut state = AppState::new(Location::Local(root.clone()));
        state.attach(cx);
        state
    });

    state.update(cx, |state, _| {
        state.navigate_to(Location::Local(root.join("second")));
        state.navigate_to(Location::Local(root.join("first")));
    });
    cx.run_until_parked();

    state.read_with(cx, |state, _| {
        assert_eq!(state.current_location, Location::Local(root.join("first")));
        assert!(state.entries.is_empty());
        assert_eq!(state.history.len(), 2);
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui::test]
fn a_refresh_does_not_cancel_a_navigation_still_loading(cx: &mut TestAppContext) {
    let root = fixture("refresh_during_read");
    let state = cx.new(|cx| {
        let mut state = AppState::new(Location::Local(root.clone()));
        state.attach(cx);
        state
    });

    state.update(cx, |state, _| {
        state.navigate_to(Location::Local(root.join("second")));
        state.reload_entries();
    });
    cx.run_until_parked();

    state.read_with(cx, |state, _| {
        assert_eq!(state.current_location, Location::Local(root.join("second")));
        assert_eq!(state.entries.len(), 1);
    });
    let _ = std::fs::remove_dir_all(&root);
}
