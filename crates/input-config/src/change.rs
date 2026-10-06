//! One field of the settings that changed, and the diff between two settings.

use crate::{InputSettings, LayoutEntry};

/// Speeds closer than this are the same value.
const SPEED_EPSILON: f64 = 1e-9;

/// One thing to apply to the system.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Mouse pointer speed.
    MouseSpeed(f64),
    /// Mouse flat acceleration profile on or off.
    MouseFlatProfile(bool),
    /// Mouse buttons swapped.
    MouseLeftHanded(bool),
    /// Mouse natural scrolling.
    MouseNaturalScroll(bool),
    /// Touchpad pointer speed.
    TouchpadSpeed(f64),
    /// Touchpad tap to click.
    TouchpadTap(bool),
    /// Touchpad natural scrolling.
    TouchpadNaturalScroll(bool),
    /// Touchpad disabled while typing.
    TouchpadDisableWhileTyping(bool),
    /// Keyboard layouts (the layout list, the switch option and the other options go together).
    Layout { layouts: Vec<LayoutEntry>, switch_option: String, extra_options: Vec<String> },
    /// Key repeat.
    Repeat { delay_ms: u32, rate: u32 },
}

/// Whether two speeds are the same value.
fn same_speed(a: f64, b: f64) -> bool {
    (a - b).abs() < SPEED_EPSILON
}

/// The changes that turn `old` into `new`, in a fixed order.
pub fn diff(old: &InputSettings, new: &InputSettings) -> Vec<Change> {
    let mut changes = Vec::new();
    if !same_speed(old.mouse.speed, new.mouse.speed) { changes.push(Change::MouseSpeed(new.mouse.speed)); }
    if old.mouse.flat_profile != new.mouse.flat_profile { changes.push(Change::MouseFlatProfile(new.mouse.flat_profile)); }
    if old.mouse.left_handed != new.mouse.left_handed { changes.push(Change::MouseLeftHanded(new.mouse.left_handed)); }
    if old.mouse.natural_scroll != new.mouse.natural_scroll { changes.push(Change::MouseNaturalScroll(new.mouse.natural_scroll)); }
    if !same_speed(old.touchpad.speed, new.touchpad.speed) { changes.push(Change::TouchpadSpeed(new.touchpad.speed)); }
    if old.touchpad.tap_to_click != new.touchpad.tap_to_click { changes.push(Change::TouchpadTap(new.touchpad.tap_to_click)); }
    if old.touchpad.natural_scroll != new.touchpad.natural_scroll { changes.push(Change::TouchpadNaturalScroll(new.touchpad.natural_scroll)); }
    if old.touchpad.disable_while_typing != new.touchpad.disable_while_typing {
        changes.push(Change::TouchpadDisableWhileTyping(new.touchpad.disable_while_typing));
    }
    let (o, n) = (&old.keyboard, &new.keyboard);
    if o.layouts != n.layouts || o.switch_option != n.switch_option || o.extra_options != n.extra_options {
        changes.push(Change::Layout { layouts: n.layouts.clone(), switch_option: n.switch_option.clone(), extra_options: n.extra_options.clone() });
    }
    if o.repeat_delay_ms != n.repeat_delay_ms || o.repeat_rate != n.repeat_rate {
        changes.push(Change::Repeat { delay_ms: n.repeat_delay_ms, rate: n.repeat_rate });
    }
    changes
}

/// Every setting as a change, for applying a saved file to the whole system.
pub fn all_changes(settings: &InputSettings) -> Vec<Change> {
    let k = &settings.keyboard;
    vec![
        Change::MouseSpeed(settings.mouse.speed),
        Change::MouseFlatProfile(settings.mouse.flat_profile),
        Change::MouseLeftHanded(settings.mouse.left_handed),
        Change::MouseNaturalScroll(settings.mouse.natural_scroll),
        Change::TouchpadSpeed(settings.touchpad.speed),
        Change::TouchpadTap(settings.touchpad.tap_to_click),
        Change::TouchpadNaturalScroll(settings.touchpad.natural_scroll),
        Change::TouchpadDisableWhileTyping(settings.touchpad.disable_while_typing),
        Change::Layout { layouts: k.layouts.clone(), switch_option: k.switch_option.clone(), extra_options: k.extra_options.clone() },
        Change::Repeat { delay_ms: k.repeat_delay_ms, rate: k.repeat_rate },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_settings_have_no_changes() {
        let s = InputSettings::default();
        assert!(diff(&s, &s.clone()).is_empty());
    }

    #[test]
    fn only_the_changed_fields_appear() {
        let old = InputSettings::default();
        let mut new = old.clone();
        new.mouse.speed = 0.5;
        new.touchpad.tap_to_click = true;
        new.keyboard.repeat_rate = 40;
        assert_eq!(
            diff(&old, &new),
            vec![Change::MouseSpeed(0.5), Change::TouchpadTap(true), Change::Repeat { delay_ms: 500, rate: 40 }]
        );
    }

    #[test]
    fn a_layout_change_carries_the_whole_layout_state() {
        let old = InputSettings::default();
        let mut new = old.clone();
        new.keyboard.switch_option = "grp:alt_shift_toggle".into();
        new.keyboard.extra_options = vec!["caps:escape".into()];
        let changes = diff(&old, &new);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::Layout { switch_option, extra_options, .. } => {
                assert_eq!(switch_option, "grp:alt_shift_toggle");
                assert_eq!(extra_options, &vec!["caps:escape".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn all_changes_covers_every_field() {
        assert_eq!(all_changes(&InputSettings::default()).len(), 10);
    }
}
