//! Desktop-window configuration: background, icon grid, and which directory
//! is shown as the desktop. Persisted to `~/.config/opusdm/desktop.json`,
//! owned by the hub, and sent from `opus-desktop-config` as
//! `ToHub::SetDesktopSettings`. Pure data - no GTK.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Lower/upper bound for [`DesktopSettings::icon_size`], in pixels. Clamped by
/// [`DesktopSettings::sanitized`] so a hand-edited desktop.json cannot make an
/// icon vanish or fill the screen.
pub const ICON_SIZE_RANGE: (i32, i32) = (24, 256);
/// Accepted range for [`DesktopSettings::dock_icon_size`], in pixels.
pub const DOCK_ICON_SIZE_RANGE: (i32, i32) = (16, 128);

/// Lower/upper bound for [`DesktopSettings::grid_spacing`], in pixels. The low
/// end is below the smallest icon so a tight grid can pack icons closer than
/// their labels are wide (labels then overlap, which is the user's choice).
pub const GRID_SPACING_RANGE: (i32, i32) = (24, 320);

/// Lower/upper bound for [`DesktopSettings::grid_gap`], in pixels.
pub const GRID_GAP_RANGE: (i32, i32) = (0, 64);

/// Largest accepted gradient angle, in degrees.
pub const GRADIENT_ANGLE_MAX: u16 = 360;

/// Which screen edge the panel (`opusdm-hub`'s dock bar) is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelPosition {
    /// Docked along the top edge of every monitor.
    #[default]
    Top,
    /// Docked along the bottom edge of every monitor.
    Bottom,
}

/// Which screen edge the dock (`opusdm-dock`) is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    /// Docked along the bottom edge of the primary monitor.
    #[default]
    Bottom,
    /// Docked along the top edge of the primary monitor.
    Top,
    /// Docked along the left edge of the primary monitor.
    Left,
    /// Docked along the right edge of the primary monitor.
    Right,
}

impl DockPosition {
    /// The edge the dock really uses: the dock and the panel never share an
    /// edge, so when `self` is the panel's edge the dock jumps to the
    /// opposite one.
    pub fn avoiding(self, panel: PanelPosition) -> DockPosition {
        match (self, panel) {
            (DockPosition::Top, PanelPosition::Top) => DockPosition::Bottom,
            (DockPosition::Bottom, PanelPosition::Bottom) => DockPosition::Top,
            (other, _) => other,
        }
    }
}

/// Which kind of background the desktop window paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode {
    /// A single flat colour ([`Background::solid`]).
    #[default]
    Solid,
    /// A two-stop linear gradient.
    Gradient,
    /// An image file ([`Background::image_path`]).
    Image,
}

/// How a background image is fitted to the desktop window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Cover the window, cropping overflow, keeping aspect ratio.
    #[default]
    Fill,
    /// Fit the whole image inside the window, keeping aspect ratio.
    Fit,
    /// Draw at natural size, centred.
    Center,
    /// Repeat at natural size from the top-left.
    Tile,
}

impl ImageFit {
    /// Parse `fill` / `fit` / `center` / `tile`, ignoring case and
    /// surrounding whitespace. Anything else is `None`.
    pub fn parse(text: &str) -> Option<ImageFit> {
        match text.trim().to_ascii_lowercase().as_str() {
            "fill" => Some(ImageFit::Fill),
            "fit" => Some(ImageFit::Fit),
            "center" => Some(ImageFit::Center),
            "tile" => Some(ImageFit::Tile),
            _ => None,
        }
    }
}

/// File extensions (lower case) accepted as a desktop background image.
pub const BACKGROUND_IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

/// Whether `path` has one of the [`BACKGROUND_IMAGE_EXTS`] extensions,
/// ignoring case. Looks at the name only, never at the file content.
pub fn is_background_image(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| BACKGROUND_IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// The desktop background. Only the field selected by [`Background::mode`] is
/// painted; the others keep their last value so switching modes back and forth
/// is lossless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub mode: BackgroundMode,
    /// Hex `#rrggbb`. Also the fallback colour behind an image while it loads
    /// or when its path is unreadable.
    pub solid: String,
    /// Gradient start colour, hex `#rrggbb`.
    pub gradient_start: String,
    /// Gradient end colour, hex `#rrggbb`.
    pub gradient_end: String,
    /// Gradient direction in degrees, `0..=GRADIENT_ANGLE_MAX`. 0 points up,
    /// 90 points right (CSS `linear-gradient` convention).
    pub gradient_angle: u16,
    /// Absolute path to the background image, or `None`.
    pub image_path: Option<PathBuf>,
    pub image_fit: ImageFit,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            mode: BackgroundMode::Solid,
            solid: "#2e3436".to_string(),
            gradient_start: "#2e3436".to_string(),
            gradient_end: "#555753".to_string(),
            gradient_angle: 180,
            image_path: None,
            image_fit: ImageFit::Fill,
        }
    }
}

