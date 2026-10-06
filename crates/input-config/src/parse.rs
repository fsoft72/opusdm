//! Parsers for the output of `xinput`, `setxkbmap` and `xset`, and for the
//! XKB `evdev.lst` rules file. Pure functions: no process is started here.

use std::collections::HashMap;

use crate::{KeyboardSettings, LayoutEntry, MouseSettings, TouchpadSettings};

/// Where the XKB rules list lives.
const EVDEV_LST_PATH: &str = "/usr/share/X11/xkb/rules/evdev.lst";
/// Prefix of the XKB options that switch layout.
const SWITCH_OPTION_PREFIX: &str = "grp:";

/// A device line of `xinput list`.
#[derive(Clone, Debug, PartialEq)]
pub struct RawDevice {
    /// The X input device id.
    pub id: u32,
    /// The device name.
    pub name: String,
}

/// The properties of one device: name to values (`"0, 1, 0"` is three values).
pub type Props = HashMap<String, Vec<String>>;

/// What kind of pointer a device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// A mouse (anything libinput that is not a touchpad).
    Mouse,
    /// A touchpad.
    Touchpad,
}

/// The slave pointers of `xinput list` output (the virtual core devices are left out).
pub fn parse_pointer_list(out: &str) -> Vec<RawDevice> {
    out.lines()
        .filter(|l| l.contains("slave  pointer"))
        .filter_map(|line| {
            let (left, _) = line.split_once('\t')?;
            let name = left.split('\u{21b3}').nth(1)?.trim().to_string();
            let id = line.split("id=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
            Some(RawDevice { id, name })
        })
        .filter(|d| !d.name.starts_with("Virtual core"))
        .collect()
}

/// The properties printed by `xinput list-props`.
pub fn parse_props(out: &str) -> Props {
    let mut props = Props::new();
    for line in out.lines().filter(|l| l.starts_with('\t')) {
        let Some((head, value)) = line.trim_start().split_once("):") else { continue };
        let Some((name, _id)) = head.rsplit_once(" (") else { continue };
        props.insert(name.to_string(), value.split(',').map(|v| v.trim().to_string()).collect());
    }
    props
}

/// Mouse or touchpad, or `None` for a device with no libinput pointer properties.
pub fn device_kind(props: &Props) -> Option<PointerKind> {
    if !props.contains_key("libinput Accel Speed") { return None; }
    if props.contains_key("libinput Tapping Enabled") { return Some(PointerKind::Touchpad); }
    Some(PointerKind::Mouse)
}

/// The first value of property `name` as a number.
fn first_f64(props: &Props, name: &str) -> Option<f64> {
    props.get(name)?.first()?.parse().ok()
}

/// Whether the first value of property `name` is `1`.
fn flag(props: &Props, name: &str) -> Option<bool> {
    Some(props.get(name)?.first()? == "1")
}

/// The mouse settings a mouse reports (defaults for what it does not expose).
pub fn mouse_from_props(props: &Props) -> MouseSettings {
    let default = MouseSettings::default();
    MouseSettings {
        speed: first_f64(props, "libinput Accel Speed").unwrap_or(default.speed),
        flat_profile: props.get("libinput Accel Profile Enabled").and_then(|v| v.get(1)).map(|v| v == "1").unwrap_or(default.flat_profile),
        left_handed: flag(props, "libinput Left Handed Enabled").unwrap_or(default.left_handed),
        natural_scroll: flag(props, "libinput Natural Scrolling Enabled").unwrap_or(default.natural_scroll),
    }
}

/// The touchpad settings a touchpad reports (defaults for what it does not expose).
pub fn touchpad_from_props(props: &Props) -> TouchpadSettings {
    let default = TouchpadSettings::default();
    TouchpadSettings {
        speed: first_f64(props, "libinput Accel Speed").unwrap_or(default.speed),
        tap_to_click: flag(props, "libinput Tapping Enabled").unwrap_or(default.tap_to_click),
        natural_scroll: flag(props, "libinput Natural Scrolling Enabled").unwrap_or(default.natural_scroll),
        disable_while_typing: flag(props, "libinput Disable While Typing Enabled").unwrap_or(default.disable_while_typing),
    }
}

/// The layout part of `setxkbmap -query`.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardLayoutState {
    /// Active layouts with their variants.
    pub layouts: Vec<LayoutEntry>,
    /// The `grp:` option, or empty.
    pub switch_option: String,
    /// Every other option.
    pub extra_options: Vec<String>,
}

