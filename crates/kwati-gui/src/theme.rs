use gpui::{rgb, rgba, Rgba};

/// Colors and styling tokens for kwati-gui.
pub struct Theme {
    // Backgrounds
    pub bg_app: Rgba,
    pub bg_header: Rgba,
    pub bg_sidebar: Rgba,
    pub bg_content: Rgba,
    pub bg_panel: Rgba,
    pub bg_status: Rgba,
    pub bg_input: Rgba,
    pub bg_hover: Rgba,
    pub bg_selected: Rgba,

    // Borders
    pub border: Rgba,
    pub border_focus: Rgba,

    // Foregrounds / Text
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub text_accent: Rgba,
    pub text_error: Rgba,
    pub text_warning: Rgba,
    pub text_success: Rgba,

    // Accent
    pub accent: Rgba,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Modern dark theme palette (neutral slate / modern editor aesthetic).
    pub fn dark() -> Self {
        Self {
            bg_app: rgb(0x181825),
            bg_header: rgb(0x1e1e2e),
            bg_sidebar: rgb(0x181825),
            bg_content: rgb(0x1e1e2e),
            bg_panel: rgb(0x181825),
            bg_status: rgb(0x11111b),
            bg_input: rgb(0x313244),
            bg_hover: rgba(0x45475a80),
            bg_selected: rgba(0x585b7080),

            border: rgb(0x313244),
            border_focus: rgb(0x89b4fa),

            text_primary: rgb(0xcdd6f4),
            text_secondary: rgb(0xa6adc8),
            text_muted: rgb(0x6c7086),
            text_accent: rgb(0x89b4fa),
            text_error: rgb(0xf38ba8),
            text_warning: rgb(0xf9e2af),
            text_success: rgb(0xa6e3a1),

            accent: rgb(0x89b4fa),
        }
    }

    /// Clean light theme palette.
    pub fn light() -> Self {
        Self {
            bg_app: rgb(0xf8f9fa),
            bg_header: rgb(0xffffff),
            bg_sidebar: rgb(0xf1f3f5),
            bg_content: rgb(0xffffff),
            bg_panel: rgb(0xf8f9fa),
            bg_status: rgb(0xe9ecef),
            bg_input: rgb(0xf1f3f5),
            bg_hover: rgba(0xe9ecef80),
            bg_selected: rgba(0xdbe4ff80),

            border: rgb(0xdee2e6),
            border_focus: rgb(0x339af0),

            text_primary: rgb(0x212529),
            text_secondary: rgb(0x495057),
            text_muted: rgb(0x868e96),
            text_accent: rgb(0x1c7ed6),
            text_error: rgb(0xe03131),
            text_warning: rgb(0xf08c00),
            text_success: rgb(0x2f9e44),

            accent: rgb(0x339af0),
        }
    }
}
