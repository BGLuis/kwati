//! Contrast rules from DESIGN.md ("Colour"). A palette change that breaks
//! legibility in either theme must fail here, not in a screenshot review.

use gpui::{Rgba, TestAppContext, WindowAppearance};
use kwati_gui::theme::Theme;

fn luminance(c: Rgba) -> f32 {
    let channel = |v: f32| {
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn themes() -> [(&'static str, Theme); 2] {
    [("light", Theme::light()), ("dark", Theme::dark())]
}

#[test]
fn text_and_state_colours_reach_4_5_on_every_surface() {
    for (name, t) in themes() {
        let foregrounds = [
            ("text", t.text),
            ("text_muted", t.text_muted),
            ("accent", t.accent),
            ("secondary", t.secondary),
            ("danger", t.danger),
            ("warning", t.warning),
        ];
        let surfaces = [
            ("bg", t.bg),
            ("surface", t.surface),
            ("surface_raised", t.surface_raised),
        ];
        for (fg_name, fg) in foregrounds {
            for (bg_name, bg) in surfaces {
                let ratio = contrast(fg, bg);
                assert!(
                    ratio >= 4.5,
                    "{name}: {fg_name} on {bg_name} is {ratio:.2}:1"
                );
            }
        }
    }
}

#[test]
fn text_stays_legible_on_selection_and_accent_fills() {
    for (name, t) in themes() {
        let ratio = contrast(t.text, t.selection);
        assert!(ratio >= 4.5, "{name}: text on selection is {ratio:.2}:1");
        let ratio = contrast(t.on_accent, t.accent);
        assert!(ratio >= 4.5, "{name}: on_accent on accent is {ratio:.2}:1");
    }
}

#[test]
fn border_strong_reaches_3_on_every_surface() {
    for (name, t) in themes() {
        for (bg_name, bg) in [
            ("bg", t.bg),
            ("surface", t.surface),
            ("surface_raised", t.surface_raised),
        ] {
            let ratio = contrast(t.border_strong, bg);
            assert!(
                ratio >= 3.0,
                "{name}: border_strong on {bg_name} is {ratio:.2}:1"
            );
        }
    }
}

#[test]
fn each_system_appearance_maps_to_its_theme() {
    assert_eq!(
        Theme::for_appearance(WindowAppearance::Light),
        Theme::light()
    );
    assert_eq!(
        Theme::for_appearance(WindowAppearance::VibrantLight),
        Theme::light()
    );
    assert_eq!(Theme::for_appearance(WindowAppearance::Dark), Theme::dark());
    assert_eq!(
        Theme::for_appearance(WindowAppearance::VibrantDark),
        Theme::dark()
    );
}

// The test platform reports a light system, while the fallback theme is dark: views must
// pick up the system theme, not the fallback.
#[gpui::test]
fn views_read_the_theme_of_the_system(cx: &mut TestAppContext) {
    cx.update(|cx| {
        assert_eq!(Theme::get(cx), Theme::default());
        Theme::apply_appearance(cx.window_appearance(), cx);
        assert_eq!(Theme::get(cx), Theme::light());
    });
}
