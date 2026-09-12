//! Icon set choice and the `Theme` bundle: icon set + colours + desktop
//! background, edited together by `opusdm-theme` and applied to the running
//! desktop in one atomic `ToHub::SetTheme`.

use crate::desktop::Background;
use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// Which icon library the Lister and desktop icon grid use. No icon set
/// ships compiled into the binary: `None` falls back to glyphs, and every
/// other option is a user-imported theme under `~/.config/opusdm/themes/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub enum IconSetId {
    /// No icon images: Nerd Font glyphs when a patched font is available, a
    /// plain name otherwise.
    #[default]
    None,
    /// A user-imported theme's icon set, identified by its `manifest.json`
    /// `name` (the theme directory's name under `~/.config/opusdm/themes/`).
    Custom(String),
}

/// Deserialization mirror of [`IconSetId`], carrying one extra tag:
/// pre-migration `settings.json` files wrote the literal `"BreezeDark"` tag
/// for the icon set that used to be compiled into the binary. That icon set
/// is gone - the equivalent theme is now importable as `breeze-dark.zip`
/// (see `themes/breeze-dark/`) - so an old file's `"BreezeDark"` value maps
/// to [`IconSetId::None`] rather than failing the whole `Settings` parse.
#[derive(Deserialize)]
enum IconSetIdRepr {
    None,
    Custom(String),
    #[serde(rename = "BreezeDark")]
    LegacyBreezeDark,
}

impl<'de> Deserialize<'de> for IconSetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match IconSetIdRepr::deserialize(deserializer)? {
            IconSetIdRepr::None => IconSetId::None,
            IconSetIdRepr::Custom(slug) => IconSetId::Custom(slug),
            IconSetIdRepr::LegacyBreezeDark => IconSetId::None,
        })
    }
}

impl IconSetId {
    /// Every icon set that exists independent of what's installed on disk.
    /// `Custom` entries are never part of this list - `opusdm-theme` builds
    /// its picker from `ALL` plus a live directory scan.
    pub const ALL: [IconSetId; 1] = [IconSetId::None];

    /// Stable identifier stored in settings.json.
    pub fn id(&self) -> String {
        match self {
            IconSetId::None => "none".to_string(),
            IconSetId::Custom(slug) => format!("custom:{slug}"),
        }
    }

    /// Human-readable name for `opusdm-theme`'s picker. `Custom` variants
    /// are never shown via this method - the picker reads the theme's own
    /// `manifest.json` label instead.
    pub fn label(&self) -> String {
        match self {
            IconSetId::None => "None (text glyphs)".to_string(),
            IconSetId::Custom(slug) => slug.clone(),
        }
    }

    /// Look an icon set up by its [`id`](Self::id). Accepts the `"custom:"`
    /// prefix for any slug, `"none"` for [`IconSetId::None`], `None` for
    /// anything else.
    pub fn from_id(id: &str) -> Option<IconSetId> {
        if id == "none" {
            return Some(IconSetId::None);
        }
        id.strip_prefix("custom:").map(|slug| IconSetId::Custom(slug.to_string()))
    }
}

/// A theme bundle: icon set, Lister colours, and desktop background. What
/// `opusdm-theme` edits and applies to the hub as one unit via
/// `ToHub::SetTheme`.
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
    fn icon_set_ids_are_unique_and_round_trip() {
        let mut ids: Vec<String> = IconSetId::ALL.iter().map(|s| s.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate icon set id");
        for s in IconSetId::ALL {
            assert_eq!(IconSetId::from_id(&s.id()), Some(s));
        }
        assert_eq!(IconSetId::from_id("nonsense"), None);
    }

    #[test]
    fn custom_ids_round_trip_through_the_custom_prefix() {
        let custom = IconSetId::Custom("breeze-dark".to_string());
        assert_eq!(custom.id(), "custom:breeze-dark");
        assert_eq!(IconSetId::from_id("custom:breeze-dark"), Some(custom));
        assert_eq!(IconSetId::from_id("custom:"), Some(IconSetId::Custom(String::new())));
    }

    #[test]
    fn a_pre_migration_breeze_dark_tag_deserializes_to_none() {
        let v: IconSetId = serde_json::from_str("\"BreezeDark\"").unwrap();
        assert_eq!(v, IconSetId::None);
    }

    #[test]
    fn theme_default_has_no_icon_set_and_default_colors() {
        let t = Theme::default();
        assert_eq!(t.icon_set, IconSetId::None);
        assert_eq!(t.colors, crate::proto::ColorSettings::default());
        assert_eq!(t.background, crate::desktop::Background::default());
    }
}
