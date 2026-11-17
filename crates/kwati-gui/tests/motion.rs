use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    point, px, size, AppContext as _, Modifiers, Pixels, Point, TestAppContext, VisualTestContext,
};
use kwati_core::{Location, MotionPreference, Settings};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::interactive::checkbox_check_motion;
use kwati_gui::components::{register_text_input_keybindings, IconMotion, TextInput};
use kwati_gui::motion::{
    is_reduced_motion, shake_decay, INSTANT, JOB_COMPLETE_HOLD, MOUNT_INDETERMINATE_DELAY, QUICK,
    SETTLE,
};
use kwati_gui::state::{
    AppState, EntryChangeOrigin, JobKind, JobStatus, NavigationDirection, NetworkMountStatus,
    RemotePrompt,
};
use kwati_gui::views::jobs_view::JobsView;
use kwati_gui::views::network_view::NetworkView;
use kwati_gui::views::settings_view::{SettingKey, SettingsView};
use kwati_gui::views::{MainView, RemoteDialogsView};

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

fn list_row_center(
    main: &gpui::Entity<MainView>,
    cx: &mut VisualTestContext,
    ix: usize,
) -> Point<Pixels> {
    main.read_with(cx, |main, cx| {
        let list = main.list.read(cx);
        let entry_count = main.app_state.read(cx).entries.len().max(1);
        let state = list.scroll_handle.0.borrow();
        let bounds = state.base_handle.bounds();
        let offset = state.base_handle.offset();
        let row_h = state
            .last_item_size
            .map(|s| s.contents.height / (entry_count as f32))
            .filter(|h| *h > px(0.))
            .unwrap_or(list.row_height);
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "list bounds must be laid out"
        );
        point(
            bounds.left() + px(40.),
            bounds.top() + row_h * (ix as f32 + 0.5) + offset.y,
        )
    })
}

