use kwati_core::{Entry, Location};
use kwati_gui::state::{
    AppState, JobKind, JobStatus, NetworkMountStatus, PreviewData, PreviewKind, UndoOperation,
    UndoStack,
};
use kwati_gui::theme::Theme;
use std::path::PathBuf;

#[test]
fn app_state_navigation_and_history() {
    let root = std::env::temp_dir().join(format!("kwati_test_state_nav_{}", std::process::id()));
    let docs = root.join("docs");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&docs).unwrap();

    let initial = Location::Local(root.clone());
    let mut state = AppState::new(initial.clone());

    assert_eq!(state.current_location, initial);
    assert!(state.selection.is_empty());

    let next = Location::Local(docs);
    state.navigate_to(next.clone());

    assert_eq!(state.current_location, next);
    assert!(state.history.can_go_back());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn app_state_entries_and_selection() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let entry_a = Entry::new("a.txt".into(), PathBuf::from("/tmp/a.txt"), false);
    let entry_b = Entry::new("b.txt".into(), PathBuf::from("/tmp/b.txt"), false);

    state.set_entries(vec![entry_a.clone(), entry_b.clone()]);
    assert_eq!(state.entries.len(), 2);

    state.selection.select_only(entry_a.location());
    assert_eq!(state.selection.len(), 1);
    assert!(state.selection.contains(&entry_a.location()));

    // Reload with only entry_b - selection of entry_a should be pruned
    state.set_entries(vec![entry_b.clone()]);
    assert!(state.selection.is_empty());
}

#[test]
fn app_state_jobs_and_preview() {
    let mut state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    let job = JobStatus::new(1, "Copy file", JobKind::Copy, 1000);
    state.add_job(job);
    assert_eq!(state.active_jobs.len(), 1);

    state.active_jobs[0].update_progress(500);
    assert_eq!(state.active_jobs[0].progress_pct, 50);

    state.remove_job(1);
    assert!(state.active_jobs.is_empty());

    let preview = PreviewData {
        path: PathBuf::from("/tmp/test.txt"),
        kind: PreviewKind::Text("hello world".into()),
    };
    state.set_preview(Some(preview));
    assert!(state.preview_state.is_some());

    state.set_network_status(NetworkMountStatus::Connecting);
    assert_eq!(state.network_status, NetworkMountStatus::Connecting);
}

#[test]
fn undo_stack_operations() {
    let mut stack = UndoStack::new();
    assert!(stack.is_empty());

    stack.push(UndoOperation::CreateDir {
        path: PathBuf::from("/tmp/new_dir"),
    });
    assert_eq!(stack.len(), 1);
    assert!(!stack.is_empty());

    let popped = stack.pop();
    assert!(matches!(popped, Some(UndoOperation::CreateDir { .. })));
    assert!(stack.is_empty());
}

#[test]
fn theme_defaults() {
    let dark = Theme::dark();
    let light = Theme::light();

    assert_ne!(dark.bg_app, light.bg_app);
    assert_ne!(dark.text_primary, light.text_primary);
}
