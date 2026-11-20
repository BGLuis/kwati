//! The sidebar and preview give way to the file list as the window narrows, and follow the
//! pointer when their edges are dragged.

use std::fs;
use std::path::PathBuf;

use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, MouseButton, Pixels, TestAppContext,
    VisualTestContext,
};
use kwati_core::Location;
use kwati_gui::layout::{
    PanelLayout, ResolvedLayout, SidebarWidth, LIST_MIN, PREVIEW_DEFAULT, PREVIEW_MIN,
    SIDEBAR_COMPACT, SIDEBAR_DEFAULT, SIDEBAR_MIN, WINDOW_MIN_WIDTH,
};
use kwati_gui::state::AppState;
use kwati_gui::views::MainView;

fn expanded(sidebar: Pixels, preview: Pixels) -> ResolvedLayout {
    ResolvedLayout {
        sidebar,
        sidebar_compact: false,
        preview: Some(preview),
    }
}

#[test]
fn a_wide_window_shows_the_widths_the_user_chose() {
    let layout = PanelLayout::default();
    assert_eq!(
        layout.resolve(px(1400.)),
        expanded(SIDEBAR_DEFAULT, PREVIEW_DEFAULT)
    );
}

#[test]
fn the_preview_narrows_before_the_sidebar_does() {
    let layout = PanelLayout::default();
    // 40 px short of the chosen widths: all of it comes out of the preview.
    let width = LIST_MIN + SIDEBAR_DEFAULT + PREVIEW_DEFAULT - px(40.);
    assert_eq!(
        layout.resolve(width),
        expanded(SIDEBAR_DEFAULT, PREVIEW_DEFAULT - px(40.))
    );
}

#[test]
fn the_sidebar_narrows_once_the_preview_is_at_its_minimum() {
    let layout = PanelLayout::default();
    let width = LIST_MIN + SIDEBAR_MIN + PREVIEW_MIN + px(10.);
    assert_eq!(
        layout.resolve(width),
        expanded(SIDEBAR_MIN + px(10.), PREVIEW_MIN)
    );
}

#[test]
fn the_sidebar_drops_to_icons_before_the_preview_hides() {
    let layout = PanelLayout::default();
    let width = LIST_MIN + SIDEBAR_MIN + PREVIEW_MIN - px(1.);
    let resolved = layout.resolve(width);
    assert!(resolved.sidebar_compact);
    assert_eq!(resolved.sidebar, SIDEBAR_COMPACT);
    assert!(resolved.preview.is_some());
}

#[test]
fn the_narrowest_window_keeps_the_list_and_icon_sidebar_only() {
    let resolved = PanelLayout::default().resolve(WINDOW_MIN_WIDTH);
    assert!(resolved.sidebar_compact);
    assert_eq!(resolved.preview, None);
    assert!(WINDOW_MIN_WIDTH - resolved.sidebar >= LIST_MIN);
}

#[test]
fn the_list_never_gets_less_than_its_minimum_while_both_panes_show() {
    let layout = PanelLayout {
        sidebar: SidebarWidth::Expanded(px(360.)),
        preview: px(600.),
        preview_open: true,
        sidebar_open: true,
    };
    for width in (540..=1600).step_by(7) {
        let width = px(width as f32);
        let resolved = layout.resolve(width);
        let panes = resolved.sidebar + resolved.preview.unwrap_or(px(0.));
        assert!(width - panes >= LIST_MIN, "list squeezed at {width:?}");
    }
}

#[test]
fn widening_the_window_restores_the_chosen_widths() {
    let layout = PanelLayout::default();
    layout.resolve(WINDOW_MIN_WIDTH);
    assert_eq!(
        layout.resolve(px(1400.)),
        expanded(SIDEBAR_DEFAULT, PREVIEW_DEFAULT)
    );
}

#[test]
fn dragging_the_sidebar_near_the_window_edge_collapses_it_to_icons() {
    let mut layout = PanelLayout::default();
    layout.drag_sidebar(px(60.), px(1200.));
    assert_eq!(layout.sidebar, SidebarWidth::Compact);
    layout.drag_sidebar(px(240.), px(1200.));
    assert_eq!(layout.sidebar, SidebarWidth::Expanded(px(240.)));
}

