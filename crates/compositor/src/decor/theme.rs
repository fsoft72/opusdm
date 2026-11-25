//! Colours and metrics of the server side window frame, read from the
//! OpusDMTheme openbox `themerc` (the theme the X11 session's Openbox draws
//! its frames with), with the same search path as the window menu theme.

use crate::menu::theme::{gradient_flag, read_themerc, theme_dirs, themerc_entries, Rgb, THEME_NAME};
use crate::state::LOG_TAG;

/// Border width of the OpusDMTheme frame, in pixels (`border.width`).
pub const DEFAULT_BORDER_WIDTH: u32 = 4;

/// What the frame of a decorated window looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorTheme {
    /// Title bar gradient of the active window (`window.active.title.bg.color`, `colorTo`).
    pub active_title: (Rgb, Rgb),
    /// Title bar gradient of an inactive window (`window.inactive.title.bg.color`, `colorTo`).
    pub inactive_title: (Rgb, Rgb),
    /// Title text of the active window (`window.active.label.text.color`).
    pub active_label: Rgb,
    /// Title text of an inactive window (`window.inactive.label.text.color`).
    pub inactive_label: Rgb,
    /// Button glyphs of the active window (`window.active.button.*.image.color`).
    pub active_button: Rgb,
    /// Button glyphs of an inactive window (`window.inactive.button.*.image.color`).
    pub inactive_button: Rgb,
    /// Background of the button under the pointer (`window.active.button.*.hover.bg.color`, `colorTo`).
    pub hover_bg: (Rgb, Rgb),
    /// Background of a pressed button (`window.active.button.*.pressed.bg.color`, `colorTo`).
    pub pressed_bg: (Rgb, Rgb),
    /// Border of the active window (`window.active.border.color`).
    pub active_border: Rgb,
    /// Border of an inactive window (`window.inactive.border.color`).
    pub inactive_border: Rgb,
    /// Width of the border around the title bar and the client, in pixels (`border.width`).
    pub border_width: u32,
}

impl Default for DecorTheme {
    /// The OpusDMTheme values, used for every key the themerc lacks.
    fn default() -> DecorTheme {
        DecorTheme {
            active_title: (Rgb(0x41, 0x41, 0x41), Rgb(0x20, 0x20, 0x20)),
            inactive_title: (Rgb(0xde, 0xe0, 0xd8), Rgb(0xe0, 0xe0, 0xd8)),
            active_label: Rgb(0xf8, 0xf8, 0xf8),
            inactive_label: Rgb(0x74, 0x74, 0x74),
            active_button: Rgb(0xe0, 0xe0, 0xe0),
            inactive_button: Rgb(0x74, 0x74, 0x74),
            hover_bg: (Rgb(0x39, 0x8d, 0xc6), Rgb(0x23, 0x6d, 0x83)),
            pressed_bg: (Rgb(0x23, 0x56, 0x79), Rgb(0x15, 0x43, 0x50)),
            active_border: Rgb(0x18, 0x18, 0x18),
            inactive_border: Rgb(0x40, 0x40, 0x40),
            border_width: DEFAULT_BORDER_WIDTH,
        }
    }
}

impl DecorTheme {
    /// The frame theme of the themerc text `themerc`; a missing key keeps
    /// its default, a value that does not parse is logged and keeps it too.
    /// A texture whose description has no `gradient` (`flat solid`) draws
    /// its `color` alone (`gradient_flag`, shared with the window menu).
    pub fn parse(themerc: &str) -> DecorTheme {
        let mut theme = DecorTheme::default();
        for (key, value) in themerc_entries(themerc) {
            theme._set(key, value);
        }
        let solid = |key: &str| !gradient_flag(themerc, key);
        for (key, pair) in [
            ("window.active.title.bg", &mut theme.active_title),
            ("window.inactive.title.bg", &mut theme.inactive_title),
            ("window.active.button.*.hover.bg", &mut theme.hover_bg),
            ("window.active.button.*.pressed.bg", &mut theme.pressed_bg),
        ] {
            if solid(key) {
                pair.1 = pair.0;
            }
        }
        theme
    }

    /// Sets the field of the themerc `key` to `value`; an unknown key is
    /// ignored (the themerc has many keys the frame does not use), a value
    /// that does not parse is logged and the field keeps its value.
    fn _set(&mut self, key: &str, value: &str) {
        let colour = |slot: &mut Rgb| match Rgb::parse(value) {
            Some(rgb) => *slot = rgb,
            None => opusdm_ipc::err_log!(LOG_TAG, "themerc {key}: '{value}' is not a colour, default kept"),
        };
        match key {
            "window.active.title.bg.color" => colour(&mut self.active_title.0),
            "window.active.title.bg.colorTo" => colour(&mut self.active_title.1),
            "window.inactive.title.bg.color" => colour(&mut self.inactive_title.0),
            "window.inactive.title.bg.colorTo" => colour(&mut self.inactive_title.1),
            "window.active.label.text.color" => colour(&mut self.active_label),
            "window.inactive.label.text.color" => colour(&mut self.inactive_label),
            "window.active.button.*.image.color" => colour(&mut self.active_button),
            "window.inactive.button.*.image.color" => colour(&mut self.inactive_button),
            "window.active.button.*.hover.bg.color" => colour(&mut self.hover_bg.0),
            "window.active.button.*.hover.bg.colorTo" => colour(&mut self.hover_bg.1),
            "window.active.button.*.pressed.bg.color" => colour(&mut self.pressed_bg.0),
            "window.active.button.*.pressed.bg.colorTo" => colour(&mut self.pressed_bg.1),
            "window.active.border.color" => colour(&mut self.active_border),
            "window.inactive.border.color" => colour(&mut self.inactive_border),
            "border.width" => match value.parse::<u32>() {
                Ok(w) => self.border_width = w,
                Err(e) => opusdm_ipc::err_log!(LOG_TAG, "themerc {key}: '{value}': {e}, default kept"),
            },
            _ => {}
        }
    }

