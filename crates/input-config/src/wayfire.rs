//! The Wayland side of the input settings, under Wayfire: a Wayland client
//! cannot set input devices, so the settings go into the `[input]` section of
//! the `wayfire.ini` Wayfire runs with (see `opusdm_wayland::wayfire_ini`),
//! as one OpusDM block. Wayfire reloads the file by itself and applies the
//! settings to every device, also to one plugged in later.

use std::cell::RefCell;

use opusdm_wayland::labwc;
use opusdm_wayland::wayfire_ini;

use crate::backend::{Backend, Status};
use crate::change::Change;
use crate::labwc::fold;
use crate::{store, InputError, InputSettings};

/// INI section of the input settings.
const SECTION: &str = "input";

/// Name of the `wayfire.ini` block holding the input settings.
const INPUT_BLOCK: &str = "input settings";

/// The `[input]` lines for `s`. Layout, variant and options are always
/// written (empty when unused), so none of the user's own lines above the
/// block mixes with them.
pub fn input_lines(s: &InputSettings) -> Vec<String> {
    let (m, t, kb) = (&s.mouse, &s.touchpad, &s.keyboard);
    let options: Vec<&str> = std::iter::once(kb.switch_option.as_str()).chain(kb.extra_options.iter().map(String::as_str)).filter(|o| !o.is_empty()).collect();
    let joined = |f: fn(&crate::LayoutEntry) -> &str| kb.layouts.iter().map(f).collect::<Vec<_>>().join(",");
    vec![
        format!("mouse_cursor_speed = {:.2}", m.speed),
        format!("mouse_accel_profile = {}", if m.flat_profile { "flat" } else { "adaptive" }),
        format!("left_handed_mode = {}", m.left_handed),
        format!("mouse_natural_scroll = {}", m.natural_scroll),
        format!("touchpad_cursor_speed = {:.2}", t.speed),
        format!("tap_to_click = {}", t.tap_to_click),
        format!("natural_scroll = {}", t.natural_scroll),
        format!("disable_touchpad_while_typing = {}", t.disable_while_typing),
        format!("xkb_layout = {}", joined(|l| l.layout.as_str())),
        format!("xkb_variant = {}", joined(|l| l.variant.as_str())),
        format!("xkb_options = {}", options.join(",")),
        format!("kb_repeat_rate = {}", kb.repeat_rate),
        format!("kb_repeat_delay = {}", kb.repeat_delay_ms),
    ]
}

/// The input backend under Wayfire.
pub struct WayfireBackend {
    settings: RefCell<InputSettings>,
}

impl WayfireBackend {
    /// A backend starting from the saved settings (the file may hold values
    /// of the user's own), or the defaults.
    pub fn new() -> Self {
        Self { settings: RefCell::new(store::load_from(&store::input_path()).unwrap_or_default()) }
    }

    /// Write the block; Wayfire notices the change and reloads.
    fn write(&self) -> Result<(), InputError> {
        let s = self.settings.borrow().clone().normalized();
        let path = wayfire_ini::config_path();
        let failed = |reason: String| InputError::Write { path: path.display().to_string(), reason };
        let ini = labwc::read(&path).map_err(failed)?;
        let updated = wayfire_ini::with_block(ini.as_deref(), SECTION, INPUT_BLOCK, &input_lines(&s));
        labwc::write_if_changed(&path, ini.as_deref(), &updated).map_err(failed)?;
        Ok(())
    }
}

impl Default for WayfireBackend {
    /// Same as [`WayfireBackend::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WayfireBackend {
    /// Wayfire takes every setting.
    fn status(&self) -> Status {
        Status::all_ok()
    }

    /// The settings last written.
    fn read_current(&self) -> InputSettings {
        self.settings.borrow().clone()
    }

    /// Fold the change in and write the configuration.
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        fold(&mut self.settings.borrow_mut(), change);
        self.write()
    }

    /// Wayfire applies the settings to new devices by itself: there is never
    /// anything to reapply after a plug.
    fn signature(&self) -> String {
        "wayfire".to_string()
    }

    /// Nothing to look at again.
    fn refresh(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutEntry;

    #[test]
    fn the_lines_say_what_the_settings_say() {
        let mut s = InputSettings::default();
        s.mouse.flat_profile = true;
        s.mouse.left_handed = true;
        s.touchpad.speed = -0.25;
        s.keyboard.repeat_delay_ms = 300;
        let lines = input_lines(&s);
        for expected in ["mouse_accel_profile = flat", "left_handed_mode = true", "touchpad_cursor_speed = -0.25", "disable_touchpad_while_typing = true", "kb_repeat_delay = 300", "xkb_layout = us", "xkb_variant = ", "xkb_options = "] {
            assert!(lines.iter().any(|l| l == expected), "missing {expected:?} in {lines:?}");
        }
    }

    #[test]
    fn layouts_and_options_are_joined_by_commas() {
        let mut s = InputSettings::default();
        s.keyboard.layouts = vec![LayoutEntry { layout: "it".into(), variant: String::new() }, LayoutEntry { layout: "us".into(), variant: "intl".into() }];
        s.keyboard.switch_option = "grp:alt_shift_toggle".into();
        s.keyboard.extra_options = vec!["caps:escape".into()];
        let lines = input_lines(&s);
        assert!(lines.contains(&"xkb_layout = it,us".to_string()));
        assert!(lines.contains(&"xkb_variant = ,intl".to_string()));
        assert!(lines.contains(&"xkb_options = grp:alt_shift_toggle,caps:escape".to_string()));
    }
}