#[test]
fn a_drag_cannot_squeeze_the_list_below_its_minimum() {
    let mut layout = PanelLayout::default();
    let window = px(1000.);
    layout.drag_sidebar(px(900.), window);
    layout.drag_preview(px(900.), window);
    let resolved = layout.resolve(window);
    let panes = resolved.sidebar + resolved.preview.unwrap();
    assert!(window - panes >= LIST_MIN);
}

fn open_window(cx: &mut TestAppContext) -> (Entity<MainView>, &mut VisualTestContext) {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("kwati_test_panels_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|_| AppState::new(Location::Local(dir)));
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();
    (main, cx)
}

fn drag(cx: &mut VisualTestContext, from: Pixels, to: Pixels) {
    let y = px(300.);
    cx.simulate_mouse_move(point(from, y), None, Modifiers::none());
    cx.simulate_mouse_down(point(from, y), MouseButton::Left, Modifiers::none());
    // gpui starts the drag on the first move and reports positions from the next one.
    cx.simulate_mouse_move(
        point(from + px(5.), y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(point(to, y), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(point(to, y), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn panels(main: &Entity<MainView>, cx: &mut VisualTestContext) -> PanelLayout {
    main.update(cx, |main, cx| main.app_state.read(cx).panels)
}

#[gpui::test]
fn dragging_the_sidebar_edge_resizes_and_collapses_it(cx: &mut TestAppContext) {
    let (main, cx) = open_window(cx);
    let edge = SIDEBAR_DEFAULT - px(2.);

    drag(cx, edge, px(280.));
    assert_eq!(panels(&main, cx).sidebar, SidebarWidth::Expanded(px(280.)));

    drag(cx, px(278.), px(40.));
    assert_eq!(panels(&main, cx).sidebar, SidebarWidth::Compact);
}

#[gpui::test]
fn dragging_the_preview_edge_resizes_it(cx: &mut TestAppContext) {
    let (main, cx) = open_window(cx);
    let edge = px(1024.) - PREVIEW_DEFAULT + px(2.);

    drag(cx, edge, px(1024. - 320.));
    assert_eq!(panels(&main, cx).preview, px(320.));

    // Past the point where the list would drop below its minimum, the edge stops.
    drag(cx, px(1024. - 318.), px(100.));
    assert_eq!(
        panels(&main, cx).preview,
        px(1024.) - LIST_MIN - SIDEBAR_DEFAULT
    );
}

#[test]
fn a_closed_preview_gives_its_room_to_the_list_and_keeps_its_width() {
    let mut layout = PanelLayout {
        preview: px(350.),
        ..PanelLayout::default()
    };
    layout.toggle_preview();
    let resolved = layout.resolve(px(1024.));
    assert_eq!(resolved.preview, None);
    assert_eq!(resolved.sidebar, SIDEBAR_DEFAULT);

    layout.toggle_preview();
    assert_eq!(layout.resolve(px(1400.)).preview, Some(px(350.)));
}

#[test]
fn with_the_preview_closed_the_sidebar_stays_full_in_a_narrower_window() {
    let mut layout = PanelLayout::default();
    let width = LIST_MIN + SIDEBAR_DEFAULT;
    assert!(layout.resolve(width).sidebar_compact);
    layout.toggle_preview();
    assert_eq!(
        layout.resolve(width),
        ResolvedLayout {
            sidebar: SIDEBAR_DEFAULT,
            sidebar_compact: false,
            preview: None,
        }
    );
}

#[gpui::test]
fn dragging_the_preview_edge_to_the_window_edge_closes_it(cx: &mut TestAppContext) {
    let (main, cx) = open_window(cx);
    let edge = px(1024.) - PREVIEW_DEFAULT + px(2.);

    drag(cx, edge, px(1000.));
    let layout = panels(&main, cx);
    assert!(!layout.preview_open);
    assert_eq!(layout.preview, PREVIEW_DEFAULT);
}

#[test]
fn a_hidden_sidebar_leaves_its_room_to_the_other_panes_and_returns_at_its_width() {
    let mut layout = PanelLayout {
        sidebar: SidebarWidth::Expanded(px(240.)),
        ..PanelLayout::default()
    };
    layout.sidebar_open = false;
    let hidden = layout.resolve(px(1400.));
    assert_eq!(hidden.sidebar, px(0.));
    assert_eq!(hidden.preview, Some(PREVIEW_DEFAULT));

    // Without the sidebar, a window too narrow for both panes still fits the preview.
    let narrow = LIST_MIN + PREVIEW_MIN;
    assert_eq!(layout.resolve(narrow).preview, Some(PREVIEW_MIN));

    layout.sidebar_open = true;
    assert_eq!(layout.resolve(px(1400.)).sidebar, px(240.));
}

#[gpui::test]
fn f9_hides_the_sidebar_and_the_choice_survives_a_restart(cx: &mut TestAppContext) {
    cx.update(kwati_gui::actions::register_keybindings);
    let dir = std::env::temp_dir().join(format!("kwati_test_sidebar_f9_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let settings_file = dir.join("settings.toml");
    let path = settings_file.clone();
    let location = Location::Local(dir.clone());
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state =
            cx.new(|_| AppState::new(location).with_settings_path(Some(settings_file.clone())));
        MainView::new(state, window, cx)
    });
    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();
    main.update_in(cx, |main, window, cx| {
        main.list.read(cx).focus_handle.focus(window)
    });

    cx.simulate_keystrokes("f9");
    assert!(!panels(&main, cx).sidebar_open);

    let mut restarted = AppState::new(Location::Local(dir.clone()));
    restarted.load_settings_from(&path).unwrap();
    assert!(!restarted.panels.sidebar_open);

    cx.simulate_keystrokes("f9");
    assert!(panels(&main, cx).sidebar_open);
    let mut restarted = AppState::new(Location::Local(dir.clone()));
    restarted.load_settings_from(&path).unwrap();
    assert!(restarted.panels.sidebar_open);

    let _ = fs::remove_dir_all(&dir);
}

#[gpui::test]
fn alt_p_hides_and_shows_the_preview(cx: &mut TestAppContext) {
    cx.update(kwati_gui::actions::register_keybindings);
    let (main, cx) = open_window(cx);
    main.update_in(cx, |main, window, cx| {
        main.list.read(cx).focus_handle.focus(window)
    });

    cx.simulate_keystrokes("alt-p");
    assert!(!panels(&main, cx).preview_open);
    cx.simulate_keystrokes("alt-p");
    assert!(panels(&main, cx).preview_open);
}

#[test]
fn standard_places_give_special_folders_their_own_icons() {
    use kwati_gui::components::{icon_for_with, IconName, SpecialFolders};
    use kwati_gui::views::sidebar_view::standard_places;

    let expected = [
        ("Home", IconName::FolderHome),
        ("Desktop", IconName::FolderDesktop),
        ("Documents", IconName::FolderDocuments),
        ("Downloads", IconName::FolderDownloads),
        ("Pictures", IconName::FolderPictures),
        ("Music", IconName::FolderMusic),
        ("Videos", IconName::FolderVideos),
    ];

    let places = standard_places();
    for (name, expected_icon) in expected {
        if let Some(place) = places.iter().find(|p| p.name == name) {
            assert_eq!(
                place.icon, expected_icon,
                "standard place {name} should use {expected_icon:?}"
            );
        }
    }

    // Also verify the special-folder icon resolution directly for all standard user dirs
    // regardless of which XDG user dirs exist on the test host.
    let base = PathBuf::from("/home/kwati-test-user");
    let specials = SpecialFolders {
        home: Some(base.clone()),
        desktop: Some(base.join("Desktop")),
        documents: Some(base.join("Documents")),
        downloads: Some(base.join("Downloads")),
        pictures: Some(base.join("Pictures")),
        music: Some(base.join("Music")),
        videos: Some(base.join("Videos")),
        public: Some(base.join("Public")),
    };
    assert_eq!(icon_for_with(&base, true, &specials), IconName::FolderHome);
    assert_eq!(
        icon_for_with(&base.join("Desktop"), true, &specials),
        IconName::FolderDesktop
    );
    assert_eq!(
        icon_for_with(&base.join("Documents"), true, &specials),
        IconName::FolderDocuments
    );
    assert_eq!(
        icon_for_with(&base.join("Downloads"), true, &specials),
        IconName::FolderDownloads
    );
    assert_eq!(
        icon_for_with(&base.join("Pictures"), true, &specials),
        IconName::FolderPictures
    );
    assert_eq!(
        icon_for_with(&base.join("Music"), true, &specials),
        IconName::FolderMusic
    );
    assert_eq!(
        icon_for_with(&base.join("Videos"), true, &specials),
        IconName::FolderVideos
    );
    assert_eq!(
        icon_for_with(&base.join("Public"), true, &specials),
        IconName::FolderPublic
    );
}
