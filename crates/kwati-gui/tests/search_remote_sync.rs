use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, Context, Entity, FocusHandle, Focusable, TestAppContext, VisualTestContext,
    Window,
};
use kwati_core::{
    AuthMethod, Backend, BackendError, ConnectionProfile, Entry, Listing, Location, Protocol,
    ReconnectingBackend, RetryPolicy,
};
use kwati_gui::actions::register_keybindings;
use kwati_gui::actions::remote_sync::{RemotePollerHost, RemotePollingState, RemoteSyncController};
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::AppState;
use kwati_gui::views::search_view::{SearchView, ToggleSearch};

struct SearchHarness {
    app_state: Entity<AppState>,
    search: Entity<SearchView>,
    list_focus: FocusHandle,
}

impl SearchHarness {
    fn new(app_state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| SearchView::new(app_state.clone(), cx));
        let list_focus = cx.focus_handle();
        list_focus.focus(window);
        cx.observe_in(&search, window, |this, search, window, cx| {
            if !search.read(cx).is_active() {
                this.list_focus.focus(window);
            }
            cx.notify();
        })
        .detach();
        Self {
            app_state,
            search,
            list_focus,
        }
    }
}

impl Focusable for SearchHarness {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl Render for SearchHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.list_focus)
            .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                this.search.update(cx, |search, cx| {
                    search.toggle(window, cx);
                });
            }))
            .child(self.search.clone())
    }
}

struct MockTreeBackend {
    width: usize,
    max_height: usize,
    delay: Duration,
    listed: Mutex<Vec<PathBuf>>,
    request_count: AtomicUsize,
}

impl MockTreeBackend {
    fn new(width: usize, max_height: usize, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            width,
            max_height,
            delay,
            listed: Mutex::new(Vec::new()),
            request_count: AtomicUsize::new(0),
        })
    }

    fn listed(&self) -> Vec<PathBuf> {
        self.listed.lock().unwrap().clone()
    }
}

fn tree_depth(path: &Path) -> usize {
    path.strip_prefix("/root")
        .map(|rel| rel.components().count() + 1)
        .unwrap_or(1)
}

impl Backend for MockTreeBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        self.request_count.fetch_add(1, Ordering::SeqCst);
        let dir = location.path().to_path_buf();
        self.listed.lock().unwrap().push(dir.clone());
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }

        let depth = tree_depth(&dir);
        let file_name = format!("target_d{depth}.txt");
        let mut entries = vec![Entry::new(
            OsString::from(&file_name),
            dir.join(&file_name),
            false,
        )];
        if depth < self.max_height {
            for i in 0..self.width {
                let sub = if self.width == 1 {
                    "sub".to_string()
                } else {
                    format!("sub{i}")
                };
                entries.push(Entry::new(OsString::from(&sub), dir.join(&sub), true));
            }
        }
        Ok(Listing::new(entries))
    }
}

struct SharedBackend(Arc<MockTreeBackend>);

impl Backend for SharedBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        self.0.list(location)
    }
}

struct MockPollBackend {
    entries: Mutex<Vec<Entry>>,
    request_count: AtomicUsize,
}

impl MockPollBackend {
    fn new(names: &[&str]) -> Arc<Self> {
        let entries = names
            .iter()
            .map(|name| {
                Entry::new(
                    OsString::from(name),
                    PathBuf::from("/remote").join(name),
                    false,
                )
            })
            .collect();
        Arc::new(Self {
            entries: Mutex::new(entries),
            request_count: AtomicUsize::new(0),
        })
    }

    fn set_names(&self, names: &[&str]) {
        let entries = names
            .iter()
            .map(|name| {
                Entry::new(
                    OsString::from(name),
                    PathBuf::from("/remote").join(name),
                    false,
                )
            })
            .collect();
        *self.entries.lock().unwrap() = entries;
    }

    fn requests(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }
}

struct SharedPollBackend(Arc<MockPollBackend>);

