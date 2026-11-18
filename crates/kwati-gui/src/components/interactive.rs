//! Shared motion chrome primitives: press-scale/hover buttons, toggle switches, checkboxes, and modal cards.

use gpui::{div, prelude::*, px, AnyElement, Div, Hsla, Pixels, Stateful, Styled};

use crate::components::icon::{animated_icon_sized, IconMotion, IconName};
use crate::motion::{
    color_lerp, lerp_f32, Curve, DIALOG_RISE_PX, DIALOG_START_SCALE, MENU_START_SCALE, PRESS_SCALE,
};
use crate::theme::Theme;
use crate::tokens::{radius, size, space, TextRole};

/// Wraps an inner control in a fixed-size slot so `0.93` press scaling never shifts siblings.
pub fn press_scaled_slot(slot_size: Pixels, press_t: f32, reduced: bool, inner: Div) -> Div {
    let scale = if reduced {
        1.0
    } else {
        lerp_f32(1.0, PRESS_SCALE, press_t.clamp(0.0, 1.0))
    };
    let inner_size = slot_size * scale;
    div()
        .size(slot_size)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .child(inner.size(inner_size))
}

/// Computes the interpolated background for a chrome control transitioning into `surface_raised`.
#[inline]
pub fn hover_bg(rest: impl Into<Hsla>, hover_t: f32, theme: &Theme) -> Hsla {
    color_lerp(rest, theme.surface_raised, hover_t)
}

/// Computes the `1 px` focus ring border color blending `rest_border` toward `theme.accent`.
#[inline]
pub fn focus_ring_color(rest_border: impl Into<Hsla>, focus_t: f32, theme: &Theme) -> Hsla {
    color_lerp(rest_border, theme.accent, focus_t)
}

/// Renders an animated pill toggle switch (`36 x 20 px` track, `14 px` knob) driven by `slide_t` (`0.0..=1.0`).
pub fn animated_toggle_switch(
    id: impl Into<gpui::ElementId>,
    slide_t: f32,
    press_t: f32,
    theme: &Theme,
    reduced: bool,
) -> Stateful<Div> {
    const TRACK_W: f32 = 36.0;
    const TRACK_H: f32 = 20.0;
    const KNOB_SIZE: f32 = 14.0;
    const INSET: f32 = 2.0;

    let t = slide_t.clamp(0.0, 1.0);
    let knob_travel = TRACK_W - KNOB_SIZE - INSET * 2.0 - 2.0; // -2px border width
    let knob_x = if reduced {
        if t >= 0.5 {
            knob_travel
        } else {
            0.0
        }
    } else {
        lerp_f32(0.0, knob_travel, t)
    };
    let track_bg = color_lerp(theme.surface_raised, theme.accent, t);
    let track_border = color_lerp(theme.border_strong, theme.accent, t);
    let knob_scale_w = if reduced {
        KNOB_SIZE
    } else {
        KNOB_SIZE + 2.0 * press_t.clamp(0.0, 1.0)
    };
    let knob_left = if t > 0.5 && !reduced {
        (knob_x - (knob_scale_w - KNOB_SIZE)).max(0.0)
    } else {
        knob_x
    };

    div()
        .id(id)
        .w(px(TRACK_W))
        .h(px(TRACK_H))
        .rounded(px(TRACK_H * 0.5))
        .border_1()
        .border_color(track_border)
        .bg(track_bg)
        .p(px(INSET))
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            div()
                .relative()
                .left(px(knob_left))
                .w(px(knob_scale_w))
                .h(px(KNOB_SIZE))
                .rounded(px(KNOB_SIZE * 0.5))
                .bg(color_lerp(theme.text_muted, theme.on_accent, t)),
        )
}

/// Resolves the checkmark icon motion for a checkbox at normalized progress `check_t`.
#[inline]
pub fn checkbox_check_motion(check_t: f32, reduced: bool) -> Option<IconMotion> {
    let t = check_t.clamp(0.0, 1.0);
    if t > 0.02 {
        Some(if reduced {
            IconMotion::Rest
        } else {
            IconMotion::Draw(t)
        })
    } else {
        None
    }
}

/// Renders an animated `16 px` checkbox row with smooth fill and checkmark stroke draw-in.
pub fn animated_checkbox_row(
    id: impl Into<gpui::ElementId>,
    check_t: f32,
    label: &str,
    theme: &Theme,
    reduced: bool,
) -> Stateful<Div> {
    let t = check_t.clamp(0.0, 1.0);
    let border_col = color_lerp(theme.border_strong, theme.accent, t);
    let box_bg = color_lerp(theme.surface_raised, theme.accent, t);

    let check_child: Option<AnyElement> = checkbox_check_motion(t, reduced)
        .map(|motion| animated_icon_sized(IconName::Check, motion, size::ICON, theme.on_accent));

    div()
        .id(id)
        .flex()
        .items_center()
        .gap(space::S8)
        .cursor_pointer()
        .child(
            div()
                .size(size::ICON)
                .rounded(radius::CONTROL)
                .border_1()
                .border_color(border_col)
                .bg(box_bg)
                .flex()
                .items_center()
                .justify_center()
                .children(check_child),
        )
        .child(
            div()
                .text_body()
                .text_color(theme.text_muted)
                .child(label.to_owned()),
        )
}

/// Applies the `DESIGN.md` dialog entrance (`28 px` rise + `0.72` scale + opacity) and returns
/// the faded backdrop scrim color.
pub fn modal_scrim_color(theme: &Theme, open_linear_t: f32) -> Hsla {
    let eased = Curve::EaseOut.sample(open_linear_t);
    let mut scrim = theme.scrim;
    scrim.a *= eased;
    scrim.into()
}

/// Wraps a modal card (`target_width`) with animated rise (`28 px`), scale (`0.72`), and opacity.
pub fn wrap_modal_card_entrance(
    card: AnyElement,
    target_width: Pixels,
    open_linear_t: f32,
    reduced: bool,
) -> Div {
    let t = open_linear_t.clamp(0.0, 1.0);
    let opacity = Curve::EaseOut.sample(t);
    if reduced || t >= 0.999 {
        return div().opacity(opacity).child(card);
    }
    let spring = Curve::Spring.sample(t);
    let dy = DIALOG_RISE_PX * (1.0 - spring);
    let scale = lerp_f32(DIALOG_START_SCALE, 1.0, spring).max(0.5);
    let visible_width = target_width * scale;

    div()
        .relative()
        .top(px(dy))
        .opacity(opacity)
        .flex()
        .justify_center()
        .child(
            div()
                .w(visible_width)
                .overflow_hidden()
                .flex()
                .justify_center()
                .child(div().w(target_width).flex_none().child(card)),
        )
}

/// Computes `(visible_scale, opacity)` for menu and tooltip entrance from [`MENU_START_SCALE`].
#[inline]
pub fn popup_entrance_metrics(t: f32, reduced: bool) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let opacity = Curve::EaseOut.sample(t);
    let scale = if reduced {
        1.0
    } else {
        lerp_f32(MENU_START_SCALE, 1.0, Curve::Spring.sample(t)).max(0.5)
    };
    (scale, opacity)
}
