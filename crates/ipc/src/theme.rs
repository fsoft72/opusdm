//! Icon set choice and the `Theme` bundle: icon set + colours + desktop
//! background, edited together by `opusdm-theme` and applied to the running
//! desktop in one atomic `ToHub::SetTheme`.

use crate::desktop::Background;
use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// Which bundled icon library the Lister and desktop icon grid use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSetId {
    /// No icon images: Nerd Font glyphs when a patched font is available, a
    /// plain name otherwise. This is the only icon "set" that does not need
    /// bundled assets.
    None,
    /// Breeze Dark (KDE), bundled subset covering `IconKind` plus the file
    /// categories `opusdm_iconview::glyphs` already classifies by extension.
    BreezeDark,
}

impl IconSetId {
    /// Every icon set, in the order `opusdm-theme` lists them.
    pub const ALL: [IconSetId; 2] = [IconSetId::None, IconSetId::BreezeDark];

    /// Stable identifier stored in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            IconSetId::None => "none",
            IconSetId::BreezeDark => "breeze-dark",
        }
    }

    /// Human-readable name for `opusdm-theme`'s picker.
    pub fn label(self) -> &'static str {
        match self {
            IconSetId::None => "None (text glyphs)",
            IconSetId::BreezeDark => "Breeze Dark",
        }
    }

    /// Look an icon set up by its [`id`](Self::id). `None` for an unknown
    /// value written by a newer version.
    pub fn from_id(id: &str) -> Option<IconSetId> {
        IconSetId::ALL.into_iter().find(|s| s.id() == id)
    }
}

impl Default for IconSetId {
    fn default() -> Self {
        IconSetId::None
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
        let mut ids: Vec<&str> = IconSetId::ALL.iter().map(|s| s.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate icon set id");
        for s in IconSetId::ALL {
            assert_eq!(IconSetId::from_id(s.id()), Some(s));
        }
        assert_eq!(IconSetId::from_id("nonsense"), None);
    }

    #[test]
    fn icon_set_labels_are_distinct() {
        let mut labels: Vec<&str> = IconSetId::ALL.iter().map(|s| s.label()).collect();
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count);
    }

    #[test]
    fn theme_default_has_no_icon_set_and_default_colors() {
        let t = Theme::default();
        assert_eq!(t.icon_set, IconSetId::None);
        assert_eq!(t.colors, crate::proto::ColorSettings::default());
        assert_eq!(t.background, crate::desktop::Background::default());
    }
}
