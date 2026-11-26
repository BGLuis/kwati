//! Grid thumbnails: the cache's bookkeeping and the list view building pictures for image files.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{px, size, AppContext as _, Entity, TestAppContext, VisualTestContext};
use image::{Rgba, RgbaImage};
use kwati_core::{Entry, Location, Settings, ViewMode};
use kwati_gui::components::thumbnails::{
    render_image_from_rgba, ThumbnailCache, ThumbnailKey, MAX_QUEUED_THUMBNAILS,
    MAX_READY_THUMBNAILS, MAX_THUMBNAIL_JOBS,
};
use kwati_gui::state::AppState;
use kwati_gui::views::MainView;

fn key(name: &str) -> ThumbnailKey {
    ThumbnailKey::for_entry(
        Location::Local(PathBuf::from("/pics").join(name)),
        &Entry::new(name.into(), PathBuf::from("/pics").join(name), false).with_size(10),
    )
}

fn picture() -> Arc<gpui::RenderImage> {
    render_image_from_rgba(RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255])))
}

#[test]
fn render_image_swaps_to_bgra_for_gpui() {
    let image = render_image_from_rgba(RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 40])));
    assert_eq!(image.as_bytes(0), Some(&[30, 20, 10, 40][..]));
}

#[test]
fn cache_requests_once_limits_jobs_and_serves_the_result() {
    let mut cache = ThumbnailCache::default();
    cache.sync_settings(&Settings::default(), 80);
    let keys: Vec<_> = (0..MAX_THUMBNAIL_JOBS + 2)
        .map(|i| key(&format!("{i}.png")))
        .collect();
    for k in &keys {
        assert!(cache.get_or_request(k).is_none());
        assert!(
            cache.get_or_request(k).is_none(),
            "a second sighting must not queue again"
        );
    }
    assert_eq!(cache.queued_count(), keys.len());

    let jobs = cache.start_jobs();
    assert_eq!(jobs.len(), MAX_THUMBNAIL_JOBS);
    assert_eq!(
        jobs[0].key,
        *keys.last().unwrap(),
        "the newest request is what is on screen now"
    );
    assert!(cache.start_jobs().is_empty(), "every worker is busy");

    cache.finish(&jobs[0], Some(picture()));
    cache.finish(&jobs[1], None);
    assert!(cache.get_or_request(&jobs[0].key).is_some());
    assert!(
        cache.get_or_request(&jobs[1].key).is_none(),
        "a failed file keeps its icon"
    );
    assert_eq!(cache.queued_count(), 2, "failures are not retried");
    assert_eq!(
        cache.start_jobs().len(),
        2,
        "finished jobs free their workers"
    );
}

#[test]
fn cache_drops_oldest_requests_and_evicts_oldest_pictures() {
    let mut cache = ThumbnailCache::default();
    cache.sync_settings(&Settings::default(), 80);
    for i in 0..MAX_QUEUED_THUMBNAILS + 1 {
        cache.get_or_request(&key(&format!("{i}.png")));
    }
    assert_eq!(cache.queued_count(), MAX_QUEUED_THUMBNAILS);

    let mut cache = ThumbnailCache::default();
    cache.sync_settings(&Settings::default(), 80);
    let first = key("first.png");
    cache.get_or_request(&first);
    let job = cache.start_jobs().remove(0);
    cache.finish(&job, Some(picture()));
    for i in 0..MAX_READY_THUMBNAILS {
        let k = key(&format!("{i}.png"));
        cache.get_or_request(&k);
        for job in cache.start_jobs() {
            cache.finish(&job, Some(picture()));
        }
    }
    assert_eq!(cache.ready_count(), MAX_READY_THUMBNAILS);
    assert_eq!(
        cache.take_evicted().len(),
        1,
        "evicted pictures go back to the GPU atlas"
    );
    assert!(
        cache.get_or_request(&first).is_none(),
        "the oldest picture was evicted"
    );
}

