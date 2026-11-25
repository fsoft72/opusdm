//! Colours and metrics of the window menu, read from the OpusDMTheme
//! openbox `themerc` (the theme the X11 session uses for its menus).

use std::path::{Path, PathBuf};

use crate::state::LOG_TAG;

/// Name of the theme the window menu reads.
pub const THEME_NAME: &str = "OpusDMTheme";

/// The themerc key of the menu border colour (it falls back to
/// `window.active.border.color` when absent).
const MENU_BORDER_KEY: &str = "menu.border.color";

/// Path of the themerc inside a themes directory.
const THEMERC_PATH: &str = "openbox-3/themerc";

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// The colour written `text` in a themerc: `#rrggbb`, `#rgb`, `white`
    /// or `black` (case-insensitive); `None` otherwise.
    pub fn parse(text: &str) -> Option<Rgb> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("white") { return Some(Rgb(0xff, 0xff, 0xff)); }
        if text.eq_ignore_ascii_case("black") { return Some(Rgb(0, 0, 0)); }
        let hex = text.strip_prefix('#')?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
        let digit = |i: usize| hex.get(i..=i).and_then(|d| u8::from_str_radix(d, 16).ok());
        let byte = |i: usize| hex.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
        match hex.len() {
            6 => Some(Rgb(byte(0)?, byte(2)?, byte(4)?)),
            3 => Some(Rgb(digit(0)? * 0x11, digit(1)? * 0x11, digit(2)? * 0x11)),
            _ => None,
        }
    }
}

/// Colours and metrics of the window menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuTheme {
    /// Item background gradient (`menu.items.bg.color`, `colorTo`).
    pub item_bg: (Rgb, Rgb),
    /// Item text (`menu.items.text.color`).
    pub item_text: Rgb,
    /// Disabled item text (`menu.items.disabled.text.color`).
    pub disabled_text: Rgb,
    /// Selected item background gradient (`menu.items.active.bg.color`, `colorTo`).
    pub active_bg: (Rgb, Rgb),
    /// Selected item text (`menu.items.active.text.color`).
    pub active_text: Rgb,
    /// Border colour (`menu.border.color`, else
    /// `window.active.border.color`, as Openbox).
    pub border: Rgb,
    /// Border width in pixels (`border.width`).
    pub border_width: u32,
    /// Padding in pixels (`padding.width`).
    pub padding: u32,
}

impl Default for MenuTheme {
    /// The values of OpusDMTheme, used for every key the themerc lacks.
    fn default() -> MenuTheme {
        MenuTheme {
            item_bg: (Rgb(0x30, 0x30, 0x30), Rgb(0x08, 0x08, 0x08)),
            item_text: Rgb(0xb8, 0xb8, 0xb8),
            disabled_text: Rgb(0x60, 0x60, 0x60),
            active_bg: (Rgb(0x6d, 0x95, 0xde), Rgb(0x2b, 0x82, 0x9d)),
            active_text: Rgb(0xf8, 0xf8, 0xf8),
            border: Rgb(0x18, 0x18, 0x18),
            border_width: 4,
            padding: 2,
        }
    }
}

impl MenuTheme {
    /// The menu theme of the themerc text `themerc`; a missing key keeps
    /// its default, a value that does not parse is logged and keeps it too.
    /// `menu.border.color` wins over `window.active.border.color` wherever
    /// it is in the file.
    pub fn parse(themerc: &str) -> MenuTheme {
        let mut theme = MenuTheme::default();
        let mut menu_border = None;
        for (key, value) in themerc_entries(themerc) {
            if key == MENU_BORDER_KEY {
                menu_border = theme._colour(key, value).or(menu_border);
                continue;
            }
            theme._set(key, value);
        }
        if let Some(border) = menu_border {
            theme.border = border;
        }
        let solid = |key: &str| !gradient_flag(themerc, key);
        if solid("menu.items.bg") {
            theme.item_bg.1 = theme.item_bg.0;
        }
        if solid("menu.items.active.bg") {
            theme.active_bg.1 = theme.active_bg.0;
        }
        theme
    }

    /// The colour `value` of the themerc `key`; `None`, logged, when it
    /// does not parse.
    fn _colour(&self, key: &str, value: &str) -> Option<Rgb> {
        let rgb = Rgb::parse(value);
        if rgb.is_none() {
            opusdm_ipc::err_log!(LOG_TAG, "themerc {key}: '{value}' is not a colour, default kept");
        }
        rgb
    }

