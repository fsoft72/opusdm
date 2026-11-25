//! Window manager settings taken from `desktop.json` (owned by
//! `opus-desktop-config`, read only here): desktop wrap, snapping and the
//! window opacity.

use opusdm_ipc::desktop::{DesktopSettings, WINDOW_OPACITY_RANGE};

/// The part of `DesktopSettings` the window manager uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WmSettings {
    /// The next / previous desktop steps wrap around.
    pub wrap_desktops: bool,
    /// Dragging a window to a screen edge snaps it to a zone.
    pub snap_enabled: bool,
    /// Master switch of the window opacity: off draws every window opaque.
    pub transparency_enabled: bool,
    /// Opacity in percent of the active window.
    pub active_opacity: u8,
    /// Opacity in percent of the other windows.
    pub inactive_opacity: u8,
}

impl WmSettings {
    /// The window manager settings of `ds`; the opacities are clamped to
    /// `WINDOW_OPACITY_RANGE` (as `DesktopSettings::sanitized` does).
    pub fn from_desktop(ds: &DesktopSettings) -> WmSettings {
        WmSettings {
            wrap_desktops: ds.wrap_desktops,
            snap_enabled: ds.snap_enabled,
            transparency_enabled: ds.transparency_enabled,
            active_opacity: _opacity(ds.active_opacity),
            inactive_opacity: _opacity(ds.inactive_opacity),
        }
    }
}

impl Default for WmSettings {
    /// The settings of `DesktopSettings::default()`.
    fn default() -> WmSettings {
        WmSettings::from_desktop(&DesktopSettings::default())
    }
}

/// `percent` clamped to `WINDOW_OPACITY_RANGE`, as a `u8`.
fn _opacity(percent: u32) -> u8 {
    let (lo, hi) = WINDOW_OPACITY_RANGE;
    u8::try_from(percent.clamp(lo, hi)).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_desktop_settings_give_the_documented_values() {
        let wm = WmSettings::from_desktop(&DesktopSettings::default());
        let expected = WmSettings { wrap_desktops: true, snap_enabled: true, transparency_enabled: true, active_opacity: 100, inactive_opacity: 88 };
        assert_eq!(wm, expected);
    }

    #[test]
    fn the_fields_are_copied_and_the_opacity_is_clamped() {
        let ds = DesktopSettings {
            wrap_desktops: false,
            snap_enabled: false,
            transparency_enabled: false,
            active_opacity: 900,
            inactive_opacity: 3,
            ..DesktopSettings::default()
        };
        let expected = WmSettings { wrap_desktops: false, snap_enabled: false, transparency_enabled: false, active_opacity: 100, inactive_opacity: 20 };
        assert_eq!(WmSettings::from_desktop(&ds), expected);
    }
}
