//! Global hotkeys: the built-in bindings of `compositor.json` (quit, window
//! menu) and the table the hub sends with `hotkeys/set`. A key press is
//! matched here before it reaches the focused client (see
//! `State::keyboard_key`): a matched press and its release never reach it.

use opusdm_ipc::bindings::{format_key, parse_key, KeyCombo, Mods};
use serde_json::json;
use smithay::input::keyboard::ModifiersState;

use crate::config::{CompositorConfig, DEFAULT_QUIT, DEFAULT_WINDOW_MENU};
use crate::state::{State, LOG_TAG};

/// What a matched key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// Ends the compositor.
    Quit,
    /// Opens the window menu of the focused window.
    WindowMenu,
    /// Tells the hub (every `watch` connection) to run this function.
    Hub(String),
}

/// True when `combo` is the key `keysym_name` with exactly the modifiers
/// `mods` (Caps Lock and Num Lock are never part of `mods`). A single
/// ASCII letter is compared case-insensitively (`u` matches `U`), any
/// other name exactly.
pub fn combo_matches(combo: &KeyCombo, mods: Mods, keysym_name: &str) -> bool {
    if combo.mods != mods { return false; }
    let single_letter = combo.name.len() == 1 && combo.name.chars().all(|c| c.is_ascii_alphabetic());
    if single_letter { return combo.name.eq_ignore_ascii_case(keysym_name); }
    combo.name == keysym_name
}

/// The `Mods` of the xkb modifier state `state` (Ctrl, Alt, Shift, Super;
/// the locks are left out).
pub fn mods_of(state: &ModifiersState) -> Mods {
    Mods { ctrl: state.ctrl, alt: state.alt, shift: state.shift, sup: state.logo }
}

/// The text of the key `keysym_name` with `mods`, for the logs
/// (`Ctrl+backslash`).
pub fn combo_text(mods: Mods, keysym_name: &str) -> String {
    format_key(&KeyCombo { mods, name: keysym_name.to_string() })
}

/// The built-in bindings and the hotkeys of the hub.
#[derive(Debug, Clone, Default)]
pub struct HotkeyTable {
    /// The quit key, `None` when `quit_enabled` is false.
    quit: Option<KeyCombo>,
    /// The window menu key.
    window_menu: Option<KeyCombo>,
    /// The hotkeys of the hub: key and function name.
    hub: Vec<(KeyCombo, String)>,
}

impl HotkeyTable {
    /// The built-in bindings of `config`, no hub hotkey. A key that does not
    /// parse is logged and its default is used instead.
    pub fn new(config: &CompositorConfig) -> HotkeyTable {
        let quit = config.quit_enabled.then(|| _builtin_combo("quit", &config.quit, DEFAULT_QUIT)).flatten();
        if !config.quit_enabled {
            opusdm_ipc::debug_log!(LOG_TAG, "quit binding disabled by compositor.json");
        }
        let window_menu = _builtin_combo("window_menu", &config.window_menu, DEFAULT_WINDOW_MENU);
        HotkeyTable { quit, window_menu, hub: Vec::new() }
    }

    /// Replaces the hotkeys of the hub with `entries` (key, function).
    /// Returns the errors, each also logged: a key that does not parse, a
    /// key taken by a built-in binding (the built-in wins, the entry is
    /// dropped) and a key already bound by an earlier entry (the first wins).
    pub fn set_hub(&mut self, entries: &[(String, String)]) -> Vec<String> {
        let mut errors = Vec::new();
        let mut hub = Vec::new();
        for (key, function) in entries {
            let combo = match parse_key(key) {
                Ok(combo) => combo,
                Err(e) => {
                    errors.push(format!("hotkey {key}: {function}: {e}"));
                    continue;
                }
            };
            if let Some(name) = self._builtin_name(&combo) {
                errors.push(format!("hotkey {}: {function} conflicts with the built-in {name}, the built-in wins", format_key(&combo)));
                continue;
            }
            if let Some((_, first)) = hub.iter().find(|(c, _): &&(KeyCombo, String)| c.identity() == combo.identity()) {
                errors.push(format!("hotkey {}: {function} duplicates {first}", format_key(&combo)));
                continue;
            }
            hub.push((combo, function.clone()));
        }
        for e in &errors {
            opusdm_ipc::err_log!(LOG_TAG, "{e}");
        }
        opusdm_ipc::debug_log!(LOG_TAG, "hub hotkeys set: {} bound, {} refused", hub.len(), errors.len());
        self.hub = hub;
        errors
    }

