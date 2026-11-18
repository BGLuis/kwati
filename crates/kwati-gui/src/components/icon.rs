//! Line icons exported from the Figma "Icons" page (16 px, 1.5 px stroke),
//! plus animated icon renderers (`quick` 300 ms, once per trigger).
//!
//! The SVGs are compiled into the binary and served through [`Assets`]. gpui paints an SVG
//! as a mask in the svg element's own text colour, which is not inherited from the parent,
//! so every icon needs an explicit `.text_color(..)`.

use std::borrow::Cow;
use std::f32::consts::{PI, TAU};

use gpui::{
    canvas, div, point, px, radians, svg, AnyElement, AssetSource, Bounds, Hsla, IntoElement,
    ParentElement, PathBuilder, Pixels, Point, Result, SharedString, Size, Styled, Svg,
    Transformation, Window,
};

use crate::motion::Curve;
use crate::tokens::size;

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $($variant),+
        }

        impl IconName {
            pub const ALL: &'static [IconName] = &[$(IconName::$variant),+];

            pub fn path(self) -> &'static str {
                match self {
                    $(IconName::$variant => concat!("icons/", $file, ".svg")),+
                }
            }

            fn svg(self) -> &'static [u8] {
                match self {
                    $(IconName::$variant => include_bytes!(concat!("../../assets/icons/", $file, ".svg"))),+
                }
            }
        }
    };
}

macro_rules! icon_parts {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconPart {
            $($variant),+
        }

        impl IconPart {
            pub const ALL: &'static [IconPart] = &[$(IconPart::$variant),+];

            pub const fn path(self) -> &'static str {
                match self {
                    $(IconPart::$variant => concat!("icons/parts/", $file, ".svg")),+
                }
            }

            fn svg(self) -> &'static [u8] {
                match self {
                    $(IconPart::$variant => include_bytes!(concat!("../../assets/icons/parts/", $file, ".svg"))),+
                }
            }
        }
    };
}

icons! {
    Alert => "alert",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    Check => "check",
    ChevronRight => "chevron-right",
    Close => "close",
    Copy => "copy",
    Drive => "drive",
    Eject => "eject",
    File => "file",
    Folder => "folder",
    Home => "home",
    Image => "image",
    Network => "network",
    PanelRight => "panel-right",
    Plus => "plus",
    Refresh => "refresh",
    Search => "search",
    Settings => "settings",
    Trash => "trash",
    ViewCompact => "view-compact",
    ViewGrid => "view-grid",
    ViewList => "view-list",
}

icon_parts! {
    FolderBack => "folder-back",
    FolderFlap => "folder-flap",
    FileBody => "file-body",
    FileCorner => "file-corner",
    TrashBody => "trash-body",
    TrashLid => "trash-lid",
    CopyBack => "copy-back",
    CopyFront => "copy-front",
    EjectBar => "eject-bar",
    EjectTriangle => "eject-triangle",
}

/// Instantaneous normalized progress (`0.0` at rest, `1.0` peak/complete) driving an animated icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconMotion {
    /// Rest state — renders the static single-path SVG.
    Rest,
    /// Stateful open/separate/tilt factor (`0.0` at rest .. `1.0` active).
    State(f32),
    /// One-shot normalized pulse progress in `[0.0, 1.0)`.
    Pulse(f32),
    /// Progressive draw-in (`0.0` empty .. `1.0` complete).
    Draw(f32),
}

impl IconMotion {
    #[inline]
    fn factor(self) -> f32 {
        match self {
            Self::Rest => 0.0,
            Self::State(v) => v,
            Self::Pulse(t) => (Curve::Spring.sample(t) * PI).sin().max(0.0),
            Self::Draw(t) => t.clamp(0.0, 1.0),
        }
    }

    #[inline]
    fn linear_progress(self) -> f32 {
        match self {
            Self::Rest => 0.0,
            Self::State(v) | Self::Pulse(v) | Self::Draw(v) => v.clamp(0.0, 1.0),
        }
    }
}

/// A 16 px icon. It stays invisible until it is given a `.text_color(..)`.
pub fn icon(name: IconName) -> Svg {
    svg().path(name.path()).size(size::ICON).flex_none()
}

/// Renders an icon driven by `motion` at `size::ICON` (`16 px`) using `color`.
pub fn animated_icon(name: IconName, motion: IconMotion, color: impl Into<Hsla>) -> AnyElement {
    animated_icon_sized(name, motion, size::ICON, color)
}

