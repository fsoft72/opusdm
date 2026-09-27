//! Icon set choice and the `Theme` bundle: icon set + colours + desktop
//! background, edited together by `opusdm-settings` and applied to the running
//! desktop as part of an atomic `ToHub::SetSettings`.

use crate::desktop::Background;
use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// Slug of the theme that is always installed: the fallback whenever no
/// other theme is chosen. Its files are embedded in the binary and written
/// to the themes directory when missing (see `themes::ensure_default_theme`).
pub const DEFAULT_THEME_SLUG: &str = "breeze-dark";

/// Which icon library the Lister and desktop icon grid use. No icon set
/// ships as glyphs or as image data used directly: every option is a theme
/// installed under `~/.config/opusdm/themes/`, and the default one is always
/// installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum IconSetId {
    /// An installed theme's icon set, identified by its `manifest.json`
    /// `name` (the theme directory's name under `~/.config/opusdm/themes/`).
    Custom(String),
}

impl Default for IconSetId {
    fn default() -> Self {
        IconSetId::Custom(DEFAULT_THEME_SLUG.to_string())
    }
}

/// Deserialization mirror of [`IconSetId`], carrying the legacy tags:
/// older `settings.json` files wrote `"None"` (text glyphs, now removed) or
/// `"BreezeDark"` (the icon set that used to be compiled into the binary).
/// Both map to the default theme rather than failing the whole `Settings`
/// parse.
#[derive(Deserialize)]
enum IconSetIdRepr {
    Custom(String),
    None,
    #[serde(rename = "BreezeDark")]
    LegacyBreezeDark,
}

impl<'de> Deserialize<'de> for IconSetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match IconSetIdRepr::deserialize(deserializer)? {
            IconSetIdRepr::Custom(slug) => IconSetId::Custom(slug),
            IconSetIdRepr::None | IconSetIdRepr::LegacyBreezeDark => IconSetId::default(),
        })
    }
}

impl IconSetId {
    /// Stable identifier stored in settings.json.
    pub fn id(&self) -> String {
        match self {
            IconSetId::Custom(slug) => format!("custom:{slug}"),
        }
    }

    /// Look an icon set up by its [`id`](Self::id). Accepts the `"custom:"`
    /// prefix for any slug and the legacy `"none"` (mapped to the default
    /// theme); `None` for anything else.
    pub fn from_id(id: &str) -> Option<IconSetId> {
        if id == "none" {
            return Some(IconSetId::default());
        }
        id.strip_prefix("custom:").map(|slug| IconSetId::Custom(slug.to_string()))
    }
}

/// A theme bundle: icon set, Lister colours, and desktop background. What
/// `opusdm-settings` edits, as part of `Settings` sent to the hub via
/// `ToHub::SetSettings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Theme {
    pub icon_set: IconSetId,
    pub colors: ColorSettings,
    pub background: Background,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_ids_round_trip_through_the_custom_prefix() {
        let custom = IconSetId::Custom("breeze-dark".to_string());
        assert_eq!(custom.id(), "custom:breeze-dark");
        assert_eq!(IconSetId::from_id("custom:breeze-dark"), Some(custom));
        assert_eq!(IconSetId::from_id("custom:"), Some(IconSetId::Custom(String::new())));
    }

    #[test]
    fn legacy_none_and_breeze_dark_tags_deserialize_to_the_default_theme() {
        for tag in ["\"BreezeDark\"", "\"None\""] {
            let v: IconSetId = serde_json::from_str(tag).unwrap();
            assert_eq!(v, IconSetId::default());
        }
    }

    #[test]
    fn theme_default_uses_the_default_theme_and_default_colors() {
        let t = Theme::default();
        assert_eq!(t.icon_set, IconSetId::default());
        assert_eq!(t.colors, crate::proto::ColorSettings::default());
        assert_eq!(t.background, crate::desktop::Background::default());
    }
}