    /// Sets the field of the themerc `key` to `value`; an unknown key is
    /// ignored (the themerc has many keys the menu does not use), a value
    /// that does not parse is logged and the field keeps its value.
    fn _set(&mut self, key: &str, value: &str) {
        let colour = |slot: &mut Rgb| match Rgb::parse(value) {
            Some(rgb) => *slot = rgb,
            None => opusdm_ipc::err_log!(LOG_TAG, "themerc {key}: '{value}' is not a colour, default kept"),
        };
        let width = |slot: &mut u32| match value.parse::<u32>() {
            Ok(w) => *slot = w,
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "themerc {key}: '{value}': {e}, default kept"),
        };
        match key {
            "menu.items.bg.color" => colour(&mut self.item_bg.0),
            "menu.items.bg.colorTo" => colour(&mut self.item_bg.1),
            "menu.items.text.color" => colour(&mut self.item_text),
            "menu.items.disabled.text.color" => colour(&mut self.disabled_text),
            "menu.items.active.bg.color" => colour(&mut self.active_bg.0),
            "menu.items.active.bg.colorTo" => colour(&mut self.active_bg.1),
            "menu.items.active.text.color" => colour(&mut self.active_text),
            "window.active.border.color" => colour(&mut self.border),
            "border.width" => width(&mut self.border_width),
            "padding.width" => width(&mut self.padding),
            _ => {}
        }
    }

    /// The menu theme of the first `OpusDMTheme/openbox-3/themerc` found
    /// in `$XDG_DATA_HOME/themes` (default `~/.local/share/themes`),
    /// `~/.themes` and `/usr/share/themes`; the defaults, with one
    /// `err_log!`, when there is none.
    pub fn load() -> MenuTheme {
        let dirs = theme_dirs();
        if let Some(theme) = MenuTheme::load_from(&dirs) { return theme; }
        let searched: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        opusdm_ipc::err_log!(LOG_TAG, "window menu theme {THEME_NAME} not found in {searched:?}, built-in colours used");
        MenuTheme::default()
    }

    /// The menu theme of the first readable themerc of `THEME_NAME` in
    /// `dirs`; `None` when there is none (a file that exists but cannot be
    /// read is logged and skipped).
    pub fn load_from(dirs: &[PathBuf]) -> Option<MenuTheme> {
        read_themerc(dirs, "window menu theme").map(|text| MenuTheme::parse(&text))
    }
}

/// The `key: value` entries of the themerc text `themerc`, trimmed, in
/// file order; blank lines, comments (`!` or `#`) and lines without `:`
/// are skipped.
pub fn themerc_entries(themerc: &str) -> impl Iterator<Item = (&str, &str)> {
    themerc.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('#') { return None; }
        let (key, value) = line.split_once(':')?;
        Some((key.trim(), value.trim()))
    })
}

/// True when the texture `key` of the themerc text `themerc` (for example
/// `window.active.title.bg: raised splitvertical gradient`) is a gradient,
/// so its `colorTo` is used; false when the key says `solid`,
/// `parentrelative` or anything without `gradient` (the colour alone is
/// drawn, as Openbox). A missing key counts as a gradient: the built-in
/// colours are gradients. The last entry of `key` wins.
pub fn gradient_flag(themerc: &str, key: &str) -> bool {
    let Some(value) = themerc_entries(themerc).filter(|(k, _)| *k == key).map(|(_, v)| v).last() else { return true };
    value.split_whitespace().any(|word| word.eq_ignore_ascii_case("gradient"))
}

/// The text of the first readable themerc of `THEME_NAME` in `dirs`
/// (`what` names the theme in the logs); `None` when there is none (a file
/// that exists but cannot be read is logged and skipped).
pub fn read_themerc(dirs: &[PathBuf], what: &str) -> Option<String> {
    for dir in dirs {
        let path = _themerc_in(dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                opusdm_ipc::debug_log!(LOG_TAG, "{what} read from {}", path.display());
                return Some(text);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "{what} {}: {e}, skipped", path.display()),
        }
    }
    None
}

/// The themes directories searched for `THEME_NAME`, in order:
/// `$XDG_DATA_HOME/themes` (default `~/.local/share/themes`), `~/.themes`
/// and `/usr/share/themes`.
pub fn theme_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let mut dirs = Vec::new();
    dirs.extend(data_home.map(|d| d.join("themes")));
    dirs.extend(home.map(|h| h.join(".themes")));
    dirs.push(PathBuf::from("/usr/share/themes"));
    dirs
}

