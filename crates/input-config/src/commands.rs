//! Turns a `Change` into the command lines that apply it. Pure: nothing is
//! started here, so the generation is unit tested.

use crate::change::Change;
use crate::parse::{PointerKind, Props};
use crate::LayoutEntry;

/// A command line: program first, then its arguments (no shell involved).
pub type Argv = Vec<String>;

/// A pointer device and the libinput properties it exposes.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerDevice {
    /// The X input device id.
    pub id: u32,
    /// The device name.
    pub name: String,
    /// Mouse or touchpad.
    pub kind: PointerKind,
    /// Its properties.
    pub props: Props,
}

/// `xinput set-prop` for every device of `kind` that exposes `property`.
fn set_prop(devices: &[PointerDevice], kind: PointerKind, property: &str, values: &[&str]) -> Vec<Argv> {
    devices
        .iter()
        .filter(|d| d.kind == kind && d.props.contains_key(property))
        .map(|d| {
            let mut argv: Argv = vec!["xinput".into(), "set-prop".into(), d.id.to_string(), property.into()];
            argv.extend(values.iter().map(|v| v.to_string()));
            argv
        })
        .collect()
}

/// `xinput set-prop` for the acceleration profile of every mouse. The property
/// lists one value per profile (adaptive, flat and, with newer drivers, custom):
/// exactly one is 1, and the count is the one the device reports.
fn profile_commands(devices: &[PointerDevice], flat: bool) -> Vec<Argv> {
    const PROPERTY: &str = "libinput Accel Profile Enabled";
    const MIN_PROFILES: usize = 2;
    let chosen = usize::from(flat);
    devices
        .iter()
        .filter(|d| d.kind == PointerKind::Mouse)
        .filter_map(|d| {
            let count = d.props.get(PROPERTY)?.len().max(MIN_PROFILES);
            let mut argv: Argv = vec!["xinput".into(), "set-prop".into(), d.id.to_string(), PROPERTY.into()];
            argv.extend((0..count).map(|i| bit(i == chosen).to_string()));
            Some(argv)
        })
        .collect()
}

