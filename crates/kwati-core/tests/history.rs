use kwati_core::{History, Location};
use std::path::PathBuf;

#[test]
fn history_back_forward() {
    let mut history = History::new();
    let a = PathBuf::from("/a");
    let b = PathBuf::from("/b");
    let c = PathBuf::from("/c");

    history.visit(a.clone());
    history.visit(b.clone());
    history.visit(c.clone());

    assert_eq!(history.back(), Some(&b));
    assert_eq!(history.back(), Some(&a));
    assert_eq!(history.forward(), Some(&b));
}

#[test]
fn history_truncates() {
    let mut history = History::new();
    let a = PathBuf::from("/a");
    let b = PathBuf::from("/b");
    let c = PathBuf::from("/c");
    let d = PathBuf::from("/d");

    history.visit(a);
    history.visit(b.clone());
    history.visit(c);

    assert_eq!(history.back(), Some(&b));
    history.visit(d.clone());

    assert_eq!(history.forward(), None);
    assert_eq!(history.current(), Some(&d));
    assert_eq!(history.back(), Some(&b));
}

#[test]
fn history_bounds() {
    let mut history = History::new();

    // Empty history bounds
    assert_eq!(history.back(), None);
    assert_eq!(history.forward(), None);
    assert_eq!(history.current(), None);
    assert!(!history.can_go_back());
    assert!(!history.can_go_forward());

    let a = PathBuf::from("/a");
    history.visit(a.clone());

    // Back at oldest entry returns None and keeps position
    assert_eq!(history.back(), None);
    assert_eq!(history.current(), Some(&a));
    assert!(!history.can_go_back());

    // Forward at newest entry returns None and keeps position
    assert_eq!(history.forward(), None);
    assert_eq!(history.current(), Some(&a));
    assert!(!history.can_go_forward());

    let b = PathBuf::from("/b");
    history.visit(b.clone());

    assert!(history.can_go_back());
    assert!(!history.can_go_forward());

    // Forward at newest entry still returns None and keeps position
    assert_eq!(history.forward(), None);
    assert_eq!(history.current(), Some(&b));

    // Back to oldest
    assert_eq!(history.back(), Some(&a));
    assert!(!history.can_go_back());
    assert!(history.can_go_forward());

    // Back again at oldest
    assert_eq!(history.back(), None);
    assert_eq!(history.current(), Some(&a));
}

#[test]
fn history_with_locations() {
    let loc_local = Location::Local(PathBuf::from("/local/dir"));
    let loc_remote = Location::Remote {
        profile_id: "sftp_home".to_string(),
        path: PathBuf::from("/remote/home"),
    };

    let mut history = History::with_initial(loc_local.clone());
    assert_eq!(history.len(), 1);
    assert_eq!(history.current(), Some(&loc_local));

    history.visit(loc_remote.clone());
    assert_eq!(history.len(), 2);
    assert_eq!(history.current(), Some(&loc_remote));

    assert_eq!(history.back(), Some(&loc_local));
    assert_eq!(history.forward(), Some(&loc_remote));
}

#[test]
fn history_clear_and_default() {
    let mut history: History<PathBuf> = History::default();
    assert!(history.is_empty());

    history.visit(PathBuf::from("/1"));
    history.visit(PathBuf::from("/2"));
    assert_eq!(history.len(), 2);
    assert_eq!(history.index(), 1);

    history.clear();
    assert!(history.is_empty());
    assert_eq!(history.len(), 0);
    assert_eq!(history.current(), None);
    assert_eq!(history.back(), None);
    assert_eq!(history.forward(), None);
}
