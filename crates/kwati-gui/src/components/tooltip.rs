use gpui::{div, prelude::*, AnyView, App, Context, SharedString, Window};

use crate::theme::Theme;
use crate::tokens::{radius, space, Fonts, TextRole};

/// A one-line label shown while the pointer rests on an element; pass the result of
/// [`Tooltip::text`] to `.tooltip(..)`.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .px(space::S8)
            .py(space::S4)
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .rounded(radius::CONTROL)
            .shadow_md()
            .font_family(Fonts::get(cx).sans)
            .text_body()
            .text_color(theme.text)
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}
