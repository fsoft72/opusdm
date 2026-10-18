//! Font and background-picture settings shared by `Settings` and a theme's
//! `theme.json`. Pure data, no GTK.

use crate::desktop::ImageFit;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One font choice. An empty `family` or a `size` of 0 means "keep the
/// system default" for that part.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSpec {
    pub family: String,
    /// Size in points.
    pub size: u32,
}

/// The fonts a theme can set: the Lister file list and the rest of the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fonts {
    pub list: FontSpec,
    pub ui: FontSpec,
}

/// A background picture for one kind of surface, as stored in `Settings`
/// (absolute path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceImage {
    pub image_path: Option<PathBuf>,
    pub fit: ImageFit,
}

/// Background pictures for the Lister file list and for requesters / dialogs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Surfaces {
    pub lister: SurfaceImage,
    pub requester: SurfaceImage,
}

/// One surface as written in `theme.json`: `image` is a file name inside the
/// theme's `backgrounds/` folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSurface {
    pub image: Option<String>,
    pub fit: Option<ImageFit>,
}

/// `theme.json`'s `surfaces` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSurfaces {
    pub lister: Option<ThemeSurface>,
    pub requester: Option<ThemeSurface>,
}

/// How the hub bar (the panel) is painted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HubBarKind {
    #[default]
    Solid,
    Gradient,
    Image,
}

/// The paint of the hub bar. Only the fields of `kind` are used; the others
/// keep their values so switching kind in the Theme Editor loses nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubBar {
    pub kind: HubBarKind,
    /// Flat colour, hex `#rrggbb`.
    pub color: String,
    /// Gradient colours, hex `#rrggbb`.
    pub start: String,
    pub end: String,
    /// Gradient angle, `0..=GRADIENT_ANGLE_MAX` (0 points up, 90 right).
    pub angle: u16,
    /// Picture used by [`HubBarKind::Image`] (absolute path).
    pub image: SurfaceImage,
}

impl Default for HubBar {
    /// The look the panel has without a theme: the default dark grey.
    fn default() -> Self {
        HubBar {
            kind: HubBarKind::Solid,
            color: crate::desktop::DEFAULT_SOLID.to_string(),
            start: crate::desktop::DEFAULT_SOLID.to_string(),
            end: "#555753".to_string(),
            angle: 180,
            image: SurfaceImage::default(),
        }
    }
}

/// What a theme sets about the hub (the panel). `None` means the theme does
/// not set that part and the built-in look is used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubLook {
    pub bar: Option<HubBar>,
    /// Picture of the start menu button (absolute path, png or svg).
    pub menu_icon: Option<PathBuf>,
    /// Font of the start menu once opened.
    pub menu_font: Option<FontSpec>,
}

/// [`HubBar`] as written in `theme.json`: `image` is a file name inside the
/// theme's `backgrounds/` folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeHubBar {
    pub kind: HubBarKind,
    pub color: String,
    pub start: String,
    pub end: String,
    pub angle: u16,
    pub image: Option<String>,
    pub fit: Option<ImageFit>,
}

impl Default for ThemeHubBar {
    fn default() -> Self {
        let bar = HubBar::default();
        ThemeHubBar { kind: bar.kind, color: bar.color, start: bar.start, end: bar.end, angle: bar.angle, image: None, fit: None }
    }
}

/// `theme.json`'s `hub` section: `menu_icon` is a file name inside the
/// theme's `backgrounds/` folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeHub {
    pub bar: Option<ThemeHubBar>,
    pub menu_icon: Option<String>,
    pub menu_font: Option<FontSpec>,
}

/// Whether `s` is a CSS hex colour: `#rgb`, `#rrggbb` or `#rrggbbaa`. Colours
/// of a theme go into a stylesheet, so nothing else is let through.
pub fn is_hex_color(s: &str) -> bool {
    let Some(digits) = s.strip_prefix('#') else { return false };
    matches!(digits.len(), 3 | 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

impl HubBar {
    /// The bar with every colour that is not a hex colour replaced by its
    /// default and the angle brought into range.
    pub fn sanitized(mut self) -> HubBar {
        let default = HubBar::default();
        if !is_hex_color(&self.color) {
            self.color = default.color;
        }
        if !is_hex_color(&self.start) {
            self.start = default.start;
        }
        if !is_hex_color(&self.end) {
            self.end = default.end;
        }
        self.angle = self.angle.min(crate::desktop::GRADIENT_ANGLE_MAX);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_defaults_to_empty() {
        let f: Fonts = serde_json::from_str("{}").unwrap();
        assert_eq!(f, Fonts::default());
        let s: Surfaces = serde_json::from_str("{}").unwrap();
        assert_eq!(s.lister.image_path, None);
        assert_eq!(s.requester.fit, ImageFit::default());
    }

    #[test]
    fn hub_look_defaults_to_nothing_set() {
        let h: HubLook = serde_json::from_str("{}").unwrap();
        assert_eq!(h, HubLook::default());
        assert!(h.bar.is_none() && h.menu_icon.is_none() && h.menu_font.is_none());
    }

    #[test]
    fn theme_hub_reads_the_documented_shape() {
        let h: ThemeHub = serde_json::from_str(
            r##"{"bar":{"kind":"gradient","start":"#000000","end":"#ffffff","angle":90},
                "menu_icon":"hub-menu-icon.svg","menu_font":{"family":"Cantarell","size":13}}"##,
        )
        .unwrap();
        let bar = h.bar.unwrap();
        assert_eq!((bar.kind, bar.angle), (HubBarKind::Gradient, 90));
        assert_eq!(bar.color, HubBar::default().color);
        assert_eq!(h.menu_icon.as_deref(), Some("hub-menu-icon.svg"));
        assert_eq!(h.menu_font.unwrap().size, 13);
    }

    #[test]
    fn hex_colours_are_checked() {
        for ok in ["#fff", "#2e3436", "#2e3436ff"] {
            assert!(is_hex_color(ok), "{ok}");
        }
        for bad in ["", "red", "#12", "#12345", "#gggggg", "#fff; x: y", "2e3436"] {
            assert!(!is_hex_color(bad), "{bad}");
        }
    }

    #[test]
    fn a_bar_with_bad_values_is_sanitized() {
        let bar = HubBar { color: "red".into(), start: "#fff}".into(), angle: 900, ..HubBar::default() }.sanitized();
        assert_eq!(bar, HubBar { angle: crate::desktop::GRADIENT_ANGLE_MAX, ..HubBar::default() });
    }

    #[test]
    fn fonts_round_trip_through_json() {
        let f = Fonts {
            list: FontSpec { family: "Fira Code".into(), size: 11 },
            ui: FontSpec { family: String::new(), size: 10 },
        };
        let back: Fonts = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn theme_surfaces_read_the_documented_shape() {
        let s: ThemeSurfaces =
            serde_json::from_str(r#"{"lister":{"image":"lister.png","fit":"tile"}}"#).unwrap();
        assert_eq!(s.lister.as_ref().unwrap().image.as_deref(), Some("lister.png"));
        assert_eq!(s.lister.unwrap().fit, Some(ImageFit::Tile));
        assert!(s.requester.is_none());
    }
}
