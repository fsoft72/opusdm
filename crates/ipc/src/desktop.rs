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
/// Accepted range for [`DesktopSettings::virtual_screens`].
pub const VIRTUAL_SCREENS_RANGE: (u32, u32) = (1, 16);
/// Default for [`DesktopSettings::virtual_screens`].
pub const DEFAULT_VIRTUAL_SCREENS: u32 = 4;
/// Accepted range, in percent, for [`DesktopSettings::inactive_opacity`] and
/// [`DesktopSettings::active_opacity`]. The low end keeps a window readable.
pub const WINDOW_OPACITY_RANGE: (u32, u32) = (20, 100);
/// Default for [`DesktopSettings::inactive_opacity`], in percent.
pub const DEFAULT_INACTIVE_OPACITY: u32 = 88;
/// Default for [`DesktopSettings::active_opacity`], in percent.
pub const DEFAULT_ACTIVE_OPACITY: u32 = 100;
/// Accepted range, in seconds, for [`StartupApp::wait_secs`].
pub const STARTUP_WAIT_SECS_RANGE: (u32, u32) = (0, 600);

/// Lower/upper bound for [`DesktopSettings::grid_spacing`], in pixels. The low
/// end is below the smallest icon so a tight grid can pack icons closer than
/// their labels are wide (labels then overlap, which is the user's choice).
pub const GRID_SPACING_RANGE: (i32, i32) = (24, 320);

/// Lower/upper bound for [`DesktopSettings::grid_gap`], in pixels.
pub const GRID_GAP_RANGE: (i32, i32) = (0, 64);

/// Largest accepted gradient angle, in degrees.
pub const GRADIENT_ANGLE_MAX: u16 = 360;

/// Accepted range for [`Background::interval_secs`], in seconds.
pub const INTERVAL_SECS_RANGE: (u32, u32) = (5, 86_400);

/// Accepted range for [`Background::fade_ms`], in milliseconds.
pub const FADE_MS_RANGE: (u32, u32) = (0, 10_000);

/// Default colour of a solid background.
pub const DEFAULT_SOLID: &str = "#2e3436";

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

/// Which kind of background the legacy single-background shape selected.
/// Only read when migrating an old settings file (see [`Background`]) and by
/// the legacy `theme.json` section.
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

/// File extensions (lower case) accepted as a desktop background video.
pub const BACKGROUND_VIDEO_EXTS: [&str; 7] = ["mp4", "mkv", "webm", "avi", "mov", "m4v", "ogv"];

/// Whether `path` has one of the [`BACKGROUND_VIDEO_EXTS`] extensions,
/// ignoring case. Looks at the name only, never at the file content.
pub fn is_background_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| BACKGROUND_VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Whether `path` is a picture or a video the desktop background can show.
pub fn is_background_media(path: &Path) -> bool {
    is_background_image(path) || is_background_video(path)
}

/// One item of the desktop background rotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundEntry {
    /// A flat colour, hex `#rrggbb`.
    Solid { color: String },
    /// A two-stop linear gradient. The angle is in degrees,
    /// `0..=GRADIENT_ANGLE_MAX`: 0 points up, 90 points right (CSS
    /// `linear-gradient` convention).
    Gradient { start: String, end: String, angle: u16 },
    /// A picture file.
    Image { path: PathBuf, fit: ImageFit },
    /// A silent, looping video file. `Center` and `Tile` behave as `Fill`.
    Video { path: PathBuf, fit: ImageFit },
}

impl BackgroundEntry {
    /// The default solid entry.
    pub fn default_solid() -> Self {
        BackgroundEntry::Solid { color: DEFAULT_SOLID.to_string() }
    }

    /// The default gradient entry.
    pub fn default_gradient() -> Self {
        BackgroundEntry::Gradient { start: DEFAULT_SOLID.to_string(), end: "#555753".to_string(), angle: 180 }
    }

    /// A picture or video entry for `path`, chosen by its extension, with the
    /// default fit. `None` when `path` is neither.
    pub fn from_path(path: PathBuf) -> Option<Self> {
        let fit = ImageFit::default();
        if is_background_video(&path) {
            Some(BackgroundEntry::Video { path, fit })
        } else if is_background_image(&path) {
            Some(BackgroundEntry::Image { path, fit })
        } else {
            None
        }
    }

