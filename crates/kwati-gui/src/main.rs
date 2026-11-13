use std::env;
use std::path::PathBuf;

use gpui::{actions, prelude::*, px, size, App, Application, Bounds, WindowBounds, WindowOptions};
use kwati_core::Location;
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::{register_text_input_keybindings, Assets};
use kwati_gui::layout::{WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH};
use kwati_gui::state::AppState;
use kwati_gui::theme::Theme;
use kwati_gui::tokens::Fonts;
use kwati_gui::views::MainView;

actions!(app, [Quit]);

fn main() {
    let initial_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let initial_location = Location::Local(initial_path);

    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            register_keybindings(cx);
            register_text_input_keybindings(cx);
            Theme::apply_appearance(cx.window_appearance(), cx);
            cx.set_global(Fonts::system());

            let bounds = Bounds::centered(None, size(px(1024.0), px(720.0)), cx);

            let _window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)),
                        ..Default::default()
                    },
                    |window, cx| {
                        let app_state = cx.new(|cx| {
                            let mut state = AppState::new(initial_location.clone());
                            state.attach(cx);
                            state.reload_entries();
                            state
                        });
                        cx.new(|cx| MainView::new(app_state, window, cx))
                    },
                )
                .expect("failed to open main window");

            cx.activate(true);
        });
}