    /// The binding of the key `keysym_name` with `mods`, looked up in the
    /// order Quit (when enabled), WindowMenu, the hub hotkeys.
    pub fn lookup(&self, mods: Mods, keysym_name: &str) -> Option<Binding> {
        let hit = |combo: &Option<KeyCombo>| combo.as_ref().is_some_and(|c| combo_matches(c, mods, keysym_name));
        if hit(&self.quit) { return Some(Binding::Quit); }
        if hit(&self.window_menu) { return Some(Binding::WindowMenu); }
        self.hub.iter().find(|(c, _)| combo_matches(c, mods, keysym_name)).map(|(_, f)| Binding::Hub(f.clone()))
    }

    /// Name of the built-in binding that has the key `combo`, if any.
    fn _builtin_name(&self, combo: &KeyCombo) -> Option<&'static str> {
        let same = |builtin: &Option<KeyCombo>| builtin.as_ref().is_some_and(|b| b.identity() == combo.identity());
        if same(&self.quit) { return Some("quit"); }
        if same(&self.window_menu) { return Some("window menu"); }
        None
    }
}

/// The combo of the built-in `name` written `text`; a text that does not
/// parse is logged and `default` is used.
fn _builtin_combo(name: &str, text: &str, default: &str) -> Option<KeyCombo> {
    match parse_key(text) {
        Ok(combo) => return Some(combo),
        Err(e) => opusdm_ipc::err_log!(LOG_TAG, "compositor.json {name} '{text}': {e}, using {default}"),
    }
    match parse_key(default) {
        Ok(combo) => Some(combo),
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "built-in {name} default '{default}' does not parse: {e}, binding off");
            None
        }
    }
}