    /// The file this entry shows, for picture and video entries.
    pub fn path(&self) -> Option<&Path> {
        match self {
            BackgroundEntry::Image { path, .. } | BackgroundEntry::Video { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Whether the entry plays a video.
    pub fn is_video(&self) -> bool {
        matches!(self, BackgroundEntry::Video { .. })
    }

    /// Whether the entry can be shown right now: colours and gradients
    /// always, pictures and videos when their file exists.
    pub fn is_usable(&self) -> bool {
        self.path().map(Path::is_file).unwrap_or(true)
    }
}

/// The desktop background: a list of entries shown in turn, `interval_secs`
/// each, with a `fade_ms` cross-fade between them. A list of one entry never
/// rotates.
///
/// Deserialisation also accepts the legacy single-background shape (`mode`,
/// `solid`, `gradient_start`, `gradient_end`, `gradient_angle`, `image_path`,
/// `image_fit`), which becomes a list of one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "BackgroundWire")]
pub struct Background {
    pub entries: Vec<BackgroundEntry>,
    /// Seconds each entry stays on screen, `INTERVAL_SECS_RANGE`.
    pub interval_secs: u32,
    /// Cross-fade duration in milliseconds, `FADE_MS_RANGE`.
    pub fade_ms: u32,
    /// Walk the entries in a random order instead of the list order.
    pub shuffle: bool,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            entries: vec![BackgroundEntry::default_solid()],
            interval_secs: 300,
            fade_ms: 1000,
            shuffle: false,
        }
    }
}

/// Every field either shape of a stored background can carry. Private: the
/// serde bridge behind `Background`'s `Deserialize`.
#[derive(Deserialize)]
#[serde(default)]
struct BackgroundWire {
    entries: Option<Vec<BackgroundEntry>>,
    interval_secs: u32,
    fade_ms: u32,
    shuffle: bool,
    mode: BackgroundMode,
    solid: String,
    gradient_start: String,
    gradient_end: String,
    gradient_angle: u16,
    image_path: Option<PathBuf>,
    image_fit: ImageFit,
}

impl Default for BackgroundWire {
    fn default() -> Self {
        let defaults = Background::default();
        let BackgroundEntry::Gradient { start, end, angle } = BackgroundEntry::default_gradient() else {
            unreachable!("default_gradient builds a gradient")
        };
        Self {
            entries: None,
            interval_secs: defaults.interval_secs,
            fade_ms: defaults.fade_ms,
            shuffle: defaults.shuffle,
            mode: BackgroundMode::Solid,
            solid: DEFAULT_SOLID.to_string(),
            gradient_start: start,
            gradient_end: end,
            gradient_angle: angle,
            image_path: None,
            image_fit: ImageFit::Fill,
        }
    }
}

impl From<BackgroundWire> for Background {
    fn from(w: BackgroundWire) -> Self {
        let entries = w.entries.unwrap_or_else(|| {
            let entry = match (w.mode, w.image_path) {
                (BackgroundMode::Image, Some(path)) => BackgroundEntry::Image { path, fit: w.image_fit },
                (BackgroundMode::Gradient, _) => BackgroundEntry::Gradient {
                    start: w.gradient_start,
                    end: w.gradient_end,
                    angle: w.gradient_angle,
                },
                _ => BackgroundEntry::Solid { color: w.solid },
            };
            vec![entry]
        });
        Background { entries, interval_secs: w.interval_secs, fade_ms: w.fade_ms, shuffle: w.shuffle }
    }
}

/// One application started when the session starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupApp {
    /// Program and arguments, split like an `exec:` hotkey (quotes group).
    pub command: String,
    /// Seconds to wait, counted from the hub start, before launching it.
    #[serde(default)]
    pub wait_secs: u32,
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
    /// Number of virtual screens (desktops) the window manager offers.
    pub virtual_screens: u32,
    /// Master switch for window transparency. Off means every window is drawn
    /// fully opaque, whatever the two opacity values below say.
    pub transparency_enabled: bool,
    /// Opacity in percent of the windows that do not have the focus.
    pub inactive_opacity: u32,
    /// Opacity in percent of the window that has the focus.
    pub active_opacity: u32,
    /// Applications started when the session starts, each after its own wait.
    pub startup_apps: Vec<StartupApp>,
    /// Whether dragging a window to a screen edge snaps it to a zone (half
    /// screen, quarters, thirds, maximized), with a preview of the zone.
    pub snap_enabled: bool,
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
            virtual_screens: DEFAULT_VIRTUAL_SCREENS,
            transparency_enabled: true,
            inactive_opacity: DEFAULT_INACTIVE_OPACITY,
            active_opacity: DEFAULT_ACTIVE_OPACITY,
            startup_apps: Vec::new(),
            snap_enabled: true,
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
        let (lo, hi) = VIRTUAL_SCREENS_RANGE;
        self.virtual_screens = self.virtual_screens.clamp(lo, hi);
        let (lo, hi) = WINDOW_OPACITY_RANGE;
        self.inactive_opacity = self.inactive_opacity.clamp(lo, hi);
        self.active_opacity = self.active_opacity.clamp(lo, hi);
        let (lo, hi) = STARTUP_WAIT_SECS_RANGE;
        for app in &mut self.startup_apps {
            app.command = app.command.trim().to_string();
            app.wait_secs = app.wait_secs.clamp(lo, hi);
        }
        self.startup_apps.retain(|app| !app.command.is_empty());
        self
    }
}