impl Backend for SharedPollBackend {
    fn list(&self, _location: &Location) -> Result<Listing, BackendError> {
        self.0.request_count.fetch_add(1, Ordering::SeqCst);
        let entries = self.0.entries.lock().unwrap().clone();
        Ok(Listing::new(entries))
    }
}

fn test_profile(id: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.to_string(),
        name: "Test Server".to_string(),
        protocol: Protocol::Sftp,
        host: "example.invalid".to_string(),
        port: 22,
        user: "tester".to_string(),
        start_path: "/root".to_string(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    }
}

fn wait_until(
    cx: &mut VisualTestContext,
    timeout: Duration,
    mut pred: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        cx.run_until_parked();
        if pred(cx) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.run_until_parked();
    pred(cx)
}

fn wait_until_app(
    cx: &mut TestAppContext,
    timeout: Duration,
    mut pred: impl FnMut(&mut TestAppContext) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        cx.run_until_parked();
        if pred(cx) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.run_until_parked();
    pred(cx)
}

fn setup_harness(
    cx: &mut TestAppContext,
    initial_location: Location,
    setup_state: impl FnOnce(&mut AppState) + 'static,
) -> (Entity<SearchHarness>, &mut VisualTestContext) {
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });
    cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut state = AppState::new(initial_location);
            state.attach(cx);
            setup_state(&mut state);
            state
        });
        SearchHarness::new(state, window, cx)
    })
}

#[gpui::test]
fn search_toggle_focuses_input_and_escape_cancels_and_closes(cx: &mut TestAppContext) {
    let temp = std::env::temp_dir().join(format!("kwati_search_toggle_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).unwrap();
    fs::write(temp.join("notes.txt"), "hello").unwrap();

    let (harness, cx) = setup_harness(cx, Location::Local(temp.clone()), |state| {
        state.reload_entries();
    });
    cx.run_until_parked();

    harness.read_with(cx, |h, cx| {
        assert!(!h.search.read(cx).is_active());
    });

    // Ctrl+F opens SearchView even while collapsed and focuses search_input.
    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();

    harness.update_in(cx, |h, window, cx| {
        let search = h.search.read(cx);
        assert!(search.is_active());
        assert!(
            search.search_input.read(cx).focus_handle.is_focused(window),
            "search_input must be focused when SearchView opens"
        );
    });

    cx.simulate_input("notes");
    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| {
            let s = h.search.read(cx);
            !s.is_searching && !s.hits().is_empty()
        })
    }));

    // Escape closes SearchView, clears query, and returns focus to the list.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    harness.update_in(cx, |h, window, cx| {
        let search = h.search.read(cx);
        assert!(!search.is_active());
        assert!(!search.is_searching);
        assert!(h.app_state.read(cx).search_query.is_none());
        assert!(h.list_focus.is_focused(window));
    });

    // Ctrl+F opens again, and Ctrl+F a second time toggles it closed.
    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    harness.read_with(cx, |h, cx| assert!(h.search.read(cx).is_active()));

    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    harness.read_with(cx, |h, cx| assert!(!h.search.read(cx).is_active()));

    let _ = fs::remove_dir_all(&temp);
}

