//! Spacing, sizes, radii, and type from the Kwatí design standard (`DESIGN.md`).

use gpui::{px, App, FontWeight, Global, Pixels, SharedString, Styled};

pub mod space {
    use gpui::{px, Pixels};

    pub const S2: Pixels = px(2.);
    pub const S4: Pixels = px(4.);
    pub const S8: Pixels = px(8.);
    pub const S12: Pixels = px(12.);
    pub const S16: Pixels = px(16.);
    pub const S24: Pixels = px(24.);
}

pub mod size {
    use gpui::{px, Pixels};

    pub const ROW: Pixels = px(24.);
    pub const SIDEBAR_ROW: Pixels = px(26.);
    pub const TOOLBAR: Pixels = px(32.);
    pub const STATUS_BAR: Pixels = px(22.);
    /// Path bar, search field, and other controls that sit inside the toolbar.
    pub const CONTROL: Pixels = px(24.);
    pub const ICON: Pixels = px(16.);
}

pub mod radius {
    use gpui::{px, Pixels};

    pub const CONTROL: Pixels = px(4.);
    pub const DIALOG: Pixels = px(6.);
}

pub const BODY_SIZE: Pixels = px(13.);
pub const DATA_SIZE: Pixels = px(12.);
pub const LABEL_SIZE: Pixels = px(11.);
pub const TITLE_SIZE: Pixels = px(15.);

/// The platform's interface and monospace font families.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    pub sans: SharedString,
    pub mono: SharedString,
}

impl Global for Fonts {}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            sans: ".SystemUIFont".into(),
            mono: "monospace".into(),
        }
    }
}

impl Fonts {
    #[cfg(target_os = "macos")]
    pub fn system() -> Self {
        Self {
            sans: ".SystemUIFont".into(),
            mono: "Menlo".into(),
        }
    }

    #[cfg(target_os = "windows")]
    pub fn system() -> Self {
        Self {
            sans: ".SystemUIFont".into(),
            mono: "Consolas".into(),
        }
    }

    /// gpui resolves ".SystemUIFont" to IBM Plex Sans on Linux rather than to the desktop's
    /// font, and has no monospace alias, so the families come from fontconfig.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub fn system() -> Self {
        let fallback = Self::default();
        Self {
            sans: fontconfig_family("sans-serif").unwrap_or(fallback.sans),
            mono: fontconfig_family("monospace").unwrap_or(fallback.mono),
        }
    }

    /// The active fonts. Views built without a running app (as in tests) get the default.
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn fontconfig_family(pattern: &str) -> Option<SharedString> {
    let output = std::process::Command::new("fc-match")
        .args(["--format=%{family[0]}", pattern])
        .output()
        .ok()?;
    let family = String::from_utf8(output.stdout).ok()?;
    let family = family.trim();
    (output.status.success() && !family.is_empty()).then(|| family.to_string().into())
}

/// The type roles of the design standard. `data` is the monospace role for sizes, dates,
/// paths, and the status bar.
pub trait TextRole: Styled + Sized {
    fn text_body(self) -> Self {
        self.text_size(BODY_SIZE).font_weight(FontWeight::NORMAL)
    }

    fn text_data(self, fonts: &Fonts) -> Self {
        self.font_family(fonts.mono.clone())
            .text_size(DATA_SIZE)
            .font_weight(FontWeight::NORMAL)
    }

    fn text_label(self) -> Self {
        self.text_size(LABEL_SIZE).font_weight(FontWeight::MEDIUM)
    }

    fn text_title(self) -> Self {
        self.text_size(TITLE_SIZE).font_weight(FontWeight::SEMIBOLD)
    }
}

impl<T: Styled> TextRole for T {}