impl Background {
    /// Clamp the interval, the fade and every gradient angle into their
    /// accepted ranges. Call right after loading from disk and right after
    /// receiving a value over IPC.
    pub fn sanitized(mut self) -> Background {
        let (lo, hi) = INTERVAL_SECS_RANGE;
        self.interval_secs = self.interval_secs.clamp(lo, hi);
        let (lo, hi) = FADE_MS_RANGE;
        self.fade_ms = self.fade_ms.clamp(lo, hi);
        for entry in &mut self.entries {
            if let BackgroundEntry::Gradient { angle, .. } = entry {
                *angle = (*angle).min(GRADIENT_ANGLE_MAX);
            }
        }
        self
    }

    /// The entries to rotate through: the list itself, or the default solid
    /// colour when the list is empty so the desktop is never left bare.
    pub fn effective_entries(&self) -> Vec<BackgroundEntry> {
        if self.entries.is_empty() {
            vec![BackgroundEntry::default_solid()]
        } else {
            self.entries.clone()
        }
    }

    /// Replace the list with the single picture or video at `path`. `fit`
    /// overrides the default fit when `Some`. The interval, fade and shuffle
    /// values are kept. Does nothing when `path` is neither a picture nor a
    /// video.
    pub fn set_single(&mut self, path: PathBuf, fit: Option<ImageFit>) {
        let Some(mut entry) = BackgroundEntry::from_path(path) else { return };
        if let (Some(new_fit), BackgroundEntry::Image { fit, .. } | BackgroundEntry::Video { fit, .. }) = (fit, &mut entry) {
            *fit = new_fit;
        }
        self.entries = vec![entry];
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

/// Build the declaration body of the CSS rule that paints `entry`. Returns
/// declarations only - no selector, no braces - so the caller can drop it
/// straight into `".some-class {{ {} }}"`.
///
/// A video entry yields a black backdrop: the picture that plays it covers
/// it, and it shows around a `Fit` video.
pub fn background_css(entry: &BackgroundEntry) -> String {
    match entry {
        BackgroundEntry::Solid { color } => format!("background: {color};"),
        BackgroundEntry::Gradient { start, end, angle } => {
            format!("background: linear-gradient({angle}deg, {start}, {end});")
        }
        BackgroundEntry::Image { path, fit } => {
            let url = path_to_file_url(path);
            let (size, repeat) = match fit {
                ImageFit::Fill => ("cover", "no-repeat"),
                ImageFit::Fit => ("contain", "no-repeat"),
                ImageFit::Center => ("auto", "no-repeat"),
                ImageFit::Tile => ("auto", "repeat"),
            };
            format!(
                "background-color: {DEFAULT_SOLID}; background-image: url(\"{url}\"); \
                 background-position: center; background-size: {size}; \
                 background-repeat: {repeat};"
            )
        }
        BackgroundEntry::Video { .. } => "background: #000000;".to_string(),
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
        assert_eq!(bg.entries, vec![BackgroundEntry::default_solid()]);
        assert_eq!((bg.interval_secs, bg.fade_ms, bg.shuffle), (300, 1000, false));
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let d: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(d, DesktopSettings::default());
        let bg: Background = serde_json::from_str("{}").unwrap();
        assert_eq!(bg, Background::default());
    }

    #[test]
    fn roundtrips_through_json() {
        let d = DesktopSettings {
            icon_size: 96,
            desktop_dir: Some(PathBuf::from("/home/u/Desk")),
            ..DesktopSettings::default()
        };
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<DesktopSettings>(&s).unwrap(), d);

        let bg = Background {
            entries: vec![
                BackgroundEntry::Image { path: PathBuf::from("/home/u/pic.png"), fit: ImageFit::Tile },
                BackgroundEntry::Video { path: PathBuf::from("/home/u/clip.mp4"), fit: ImageFit::Fit },
                BackgroundEntry::default_gradient(),
                BackgroundEntry::default_solid(),
            ],
            interval_secs: 60,
            fade_ms: 250,
            shuffle: true,
        };
        let s = serde_json::to_string(&bg).unwrap();
        assert!(s.contains("\"kind\":\"image\"") && s.contains("\"kind\":\"video\""));
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
        let d = DesktopSettings {
            icon_size: 5,
            grid_spacing: 9999,
            grid_gap: 9999,
            ..DesktopSettings::default()
        };
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.0);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.1);
        assert_eq!(d.grid_gap, GRID_GAP_RANGE.1);