/// Renders an icon driven by `motion` at an explicit square size using `color`.
pub fn animated_icon_sized(
    name: IconName,
    motion: IconMotion,
    icon_size: Pixels,
    color: impl Into<Hsla>,
) -> AnyElement {
    let color = color.into();
    let f = motion.factor();
    let t = motion.linear_progress();

    match name {
        IconName::ArrowLeft if f.abs() > 1e-3 => transformed_svg(
            name,
            icon_size,
            color,
            Transformation::translate(point(px(-3.0 * f), px(0.0))),
        ),
        IconName::ArrowRight if f.abs() > 1e-3 => transformed_svg(
            name,
            icon_size,
            color,
            Transformation::translate(point(px(3.0 * f), px(0.0))),
        ),
        IconName::ArrowUp if f.abs() > 1e-3 => transformed_svg(
            name,
            icon_size,
            color,
            Transformation::translate(point(px(0.0), px(-3.0 * f))),
        ),
        IconName::Refresh if t > 1e-3 && t < 0.999 => transformed_svg(
            name,
            icon_size,
            color,
            Transformation::rotate(radians(Curve::Spring.sample(t) * TAU)),
        ),
        IconName::Search if f.abs() > 1e-3 => transformed_svg(
            name,
            icon_size,
            color,
            Transformation::rotate(radians(-0.28 * f)),
        ),
        IconName::Folder if f.abs() > 1e-3 => layered_parts(
            icon_size,
            color,
            [
                (IconPart::FolderBack, Transformation::default()),
                (
                    IconPart::FolderFlap,
                    Transformation::scale(Size::new(1.0, (1.0 - 0.24 * f).max(0.6)))
                        .with_translation(point(px(0.0), px(1.1 * f))),
                ),
            ],
        ),
        IconName::File if f.abs() > 1e-3 => layered_parts(
            icon_size,
            color,
            [
                (IconPart::FileBody, Transformation::default()),
                (
                    IconPart::FileCorner,
                    Transformation::scale(Size::new(1.0 + 0.1 * f, 1.0 + 0.1 * f))
                        .with_translation(point(px(0.8 * f), px(-0.8 * f))),
                ),
            ],
        ),
        IconName::Trash if f.abs() > 1e-3 => layered_parts(
            icon_size,
            color,
            [
                (IconPart::TrashBody, Transformation::default()),
                (
                    IconPart::TrashLid,
                    Transformation::rotate(radians(-0.24 * f))
                        .with_translation(point(px(0.0), px(-1.75 * f))),
                ),
            ],
        ),
        IconName::Copy if f.abs() > 1e-3 => layered_parts(
            icon_size,
            color,
            [
                (
                    IconPart::CopyBack,
                    Transformation::translate(point(px(-f), px(-f))),
                ),
                (
                    IconPart::CopyFront,
                    Transformation::translate(point(px(f), px(f))),
                ),
            ],
        ),
        IconName::Eject if f.abs() > 1e-3 => layered_parts(
            icon_size,
            color,
            [
                (IconPart::EjectBar, Transformation::default()),
                (
                    IconPart::EjectTriangle,
                    Transformation::translate(point(px(0.0), px(-2.4 * f))),
                ),
            ],
        ),
        IconName::Settings if f.abs() > 1e-3 => draw_settings_canvas(icon_size, color, f),
        IconName::Check if matches!(motion, IconMotion::Draw(_)) && t < 0.999 => {
            draw_check_canvas(icon_size, color, t)
        }
        IconName::Network if matches!(motion, IconMotion::Draw(_)) && t > 1e-3 && t < 0.999 => {
            draw_network_canvas(icon_size, color, t)
        }
        _ => svg()
            .path(name.path())
            .size(icon_size)
            .flex_none()
            .text_color(color)
            .into_any_element(),
    }
}

fn transformed_svg(
    name: IconName,
    icon_size: Pixels,
    color: Hsla,
    transformation: Transformation,
) -> AnyElement {
    svg()
        .path(name.path())
        .size(icon_size)
        .flex_none()
        .text_color(color)
        .with_transformation(transformation)
        .into_any_element()
}

fn layered_parts<const N: usize>(
    icon_size: Pixels,
    color: Hsla,
    parts: [(IconPart, Transformation); N],
) -> AnyElement {
    let mut container = div().relative().size(icon_size).flex_none();
    for (part, transform) in parts {
        container = container.child(
            svg()
                .path(part.path())
                .absolute()
                .inset_0()
                .size(icon_size)
                .text_color(color)
                .with_transformation(transform),
        );
    }
    container.into_any_element()
}

#[inline]
fn map_pt(bounds: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(
        bounds.origin.x + bounds.size.width * (x / 16.0),
        bounds.origin.y + bounds.size.height * (y / 16.0),
    )
}

#[inline]
fn scaled_stroke(bounds: Bounds<Pixels>) -> Pixels {
    bounds.size.width * (1.5 / 16.0)
}