/// Parse `setxkbmap -query`. `None` when there is no `layout:` line.
pub fn parse_xkb_query(out: &str) -> Option<KeyboardLayoutState> {
    let value = |key: &str| -> Option<String> {
        out.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string())
    };
    let layouts_text = value("layout:").filter(|v| !v.is_empty())?;
    let variants_text = value("variant:").unwrap_or_default();
    let variants: Vec<&str> = variants_text.split(',').collect();
    let layouts = layouts_text
        .split(',')
        .enumerate()
        .map(|(i, l)| LayoutEntry { layout: l.trim().to_string(), variant: variants.get(i).map(|v| v.trim().to_string()).unwrap_or_default() })
        .collect();
    let options_text = value("options:").unwrap_or_default();
    let mut switch_option = String::new();
    let mut extra_options = Vec::new();
    for option in options_text.split(',').map(str::trim).filter(|o| !o.is_empty()) {
        if option.starts_with(SWITCH_OPTION_PREFIX) && switch_option.is_empty() {
            switch_option = option.to_string();
        } else {
            extra_options.push(option.to_string());
        }
    }
    Some(KeyboardLayoutState { layouts, switch_option, extra_options })
}

/// The key repeat delay (ms) and rate of `xset q`.
pub fn parse_xset_repeat(out: &str) -> Option<(u32, u32)> {
    let line = out.lines().find(|l| l.contains("auto repeat delay:"))?;
    let number_after = |key: &str| -> Option<u32> { line.split(key).nth(1)?.split_whitespace().next()?.parse().ok() };
    Some((number_after("auto repeat delay:")?, number_after("repeat rate:")?))
}

/// A layout or option with its description.
#[derive(Clone, Debug, PartialEq)]
pub struct XkbItem {
    /// The XKB code.
    pub code: String,
    /// The human description.
    pub description: String,
}

/// A layout variant.
#[derive(Clone, Debug, PartialEq)]
pub struct XkbVariant {
    /// The layout the variant belongs to.
    pub layout: String,
    /// The variant code.
    pub code: String,
    /// The human description.
    pub description: String,
}

/// What `evdev.lst` offers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XkbLists {
    /// All layouts.
    pub layouts: Vec<XkbItem>,
    /// All variants.
    pub variants: Vec<XkbVariant>,
    /// The `grp:` options (ways to switch layout).
    pub switch_options: Vec<XkbItem>,
}

/// Parse the text of `evdev.lst`.
pub fn parse_evdev_lst(text: &str) -> XkbLists {
    let mut lists = XkbLists::default();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("! ") {
            section = name.trim();
            continue;
        }
        let Some((code, rest)) = line.trim().split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => lists.layouts.push(XkbItem { code: code.to_string(), description: rest.to_string() }),
            "variant" => {
                let Some((layout, description)) = rest.split_once(": ") else { continue };
                lists.variants.push(XkbVariant { layout: layout.to_string(), code: code.to_string(), description: description.to_string() });
            }
            "option" if code.starts_with(SWITCH_OPTION_PREFIX) => {
                lists.switch_options.push(XkbItem { code: code.to_string(), description: rest.to_string() });
            }
            _ => {}
        }
    }
    lists
}

/// Read `evdev.lst` from the system; empty lists when it is missing.
pub fn load_xkb_lists() -> XkbLists {
    std::fs::read_to_string(EVDEV_LST_PATH).map(|t| parse_evdev_lst(&t)).unwrap_or_default()
}

