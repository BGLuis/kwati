//! Line icons exported from the Figma "Icons" page (16 px, 1.5 px stroke).
//!
//! The SVGs are compiled into the binary and served through [`Assets`]. gpui paints an SVG
//! as a mask in the svg element's own text colour, which is not inherited from the parent,
//! so every icon needs an explicit `.text_color(..)`.

use std::borrow::Cow;

use gpui::{svg, AssetSource, Result, SharedString, Styled, Svg};

use crate::tokens::size;

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    ViewGrid => "view-grid",
    ViewList => "view-list",
}

/// A 16 px icon. It stays invisible until it is given a `.text_color(..)`.
pub fn icon(name: IconName) -> Svg {
    svg().path(name.path()).size(size::ICON).flex_none()
}

/// The app's bundled assets, registered with `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(IconName::ALL
            .iter()
            .find(|name| name.path() == path)
            .map(|name| Cow::Borrowed(name.svg())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(IconName::ALL
            .iter()
            .map(|name| name.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}
