//! Named colour presets.
//!
//! A preset is nothing more than a canned [`ColorSettings`]: picking one fills
//! every colour at once, and the user is free to keep editing individual
//! colours afterwards (which switches the stored preset id to `custom`).
//! Presets live in the shared crate because the hub's settings dialog offers
//! them and the Lister records which one is active.

use crate::proto::ColorSettings;
use serde::{Deserialize, Serialize};

/// One selectable colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreset {
    /// The original light scheme: white rows, green SOURCE, pink DEST.
    Default,
    /// Amiga Workbench 1.3: deep blue ground, white and amber text.
    AmigaWorkbench,
    /// Directory Opus 4/5: grey panels with navy accents.
    DopusClassic,
    /// Tokyo Night flavoured neon on dark slate.
    Cyberpunk,
    /// Green phosphor CRT.
    PhosphorGreen,
    /// Amber phosphor CRT.
    PhosphorAmber,
}

impl ThemePreset {
    /// Every preset, in the order the settings dialog lists them.
    pub const ALL: [ThemePreset; 6] = [
        ThemePreset::Default,
        ThemePreset::AmigaWorkbench,
        ThemePreset::DopusClassic,
        ThemePreset::Cyberpunk,
        ThemePreset::PhosphorGreen,
        ThemePreset::PhosphorAmber,
    ];

    /// Stable identifier stored in settings.json.
    pub fn id(self) -> &'static str {
        match self {
            ThemePreset::Default => "default",
            ThemePreset::AmigaWorkbench => "amiga-workbench",
            ThemePreset::DopusClassic => "dopus-classic",
            ThemePreset::Cyberpunk => "cyberpunk",
            ThemePreset::PhosphorGreen => "phosphor-green",
            ThemePreset::PhosphorAmber => "phosphor-amber",
        }
    }

    /// Human-readable name for the settings dialog.
    pub fn label(self) -> &'static str {
        match self {
            ThemePreset::Default => "Default (light)",
            ThemePreset::AmigaWorkbench => "Amiga Workbench 1.3",
            ThemePreset::DopusClassic => "Directory Opus 4/5 Classic",
            ThemePreset::Cyberpunk => "Cyberpunk / Tokyo Night",
            ThemePreset::PhosphorGreen => "Monochrome Phosphor (green)",
            ThemePreset::PhosphorAmber => "Monochrome Phosphor (amber)",
        }
    }

    /// Look a preset up by its [`id`](Self::id). `None` for `custom` or an
    /// unknown value written by a newer version.
    pub fn from_id(id: &str) -> Option<ThemePreset> {
        ThemePreset::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The colours this preset stands for.
    pub fn colors(self) -> ColorSettings {
        match self {
            ThemePreset::Default => ColorSettings::default(),
            ThemePreset::AmigaWorkbench => ColorSettings {
                source_bg: "#FF8800".into(),
                source_fg: "#000000".into(),
                dest_bg: "#FFFFFF".into(),
                dest_fg: "#0055AA".into(),
                dir_bg: "#0055AA".into(),
                dir_fg: "#FFFFFF".into(),
                file_bg: "#0055AA".into(),
                file_fg: "#FF8800".into(),
                selected_bg: "#FF8800".into(),
                selected_fg: "#000000".into(),
            },
            ThemePreset::DopusClassic => ColorSettings {
                source_bg: "#00A0C0".into(),
                source_fg: "#000000".into(),
                dest_bg: "#D8D8D8".into(),
                dest_fg: "#000080".into(),
                dir_bg: "#B4B4B4".into(),
                dir_fg: "#000080".into(),
                file_bg: "#B4B4B4".into(),
                file_fg: "#000000".into(),
                selected_bg: "#000080".into(),
                selected_fg: "#FFFFFF".into(),
            },
            ThemePreset::Cyberpunk => ColorSettings {
                source_bg: "#7DCFFF".into(),
                source_fg: "#1A1B26".into(),
                dest_bg: "#F7768E".into(),
                dest_fg: "#1A1B26".into(),
                dir_bg: "#1A1B26".into(),
                dir_fg: "#7DCFFF".into(),
                file_bg: "#1A1B26".into(),
                file_fg: "#C0CAF5".into(),
                selected_bg: "#BB9AF7".into(),
                selected_fg: "#1A1B26".into(),
            },
            ThemePreset::PhosphorGreen => ColorSettings {
                source_bg: "#33FF33".into(),
                source_fg: "#0A1A0A".into(),
                dest_bg: "#1F6B1F".into(),
                dest_fg: "#CCFFCC".into(),
                dir_bg: "#0A1A0A".into(),
                dir_fg: "#7CFF7C".into(),
                file_bg: "#0A1A0A".into(),
                file_fg: "#33FF33".into(),
                selected_bg: "#33FF33".into(),
                selected_fg: "#0A1A0A".into(),
            },
            ThemePreset::PhosphorAmber => ColorSettings {
                source_bg: "#FFB000".into(),
                source_fg: "#1A1000".into(),
                dest_bg: "#7A5400".into(),
                dest_fg: "#FFE0A0".into(),
                dir_bg: "#1A1000".into(),
                dir_fg: "#FFD070".into(),
                file_bg: "#1A1000".into(),
                file_fg: "#FFB000".into(),
                selected_bg: "#FFB000".into(),
                selected_fg: "#1A1000".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        let mut ids: Vec<&str> = ThemePreset::ALL.iter().map(|p| p.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate preset id");
        for p in ThemePreset::ALL {
            assert_eq!(ThemePreset::from_id(p.id()), Some(p));
        }
        assert_eq!(ThemePreset::from_id("custom"), None);
    }

    #[test]
    fn every_preset_defines_every_colour_as_hex() {
        for p in ThemePreset::ALL {
            let c = p.colors();
            let fields = [
                c.source_bg, c.source_fg, c.dest_bg, c.dest_fg,
                c.dir_bg, c.dir_fg, c.file_bg, c.file_fg,
                c.selected_bg, c.selected_fg,
            ];
            for hex in fields {
                assert!(
                    hex.len() == 7
                        && hex.starts_with('#')
                        && hex[1..].chars().all(|c| c.is_ascii_hexdigit()),
                    "{}: bad colour {hex:?}",
                    p.id()
                );
            }
        }
    }

    #[test]
    fn default_preset_matches_the_default_colours() {
        assert_eq!(ThemePreset::Default.colors(), ColorSettings::default());
    }

    #[test]
    fn labels_are_distinct() {
        let mut labels: Vec<&str> = ThemePreset::ALL.iter().map(|p| p.label()).collect();
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count);
    }
}