/// Build the keyboard settings from the two tool outputs; `None` when the
/// layout cannot be read. The repeat values fall back to the defaults.
pub fn keyboard_from_outputs(xkb_query: &str, xset_q: &str) -> Option<KeyboardSettings> {
    let layout = parse_xkb_query(xkb_query)?;
    let default = KeyboardSettings::default();
    let (repeat_delay_ms, repeat_rate) = parse_xset_repeat(xset_q).unwrap_or((default.repeat_delay_ms, default.repeat_rate));
    Some(KeyboardSettings {
        layouts: layout.layouts,
        switch_option: layout.switch_option,
        extra_options: layout.extra_options,
        repeat_delay_ms,
        repeat_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "⎡ Virtual core pointer                    \tid=2\t[master pointer  (3)]\n\
⎜   ↳ Virtual core XTEST pointer              \tid=4\t[slave  pointer  (2)]\n\
⎜   ↳ E-Signal Trust Gaming Mouse             \tid=9\t[slave  pointer  (2)]\n\
⎜   ↳ COOLER MASTER CK352 Mechanlcal Keyboard Mouse\tid=12\t[slave  pointer  (2)]\n\
⎣ Virtual core keyboard                   \tid=3\t[master keyboard (2)]\n\
    ↳ Power Button                            \tid=6\t[slave  keyboard (3)]\n";

    const MOUSE_PROPS: &str = "Device 'E-Signal Trust Gaming Mouse':\n\
\tDevice Enabled (197):\t1\n\
\tlibinput Natural Scrolling Enabled (327):\t1\n\
\tlibinput Natural Scrolling Enabled Default (328):\t0\n\
\tlibinput Accel Speed (338):\t0.250000\n\
\tlibinput Accel Speed Default (339):\t0.000000\n\
\tlibinput Accel Profile Enabled (341):\t0, 1, 0\n\
\tlibinput Left Handed Enabled (349):\t1\n\
\tDevice Node (315):\t\"/dev/input/event4\"\n\
\tlibinput Accel Custom Fallback Points (343):\t<no items>\n";

    const TOUCHPAD_PROPS: &str = "Device 'SYNA Touchpad':\n\
\tlibinput Tapping Enabled (300):\t1\n\
\tlibinput Natural Scrolling Enabled (327):\t1\n\
\tlibinput Accel Speed (338):\t-0.500000\n\
\tlibinput Disable While Typing Enabled (360):\t0\n";

    #[test]
    fn pointer_list_keeps_slave_pointers_only() {
        let devices = parse_pointer_list(LIST);
        let ids: Vec<u32> = devices.iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![9, 12]);
        assert_eq!(devices[0].name, "E-Signal Trust Gaming Mouse");
    }

    #[test]
    fn props_are_keyed_by_name_with_their_values() {
        let p = parse_props(MOUSE_PROPS);
        assert_eq!(p["libinput Accel Speed"], vec!["0.250000"]);
        assert_eq!(p["libinput Accel Profile Enabled"], vec!["0", "1", "0"]);
        assert_eq!(p["Device Node"], vec!["\"/dev/input/event4\""]);
    }

    #[test]
    fn device_kind_needs_libinput() {
        assert_eq!(device_kind(&parse_props(MOUSE_PROPS)), Some(PointerKind::Mouse));
        assert_eq!(device_kind(&parse_props(TOUCHPAD_PROPS)), Some(PointerKind::Touchpad));
        assert_eq!(device_kind(&parse_props("Device 'Xephyr pointer':\n\tDevice Enabled (150):\t1\n")), None);
    }

    #[test]
    fn mouse_settings_come_from_the_properties() {
        let m = mouse_from_props(&parse_props(MOUSE_PROPS));
        assert_eq!(m, MouseSettings { speed: 0.25, flat_profile: true, left_handed: true, natural_scroll: true });
    }

    #[test]
    fn touchpad_settings_come_from_the_properties() {
        let t = touchpad_from_props(&parse_props(TOUCHPAD_PROPS));
        assert_eq!(t, TouchpadSettings { speed: -0.5, tap_to_click: true, natural_scroll: true, disable_while_typing: false });
    }

    #[test]
    fn xkb_query_reads_layouts_variants_and_options() {
        let out = "rules:      evdev\nmodel:      pc105\nlayout:     it,us\nvariant:    ,intl\noptions:    grp:alt_shift_toggle,caps:escape\n";
        let k = parse_xkb_query(out).unwrap();
        assert_eq!(k.layouts.len(), 2);
        assert_eq!(k.layouts[1], LayoutEntry { layout: "us".into(), variant: "intl".into() });
        assert_eq!(k.switch_option, "grp:alt_shift_toggle");
        assert_eq!(k.extra_options, vec!["caps:escape"]);
    }

    #[test]
    fn xkb_query_keeps_options() {
        let out = "layout:     it\noptions:    caps:escape\n";
        let k = parse_xkb_query(out).unwrap();
        assert_eq!(k.layouts, vec![LayoutEntry { layout: "it".into(), variant: String::new() }]);
        assert_eq!(k.switch_option, "");
        assert_eq!(k.extra_options, vec!["caps:escape"]);
    }

    #[test]
    fn xkb_query_without_a_layout_is_none() {
        assert_eq!(parse_xkb_query("rules: evdev\n"), None);
    }

    #[test]
    fn xset_repeat_values_are_read() {
        let out = "Keyboard Control:\n  auto repeat:  on    key click percent:  0    LED mask:  00000000\n  auto repeat delay:  500    repeat rate:  33\n";
        assert_eq!(parse_xset_repeat(out), Some((500, 33)));
        assert_eq!(parse_xset_repeat("nothing"), None);
    }

    #[test]
    fn evdev_lst_gives_layouts_variants_and_switch_options() {
        let text = "! model\n  pc105           Generic 105-key PC\n\n! layout\n  us              English (US)\n  it              Italian\n\n! variant\n  intl            us: English (US, intl., with dead keys)\n  winkeys         it: Italian (Windows)\n\n! option\n  grp                  Switching to another layout\n  grp:alt_shift_toggle Alt+Shift\n  caps:escape          Make Caps Lock an additional Esc\n";
        let l = parse_evdev_lst(text);
        assert_eq!(l.layouts.len(), 2);
        assert_eq!(l.layouts[1].code, "it");
        assert_eq!(l.variants[0], XkbVariant { layout: "us".into(), code: "intl".into(), description: "English (US, intl., with dead keys)".into() });
        assert_eq!(l.switch_options.len(), 1);
        assert_eq!(l.switch_options[0].code, "grp:alt_shift_toggle");
        assert_eq!(l.switch_options[0].description, "Alt+Shift");
    }
}
