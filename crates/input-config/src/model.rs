//! The input settings model, stored in `input.json`.

use serde::{Deserialize, Serialize};

/// Lowest pointer speed (libinput `Accel Speed`).
pub const SPEED_MIN: f64 = -1.0;
/// Highest pointer speed.
pub const SPEED_MAX: f64 = 1.0;
/// Shortest delay before a held key repeats, in milliseconds.
pub const REPEAT_DELAY_MIN_MS: u32 = 100;
/// Longest delay before a held key repeats, in milliseconds.
pub const REPEAT_DELAY_MAX_MS: u32 = 2000;
/// Slowest key repeat rate, per second.
pub const REPEAT_RATE_MIN: u32 = 1;
/// Fastest key repeat rate, per second.
pub const REPEAT_RATE_MAX: u32 = 100;
/// Most keyboard layouts that can be active together.
pub const MAX_LAYOUTS: usize = 4;

/// Options shared by every mouse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    /// Pointer speed, -1.0 to 1.0.
    pub speed: f64,
    /// Flat acceleration profile (no acceleration curve) instead of adaptive.
    pub flat_profile: bool,
    /// Swap the left and right buttons.
    pub left_handed: bool,
    /// Natural (reversed) wheel scrolling.
    pub natural_scroll: bool,
}

impl Default for MouseSettings {
    /// The libinput defaults.
    fn default() -> Self {
        Self { speed: 0.0, flat_profile: false, left_handed: false, natural_scroll: false }
    }
}

/// Options shared by every touchpad.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TouchpadSettings {
    /// Pointer speed, -1.0 to 1.0.
    pub speed: f64,
    /// A tap is a click.
    pub tap_to_click: bool,
    /// Natural (reversed) two finger scrolling.
    pub natural_scroll: bool,
    /// Ignore the touchpad while typing.
    pub disable_while_typing: bool,
}

impl Default for TouchpadSettings {
    /// The libinput defaults (disable while typing is on).
    fn default() -> Self {
        Self { speed: 0.0, tap_to_click: false, natural_scroll: false, disable_while_typing: true }
    }
}

/// One active keyboard layout and its variant (empty = the default variant).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutEntry {
    /// XKB layout code, for example `it`.
    pub layout: String,
    /// XKB variant code, for example `intl`, or empty.
    pub variant: String,
}

/// Keyboard layouts and key repeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    /// Active layouts, in switching order (1 to `MAX_LAYOUTS`).
    pub layouts: Vec<LayoutEntry>,
    /// The XKB option that switches layout (`grp:alt_shift_toggle`), or empty.
    pub switch_option: String,
    /// Other XKB options already set (`caps:escape`). Kept, never edited.
    pub extra_options: Vec<String>,
    /// Delay before a held key repeats, in milliseconds.
    pub repeat_delay_ms: u32,
    /// Repeats per second.
    pub repeat_rate: u32,
}

impl Default for KeyboardSettings {
    /// US layout with the usual X repeat values.
    fn default() -> Self {
        Self {
            layouts: vec![LayoutEntry { layout: "us".to_string(), variant: String::new() }],
            switch_option: String::new(),
            extra_options: Vec::new(),
            repeat_delay_ms: 500,
            repeat_rate: 33,
        }
    }
}

/// Everything `opusdm-input-config` edits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettings {
    /// Options shared by every mouse.
    pub mouse: MouseSettings,
    /// Options shared by every touchpad.
    pub touchpad: TouchpadSettings,
    /// Layouts and key repeat.
    pub keyboard: KeyboardSettings,
}

/// Clamp a speed into range; anything that is not a number becomes 0.
fn clamp_speed(value: f64) -> f64 {
    if !value.is_finite() { return 0.0; }
    value.clamp(SPEED_MIN, SPEED_MAX)
}

impl InputSettings {
    /// A copy that is safe to apply: speeds and repeat values clamped, empty
    /// layout codes dropped, at most `MAX_LAYOUTS` layouts and at least one.
    pub fn normalized(mut self) -> Self {
        self.mouse.speed = clamp_speed(self.mouse.speed);
        self.touchpad.speed = clamp_speed(self.touchpad.speed);
        let kb = &mut self.keyboard;
        kb.layouts.retain(|l| !l.layout.trim().is_empty());
        kb.layouts.truncate(MAX_LAYOUTS);
        if kb.layouts.is_empty() {
            kb.layouts = KeyboardSettings::default().layouts;
        }
        kb.repeat_delay_ms = kb.repeat_delay_ms.clamp(REPEAT_DELAY_MIN_MS, REPEAT_DELAY_MAX_MS);
        kb.repeat_rate = kb.repeat_rate.clamp(REPEAT_RATE_MIN, REPEAT_RATE_MAX);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_a_json_round_trip() {
        let json = serde_json::to_string(&InputSettings::default()).unwrap();
        assert_eq!(serde_json::from_str::<InputSettings>(&json).unwrap(), InputSettings::default());
    }

    #[test]
    fn a_partial_old_file_fills_the_rest_with_defaults() {
        let s: InputSettings = serde_json::from_str(r#"{"mouse":{"speed":0.5},"unknown":1}"#).unwrap();
        assert_eq!(s.mouse.speed, 0.5);
        assert!(!s.mouse.left_handed);
        assert_eq!(s.keyboard, KeyboardSettings::default());
    }

    #[test]
    fn normalized_clamps_and_repairs() {
        let mut s = InputSettings::default();
        s.mouse.speed = 5.0;
        s.touchpad.speed = f64::NAN;
        s.keyboard.layouts = vec![LayoutEntry { layout: " ".into(), variant: "x".into() }];
        s.keyboard.repeat_delay_ms = 5;
        s.keyboard.repeat_rate = 1000;
        let n = s.normalized();
        assert_eq!(n.mouse.speed, SPEED_MAX);
        assert_eq!(n.touchpad.speed, 0.0);
        assert_eq!(n.keyboard.layouts, KeyboardSettings::default().layouts);
        assert_eq!(n.keyboard.repeat_delay_ms, REPEAT_DELAY_MIN_MS);
        assert_eq!(n.keyboard.repeat_rate, REPEAT_RATE_MAX);
    }

    #[test]
    fn normalized_keeps_at_most_max_layouts() {
        let mut s = InputSettings::default();
        s.keyboard.layouts = (0..9).map(|i| LayoutEntry { layout: format!("l{i}"), variant: String::new() }).collect();
        assert_eq!(s.normalized().keyboard.layouts.len(), MAX_LAYOUTS);
    }
}