/// Path of the themerc of `THEME_NAME` inside the themes directory `dir`.
fn _themerc_in(dir: &Path) -> PathBuf {
    dir.join(THEME_NAME).join(THEMERC_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The themerc of OpusDMTheme shipped in the repository.
    const REPO_THEMERC: &str = include_str!("../../../../assets/openbox-share/themes/OpusDMTheme/openbox-3/themerc");

    #[test]
    fn colours_parse_in_hex_short_hex_and_by_name() {
        assert_eq!(Rgb::parse("#6d95de"), Some(Rgb(0x6d, 0x95, 0xde)));
        assert_eq!(Rgb::parse(" #6D95DE "), Some(Rgb(0x6d, 0x95, 0xde)));
        assert_eq!(Rgb::parse("#fa0"), Some(Rgb(0xff, 0xaa, 0x00)));
        assert_eq!(Rgb::parse("White"), Some(Rgb(0xff, 0xff, 0xff)));
        assert_eq!(Rgb::parse("black"), Some(Rgb(0, 0, 0)));
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("#gg0000"), None);
        assert_eq!(Rgb::parse("teal"), None);
    }

    #[test]
    fn the_repo_themerc_gives_the_opusdm_colours() {
        let theme = MenuTheme::parse(REPO_THEMERC);
        assert_eq!(theme.active_bg.0, Rgb(0x6d, 0x95, 0xde));
        assert_eq!(theme.active_bg.1, Rgb(0x2b, 0x82, 0x9d));
        assert_eq!(theme.item_bg, (Rgb(0x30, 0x30, 0x30), Rgb(0x08, 0x08, 0x08)));
        assert_eq!(theme.item_text, Rgb(0xb8, 0xb8, 0xb8));
        assert_eq!(theme.disabled_text, Rgb(0x60, 0x60, 0x60));
        assert_eq!(theme.active_text, Rgb(0xf8, 0xf8, 0xf8));
        assert_eq!((theme.border_width, theme.padding), (4, 2));
        assert_eq!(theme.border, Rgb(0x18, 0x18, 0x18));
        assert_eq!(theme, MenuTheme::default(), "the defaults are OpusDMTheme's values");
    }

    #[test]
    fn the_menu_border_colour_wins_over_the_window_border_colour() {
        let theme = MenuTheme::parse("menu.border.color: #010101\nwindow.active.border.color: #020202\n");
        assert_eq!(theme.border, Rgb(1, 1, 1));
        let theme = MenuTheme::parse("window.active.border.color: #020202\n");
        assert_eq!(theme.border, Rgb(2, 2, 2));
        let theme = MenuTheme::parse("menu.border.color: bad\nwindow.active.border.color: #020202\n");
        assert_eq!(theme.border, Rgb(2, 2, 2), "a bad menu border falls back to the window border");
    }

    #[test]
    fn missing_or_bad_keys_keep_the_defaults() {
        let theme = MenuTheme::parse("! comment\nmenu.items.text.color: #010203\nborder.width: wide\nmenu.items.active.bg.color: nope\n");
        assert_eq!(theme.item_text, Rgb(1, 2, 3));
        assert_eq!(theme.border_width, 4);
        assert_eq!(theme.active_bg, MenuTheme::default().active_bg);
        assert_eq!(MenuTheme::parse(""), MenuTheme::default());
    }

    #[test]
    fn a_texture_without_gradient_draws_its_colour_alone() {
        assert!(gradient_flag(REPO_THEMERC, "window.active.title.bg"));
        assert!(gradient_flag(REPO_THEMERC, "window.inactive.title.bg"), "gradient splitvertical gradient flat");
        assert!(!gradient_flag(REPO_THEMERC, "window.active.label.bg"), "parentrelative");
        assert!(gradient_flag("", "menu.items.bg"), "a missing key keeps the built-in gradient");
        assert!(!gradient_flag("menu.items.bg: flat solid\n", "menu.items.bg"));
        let theme = MenuTheme::parse("menu.items.bg: flat solid\nmenu.items.bg.color: #010101\nmenu.items.bg.colorTo: #020202\n");
        assert_eq!(theme.item_bg, (Rgb(1, 1, 1), Rgb(1, 1, 1)), "a solid texture ignores colorTo");
        assert_eq!(theme.active_bg, MenuTheme::default().active_bg);
    }

    #[test]
    fn themerc_entries_skip_comments_and_lines_without_a_colon() {
        let entries: Vec<_> = themerc_entries("! c\n# c\n\nno colon\n a.b : x y \n").collect();
        assert_eq!(entries, vec![("a.b", "x y")]);
    }

    #[test]
    fn the_first_themes_directory_with_the_theme_wins() {
        let empty = tempfile::tempdir().unwrap();
        let found = tempfile::tempdir().unwrap();
        let rc = _themerc_in(found.path());
        std::fs::create_dir_all(rc.parent().unwrap()).unwrap();
        std::fs::write(&rc, "padding.width: 7\n").unwrap();
        let dirs = [empty.path().to_path_buf(), found.path().to_path_buf()];
        assert_eq!(MenuTheme::load_from(&dirs).map(|t| t.padding), Some(7));
        assert_eq!(MenuTheme::load_from(&[empty.path().to_path_buf()]), None);
    }
}
