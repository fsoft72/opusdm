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