        let bg = Background {
            entries: vec![BackgroundEntry::Gradient { start: "#000000".into(), end: "#ffffff".into(), angle: 900 }],
            interval_secs: 1,
            fade_ms: 999_999,
            shuffle: false,
        }
        .sanitized();
        assert!(matches!(bg.entries[0], BackgroundEntry::Gradient { angle: GRADIENT_ANGLE_MAX, .. }));
        assert_eq!((bg.interval_secs, bg.fade_ms), (INTERVAL_SECS_RANGE.0, FADE_MS_RANGE.1));
        let bg = Background { interval_secs: 999_999, ..Background::default() }.sanitized();
        assert_eq!(bg.interval_secs, INTERVAL_SECS_RANGE.1);

        let d = DesktopSettings {
            icon_size: 999,
            grid_spacing: 1,
            grid_gap: -5,
            ..DesktopSettings::default()
        };
        let d = d.sanitized();
        assert_eq!(d.icon_size, ICON_SIZE_RANGE.1);
        assert_eq!(d.grid_spacing, GRID_SPACING_RANGE.0);
        assert_eq!(d.grid_gap, GRID_GAP_RANGE.0);
    }

    #[test]
    fn solid_css_is_one_background_declaration() {
        let entry = BackgroundEntry::Solid { color: "#123456".into() };
        assert_eq!(background_css(&entry), "background: #123456;");
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
    fn virtual_screens_is_clamped_and_defaults_to_four() {
        assert_eq!(DesktopSettings::default().virtual_screens, 4);
        let d = DesktopSettings { virtual_screens: 99, ..Default::default() }.sanitized();
        assert_eq!(d.virtual_screens, VIRTUAL_SCREENS_RANGE.1);
        let d = DesktopSettings { virtual_screens: 0, ..Default::default() }.sanitized();
        assert_eq!(d.virtual_screens, VIRTUAL_SCREENS_RANGE.0);
    }

    #[test]
    fn old_file_without_virtual_screens_gets_the_default() {
        let d: DesktopSettings = serde_json::from_str(r#"{"icon_size":64}"#).unwrap();
        assert_eq!(d.virtual_screens, DEFAULT_VIRTUAL_SCREENS);
    }

    #[test]
    fn window_opacity_is_clamped_and_has_defaults() {
        let d = DesktopSettings::default();
        assert!(d.transparency_enabled);
        assert_eq!((d.inactive_opacity, d.active_opacity), (88, 100));
        let d = DesktopSettings { inactive_opacity: 0, active_opacity: 500, ..Default::default() }.sanitized();
        assert_eq!(d.inactive_opacity, WINDOW_OPACITY_RANGE.0);
        assert_eq!(d.active_opacity, WINDOW_OPACITY_RANGE.1);
    }

    #[test]
    fn snapping_is_on_by_default_and_old_files_get_it() {
        assert!(DesktopSettings::default().snap_enabled);
        let d: DesktopSettings = serde_json::from_str(r#"{"icon_size":64}"#).unwrap();
        assert!(d.snap_enabled);
    }

    #[test]
    fn snapping_off_roundtrips() {
        let d = DesktopSettings { snap_enabled: false, ..Default::default() };
        let back: DesktopSettings = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(!back.snap_enabled);
    }

    #[test]
    fn old_file_without_opacity_gets_the_defaults() {
        let d: DesktopSettings = serde_json::from_str(r#"{"icon_size":64}"#).unwrap();
        assert!(d.transparency_enabled);
        assert_eq!(d.inactive_opacity, DEFAULT_INACTIVE_OPACITY);
        assert_eq!(d.active_opacity, DEFAULT_ACTIVE_OPACITY);
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
        let entry = BackgroundEntry::Gradient { start: "#000000".into(), end: "#ffffff".into(), angle: 45 };
        assert_eq!(background_css(&entry), "background: linear-gradient(45deg, #000000, #ffffff);");
    }

    #[test]
    fn image_css_maps_each_fit_mode() {
        let base = |fit| background_css(&BackgroundEntry::Image { path: PathBuf::from("/x/y.png"), fit });
        let fill = base(ImageFit::Fill);
        assert!(fill.contains("background-image: url(\"file:///x/y.png\");"));
        assert!(fill.contains("background-color: #2e3436;"));
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
    fn video_css_is_a_black_backdrop() {
        let entry = BackgroundEntry::Video { path: PathBuf::from("/x/y.mp4"), fit: ImageFit::Fill };
        assert_eq!(background_css(&entry), "background: #000000;");
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
        let entry = BackgroundEntry::Image { path: PathBuf::from("/tmp/my wall.png"), fit: ImageFit::Fill };
        assert!(background_css(&entry).contains("my%20wall.png"));
    }

    #[test]
    fn legacy_single_background_becomes_one_entry() {
        let solid: Background = serde_json::from_str(r##"{"mode":"solid","solid":"#112233"}"##).unwrap();
        assert_eq!(solid.entries, vec![BackgroundEntry::Solid { color: "#112233".into() }]);

        let gradient: Background = serde_json::from_str(
            r##"{"mode":"gradient","gradient_start":"#000000","gradient_end":"#ffffff","gradient_angle":45}"##,
        )
        .unwrap();
        assert_eq!(
            gradient.entries,
            vec![BackgroundEntry::Gradient { start: "#000000".into(), end: "#ffffff".into(), angle: 45 }]
        );

        let image: Background =
            serde_json::from_str(r#"{"mode":"image","image_path":"/a/b.png","image_fit":"tile"}"#).unwrap();
        assert_eq!(image.entries, vec![BackgroundEntry::Image { path: "/a/b.png".into(), fit: ImageFit::Tile }]);
        assert_eq!((image.interval_secs, image.fade_ms, image.shuffle), (300, 1000, false));

        // Image mode with no path used to fall back to the solid colour.
        let bare: Background = serde_json::from_str(r##"{"mode":"image","solid":"#abcdef"}"##).unwrap();
        assert_eq!(bare.entries, vec![BackgroundEntry::Solid { color: "#abcdef".into() }]);

        // A gradient-only legacy file keeps the default stops.
        let only_mode: Background = serde_json::from_str(r#"{"mode":"gradient"}"#).unwrap();
        assert_eq!(only_mode.entries, vec![BackgroundEntry::default_gradient()]);
    }

    #[test]
    fn video_extensions_are_matched_case_insensitively() {
        assert!(is_background_video(Path::new("a.MP4")));
        assert!(is_background_video(Path::new("a.webm")));
        assert!(!is_background_video(Path::new("a.png")));
        assert!(is_background_media(Path::new("a.png")));
        assert!(is_background_media(Path::new("a.mkv")));
        assert!(!is_background_media(Path::new("a.txt")));
    }

    #[test]
    fn from_path_picks_the_entry_kind_by_extension() {
        assert!(matches!(BackgroundEntry::from_path("/a/b.jpg".into()), Some(BackgroundEntry::Image { .. })));
        assert!(matches!(BackgroundEntry::from_path("/a/b.mp4".into()), Some(BackgroundEntry::Video { .. })));
        assert_eq!(BackgroundEntry::from_path("/a/b.txt".into()), None);
    }

    #[test]
    fn set_single_replaces_the_list_and_keeps_the_timing() {
        let mut bg = Background { interval_secs: 42, fade_ms: 7, shuffle: true, ..Background::default() };
        bg.entries.push(BackgroundEntry::default_gradient());
        bg.set_single("/a/b.png".into(), Some(ImageFit::Tile));
        assert_eq!(bg.entries, vec![BackgroundEntry::Image { path: "/a/b.png".into(), fit: ImageFit::Tile }]);
        assert_eq!((bg.interval_secs, bg.fade_ms, bg.shuffle), (42, 7, true));

        bg.set_single("/a/c.mp4".into(), None);
        assert_eq!(bg.entries, vec![BackgroundEntry::Video { path: "/a/c.mp4".into(), fit: ImageFit::Fill }]);

        bg.set_single("/a/d.txt".into(), None);
        assert_eq!(bg.entries.len(), 1, "an unsupported file leaves the list alone");
    }

    #[test]
    fn effective_entries_never_comes_back_empty() {
        let bg = Background { entries: vec![], ..Background::default() };
        assert_eq!(bg.effective_entries(), vec![BackgroundEntry::default_solid()]);
    }

    #[test]
    fn usable_entries_need_their_file() {
        assert!(BackgroundEntry::default_solid().is_usable());
        assert!(BackgroundEntry::default_gradient().is_usable());
        let missing = BackgroundEntry::Image { path: "/no/such/file.png".into(), fit: ImageFit::Fill };
        assert!(!missing.is_usable());
        let here = BackgroundEntry::Image { path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"), fit: ImageFit::Fill };
        assert!(here.is_usable());
    }

    #[test]
    fn startup_apps_default_to_none_and_old_files_load() {
        assert!(DesktopSettings::default().startup_apps.is_empty());
        let d: DesktopSettings = serde_json::from_str("{\"icon_size\": 80}").unwrap();
        assert!(d.startup_apps.is_empty());
    }

    #[test]
    fn startup_apps_roundtrip_and_wait_defaults_to_zero() {
        let d = DesktopSettings {
            startup_apps: vec![StartupApp { command: "firefox".into(), wait_secs: 5 }],
            ..DesktopSettings::default()
        };
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<DesktopSettings>(&s).unwrap(), d);
        let app: StartupApp = serde_json::from_str("{\"command\": \"nm-applet\"}").unwrap();
        assert_eq!(app, StartupApp { command: "nm-applet".into(), wait_secs: 0 });
    }

    #[test]
    fn sanitized_clamps_the_startup_wait_and_drops_blank_commands() {
        let d = DesktopSettings {
            startup_apps: vec![
                StartupApp { command: "  firefox  ".into(), wait_secs: 99_999 },
                StartupApp { command: "   ".into(), wait_secs: 3 },
                StartupApp { command: String::new(), wait_secs: 0 },
                StartupApp { command: "xterm".into(), wait_secs: 2 },
            ],
            ..DesktopSettings::default()
        };
        let d = d.sanitized();
        assert_eq!(
            d.startup_apps,
            vec![
                StartupApp { command: "firefox".into(), wait_secs: STARTUP_WAIT_SECS_RANGE.1 },
                StartupApp { command: "xterm".into(), wait_secs: 2 },
            ]
        );
    }
}
