//! Load desktop settings, and colour hex/RGBA conversions shared by
//! `ColorRow`. Applying settings lives in `opusdm-theme`, the only window
//! that edits them now.

use gtk4 as gtk;
use opusdm_ipc::DesktopSettings;
use std::path::PathBuf;

/// Path to `~/.config/opusdm/desktop.json`. Same resolution as the hub.
pub fn config_path() -> PathBuf {
    opusdm_ipc::settings::desktop_settings_path()
}

/// Load current desktop settings, or defaults if the file is missing or
/// unparseable. Always clamped.
pub fn load() -> DesktopSettings {
    opusdm_ipc::settings::load_desktop_settings("opus-desktop-config")
}

/// Testable core of [`load`].
pub fn load_from(path: &std::path::Path) -> DesktopSettings {
    opusdm_ipc::settings::load_desktop_settings_from(path, "opus-desktop-config")
}

/// `gdk::RGBA` -> `#rrggbb` (alpha dropped).
pub fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String {
    let to_u8 = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", to_u8(c.red()), to_u8(c.green()), to_u8(c.blue()))
}

/// `#rrggbb` (or any `gdk::RGBA::parse`-able string) -> `gdk::RGBA`.
/// Unparseable input yields opaque black.
pub fn hex_to_rgba(hex: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(hex).unwrap_or_else(|_| gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips_through_rgba() {
        let c = hex_to_rgba("#3c8ad9");
        assert_eq!(rgba_to_hex(&c), "#3c8ad9");
    }

    #[test]
    fn bad_hex_is_opaque_black() {
        assert_eq!(rgba_to_hex(&hex_to_rgba("not-a-colour")), "#000000");
    }

    #[test]
    fn load_defaults_when_file_is_missing() {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        assert_eq!(load(), DesktopSettings::default());
    }

    #[test]
    fn load_defaults_when_file_is_malformed() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("desktop.json");
        std::fs::write(&path, b"bad json").unwrap();
        assert_eq!(load_from(&path), DesktopSettings::default());
    }
}