fn draw_check_canvas(icon_size: Pixels, color: Hsla, progress: f32) -> AnyElement {
    let p = progress.clamp(0.0, 1.0);
    canvas(
        move |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
            if p <= 0.01 {
                return;
            }
            let mut builder = PathBuilder::stroke(scaled_stroke(bounds));
            // Total normalized length split: leg 1 is ~0.325, leg 2 is ~0.675.
            const SPLIT: f32 = 0.325;
            builder.move_to(map_pt(bounds, 3.0, 8.5));
            if p <= SPLIT {
                let f = p / SPLIT;
                builder.line_to(map_pt(bounds, 3.0 + 3.25 * f, 8.5 + 3.25 * f));
            } else {
                let f = (p - SPLIT) / (1.0 - SPLIT);
                builder.line_to(map_pt(bounds, 6.25, 11.75));
                builder.line_to(map_pt(bounds, 6.25 + 6.75 * f, 11.75 - 6.75 * f));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(icon_size)
    .flex_none()
    .into_any_element()
}

fn draw_settings_canvas(icon_size: Pixels, color: Hsla, slide: f32) -> AnyElement {
    canvas(
        move |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
            let mut builder = PathBuilder::stroke(scaled_stroke(bounds));
            let k1_x = 10.75 - 4.75 * slide;
            let k2_x = 5.25 + 4.75 * slide;

            draw_slider_track(&mut builder, bounds, 5.0, k1_x);
            draw_slider_track(&mut builder, bounds, 11.0, k2_x);

            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(icon_size)
    .flex_none()
    .into_any_element()
}

fn draw_slider_track(builder: &mut PathBuilder, bounds: Bounds<Pixels>, y: f32, knob_x: f32) {
    const R: f32 = 1.25;
    if knob_x - R > 2.5 {
        builder.move_to(map_pt(bounds, 2.5, y));
        builder.line_to(map_pt(bounds, knob_x - R, y));
    }
    if knob_x + R < 13.5 {
        builder.move_to(map_pt(bounds, knob_x + R, y));
        builder.line_to(map_pt(bounds, 13.5, y));
    }
    // 16-segment circle around (knob_x, y).
    const SEGS: usize = 16;
    for i in 0..=SEGS {
        let angle = (i as f32 / SEGS as f32) * TAU;
        let pt = map_pt(bounds, knob_x + R * angle.cos(), y + R * angle.sin());
        if i == 0 {
            builder.move_to(pt);
        } else {
            builder.line_to(pt);
        }
    }
}

fn draw_network_canvas(icon_size: Pixels, color: Hsla, progress: f32) -> AnyElement {
    let p = progress.clamp(0.0, 1.0);
    canvas(
        move |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
            let mut builder = PathBuilder::stroke(scaled_stroke(bounds));

            // Stage 1 (0..0.55): outer globe ring draws in clockwise from top.
            let ring_p = (p / 0.55).clamp(0.0, 1.0);
            const SEGS: usize = 24;
            let count = ((SEGS as f32) * ring_p).ceil() as usize;
            for i in 0..=count.max(1) {
                let frac = (i as f32 / SEGS as f32).min(ring_p);
                let angle = -PI * 0.5 + frac * TAU;
                let pt = map_pt(bounds, 8.0 + 6.0 * angle.cos(), 8.0 + 6.0 * angle.sin());
                if i == 0 {
                    builder.move_to(pt);
                } else {
                    builder.line_to(pt);
                }
            }

            // Stage 2 (0.35..0.85): meridian curves draw top-to-bottom.
            let mer_p = ((p - 0.35) / 0.5).clamp(0.0, 1.0);
            if mer_p > 0.01 {
                for sign in [-1.0_f32, 1.0_f32] {
                    const STEPS: usize = 10;
                    let m_count = ((STEPS as f32) * mer_p).ceil() as usize;
                    for i in 0..=m_count {
                        let u = (i as f32 / STEPS as f32).min(mer_p);
                        let y = 2.0 + 12.0 * u;
                        let bulge = (u * PI).sin() * 2.0 * sign;
                        let pt = map_pt(bounds, 8.0 + bulge, y);
                        if i == 0 {
                            builder.move_to(pt);
                        } else {
                            builder.line_to(pt);
                        }
                    }
                }
            }

            // Stage 3 (0.5..1.0): equator draws left-to-right.
            let eq_p = ((p - 0.5) / 0.5).clamp(0.0, 1.0);
            if eq_p > 0.01 {
                builder.move_to(map_pt(bounds, 2.0, 8.0));
                builder.line_to(map_pt(bounds, 2.0 + 12.0 * eq_p, 8.0));
            }

            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(icon_size)
    .flex_none()
    .into_any_element()
}

/// The app's bundled assets, registered with `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(found) = IconName::ALL.iter().find(|name| name.path() == path) {
            return Ok(Some(Cow::Borrowed(found.svg())));
        }
        Ok(IconPart::ALL
            .iter()
            .find(|part| part.path() == path)
            .map(|part| Cow::Borrowed(part.svg())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = IconName::ALL
            .iter()
            .map(|name| name.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect();
        out.extend(
            IconPart::ALL
                .iter()
                .map(|part| part.path())
                .filter(|p| p.starts_with(path))
                .map(SharedString::from),
        );
        Ok(out)
    }
}