#[gpui::test]
fn remote_search_respects_depth_limit_and_navigates_to_remote_hit(cx: &mut TestAppContext) {
    let backend = MockTreeBackend::new(1, 4, Duration::ZERO);
    let backend_for_connect = Arc::clone(&backend);
    let root_loc = Location::Remote {
        profile_id: "srv1".into(),
        path: PathBuf::from("/root"),
    };

    let (harness, cx) = setup_harness(cx, root_loc.clone(), move |state| {
        let _ = state.remote.save_profile(test_profile("srv1"), None);
        let reconnecting = ReconnectingBackend::new(
            move || Ok(Box::new(SharedBackend(Arc::clone(&backend_for_connect)))),
            RetryPolicy::default(),
        )
        .unwrap();
        state.remote.add_connection("srv1", reconnecting);
        state.settings.remote_search_depth = 3;
        state.settings.remote_search_parallel = 2;
        state.reload_entries();
    });
    cx.run_until_parked();

    // Open SearchView and verify depth-limit indicator reflects settings (3).
    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    harness.read_with(cx, |h, cx| {
        assert_eq!(h.search.read(cx).remote_depth_limit(cx), Some(3));
    });

    // Search for "target" at depth 3 -> finds depths 1, 2, 3 (excludes depth 4).
    cx.simulate_input("target");
    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| {
            let s = h.search.read(cx);
            !s.is_searching && s.hits().len() == 3
        })
    }));

    let hit_paths: Vec<PathBuf> = harness.read_with(cx, |h, cx| {
        h.search
            .read(cx)
            .hits()
            .iter()
            .map(|hit| hit.path.clone())
            .collect()
    });
    assert!(hit_paths.contains(&PathBuf::from("/root/target_d1.txt")));
    assert!(hit_paths.contains(&PathBuf::from("/root/sub/target_d2.txt")));
    assert!(hit_paths.contains(&PathBuf::from("/root/sub/sub/target_d3.txt")));
    assert!(!hit_paths.contains(&PathBuf::from("/root/sub/sub/sub/target_d4.txt")));

    // Lowering remote_search_depth to 1 excludes deeper matches on the next search.
    harness.update(cx, |h, cx| {
        h.app_state.update(cx, |state, cx| {
            state.set_remote_search_limits(1, 2).unwrap();
            cx.notify();
        });
        h.search.update(cx, |search, cx| {
            assert_eq!(search.remote_depth_limit(cx), Some(1));
            search.set_query("target", cx);
        });
    });

    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| !h.search.read(cx).is_searching)
    }));
    harness.read_with(cx, |h, cx| {
        let hits = h.search.read(cx).hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, PathBuf::from("/root/target_d1.txt"));
    });

    // Raise depth back to 3, search specifically for "d3", and press Enter to open the hit.
    harness.update(cx, |h, cx| {
        h.app_state.update(cx, |state, cx| {
            state.set_remote_search_limits(3, 2).unwrap();
            cx.notify();
        });
        h.search.update(cx, |search, cx| {
            search.set_query("target_d3", cx);
        });
    });
    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| {
            let s = h.search.read(cx);
            !s.is_searching && s.hits().len() == 1
        })
    }));

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    harness.read_with(cx, |h, cx| {
        assert!(!h.search.read(cx).is_active());
        let state = h.app_state.read(cx);
        assert_eq!(
            state.current_location(),
            &Location::Remote {
                profile_id: "srv1".into(),
                path: PathBuf::from("/root/sub/sub"),
            }
        );
        assert_eq!(
            state.selected_locations(),
            vec![Location::Remote {
                profile_id: "srv1".into(),
                path: PathBuf::from("/root/sub/sub/target_d3.txt"),
            }]
        );
    });
}

#[gpui::test]
fn remote_search_cancel_stops_inflight_search(cx: &mut TestAppContext) {
    // 3 subdirs per level * 6 levels with 25ms per folder would take seconds if not cancelled.
    let backend = MockTreeBackend::new(3, 6, Duration::from_millis(25));
    let backend_for_connect = Arc::clone(&backend);
    let root_loc = Location::Remote {
        profile_id: "srv-slow".into(),
        path: PathBuf::from("/root"),
    };

    let (harness, cx) = setup_harness(cx, root_loc, move |state| {
        let _ = state.remote.save_profile(test_profile("srv-slow"), None);
        let reconnecting = ReconnectingBackend::new(
            move || Ok(Box::new(SharedBackend(Arc::clone(&backend_for_connect)))),
            RetryPolicy::default(),
        )
        .unwrap();
        state.remote.add_connection("srv-slow", reconnecting);
        state.settings.remote_search_depth = 6;
        state.settings.remote_search_parallel = 1;
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    cx.simulate_input("target");

    // Wait for the root hit to stream in while the deeper tree walk is still running.
    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| !h.search.read(cx).hits().is_empty())
    }));

    // Cancel the in-flight search (keeps bar open and preserves hits found so far).
    harness.update(cx, |h, cx| {
        h.search.update(cx, |search, cx| {
            assert!(search.is_searching);
            search.cancel_search(cx);
        });
    });
    cx.run_until_parked();

    let listed_at_cancel = backend.listed().len();
    std::thread::sleep(Duration::from_millis(80));
    cx.run_until_parked();

    harness.read_with(cx, |h, cx| {
        let search = h.search.read(cx);
        assert!(search.is_active());
        assert!(!search.is_searching);
        assert!(!search.hits().is_empty());
    });
    assert!(
        backend.listed().len() <= listed_at_cancel + 1,
        "remote search must stop issuing directory requests after cancel"
    );
    assert!(
        backend.listed().len() < 20,
        "remote search should abort far before completing full 6-level tree"
    );
}