impl State {
    /// Runs the binding `binding`, matched by the key `combo` (for the logs).
    pub fn run_binding(&mut self, binding: Binding, combo: &str) {
        opusdm_ipc::debug_log!(LOG_TAG, "hotkey {combo}: {binding:?}");
        match binding {
            Binding::Quit => {
                opusdm_ipc::err_log!(LOG_TAG, "hotkey {combo}: quit binding, stopping the compositor");
                self.quit.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // A second press closes the open menu, as in Openbox.
            Binding::WindowMenu if self.menu.is_some() => self.close_window_menu("the window menu key was pressed again"),
            Binding::WindowMenu => self.open_window_menu(),
            Binding::Hub(function) => {
                if !self.ipc.send_to_hub(&json!({"event": "hotkey", "function": function})) {
                    opusdm_ipc::err_log!(LOG_TAG, "hotkey {combo}: no hub connected");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The modifiers named in `text` (`"ca"` = Ctrl + Alt, `s` Shift, `w` Super).
    fn mods(text: &str) -> Mods {
        Mods { ctrl: text.contains('c'), alt: text.contains('a'), shift: text.contains('s'), sup: text.contains('w') }
    }

    #[test]
    fn a_letter_matches_both_cases_with_exactly_its_modifiers() {
        let combo = parse_key("Ctrl+Alt+u").unwrap();
        assert!(combo_matches(&combo, mods("ca"), "u"));
        assert!(combo_matches(&combo, mods("ca"), "U"));
        assert!(!combo_matches(&combo, mods("cas"), "u"));
        assert!(!combo_matches(&combo, mods("c"), "u"));
        assert!(!combo_matches(&combo, mods("ca"), "i"));
    }

    #[test]
    fn other_names_match_exactly() {
        let combo = parse_key("Ctrl+backslash").unwrap();
        assert!(combo_matches(&combo, mods("c"), "backslash"));
        assert!(!combo_matches(&combo, mods("c"), "Backslash"));
        assert!(!combo_matches(&combo, mods(""), "backslash"));
        let f1 = parse_key("Super+F1").unwrap();
        assert!(combo_matches(&f1, mods("w"), "F1"));
        assert!(!combo_matches(&f1, mods(""), "F1"));
        assert!(!combo_matches(&f1, mods("w"), "f1"));
    }

    #[test]
    fn the_modifier_state_maps_to_mods_without_the_locks() {
        let state = ModifiersState { ctrl: true, logo: true, caps_lock: true, num_lock: true, ..ModifiersState::default() };
        assert_eq!(mods_of(&state), mods("cw"));
    }

    #[test]
    fn lookup_order_is_quit_window_menu_then_hub() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        assert_eq!(table.lookup(mods("ca"), "BackSpace"), Some(Binding::Quit));
        assert_eq!(table.lookup(mods("a"), "space"), Some(Binding::WindowMenu));
        assert_eq!(table.lookup(mods("c"), "backslash"), None);
        let errors = table.set_hub(&[("Ctrl+backslash".into(), "terminal".into())]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(table.lookup(mods("c"), "backslash"), Some(Binding::Hub("terminal".into())));
        assert_eq!(table.lookup(mods("cs"), "backslash"), None);
    }

    #[test]
    fn set_hub_reports_a_conflict_with_a_built_in_and_bad_keys() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        let errors = table.set_hub(&[
            ("Alt+space".into(), "run".into()),
            ("Ctrl+Bogus+x".into(), "x".into()),
            ("Ctrl+Shift+space".into(), "run".into()),
        ]);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(errors[0], "hotkey Alt+space: run conflicts with the built-in window menu, the built-in wins");
        assert!(errors[1].starts_with("hotkey Ctrl+Bogus+x"), "{errors:?}");
        assert_eq!(table.lookup(mods("a"), "space"), Some(Binding::WindowMenu));
        assert_eq!(table.lookup(mods("cs"), "space"), Some(Binding::Hub("run".into())));
    }

    #[test]
    fn a_duplicated_hub_key_keeps_the_first_function() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        let errors = table.set_hub(&[("Ctrl+t".into(), "terminal".into()), ("Ctrl+T".into(), "other".into())]);
        assert_eq!(errors, ["hotkey Ctrl+t: other duplicates terminal"]);
        assert_eq!(table.lookup(mods("c"), "t"), Some(Binding::Hub("terminal".into())));
    }

    #[test]
    fn set_hub_replaces_the_previous_table() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        table.set_hub(&[("Ctrl+backslash".into(), "terminal".into())]);
        table.set_hub(&[("Ctrl+Print".into(), "shot".into())]);
        assert_eq!(table.lookup(mods("c"), "backslash"), None);
        assert_eq!(table.lookup(mods("c"), "Print"), Some(Binding::Hub("shot".into())));
    }

    #[test]
    fn a_disabled_quit_is_not_matched_and_frees_its_key_for_the_hub() {
        let config = CompositorConfig { quit_enabled: false, ..CompositorConfig::default() };
        let mut table = HotkeyTable::new(&config);
        assert_eq!(table.lookup(mods("ca"), "BackSpace"), None);
        assert!(table.set_hub(&[("Ctrl+Alt+BackSpace".into(), "logout".into())]).is_empty());
        assert_eq!(table.lookup(mods("ca"), "BackSpace"), Some(Binding::Hub("logout".into())));
    }

    #[test]
    fn a_bad_built_in_key_falls_back_to_its_default() {
        let config = CompositorConfig { window_menu: "Hyper+".into(), ..CompositorConfig::default() };
        let table = HotkeyTable::new(&config);
        assert_eq!(table.lookup(mods("a"), "space"), Some(Binding::WindowMenu));
    }
}
