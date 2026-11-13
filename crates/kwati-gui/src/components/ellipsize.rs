use gpui::Styled;

/// One line of text, cut with an ellipsis when it does not fit.
pub trait Ellipsize: Styled + Sized {
    /// gpui 0.2.2 caches a no-wrap text's first measurement, taken before its width is known,
    /// so `truncate()` never cuts. Wrapping text is measured again at the real width, and
    /// clamping it to one line keeps it on one.
    fn ellipsize(self) -> Self {
        self.min_w_0()
            .whitespace_normal()
            .line_clamp(1)
            .text_ellipsis()
    }
}

impl<T: Styled> Ellipsize for T {}
