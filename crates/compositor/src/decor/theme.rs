//! Metrics (and, from Task 7, colours) of the server side window frame,
//! from the OpusDMTheme openbox `themerc`. For now only the defaults: the
//! frame geometry needs the border width.

/// Border width of the OpusDMTheme frame, in pixels (`border.width`).
pub const DEFAULT_BORDER_WIDTH: u32 = 4;

/// What the frame of a decorated window looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorTheme {
    /// Width of the border around the title bar and the client, in pixels.
    pub border_width: u32,
}

impl Default for DecorTheme {
    /// The OpusDMTheme values.
    fn default() -> DecorTheme {
        DecorTheme { border_width: DEFAULT_BORDER_WIDTH }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_theme_has_the_opusdm_border_width() {
        assert_eq!(DecorTheme::default().border_width, 4);
    }
}
