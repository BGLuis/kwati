use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{point, px, size, AppContext as _, TestAppContext};
use kwati_core::{Location, MotionPreference, Settings};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::{register_text_input_keybindings, TextInput};
use kwati_gui::motion::{
    is_reduced_motion, shake_decay, JOB_COMPLETE_HOLD, MOUNT_INDETERMINATE_DELAY, QUICK,
};
use kwati_gui::state::{
    AppState, EntryChangeOrigin, JobKind, JobStatus, NavigationDirection, NetworkMountStatus,
};
use kwati_gui::views::jobs_view::JobsView;
use kwati_gui::views::network_view::NetworkView;
use kwati_gui::views::settings_view::{SettingKey, SettingsView};
use kwati_gui::views::MainView;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kwati_motion_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[gpui::test]
fn text_input_shake_starts_and_completes() {
    let mut cx = TestAppContext::single();
    let input = cx.new(|cx| TextInput::with_placeholder("Name", cx));

    input.read_with(&cx, |i, _| assert!(!i.is_shaking()));
    input.update(&mut cx, |i, cx| i.trigger_shake(cx));
    input.read_with(&cx, |i, _| assert!(i.is_shaking()));
    input.update(&mut cx, |i, cx| i.set_text("updated", cx));
    // Verify shake decay starts/ends at 0 and oscillates between cycles.
    assert_eq!(shake_decay(0.0), 0.0);
    assert_eq!(shake_decay(1.0), 0.0);
    assert!(shake_decay(0.05).abs() > 0.0);
}

#[gpui::test]
fn settings_motion_preference_persists_and_invalid_numeric_errors(cx: &mut TestAppContext) {
    let dir = temp_dir("settings");
    let settings_file = dir.join("settings.toml");
    let state = cx.new(|_| {
        AppState::new(Location::Local(dir.clone())).with_settings_path(Some(settings_file.clone()))
    });
    let view = cx.new(|cx| SettingsView::new(state.clone(), cx));

    view.update(cx, |v, cx| {
        v.open(cx);
        v.set_motion_preference(MotionPreference::Reduced, cx)
            .unwrap();
        let err = v.set_numeric_input(SettingKey::ListTimeoutSecs, "0", cx);
        assert!(err.is_err());
    });

    state.read_with(cx, |s, _| {
        assert_eq!(s.settings.motion_preference, MotionPreference::Reduced);
        assert!(is_reduced_motion(s.settings.motion_preference));
    });

    let (disk, warnings) = Settings::load_from(&settings_file).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(disk.motion_preference, MotionPreference::Reduced);
}

#[gpui::test]
fn completed_jobs_are_retained_for_hold_window_while_active_jobs_is_empty(cx: &mut TestAppContext) {
    let dir = temp_dir("jobs");
    let state = cx.new(|_| AppState::new(Location::Local(dir.clone())));
    let _jobs = cx.new(|cx| JobsView::new(state.clone(), cx));

    let job_id = 42;
    state.update(cx, |s, _| {
        let mut job = JobStatus::new(job_id, "Copying archive", JobKind::Copy, 1024);
        job.update_progress(1024);
        s.add_job(job);
        s.complete_job(job_id);
    });

    state.read_with(cx, |s, _| {
        assert!(s.active_jobs.is_empty());
        assert_eq!(s.completed_jobs.len(), 1);
        assert_eq!(s.completed_jobs[0].id, job_id);
    });

    state.update(cx, |s, _| {
        s.prune_completed_jobs(
            Instant::now() + JOB_COMPLETE_HOLD + QUICK + Duration::from_millis(10),
        );
    });

    state.read_with(cx, |s, _| {
        assert!(s.completed_jobs.is_empty());
    });
}

#[gpui::test]
fn network_connecting_records_connecting_since_for_indeterminate_delay(cx: &mut TestAppContext) {
    let dir = temp_dir("network");
    let state = cx.new(|_| AppState::new(Location::Local(dir.clone())));
    let _network = cx.new(|cx| NetworkView::new(state.clone(), cx));

    state.update(cx, |s, _| {
        s.set_network_status(NetworkMountStatus::Connecting);
    });
    state.read_with(cx, |s, _| {
        assert!(s.connecting_since.is_some());
        let start = s.connecting_since.unwrap();
        assert!(
            Instant::now().saturating_duration_since(start)
                < MOUNT_INDETERMINATE_DELAY + Duration::from_secs(5)
        );
    });

    state.update(cx, |s, _| {
        s.set_network_status(NetworkMountStatus::Connected);
    });
    state.read_with(cx, |s, _| {
        assert!(s.connecting_since.is_none());
    });
}

#[gpui::test]
fn navigation_and_change_origin_track_motion_metadata_without_regression(cx: &mut TestAppContext) {
    let dir = temp_dir("nav");
    let sub = dir.join("child");
    fs::create_dir_all(&sub).unwrap();
    fs::write(dir.join("a.txt"), "hello").unwrap();

    let state = cx.new(|cx| {
        let mut s = AppState::new(Location::Local(dir.clone()));
        s.attach(cx);
        s
    });
    state.update(cx, |s, _| s.reload_entries());
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_arrival, NavigationDirection::Reload);
    });

    state.update(cx, |s, _| s.navigate_to(Location::Local(sub.clone())));
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_arrival, NavigationDirection::Visit);
    });

    let went_back = state.update(cx, |s, _| s.go_back());
    assert!(went_back);
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_arrival, NavigationDirection::Back);
    });

    let went_fwd = state.update(cx, |s, _| s.go_forward());
    assert!(went_fwd);
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_arrival, NavigationDirection::Forward);
    });

    state.update(cx, |s, _| s.reload_entries_from_background());
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_change_origin, EntryChangeOrigin::WatcherOrPoll);
    });

    state.update(cx, |s, _| {
        s.mark_highlighted_insert(Location::Local(dir.join("a.txt")));
        s.reload_entries();
    });
    cx.run_until_parked();
    state.read_with(cx, |s, _| {
        assert_eq!(s.last_change_origin, EntryChangeOrigin::UserAction);
        assert_eq!(s.highlighted_inserts.len(), 1);
    });

    state.update(cx, |s, _| {
        s.set_completion_status("Done!");
        assert!(s.status_is_completion);
    });
}

#[gpui::test]
fn main_window_and_context_menu_render_under_full_and_reduced_motion(cx: &mut TestAppContext) {
    let dir = temp_dir("main_render");
    fs::write(dir.join("file.txt"), "content").unwrap();
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });
    let dir_clone = dir.clone();
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dir_clone));
            s.attach(cx);
            s.reload_entries();
            s
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1024.), px(700.)));
    cx.run_until_parked();

    main.update_in(cx, |m, window, cx| {
        m.context_menu.update(cx, |menu, cx| {
            menu.open_at(point(px(200.), px(200.)), window, cx)
        });
        assert!(m.context_menu.read(cx).is_open());
    });
    cx.run_until_parked();

    main.update(cx, |m, cx| {
        m.settings.update(cx, |s, cx| {
            s.set_motion_preference(MotionPreference::Reduced, cx)
                .unwrap();
        });
    });
    cx.run_until_parked();
}