/// Every desktop-window setting `opus-desktop-config` owns. On-disk shape of
/// `~/.config/opusdm/desktop.json`, the hub's in-memory copy, and the payload
/// of [`crate::ToHub::SetDesktopSettings`] - one struct, three roles, like
/// [`crate::Settings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    /// Icon box size in pixels for the desktop icon field.
    pub icon_size: i32,
    /// Auto-slot cell size in pixels for the desktop icon field: the room each
    /// icon gets before [`Self::grid_gap`] is added. Together they set the grid
    /// pitch, independent of how wide the labels measure.
    pub grid_spacing: i32,
    /// Dead space in pixels added between auto-slot cells on both axes.
    pub grid_gap: i32,
    /// Directory shown as the desktop. `None` means resolve it from
    /// `$XDG_DESKTOP_DIR` then `~/Desktop`, as the hub does today.
    pub desktop_dir: Option<PathBuf>,
    /// Screen edge the panel is docked to.
    pub panel_position: PanelPosition,
    /// Screen edge the dock is docked to (see [`DockPosition::avoiding`]).
    pub dock_position: DockPosition,
    /// Dock launcher icon size in pixels.
    pub dock_icon_size: i32,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            icon_size: 64,
            grid_spacing: 96,
            grid_gap: 4,
            desktop_dir: None,
            panel_position: PanelPosition::Top,
            dock_position: DockPosition::Bottom,
            dock_icon_size: 48,
        }
    }
}

impl DesktopSettings {
    /// Clamp every numeric field into its accepted range. Call right after
    /// loading from disk and right after receiving a value over IPC.
    pub fn sanitized(mut self) -> DesktopSettings {
        let (lo, hi) = ICON_SIZE_RANGE;
        self.icon_size = self.icon_size.clamp(lo, hi);
        let (lo, hi) = GRID_SPACING_RANGE;
        self.grid_spacing = self.grid_spacing.clamp(lo, hi);
        let (lo, hi) = GRID_GAP_RANGE;
        self.grid_gap = self.grid_gap.clamp(lo, hi);
        let (lo, hi) = DOCK_ICON_SIZE_RANGE;
        self.dock_icon_size = self.dock_icon_size.clamp(lo, hi);
        self
    }
}

impl Background {
    /// Clamp the gradient angle into its accepted range. Call right after
    /// loading from disk and right after receiving a value over IPC.
    pub fn sanitized(mut self) -> Background {
        self.gradient_angle = self.gradient_angle.min(GRADIENT_ANGLE_MAX);
        self
    }
}

/// Bytes that never need percent-encoding in a path -> URL conversion:
/// RFC 3986 unreserved, plus `/` which is already a path separator.
fn is_url_safe(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/')
}