#[test]
fn changing_settings_discards_pictures_and_late_results() {
    let mut settings = Settings::default();
    let mut cache = ThumbnailCache::default();
    cache.sync_settings(&settings, 80);
    let ready = key("ready.png");
    let late = key("late.png");
    cache.get_or_request(&ready);
    cache.get_or_request(&late);
    let jobs = cache.start_jobs();
    let (late_job, ready_job) = (&jobs[0], &jobs[1]);
    cache.finish(ready_job, Some(picture()));

    cache.sync_settings(&settings, 80);
    assert_eq!(cache.ready_count(), 1, "identical settings keep the cache");

    settings.thumbnail_max_file_size_mb = 5;
    cache.sync_settings(&settings, 80);
    assert_eq!(cache.ready_count(), 0);
    assert_eq!(cache.take_evicted().len(), 1);
    cache.finish(late_job, Some(picture()));
    assert_eq!(
        cache.ready_count(),
        0,
        "a result built under old settings is dropped"
    );

    cache.sync_settings(&settings, 160);
    assert_eq!(
        cache.max_px(),
        160,
        "a new display scale rebuilds at the new size"
    );
}

#[test]
fn a_changed_file_is_a_new_key() {
    let path = PathBuf::from("/pics/a.png");
    let location = Location::Local(path.clone());
    let before = Entry::new("a.png".into(), path.clone(), false).with_size(10);
    let after = Entry::new("a.png".into(), path, false).with_size(11);
    assert_ne!(
        ThumbnailKey::for_entry(location.clone(), &before),
        ThumbnailKey::for_entry(location, &after)
    );
}

fn image_fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "kwati_test_thumbnails_{name}_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("album.png")).unwrap();
    RgbaImage::from_pixel(64, 32, Rgba([0, 128, 255, 255]))
        .save(root.join("photo.png"))
        .unwrap();
    RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255]))
        .save(root.join("small.bmp"))
        .unwrap();
    fs::write(root.join("broken.png"), b"not a png").unwrap();
    fs::write(root.join("notes.txt"), b"text").unwrap();
    root
}

fn open_window<'a>(
    cx: &'a mut TestAppContext,
    dir: &Path,
    mode: ViewMode,
) -> (Entity<MainView>, &'a mut VisualTestContext) {
    let location = Location::Local(dir.to_path_buf());
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut state = AppState::new(location);
            state.attach(cx);
            state.reload_entries();
            state.set_view_mode(mode);
            state
        });
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();
    (main, cx)
}

fn ready_thumbnails(main: &Entity<MainView>, cx: &mut VisualTestContext) -> usize {
    main.read_with(cx, |main, cx| main.list.read(cx).ready_thumbnail_count())
}

#[gpui::test]
fn grid_builds_thumbnails_only_for_decodable_listed_images(cx: &mut TestAppContext) {
    let dir = image_fixture("grid");
    let (main, cx) = open_window(cx, &dir, ViewMode::Grid);
    cx.run_until_parked();
    assert_eq!(
        ready_thumbnails(&main, cx),
        2,
        "photo.png and small.bmp; not the folder, the text file, or the corrupt png"
    );

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state
                .update_settings(|s| s.set_thumbnail_extensions("bmp").map_err(|e| e.join(",")))
                .unwrap();
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(
        ready_thumbnails(&main, cx),
        1,
        "only the bmp is still wanted"
    );

    main.update(cx, |main, cx| {
        main.app_state.update(cx, |state, cx| {
            state
                .update_settings(|s| {
                    s.thumbnails_enabled = false;
                    Ok(())
                })
                .unwrap();
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(ready_thumbnails(&main, cx), 0);

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn list_modes_keep_icons_and_build_nothing(cx: &mut TestAppContext) {
    let dir = image_fixture("details");
    let (main, cx) = open_window(cx, &dir, ViewMode::Details);
    cx.run_until_parked();
    assert_eq!(ready_thumbnails(&main, cx), 0);

    let _ = fs::remove_dir_all(&dir);
}