#[gpui::test]
fn local_search_hit_navigates_to_parent_and_selects_file(cx: &mut TestAppContext) {
    let temp = std::env::temp_dir().join(format!("kwati_local_search_jump_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let nested = temp.join("alpha").join("beta");
    fs::create_dir_all(&nested).unwrap();
    let target = nested.join("needle.rs");
    fs::write(&target, "pub fn needle() {}").unwrap();

    let (harness, cx) = setup_harness(cx, Location::Local(temp.clone()), |state| {
        state.reload_entries();
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    cx.simulate_input("needle");

    assert!(wait_until(cx, Duration::from_secs(2), |cx| {
        harness.read_with(cx, |h, cx| {
            let s = h.search.read(cx);
            !s.is_searching && s.hits().len() == 1
        })
    }));

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    harness.read_with(cx, |h, cx| {
        let state = h.app_state.read(cx);
        assert_eq!(state.current_location(), &Location::Local(nested.clone()));
        assert_eq!(
            state.selected_locations(),
            vec![Location::Local(target.clone())]
        );
    });

    let _ = fs::remove_dir_all(&temp);
}

#[gpui::test]
fn remote_poller_host_pauses_on_unfocus_and_preserves_selection(cx: &mut TestAppContext) {
    let backend = MockPollBackend::new(&["alpha.txt", "beta.txt"]);
    let backend_for_connect = Arc::clone(&backend);
    let remote_loc = Location::Remote {
        profile_id: "poll-srv".into(),
        path: PathBuf::from("/remote"),
    };

    let state = cx.new(|cx| {
        let mut state = AppState::new(remote_loc.clone());
        state.attach(cx);
        let _ = state.remote.save_profile(test_profile("poll-srv"), None);
        let reconnecting = ReconnectingBackend::new(
            move || {
                Ok(Box::new(SharedPollBackend(Arc::clone(
                    &backend_for_connect,
                ))))
            },
            RetryPolicy::default(),
        )
        .unwrap();
        state.remote.add_connection("poll-srv", reconnecting);
        state.reload_entries();
        state
    });
    cx.run_until_parked();

    let host = cx.new(|cx| {
        let mut host = RemotePollerHost::new(state.clone(), cx);
        host.set_poll_duration_override(Some(Duration::from_millis(25)), cx);
        host
    });
    cx.run_until_parked();

    // While auto-refresh is disabled, zero polling requests are issued.
    host.read_with(cx, |h, _| {
        assert_eq!(h.polling_state(), RemotePollingState::Disabled);
        assert!(!h.has_poller());
    });
    let reqs_disabled = backend.requests();
    std::thread::sleep(Duration::from_millis(80));
    cx.run_until_parked();
    assert_eq!(backend.requests(), reqs_disabled);

    // Select two remote locations before enabling auto-refresh.
    let alpha_loc = Location::Remote {
        profile_id: "poll-srv".into(),
        path: PathBuf::from("/remote/alpha.txt"),
    };
    let beta_loc = Location::Remote {
        profile_id: "poll-srv".into(),
        path: PathBuf::from("/remote/beta.txt"),
    };
    state.update(cx, |state, cx| {
        state.select_location(alpha_loc.clone());
        state.toggle_select_location(beta_loc.clone());
        state.set_remote_poll(true, 10).unwrap();
        cx.notify();
    });
    cx.run_until_parked();

    assert!(wait_until_app(cx, Duration::from_secs(2), |cx| {
        host.read_with(cx, |h, _| h.has_poller() && !h.is_poller_paused())
    }));

    // External change on remote: beta.txt deleted, gamma.txt created.
    backend.set_names(&["alpha.txt", "gamma.txt"]);
    assert!(wait_until_app(cx, Duration::from_secs(2), |cx| {
        state.read_with(cx, |s, _| s.entries.iter().any(|e| e.name() == "gamma.txt"))
    }));

    // Selection is preserved by Location for alpha.txt and dropped for deleted beta.txt.
    state.read_with(cx, |s, _| {
        assert_eq!(s.selected_locations(), vec![alpha_loc.clone()]);
    });

    // Losing window focus pauses RemotePoller immediately -> zero listing requests while unfocused.
    host.update(cx, |h, cx| {
        h.on_window_activation(false, cx);
        assert_eq!(h.polling_state(), RemotePollingState::PausedUnfocused);
        assert!(h.is_poller_paused());
    });
    let reqs_when_unfocused = backend.requests();
    backend.set_names(&["alpha.txt", "gamma.txt", "delta.txt"]);
    std::thread::sleep(Duration::from_millis(120));
    cx.run_until_parked();
    assert_eq!(
        backend.requests(),
        reqs_when_unfocused,
        "poller must send zero listing requests while window is unfocused"
    );
    state.read_with(cx, |s, _| {
        assert!(
            !s.entries.iter().any(|e| e.name() == "delta.txt"),
            "no refresh should occur while unfocused"
        );
    });

    // Regaining focus resumes polling and picks up delta.txt while keeping alpha.txt selected.
    host.update(cx, |h, cx| {
        h.on_window_activation(true, cx);
        assert_eq!(h.polling_state(), RemotePollingState::Active);
        assert!(!h.is_poller_paused());
    });
    assert!(wait_until_app(cx, Duration::from_secs(2), |cx| {
        state.read_with(cx, |s, _| s.entries.iter().any(|e| e.name() == "delta.txt"))
    }));
    state.read_with(cx, |s, _| {
        assert_eq!(s.selected_locations(), vec![alpha_loc.clone()]);
    });

    // Disabling auto-refresh stops the poller cleanly.
    state.update(cx, |state, cx| {
        state.set_remote_poll(false, 10).unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    host.read_with(cx, |h, _| {
        assert_eq!(h.polling_state(), RemotePollingState::Disabled);
        assert!(!h.has_poller());
    });
}

#[test]
fn remote_sync_controller_retain_selection_for_remote_locations() {
    let remote_dir = Location::Remote {
        profile_id: "p1".into(),
        path: PathBuf::from("/srv"),
    };
    let e1 = Entry::new("one.txt".into(), PathBuf::from("/srv/one.txt"), false);
    let _e2 = Entry::new("two.txt".into(), PathBuf::from("/srv/two.txt"), false);
    let e3 = Entry::new("three.txt".into(), PathBuf::from("/srv/three.txt"), false);

    let loc1 = Location::Remote {
        profile_id: "p1".into(),
        path: PathBuf::from("/srv/one.txt"),
    };
    let loc2 = Location::Remote {
        profile_id: "p1".into(),
        path: PathBuf::from("/srv/two.txt"),
    };

    let mut selection = kwati_core::Selection::new();
    selection.select(loc1.clone());
    selection.select(loc2.clone());

    RemoteSyncController::retain_selection_for_location(&mut selection, &remote_dir, &[e1, e3]);

    assert!(selection.is_selected(&loc1));
    assert!(!selection.is_selected(&loc2));
    assert_eq!(selection.len(), 1);
}