/// Turn an absolute filesystem path into a `file://` URL, percent-encoding
/// every byte outside [`is_url_safe`]. Non-absolute paths are used as-is
/// after the `file://` prefix (the caller is expected to pass absolute
/// paths; the hub already stores absolute paths for desktop entries).
pub fn path_to_file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if is_url_safe(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Build the declaration body of the `.opusdm-desktop` CSS rule from `bg`.
/// Returns declarations only - no selector, no braces - so the caller can
/// drop it straight into `".opusdm-desktop {{ {} }}"`.
///
/// `Image` mode with no path falls back to the `Solid` branch here so the
/// desktop is never left transparent; an unreadable-but-set path is the
/// caller's concern.
pub fn background_css(bg: &Background) -> String {
    match bg.mode {
        BackgroundMode::Solid => format!("background: {};", bg.solid),
        BackgroundMode::Gradient => format!(
            "background: linear-gradient({}deg, {}, {});",
            bg.gradient_angle, bg.gradient_start, bg.gradient_end
        ),
        BackgroundMode::Image => {
            let Some(path) = bg.image_path.as_deref() else {
                return format!("background: {};", bg.solid);
            };
            let url = path_to_file_url(path);
            let (size, repeat) = match bg.image_fit {
                ImageFit::Fill => ("cover", "no-repeat"),
                ImageFit::Fit => ("contain", "no-repeat"),
                ImageFit::Center => ("auto", "no-repeat"),
                ImageFit::Tile => ("auto", "repeat"),
            };
            format!(
                "background-color: {}; background-image: url(\"{}\"); \
                 background-position: center; background-size: {}; \
                 background-repeat: {};",
                bg.solid, url, size, repeat
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reproduces_the_current_desktop_look() {
        let d = DesktopSettings::default();
        assert_eq!(d.icon_size, 64);
        assert_eq!(d.grid_spacing, 96);
        assert_eq!(d.grid_gap, 4);
        assert_eq!(d.desktop_dir, None);

        let bg = Background::default();
        assert_eq!(bg.mode, BackgroundMode::Solid);
        assert_eq!(bg.solid, "#2e3436");
        assert_eq!(bg.image_fit, ImageFit::Fill);
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let d: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(d, DesktopSettings::default());
        let bg: Background = serde_json::from_str(r#"{"mode":"gradient"}"#).unwrap();
        assert_eq!(bg.mode, BackgroundMode::Gradient);
        assert_eq!(bg.solid, Background::default().solid);
    }

    #[test]
    fn roundtrips_through_json() {
        let mut d = DesktopSettings::default();
        d.icon_size = 96;
        d.desktop_dir = Some(PathBuf::from("/home/u/Desk"));
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<DesktopSettings>(&s).unwrap(), d);

        let mut bg = Background::default();
        bg.mode = BackgroundMode::Image;
        bg.image_path = Some(PathBuf::from("/home/u/pic.png"));
        bg.image_fit = ImageFit::Tile;
        let s = serde_json::to_string(&bg).unwrap();
        assert_eq!(serde_json::from_str::<Background>(&s).unwrap(), bg);
    }

    #[test]
    fn enum_names_are_snake_case_on_the_wire() {
        let s = serde_json::to_string(&BackgroundMode::Gradient).unwrap();
        assert_eq!(s, "\"gradient\"");
        let s = serde_json::to_string(&ImageFit::Center).unwrap();
        assert_eq!(s, "\"center\"");
    }

    #[test]
    fn sanitized_clamps_every_numeric_field_at_both_ends() {
        let mut d = DesktopSettings::default();
        d.icon_size = 5;
        d.grid_spacing = 9999;
        d.grid_gap = 9999;
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.0);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.1);
        assert_eq!(d.grid_gap, GRID_GAP_RANGE.1);

        let mut bg = Background::default();
        bg.gradient_angle = 900;
        assert_eq!(bg.sanitized().gradient_angle, GRADIENT_ANGLE_MAX);

        let mut d = DesktopSettings::default();
        d.icon_size = 999;
        d.grid_spacing = 1;
        d.grid_gap = -5;
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.1);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.0);
        assert_eq!(d.grid_gap, GRID_GAP_RANGE.0);
    }

    #[test]
    fn solid_css_is_one_background_declaration() {
        let mut bg = Background::default();
        bg.mode = BackgroundMode::Solid;
        bg.solid = "#123456".into();
        assert_eq!(background_css(&bg), "background: #123456;");
    }

    #[test]
    fn panel_position_defaults_to_top_and_roundtrips() {
        assert_eq!(DesktopSettings::default().panel_position, PanelPosition::Top);
        let d = DesktopSettings { panel_position: PanelPosition::Bottom, ..Default::default() };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"bottom\""));
        assert_eq!(serde_json::from_str::<DesktopSettings>(&json).unwrap(), d);
        // An older desktop.json without the field keeps the panel on top.
        let old: DesktopSettings = serde_json::from_str("{\"icon_size\":64}").unwrap();
        assert_eq!(old.panel_position, PanelPosition::Top);
    }

    #[test]
    fn dock_defaults_and_roundtrips() {
        let d = DesktopSettings::default();
        assert_eq!((d.dock_position, d.dock_icon_size), (DockPosition::Bottom, 48));
        let d = DesktopSettings { dock_position: DockPosition::Left, ..Default::default() };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"left\""));
        assert_eq!(serde_json::from_str::<DesktopSettings>(&json).unwrap(), d);
        // An older desktop.json without the fields keeps the defaults.
        let old: DesktopSettings = serde_json::from_str("{\"icon_size\":64}").unwrap();
        assert_eq!(old.dock_position, DockPosition::Bottom);
    }

    #[test]
    fn dock_icon_size_is_clamped() {
        let d = DesktopSettings { dock_icon_size: 999, ..Default::default() }.sanitized();
        assert_eq!(d.dock_icon_size, DOCK_ICON_SIZE_RANGE.1);
        let d = DesktopSettings { dock_icon_size: 1, ..Default::default() }.sanitized();
        assert_eq!(d.dock_icon_size, DOCK_ICON_SIZE_RANGE.0);
    }

    #[test]
    fn dock_never_shares_the_panel_edge() {
        use DockPosition::*;
        assert_eq!(Top.avoiding(PanelPosition::Top), Bottom);
        assert_eq!(Bottom.avoiding(PanelPosition::Bottom), Top);
        assert_eq!(Bottom.avoiding(PanelPosition::Top), Bottom);
        assert_eq!(Top.avoiding(PanelPosition::Bottom), Top);
        assert_eq!(Left.avoiding(PanelPosition::Top), Left);
        assert_eq!(Right.avoiding(PanelPosition::Bottom), Right);
    }

    #[test]
    fn gradient_css_renders_angle_and_stops() {
        let mut bg = Background::default();
        bg.mode = BackgroundMode::Gradient;
        bg.gradient_start = "#000000".into();
        bg.gradient_end = "#ffffff".into();
        bg.gradient_angle = 45;
        assert_eq!(
            background_css(&bg),
            "background: linear-gradient(45deg, #000000, #ffffff);"
        );
    }

    #[test]
    fn image_css_maps_each_fit_mode() {
        let base = |fit| {
            let mut bg = Background::default();
            bg.mode = BackgroundMode::Image;
            bg.image_path = Some(PathBuf::from("/x/y.png"));
            bg.solid = "#111111".into();
            bg.image_fit = fit;
            background_css(&bg)
        };
        let fill = base(ImageFit::Fill);
        assert!(fill.contains("background-image: url(\"file:///x/y.png\");"));
        assert!(fill.contains("background-color: #111111;"));
        assert!(fill.contains("background-size: cover;"));
        assert!(fill.contains("background-repeat: no-repeat;"));

        assert!(base(ImageFit::Fit).contains("background-size: contain;"));
        assert!(base(ImageFit::Center).contains("background-size: auto;"));
        assert!(base(ImageFit::Center).contains("background-repeat: no-repeat;"));

        let tile = base(ImageFit::Tile);
        assert!(tile.contains("background-repeat: repeat;"));
        assert!(tile.contains("background-size: auto;"));
    }

    #[test]
    fn image_mode_without_a_path_falls_back_to_solid() {
        let mut bg = Background::default();
        bg.mode = BackgroundMode::Image;
        bg.image_path = None;
        bg.solid = "#abcdef".into();
        assert_eq!(background_css(&bg), "background: #abcdef;");
    }

    #[test]
    fn file_url_percent_encodes_spaces_unicode_and_specials() {
        assert_eq!(path_to_file_url(Path::new("/a/b.png")), "file:///a/b.png");
        assert_eq!(
            path_to_file_url(Path::new("/a b/c#d%e.png")),
            "file:///a%20b/c%23d%25e.png"
        );
        assert_eq!(
            path_to_file_url(Path::new("/home/\u{00fc}ser/p.png")),
            "file:///home/%C3%BCser/p.png"
        );
    }

    #[test]
    fn image_fit_parse_accepts_the_four_names_case_insensitively() {
        assert_eq!(ImageFit::parse("fill"), Some(ImageFit::Fill));
        assert_eq!(ImageFit::parse(" FIT "), Some(ImageFit::Fit));
        assert_eq!(ImageFit::parse("Center"), Some(ImageFit::Center));
        assert_eq!(ImageFit::parse("tile"), Some(ImageFit::Tile));
        assert_eq!(ImageFit::parse("stretch"), None);
        assert_eq!(ImageFit::parse(""), None);
    }

    #[test]
    fn is_background_image_checks_the_extension_case_insensitively() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.WebP", "a.bmp", "a.gif"] {
            assert!(is_background_image(std::path::Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.svg", "png", "a."] {
            assert!(!is_background_image(std::path::Path::new(name)), "{name}");
        }
    }

    #[test]
    fn background_css_percent_encodes_spaces_in_the_image_path() {
        let bg = Background {
            mode: BackgroundMode::Image,
            image_path: Some(PathBuf::from("/tmp/my wall.png")),
            ..Background::default()
        };
        assert!(background_css(&bg).contains("my%20wall.png"));
    }
}
