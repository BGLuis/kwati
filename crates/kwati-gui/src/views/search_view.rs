//! Search overlay view (owned by inspect-worker for #27).
//!
//! Provides interactive search query input and displays search results.

use gpui::{div, prelude::*, Context, Entity, Window};

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

pub struct SearchView {
    pub app_state: Entity<AppState>,
    pub search_input: Entity<TextInput>,
    pub is_active: bool,
    pub theme: Theme,
}

impl SearchView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| TextInput::with_placeholder("Search directory...", cx));
        Self {
            app_state,
            search_input,
            is_active: false,
            theme: Theme::dark(),
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.is_active = !self.is_active;
        cx.notify();
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_active {
            return div();
        }

        div()
            .w_full()
            .p_2()
            .bg(self.theme.bg_header)
            .border_b_1()
            .border_color(self.theme.border)
            .child(self.search_input.clone())
    }
}