    /// The frame theme of the first `OpusDMTheme/openbox-3/themerc` found
    /// in the directories `MenuTheme::load` searches; the defaults, with
    /// one `err_log!`, when there is none.
    pub fn load() -> DecorTheme {
        let dirs = theme_dirs();
        if let Some(text) = read_themerc(&dirs, "window frame theme") { return DecorTheme::parse(&text); }
        let searched: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        opusdm_ipc::err_log!(LOG_TAG, "window frame theme {THEME_NAME} not found in {searched:?}, built-in colours used");
        DecorTheme::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The themerc of OpusDMTheme shipped in the repository.
    const REPO_THEMERC: &str = include_str!("../../../../assets/openbox-share/themes/OpusDMTheme/openbox-3/themerc");

    #[test]
    fn the_default_theme_has_the_opusdm_border_width() {
        assert_eq!(DecorTheme::default().border_width, 4);
    }

    #[test]
    fn the_repo_themerc_gives_the_opusdm_frame_colours() {
        let theme = DecorTheme::parse(REPO_THEMERC);
        assert_eq!(theme.active_title.0, Rgb(0x41, 0x41, 0x41));
        assert_eq!(theme.active_title.1, Rgb(0x20, 0x20, 0x20));
        assert_eq!(theme.inactive_title, (Rgb(0xde, 0xe0, 0xd8), Rgb(0xe0, 0xe0, 0xd8)));
        assert_eq!((theme.active_label, theme.inactive_label), (Rgb(0xf8, 0xf8, 0xf8), Rgb(0x74, 0x74, 0x74)));
        assert_eq!((theme.active_button, theme.inactive_button), (Rgb(0xe0, 0xe0, 0xe0), Rgb(0x74, 0x74, 0x74)));
        assert_eq!(theme.hover_bg, (Rgb(0x39, 0x8d, 0xc6), Rgb(0x23, 0x6d, 0x83)));
        assert_eq!(theme.pressed_bg, (Rgb(0x23, 0x56, 0x79), Rgb(0x15, 0x43, 0x50)));
        assert_eq!((theme.active_border, theme.inactive_border), (Rgb(0x18, 0x18, 0x18), Rgb(0x40, 0x40, 0x40)));
        assert_eq!(theme.border_width, 4);
        assert_eq!(theme, DecorTheme::default(), "the defaults are OpusDMTheme's values");
    }

    #[test]
    fn every_key_of_the_frame_is_read() {
        let themerc = "\
window.active.title.bg.color: #010101
window.active.title.bg.colorTo: #020202
window.inactive.title.bg.color: #030303
window.inactive.title.bg.colorTo: #040404
window.active.label.text.color: #050505
window.inactive.label.text.color: #060606
window.active.button.*.image.color: #070707
window.inactive.button.*.image.color: #080808
window.active.button.*.hover.bg.color: #090909
window.active.button.*.hover.bg.colorTo: #0a0a0a
window.active.button.*.pressed.bg.color: #0b0b0b
window.active.button.*.pressed.bg.colorTo: #0c0c0c
window.active.border.color: #0d0d0d
window.inactive.border.color: #0e0e0e
border.width: 2
";
        let theme = DecorTheme::parse(themerc);
        let grey = |v: u8| Rgb(v, v, v);
        assert_eq!(theme.active_title, (grey(1), grey(2)));
        assert_eq!(theme.inactive_title, (grey(3), grey(4)));
        assert_eq!((theme.active_label, theme.inactive_label), (grey(5), grey(6)));
        assert_eq!((theme.active_button, theme.inactive_button), (grey(7), grey(8)));
        assert_eq!(theme.hover_bg, (grey(9), grey(10)));
        assert_eq!(theme.pressed_bg, (grey(11), grey(12)));
        assert_eq!((theme.active_border, theme.inactive_border), (grey(13), grey(14)));
        assert_eq!(theme.border_width, 2);
    }

    #[test]
    fn bad_values_keep_the_defaults_and_a_solid_title_has_one_colour() {
        let theme = DecorTheme::parse("border.width: wide\nwindow.active.title.bg.color: nope\n");
        assert_eq!(theme, DecorTheme::default());
        let solid = "window.active.title.bg: flat solid\nwindow.active.title.bg.color: #101010\nwindow.active.title.bg.colorTo: #202020\n";
        assert_eq!(DecorTheme::parse(solid).active_title, (Rgb(0x10, 0x10, 0x10), Rgb(0x10, 0x10, 0x10)));
    }

    #[test]
    fn the_first_themes_directory_with_the_theme_wins() {
        let found = tempfile::tempdir().unwrap();
        let rc = found.path().join(THEME_NAME).join("openbox-3/themerc");
        std::fs::create_dir_all(rc.parent().unwrap()).unwrap();
        std::fs::write(&rc, "border.width: 7\n").unwrap();
        let text = read_themerc(&[found.path().to_path_buf()], "test theme").expect("found");
        assert_eq!(DecorTheme::parse(&text).border_width, 7);
    }
}
