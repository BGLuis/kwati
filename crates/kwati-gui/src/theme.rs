use gpui::{rgb, rgba, Rgba};

/// Colour tokens from the Kwatí design standard (`DESIGN.md`, "Colour").
pub struct Theme {
    pub bg: Rgba,
    pub surface: Rgba,
    pub surface_raised: Rgba,
    pub border: Rgba,
    pub border_strong: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub on_accent: Rgba,
    pub selection: Rgba,
    pub secondary: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    /// Dims the window behind a dialog. Not a palette colour: it is `text` (light)
    /// or black (dark) at partial opacity, so the window stays recognisable.
    pub scrim: Rgba,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg: rgb(0xF4EEE4),
            surface: rgb(0xFBF7F0),
            surface_raised: rgb(0xECE3D5),
            border: rgb(0xD9CCB8),
            border_strong: rgb(0x8C7B69),
            text: rgb(0x2B211A),
            text_muted: rgb(0x6B5B4D),
            accent: rgb(0xA8481F),
            on_accent: rgb(0xFFF8F0),
            selection: rgb(0xEBCDB6),
            secondary: rgb(0x3F6B4A),
            danger: rgb(0xB3261E),
            warning: rgb(0x8A5A00),
            scrim: rgba(0x2B211A66),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1C1714),
            surface: rgb(0x231D19),
            surface_raised: rgb(0x2C2420),
            border: rgb(0x3D332C),
            border_strong: rgb(0x8A7A6B),
            text: rgb(0xEDE4D8),
            text_muted: rgb(0xA8998A),
            accent: rgb(0xE0824F),
            on_accent: rgb(0x1C1714),
            selection: rgb(0x4A2E20),
            secondary: rgb(0x7FB08A),
            danger: rgb(0xF2857C),
            warning: rgb(0xE0B050),
            scrim: rgba(0x00000099),
        }
    }
}
