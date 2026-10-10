//! The Wayland side of the input settings, under labwc: a Wayland client
//! cannot set input devices, so the settings go into labwc's configuration
//! (see `opusdm_wayland::labwc`) and labwc applies them. Mouse and touchpad
//! become a `<libinput>` block of `rc.xml` (categories `non-touch` and
//! `touchpad`, so a device plugged in later gets them too), key repeat a
//! block inside `<keyboard>`, layouts and options `XKB_DEFAULT_*` lines of
//! the `environment` file.

use std::cell::RefCell;

use opusdm_wayland::labwc::{self, Section};

use crate::backend::{Backend, Status};
use crate::change::Change;
use crate::{store, InputError, InputSettings};

/// Name of the `rc.xml` block holding `<libinput>`.
const LIBINPUT_BLOCK: &str = "input devices";

/// Name of the `rc.xml` block holding the key repeat.
const REPEAT_BLOCK: &str = "key repeat";

/// Name of the `environment` block holding the keyboard layouts.
const LAYOUT_BLOCK: &str = "keyboard layouts";

/// `yes` or `no`, as labwc writes booleans.
fn yes_no(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

/// `settings` with `change` applied.
pub fn fold(settings: &mut InputSettings, change: &Change) {
    match change {
        Change::MouseSpeed(v) => settings.mouse.speed = *v,
        Change::MouseFlatProfile(v) => settings.mouse.flat_profile = *v,
        Change::MouseLeftHanded(v) => settings.mouse.left_handed = *v,
        Change::MouseNaturalScroll(v) => settings.mouse.natural_scroll = *v,
        Change::TouchpadSpeed(v) => settings.touchpad.speed = *v,
        Change::TouchpadTap(v) => settings.touchpad.tap_to_click = *v,
        Change::TouchpadNaturalScroll(v) => settings.touchpad.natural_scroll = *v,
        Change::TouchpadDisableWhileTyping(v) => settings.touchpad.disable_while_typing = *v,
        Change::Layout { layouts, switch_option, extra_options } => {
            settings.keyboard.layouts = layouts.clone();
            settings.keyboard.switch_option = switch_option.clone();
            settings.keyboard.extra_options = extra_options.clone();
        }
        Change::Repeat { delay_ms, rate } => {
            settings.keyboard.repeat_delay_ms = *delay_ms;
            settings.keyboard.repeat_rate = *rate;
        }
    }
}

/// The `<libinput>` element for the mouse and touchpad settings.
pub fn libinput_block(s: &InputSettings) -> String {
    let (m, t) = (&s.mouse, &s.touchpad);
    format!(
        "  <libinput>\n    <device category=\"non-touch\">\n      <pointerSpeed>{:.2}</pointerSpeed>\n      <accelProfile>{}</accelProfile>\n      <leftHanded>{}</leftHanded>\n      <naturalScroll>{}</naturalScroll>\n    </device>\n    <device category=\"touchpad\">\n      <pointerSpeed>{:.2}</pointerSpeed>\n      <tap>{}</tap>\n      <naturalScroll>{}</naturalScroll>\n      <disableWhileTyping>{}</disableWhileTyping>\n    </device>\n  </libinput>\n",
        m.speed,
        if m.flat_profile { "flat" } else { "adaptive" },
        yes_no(m.left_handed),
        yes_no(m.natural_scroll),
        t.speed,
        yes_no(t.tap_to_click),
        yes_no(t.natural_scroll),
        yes_no(t.disable_while_typing),
    )
}

/// The key repeat lines of `<keyboard>`.
pub fn repeat_block(s: &InputSettings) -> String {
    format!("    <repeatRate>{}</repeatRate>\n    <repeatDelay>{}</repeatDelay>\n", s.keyboard.repeat_rate, s.keyboard.repeat_delay_ms)
}

/// The `XKB_DEFAULT_*` lines for the keyboard layouts (labwc reads them on
/// start and on reload).
pub fn layout_lines(s: &InputSettings) -> Vec<String> {
    let kb = &s.keyboard;
    let mut lines = vec![format!("XKB_DEFAULT_LAYOUT={}", kb.layouts.iter().map(|l| l.layout.as_str()).collect::<Vec<_>>().join(","))];
    if kb.layouts.iter().any(|l| !l.variant.is_empty()) {
        lines.push(format!("XKB_DEFAULT_VARIANT={}", kb.layouts.iter().map(|l| l.variant.as_str()).collect::<Vec<_>>().join(",")));
    }
    let options: Vec<&str> = std::iter::once(kb.switch_option.as_str()).chain(kb.extra_options.iter().map(String::as_str)).filter(|o| !o.is_empty()).collect();
    if !options.is_empty() {
        lines.push(format!("XKB_DEFAULT_OPTIONS={}", options.join(",")));
    }
    lines
}

/// The input backend under labwc.
pub struct LabwcBackend {
    settings: RefCell<InputSettings>,
}

impl LabwcBackend {
    /// A backend starting from the saved settings (labwc cannot be asked
    /// for its current ones), or the defaults.
    pub fn new() -> Self {
        Self { settings: RefCell::new(store::load_from(&store::input_path()).unwrap_or_default()) }
    }

    /// Write the three blocks and have labwc reload when one changed.
    fn write(&self) -> Result<(), InputError> {
        let s = self.settings.borrow().clone().normalized();
        let failed = |reason: String| InputError::Write { path: labwc::rc_path().display().to_string(), reason };
        let rc_path = labwc::rc_path();
        let rc = labwc::read(&rc_path).map_err(failed)?;
        let with_input = labwc::with_block(rc.as_deref(), Section::Root, LIBINPUT_BLOCK, &libinput_block(&s)).map_err(failed)?;
        let updated = labwc::with_block(Some(&with_input), Section::Keyboard, REPEAT_BLOCK, &repeat_block(&s)).map_err(failed)?;
        let mut changed = labwc::write_if_changed(&rc_path, rc.as_deref(), &updated).map_err(failed)?;
        let env_path = labwc::environment_path();
        let env = labwc::read(&env_path).map_err(failed)?;
        let env_updated = labwc::with_env_block(env.as_deref(), LAYOUT_BLOCK, &layout_lines(&s));
        changed |= labwc::write_if_changed(&env_path, env.as_deref(), &env_updated).map_err(failed)?;
        if changed { labwc::reload().map_err(InputError::Session)?; }
        Ok(())
    }
}

impl Default for LabwcBackend {
    /// Same as [`LabwcBackend::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for LabwcBackend {
    /// labwc takes every setting.
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

    /// labwc applies the settings to new devices by itself: there is never
    /// anything to reapply after a plug.
    fn signature(&self) -> String {
        "labwc".to_string()
    }

    /// Nothing to look at again.
    fn refresh(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::all_changes;
    use crate::LayoutEntry;

    #[test]
    fn every_change_folds_into_the_settings() {
        let mut target = InputSettings::default();
        target.mouse.speed = 0.5;
        target.touchpad.tap_to_click = true;
        target.keyboard.repeat_rate = 40;
        target.keyboard.layouts = vec![LayoutEntry { layout: "it".into(), variant: String::new() }];
        let mut folded = InputSettings::default();
        for change in all_changes(&target) {
            fold(&mut folded, &change);
        }
        assert_eq!(folded, target);
    }

    #[test]
    fn the_blocks_say_what_the_settings_say() {
        let mut s = InputSettings::default();
        s.mouse.flat_profile = true;
        s.touchpad.speed = -0.25;
        s.keyboard.repeat_delay_ms = 300;
        let libinput = libinput_block(&s);
        assert!(libinput.contains("<accelProfile>flat</accelProfile>"));
        assert!(libinput.contains("<pointerSpeed>-0.25</pointerSpeed>\n      <tap>no</tap>"));
        assert!(libinput.contains("<disableWhileTyping>yes</disableWhileTyping>"));
        assert!(repeat_block(&s).contains("<repeatDelay>300</repeatDelay>"));
    }

    #[test]
    fn layouts_become_xkb_variables() {
        let mut s = InputSettings::default();
        assert_eq!(layout_lines(&s), ["XKB_DEFAULT_LAYOUT=us"]);
        s.keyboard.layouts = vec![LayoutEntry { layout: "it".into(), variant: String::new() }, LayoutEntry { layout: "us".into(), variant: "intl".into() }];
        s.keyboard.switch_option = "grp:alt_shift_toggle".into();
        s.keyboard.extra_options = vec!["caps:escape".into()];
        assert_eq!(layout_lines(&s), ["XKB_DEFAULT_LAYOUT=it,us", "XKB_DEFAULT_VARIANT=,intl", "XKB_DEFAULT_OPTIONS=grp:alt_shift_toggle,caps:escape"]);
    }
}