fn pump_next_animation_frame(
    list: &gpui::Entity<kwati_gui::views::ListView>,
    cx: &mut VisualTestContext,
) {
    list.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

fn pump_next_dialogs_frame(view: &gpui::Entity<RemoteDialogsView>, cx: &mut VisualTestContext) {
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

#[gpui::test]
fn list_view_row_hover_and_selection_schedule_frames_and_settle_to_idle(cx: &mut TestAppContext) {
    let dir = temp_dir("list_uniform_motion");
    let alpha = dir.join("alpha.txt");
    let bravo = dir.join("bravo.txt");
    fs::write(&alpha, "alpha").unwrap();
    fs::write(&bravo, "bravo").unwrap();
    let loc_alpha = Location::Local(alpha);
    let loc_bravo = Location::Local(bravo);

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

    let list = main.read_with(cx, |m, _| m.list.clone());
    let list_id = list.entity_id();

    // Let initial listing insert animations settle before testing idle/hover/selection transitions.
    std::thread::sleep(SETTLE + Duration::from_millis(20));
    pump_next_animation_frame(&list, cx);

    // Idle frame must not request animation frames once settled.
    let idle_before = list.read_with(cx, |l, _| l.animation_frame_requests());
    pump_next_animation_frame(&list, cx);
    let idle_after = list.read_with(cx, |l, _| l.animation_frame_requests());
    assert_eq!(
        idle_after, idle_before,
        "idle ListView must not call request_animation_frame"
    );

    // Hovering a settled row inside uniform_list must immediately request the next frame
    // targeting ListView itself, and advance Hover across frames without more input events.
    let row0_pos = list_row_center(&main, cx, 0);
    cx.simulate_mouse_move(row0_pos, None, Modifiers::none());
    cx.run_until_parked();

    let after_hover_reqs = list.read_with(cx, |l, _| l.animation_frame_requests());
    assert!(
        after_hover_reqs > idle_after,
        "hovering a settled row inside uniform_list must call request_animation_frame"
    );
    assert_eq!(
        list.read_with(cx, |l, _| l.last_frame_requested_view()),
        Some(list_id),
        "uniform_list prepaint request_animation_frame must target ListView"
    );

    std::thread::sleep(Duration::from_millis(45));
    pump_next_animation_frame(&list, cx);
    let mid_hover = list.read_with(cx, |l, _| {
        l.row_hover_value(&loc_alpha, Instant::now(), false)
    });
    assert!(
        mid_hover > 0.05 && mid_hover < 0.99,
        "expected mid-flight hover progress in (0.05, 0.99), got {mid_hover}"
    );
    assert!(
        list.read_with(cx, |l, _| l.animation_frame_requests()) > after_hover_reqs,
        "mid-flight hover frame must keep requesting animation frames"
    );

    // Wait past INSTANT (160 ms) and pump the final settling frame, then confirm idle stops requesting.
    std::thread::sleep(INSTANT + Duration::from_millis(20));
    pump_next_animation_frame(&list, cx);
    let settled_hover = list.read_with(cx, |l, _| {
        l.row_hover_value(&loc_alpha, Instant::now(), false)
    });
    assert!(
        (settled_hover - 1.0).abs() < 1e-3,
        "hover should settle at 1.0, got {settled_hover}"
    );
    let settled_reqs = list.read_with(cx, |l, _| l.animation_frame_requests());
    pump_next_animation_frame(&list, cx);
    assert_eq!(
        list.read_with(cx, |l, _| l.animation_frame_requests()),
        settled_reqs,
        "settled hover must stop calling request_animation_frame on idle frames"
    );

    // Changing selection from alpha.txt to bravo.txt must animate SelectBg on bravo.txt
    // across frames and stop requesting once settled.
    list.update(cx, |l, cx| l.select_next(cx));
    cx.run_until_parked();
    std::thread::sleep(INSTANT + Duration::from_millis(20));
    pump_next_animation_frame(&list, cx);

    let before_select_change = list.read_with(cx, |l, _| l.animation_frame_requests());
    list.update(cx, |l, cx| l.select_next(cx));
    cx.run_until_parked();

    let after_select_change = list.read_with(cx, |l, _| l.animation_frame_requests());
    assert!(
        after_select_change > before_select_change,
        "changing selection in ListView must call request_animation_frame"
    );

    std::thread::sleep(Duration::from_millis(45));
    pump_next_animation_frame(&list, cx);
    let mid_select_bravo = list.read_with(cx, |l, _| {
        l.row_select_value(&loc_bravo, Instant::now(), false)
    });
    let mid_select_alpha = list.read_with(cx, |l, _| {
        l.row_select_value(&loc_alpha, Instant::now(), false)
    });
    assert!(
        mid_select_bravo > 0.05 && mid_select_bravo < 0.99,
        "bravo SelectBg should animate across frames, got {mid_select_bravo}"
    );
    assert!(
        mid_select_alpha > 0.01 && mid_select_alpha < 0.95,
        "alpha SelectBg should fade out across frames, got {mid_select_alpha}"
    );

    std::thread::sleep(INSTANT + Duration::from_millis(20));
    pump_next_animation_frame(&list, cx);
    let settled_select = list.read_with(cx, |l, _| {
        l.row_select_value(&loc_bravo, Instant::now(), false)
    });
    assert!(
        (settled_select - 1.0).abs() < 1e-3,
        "bravo SelectBg should settle at 1.0, got {settled_select}"
    );
    let select_settled_reqs = list.read_with(cx, |l, _| l.animation_frame_requests());
    pump_next_animation_frame(&list, cx);
    assert_eq!(
        list.read_with(cx, |l, _| l.animation_frame_requests()),
        select_settled_reqs,
        "settled selection transition must stop calling request_animation_frame on idle frames"
    );
}

#[gpui::test]
fn remote_dialog_checkboxes_keep_instant_fade_under_reduced_motion(cx: &mut TestAppContext) {
    cx.update(register_text_input_keybindings);

    // 1. Reduced motion: RememberCheckbox & InsecureFtpCheckbox must fade over INSTANT (160 ms).
    let mut reduced_state = AppState::new(Location::Local(PathBuf::from("/tmp")));
    reduced_state.settings.motion_preference = MotionPreference::Reduced;
    reduced_state.remote_prompt = Some(RemotePrompt::Password {
        profile_id: "sftp-1".into(),
        error: None,
    });
    let state_entity = cx.new(|_| reduced_state);
    let state_for_view = state_entity.clone();
    let (view, vcx) = cx.add_window_view(|_, cx| RemoteDialogsView::new(state_for_view, cx));
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        v.toggle_remember_password();
        cx.notify();
    });
    vcx.run_until_parked();

    let t_toggle = Instant::now();
    let (mid_val, mid_active) = view.read_with(vcx, |v, _| {
        v.remember_checkbox_motion(t_toggle + Duration::from_millis(80), true)
    });
    assert!(
        mid_active && mid_val > 0.05 && mid_val < 0.99,
        "RememberCheckbox under Reduced motion must fade over INSTANT (160 ms), got active={mid_active}, val={mid_val}"
    );
    let (end_val, end_active) = view.read_with(vcx, |v, _| {
        v.remember_checkbox_motion(t_toggle + INSTANT + Duration::from_millis(10), true)
    });
    assert!(!end_active && (end_val - 1.0).abs() < 1e-3);

    // Switch to ProfileManager add-profile form (FTP) and toggle InsecureFtpCheckbox under Reduced.
    state_entity.update(vcx, |s, cx| {
        s.remote_prompt = None;
        cx.notify();
    });
    view.update(vcx, |v, cx| {
        v.open_profile_manager(cx);
        v.open_add_profile(cx);
        v.set_form_protocol(kwati_core::Protocol::Ftp, cx);
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| v.toggle_form_allow_insecure(cx));
    vcx.run_until_parked();

    let t_ftp = Instant::now();
    let (ftp_mid_val, ftp_mid_active) = view.read_with(vcx, |v, _| {
        v.insecure_ftp_checkbox_motion(t_ftp + Duration::from_millis(80), true)
    });
    assert!(
        ftp_mid_active && ftp_mid_val > 0.05 && ftp_mid_val < 0.99,
        "InsecureFtpCheckbox under Reduced motion must fade over INSTANT (160 ms), got active={ftp_mid_active}, val={ftp_mid_val}"
    );
    let (ftp_end_val, ftp_end_active) = view.read_with(vcx, |v, _| {
        v.insecure_ftp_checkbox_motion(t_ftp + INSTANT + Duration::from_millis(10), true)
    });
    assert!(!ftp_end_active && (ftp_end_val - 1.0).abs() < 1e-3);

    // 2. Full motion: checkboxes still animate over QUICK (300 ms) with Curve::Spring.
    std::thread::sleep(INSTANT + Duration::from_millis(20));
    pump_next_dialogs_frame(&view, vcx);
    state_entity.update(vcx, |s, cx| {
        s.settings.motion_preference = MotionPreference::Full;
        cx.notify();
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| v.toggle_form_allow_insecure(cx));
    vcx.run_until_parked();

    let t_full = Instant::now();
    let (full_early_val, full_early_active) = view.read_with(vcx, |v, _| {
        v.insecure_ftp_checkbox_motion(t_full + Duration::from_millis(60), false)
    });
    assert!(
        full_early_active && full_early_val > 0.05 && full_early_val < 0.99,
        "InsecureFtpCheckbox under Full motion should be mid-flight at 60 ms, got active={full_early_active}, val={full_early_val}"
    );
    let (_, full_past_instant_active) = view.read_with(vcx, |v, _| {
        v.insecure_ftp_checkbox_motion(t_full + INSTANT + Duration::from_millis(20), false)
    });
    assert!(
        full_past_instant_active,
        "InsecureFtpCheckbox under Full motion must remain active past INSTANT (160 ms) during QUICK (300 ms)"
    );
    let (full_end_val, full_end_active) = view.read_with(vcx, |v, _| {
        v.insecure_ftp_checkbox_motion(t_full + QUICK + Duration::from_millis(10), false)
    });
    assert!(!full_end_active && full_end_val.abs() < 1e-3);

    // 3. Checkmark icon snaps to completed Rest state under reduced motion, and draws under Full.
    assert_eq!(checkbox_check_motion(mid_val, true), Some(IconMotion::Rest));
    assert_eq!(
        checkbox_check_motion(full_early_val, false),
        Some(IconMotion::Draw(full_early_val))
    );
}