/// `"1"` or `"0"`.
fn bit(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

/// The `setxkbmap` call that sets layouts, variants and options at once.
/// `-option ""` clears the old options first; the extra options are put back.
fn layout_argv(layouts: &[LayoutEntry], switch_option: &str, extra_options: &[String]) -> Argv {
    let codes: Vec<&str> = layouts.iter().map(|l| l.layout.as_str()).collect();
    let variants: Vec<&str> = layouts.iter().map(|l| l.variant.as_str()).collect();
    let mut argv: Argv = vec!["setxkbmap".into(), "-layout".into(), codes.join(","), "-variant".into(), variants.join(","), "-option".into(), String::new()];
    let mut options: Vec<&str> = extra_options.iter().map(String::as_str).collect();
    if !switch_option.is_empty() { options.push(switch_option); }
    if !options.is_empty() {
        argv.push("-option".into());
        argv.push(options.join(","));
    }
    argv
}

/// The commands that apply `change` to `devices` (and to the keyboard).
pub fn commands_for(change: &Change, devices: &[PointerDevice]) -> Vec<Argv> {
    use PointerKind::{Mouse, Touchpad};
    match change {
        Change::MouseSpeed(v) => set_prop(devices, Mouse, "libinput Accel Speed", &[&format!("{v:.2}")]),
        Change::MouseFlatProfile(flat) => profile_commands(devices, *flat),
        Change::MouseLeftHanded(on) => set_prop(devices, Mouse, "libinput Left Handed Enabled", &[bit(*on)]),
        Change::MouseNaturalScroll(on) => set_prop(devices, Mouse, "libinput Natural Scrolling Enabled", &[bit(*on)]),
        Change::TouchpadSpeed(v) => set_prop(devices, Touchpad, "libinput Accel Speed", &[&format!("{v:.2}")]),
        Change::TouchpadTap(on) => set_prop(devices, Touchpad, "libinput Tapping Enabled", &[bit(*on)]),
        Change::TouchpadNaturalScroll(on) => set_prop(devices, Touchpad, "libinput Natural Scrolling Enabled", &[bit(*on)]),
        Change::TouchpadDisableWhileTyping(on) => set_prop(devices, Touchpad, "libinput Disable While Typing Enabled", &[bit(*on)]),
        Change::Layout { layouts, switch_option, extra_options } => vec![layout_argv(layouts, switch_option, extra_options)],
        Change::Repeat { delay_ms, rate } => vec![vec!["xset".into(), "r".into(), "rate".into(), delay_ms.to_string(), rate.to_string()]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_props;

    fn device(id: u32, kind: PointerKind, props_text: &str) -> PointerDevice {
        PointerDevice { id, name: format!("dev{id}"), kind, props: parse_props(props_text) }
    }

    const MOUSE: &str = "Device 'm':\n\tlibinput Accel Speed (1):\t0.0\n\tlibinput Accel Profile Enabled (2):\t1, 0, 0\n\tlibinput Left Handed Enabled (3):\t0\n\tlibinput Natural Scrolling Enabled (4):\t0\n";
    const PAD: &str = "Device 'p':\n\tlibinput Accel Speed (1):\t0.0\n\tlibinput Tapping Enabled (5):\t0\n\tlibinput Natural Scrolling Enabled (4):\t0\n\tlibinput Disable While Typing Enabled (6):\t1\n";

    fn argv(items: &[&str]) -> Argv {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mouse_speed_goes_to_every_mouse_and_no_touchpad() {
        let devices = [device(9, PointerKind::Mouse, MOUSE), device(10, PointerKind::Mouse, MOUSE), device(11, PointerKind::Touchpad, PAD)];
        let cmds = commands_for(&Change::MouseSpeed(0.5), &devices);
        assert_eq!(cmds, vec![argv(&["xinput", "set-prop", "9", "libinput Accel Speed", "0.50"]), argv(&["xinput", "set-prop", "10", "libinput Accel Speed", "0.50"])]);
    }

    #[test]
    fn flat_profile_sets_all_three_profile_values() {
        let devices = [device(9, PointerKind::Mouse, MOUSE)];
        assert_eq!(commands_for(&Change::MouseFlatProfile(true), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "0", "1", "0"])]);
        assert_eq!(commands_for(&Change::MouseFlatProfile(false), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "1", "0", "0"])]);
    }

    #[test]
    fn touchpad_options_use_their_properties() {
        let devices = [device(11, PointerKind::Touchpad, PAD)];
        assert_eq!(commands_for(&Change::TouchpadTap(true), &devices), vec![argv(&["xinput", "set-prop", "11", "libinput Tapping Enabled", "1"])]);
        assert_eq!(commands_for(&Change::TouchpadDisableWhileTyping(false), &devices), vec![argv(&["xinput", "set-prop", "11", "libinput Disable While Typing Enabled", "0"])]);
    }

    #[test]
    fn devices_without_the_property_are_skipped() {
        let bare = PointerDevice { id: 12, name: "bare".into(), kind: PointerKind::Mouse, props: Default::default() };
        assert!(commands_for(&Change::MouseLeftHanded(true), &[bare]).is_empty());
        assert!(commands_for(&Change::MouseSpeed(0.1), &[]).is_empty());
    }

    #[test]
    fn layout_command_sets_layouts_variants_and_options() {
        let change = Change::Layout {
            layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }, LayoutEntry { layout: "us".into(), variant: "intl".into() }],
            switch_option: "grp:alt_shift_toggle".into(),
            extra_options: vec![],
        };
        assert_eq!(
            commands_for(&change, &[]),
            vec![argv(&["setxkbmap", "-layout", "it,us", "-variant", ",intl", "-option", "", "-option", "grp:alt_shift_toggle"])]
        );
    }

    #[test]
    fn layout_command_keeps_extra_options() {
        let change = Change::Layout {
            layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }],
            switch_option: String::new(),
            extra_options: vec!["caps:escape".into()],
        };
        assert_eq!(commands_for(&change, &[]), vec![argv(&["setxkbmap", "-layout", "it", "-variant", "", "-option", "", "-option", "caps:escape"])]);
    }

    #[test]
    fn layout_command_with_no_options_only_clears_them() {
        let change = Change::Layout { layouts: vec![LayoutEntry { layout: "it".into(), variant: String::new() }], switch_option: String::new(), extra_options: vec![] };
        assert_eq!(commands_for(&change, &[]), vec![argv(&["setxkbmap", "-layout", "it", "-variant", "", "-option", ""])]);
    }

    #[test]
    fn repeat_uses_xset() {
        assert_eq!(commands_for(&Change::Repeat { delay_ms: 300, rate: 40 }, &[]), vec![argv(&["xset", "r", "rate", "300", "40"])]);
    }

    #[test]
    fn flat_profile_follows_the_size_of_the_device_property() {
        const OLD_DRIVER: &str = "Device 'm':\n\tlibinput Accel Speed (1):\t0.0\n\tlibinput Accel Profile Enabled (2):\t1, 0\n";
        let devices = [device(9, PointerKind::Mouse, OLD_DRIVER)];
        assert_eq!(commands_for(&Change::MouseFlatProfile(true), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "0", "1"])]);
        assert_eq!(commands_for(&Change::MouseFlatProfile(false), &devices), vec![argv(&["xinput", "set-prop", "9", "libinput Accel Profile Enabled", "1", "0"])]);
    }
}
