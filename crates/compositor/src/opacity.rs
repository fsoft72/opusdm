//! Window opacity, as picom does it on X11 (`crates/hub/src/compositor.rs`,
//! `picom.conf`): the active window and the other windows have their own
//! opacity from `desktop.json`, a per-window override wins, and the master
//! switch off draws every window opaque. Only toplevels: popups, layer
//! surfaces, the window menu and the switcher are always opaque (picom's
//! menu rule).

use crate::wm::settings::WmSettings;

/// Accepted range of a per-window opacity override, in percent, inclusive.
pub const OPACITY_OVERRIDE_RANGE: (u8, u8) = (20, 100);

/// Opacity in percent a toplevel is drawn with: `override_percent` when
/// set; else 100 when transparency is off; else the active or the
/// inactive opacity of `settings`.
pub fn window_opacity(settings: &WmSettings, active: bool, override_percent: Option<u8>) -> u8 {
    if let Some(percent) = override_percent { return percent; }
    if !settings.transparency_enabled { return 100; }
    if active { settings.active_opacity } else { settings.inactive_opacity }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_100_active_and_88_inactive() {
        let wm = WmSettings::default();
        assert_eq!(window_opacity(&wm, true, None), 100);
        assert_eq!(window_opacity(&wm, false, None), 88);
    }

    #[test]
    fn an_override_wins() {
        let wm = WmSettings::default();
        assert_eq!(window_opacity(&wm, true, Some(50)), 50);
        assert_eq!(window_opacity(&wm, false, Some(50)), 50);
        let off = WmSettings { transparency_enabled: false, ..WmSettings::default() };
        assert_eq!(window_opacity(&off, false, Some(50)), 50);
    }

    #[test]
    fn transparency_off_gives_100() {
        let off = WmSettings { transparency_enabled: false, active_opacity: 90, inactive_opacity: 40, ..WmSettings::default() };
        assert_eq!(window_opacity(&off, true, None), 100);
        assert_eq!(window_opacity(&off, false, None), 100);
    }

    #[test]
    fn the_configured_values_are_used() {
        let wm = WmSettings { active_opacity: 95, inactive_opacity: 70, ..WmSettings::default() };
        assert_eq!(window_opacity(&wm, true, None), 95);
        assert_eq!(window_opacity(&wm, false, None), 70);
    }
}
