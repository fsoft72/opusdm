//! Global hotkeys: the built-in bindings of `compositor.json` (quit, window
//! menu and the window manager actions of `wm::actions`) and the table the
//! hub sends with `hotkeys/set`. A key press is matched here before it
//! reaches the focused client (see `State::keyboard_key`): a matched press
//! and its release never reach it.

use opusdm_ipc::bindings::{format_key, parse_key, KeyCombo, Mods};
use serde_json::json;
use smithay::input::keyboard::ModifiersState;

use crate::config::{builtin_default, CompositorConfig};
use crate::state::{State, LOG_TAG};
use crate::wm::actions::{action_for, Action};

/// What a matched key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// Ends the compositor.
    Quit,
    /// Opens the window menu of the focused window.
    WindowMenu,
    /// Runs a built-in window manager action.
    BuiltIn(Action),
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

/// The usable hotkeys of the hub: key and function name.
type HubHotkeys = Vec<(KeyCombo, String)>;

/// The built-in bindings and the hotkeys of the hub.
#[derive(Debug, Clone, Default)]
pub struct HotkeyTable {
    /// The quit key, `None` when `quit_enabled` is false or the key is empty.
    quit: Option<KeyCombo>,
    /// The window menu key, `None` when its key is empty.
    window_menu: Option<KeyCombo>,
    /// The other enabled built-ins (name, action, key), in
    /// `BUILTIN_DEFAULTS` order; the first one wins when two share a key.
    others: Vec<(&'static str, Action, KeyCombo)>,
    /// The whole table the hub sent (key text, function), kept so that an
    /// entry a built-in takes comes back when the built-in moves away.
    hub_entries: Vec<(String, String)>,
    /// The usable hotkeys of the hub (key, function): `hub_entries` minus
    /// the bad keys, the keys of a built-in and the duplicates.
    hub: HubHotkeys,
}

impl HotkeyTable {
    /// The built-in bindings of `config`, no hub hotkey. A key that does not
    /// parse is logged and its default is used instead; an empty key
    /// disables its binding.
    pub fn new(config: &CompositorConfig) -> HotkeyTable {
        let mut table = HotkeyTable::default();
        table.rebuild_builtins(config);
        table
    }

    /// Replaces the built-in bindings with those of `config` and keeps the
    /// hotkeys of the hub, resolved again against the new built-ins.
    /// Returns the errors (each also logged): a key that does not parse
    /// (its default is used instead), two built-ins on the same key (the
    /// first in `lookup` order wins) and a hub hotkey a built-in now takes
    /// (the built-in wins; the entry comes back when the built-in moves).
    pub fn rebuild_builtins(&mut self, config: &CompositorConfig) -> Vec<String> {
        let mut errors = Vec::new();
        let mut others = Vec::new();
        self.quit = None;
        self.window_menu = None;
        for (name, text) in config.builtin_keys() {
            let combo = _builtin_combo(name, text, builtin_default(name), &mut errors);
            match name {
                "quit" => self.quit = combo.filter(|_| config.quit_enabled),
                "window_menu" => self.window_menu = combo,
                _ => match action_for(name) {
                    Some(action) => others.extend(combo.map(|c| (name, action, c))),
                    None => opusdm_ipc::err_log!(LOG_TAG, "built-in {name} has no action, binding off"),
                },
            }
        }
        if !config.quit_enabled {
            opusdm_ipc::debug_log!(LOG_TAG, "quit binding disabled by compositor.json");
        }
        self.others = others;
        errors.extend(self._builtin_duplicates());
        let (hub, problems) = self._resolve_hub();
        let clashes: Vec<String> = problems.into_iter().filter(|(clash, _)| *clash).map(|(_, e)| e).collect();
        for e in &clashes {
            opusdm_ipc::err_log!(LOG_TAG, "{e}");
        }
        self.hub = hub;
        errors.extend(clashes);
        opusdm_ipc::debug_log!(LOG_TAG, "built-in bindings set: {} enabled, {} errors, {} hub hotkeys kept", self.list().len(), errors.len(), self.hub.len());
        errors
    }

    /// The enabled built-in bindings as (name, canonical key text), in
    /// `BUILTIN_DEFAULTS` order (quit only when enabled).
    pub fn list(&self) -> Vec<(&'static str, String)> {
        let mut list: Vec<(&'static str, String)> = self.others.iter().map(|(name, _, combo)| (*name, format_key(combo))).collect();
        list.extend(self.window_menu.as_ref().map(|c| ("window_menu", format_key(c))));
        list.extend(self.quit.as_ref().map(|c| ("quit", format_key(c))));
        list
    }

    /// Replaces the hotkeys of the hub with `entries` (key, function); the
    /// whole table is kept for later `rebuild_builtins`. Returns the errors,
    /// each also logged: a key that does not parse, a key taken by a
    /// built-in binding (the built-in wins, the entry is not matched while
    /// it holds the key) and a key already bound by an earlier entry (the
    /// first wins).
    pub fn set_hub(&mut self, entries: &[(String, String)]) -> Vec<String> {
        self.hub_entries = entries.to_vec();
        let (hub, problems) = self._resolve_hub();
        let errors: Vec<String> = problems.into_iter().map(|(_, e)| e).collect();
        for e in &errors {
            opusdm_ipc::err_log!(LOG_TAG, "{e}");
        }
        opusdm_ipc::debug_log!(LOG_TAG, "hub hotkeys set: {} bound, {} refused", hub.len(), errors.len());
        self.hub = hub;
        errors
    }

    /// The usable hotkeys of `hub_entries` against the current built-ins,
    /// in order, with the messages (not logged here) of the entries left
    /// out, in entry order, each flagged true when a built-in takes its key
    /// (false: a key that does not parse, a duplicate).
    fn _resolve_hub(&self) -> (HubHotkeys, Vec<(bool, String)>) {
        let mut problems = Vec::new();
        let mut hub: HubHotkeys = Vec::new();
        for (key, function) in &self.hub_entries {
            let combo = match parse_key(key) {
                Ok(combo) => combo,
                Err(e) => {
                    problems.push((false, format!("hotkey {key}: {function}: {e}")));
                    continue;
                }
            };
            if let Some(name) = self._builtin_name(&combo) {
                problems.push((true, format!("hotkey {}: {function} conflicts with the built-in {name}, the built-in wins", format_key(&combo))));
                continue;
            }
            if let Some((_, first)) = hub.iter().find(|(c, _)| c.identity() == combo.identity()) {
                problems.push((false, format!("hotkey {}: {function} duplicates {first}", format_key(&combo))));
                continue;
            }
            hub.push((combo, function.clone()));
        }
        (hub, problems)
    }

    /// The binding of the key `keysym_name` with `mods`, looked up in the
    /// order Quit (when enabled), WindowMenu, the other built-ins, the hub
    /// hotkeys.
    pub fn lookup(&self, mods: Mods, keysym_name: &str) -> Option<Binding> {
        let hit = |combo: &Option<KeyCombo>| combo.as_ref().is_some_and(|c| combo_matches(c, mods, keysym_name));
        if hit(&self.quit) { return Some(Binding::Quit); }
        if hit(&self.window_menu) { return Some(Binding::WindowMenu); }
        if let Some((_, action, _)) = self.others.iter().find(|(_, _, c)| combo_matches(c, mods, keysym_name)) {
            return Some(Binding::BuiltIn(*action));
        }
        self.hub.iter().find(|(c, _)| combo_matches(c, mods, keysym_name)).map(|(_, f)| Binding::Hub(f.clone()))
    }

    /// The key of the built-in `action` as configured; `None` when its
    /// binding is disabled.
    pub fn combo_of(&self, action: Action) -> Option<KeyCombo> {
        self.others.iter().find(|(_, a, _)| *a == action).map(|(_, _, combo)| combo.clone())
    }

    /// The built-ins that share their key with an earlier one in `lookup`
    /// order (quit, window menu, then the others in `builtin_keys` order):
    /// the earlier one wins and the later never matches. Returns one error
    /// per later built-in, each also logged.
    fn _builtin_duplicates(&self) -> Vec<String> {
        let ordered = self.quit.iter().map(|c| ("quit", c))
            .chain(self.window_menu.iter().map(|c| ("window_menu", c)))
            .chain(self.others.iter().map(|(name, _, c)| (*name, c)));
        let mut seen: Vec<(&'static str, &KeyCombo)> = Vec::new();
        let mut errors = Vec::new();
        for (name, combo) in ordered {
            let Some((winner, _)) = seen.iter().find(|(_, c)| c.identity() == combo.identity()) else {
                seen.push((name, combo));
                continue;
            };
            let error = format!("compositor.json {name} '{}': same key as {winner}, {winner} wins", format_key(combo));
            opusdm_ipc::err_log!(LOG_TAG, "{error}");
            errors.push(error);
        }
        errors
    }

    /// Name of the built-in binding that has the key `combo`, if any.
    fn _builtin_name(&self, combo: &KeyCombo) -> Option<&'static str> {
        let same = |builtin: &Option<KeyCombo>| builtin.as_ref().is_some_and(|b| b.identity() == combo.identity());
        if same(&self.quit) { return Some("quit"); }
        if same(&self.window_menu) { return Some("window menu"); }
        self.others.iter().find(|(_, _, c)| c.identity() == combo.identity()).map(|(name, _, _)| *name)
    }
}

/// The combo of the built-in `name` written `text`: `None` when `text` is
/// empty (the binding is disabled); a text that does not parse is logged,
/// pushed to `errors`, and `default` is used.
fn _builtin_combo(name: &str, text: &str, default: &str, errors: &mut Vec<String>) -> Option<KeyCombo> {
    if text.trim().is_empty() {
        opusdm_ipc::debug_log!(LOG_TAG, "built-in {name} disabled by an empty key");
        return None;
    }
    match parse_key(text) {
        Ok(combo) => return Some(combo),
        Err(e) => {
            let error = format!("compositor.json {name} '{text}': {e}, using {default}");
            opusdm_ipc::err_log!(LOG_TAG, "{error}");
            errors.push(error);
        }
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
            Binding::BuiltIn(action) => self.run_action(action),
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
    fn the_list_has_every_enabled_built_in_with_its_canonical_key() {
        let table = HotkeyTable::new(&CompositorConfig::default());
        let list = table.list();
        assert_eq!(list.len(), 20, "{list:?}");
        assert!(list.contains(&("close", "Alt+F4".to_string())), "{list:?}");
        assert!(list.contains(&("send_left", "Alt+Shift+Left".to_string())), "canonical text: {list:?}");
        assert!(list.contains(&("quit", "Ctrl+Alt+BackSpace".to_string())), "{list:?}");
        let config = CompositorConfig { close: String::new(), quit_enabled: false, ..CompositorConfig::default() };
        let list = HotkeyTable::new(&config).list();
        assert_eq!(list.len(), 18, "{list:?}");
        assert!(!list.iter().any(|(name, _)| *name == "close" || *name == "quit"), "{list:?}");
    }

    #[test]
    fn an_empty_window_menu_or_quit_key_disables_it() {
        let config = CompositorConfig { window_menu: String::new(), quit: " ".into(), ..CompositorConfig::default() };
        let table = HotkeyTable::new(&config);
        assert_eq!(table.lookup(mods("a"), "space"), None);
        assert_eq!(table.lookup(mods("ca"), "BackSpace"), None);
    }

    #[test]
    fn the_window_manager_built_ins_take_their_keys_before_the_hub() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        assert_eq!(table.lookup(mods("a"), "Tab"), Some(Binding::BuiltIn(Action::SwitcherNext)));
        assert_eq!(table.lookup(mods("as"), "Tab"), Some(Binding::BuiltIn(Action::SwitcherPrev)));
        assert_eq!(table.lookup(mods("a"), "F4"), Some(Binding::BuiltIn(Action::Close)));
        assert_eq!(table.lookup(mods("w"), "F3"), Some(Binding::BuiltIn(Action::Desktop(2))));
        assert_eq!(table.lookup(mods("w"), "D"), Some(Binding::BuiltIn(Action::ShowDesktop)));
        assert_eq!(table.lookup(mods("ws"), "Up"), Some(Binding::BuiltIn(Action::Focus(crate::wm::actions::Direction::Up))));
        let errors = table.set_hub(&[("Alt+F4".into(), "x".into())]);
        assert_eq!(errors, ["hotkey Alt+F4: x conflicts with the built-in close, the built-in wins"]);
        assert_eq!(table.lookup(mods("a"), "F4"), Some(Binding::BuiltIn(Action::Close)));
    }

    #[test]
    fn a_hub_hotkey_shadowed_after_a_rebuild_is_reported_and_comes_back_later() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        assert!(table.set_hub(&[("Super+q".into(), "run".into())]).is_empty());
        assert_eq!(table.lookup(mods("w"), "q"), Some(Binding::Hub("run".into())));
        let config = CompositorConfig { close: "Super+q".into(), ..CompositorConfig::default() };
        let errors = table.rebuild_builtins(&config);
        assert_eq!(errors, ["hotkey Super+q: run conflicts with the built-in close, the built-in wins"]);
        assert_eq!(table.lookup(mods("w"), "q"), Some(Binding::BuiltIn(Action::Close)));
        let errors = table.rebuild_builtins(&CompositorConfig { quit: "Super+q".into(), ..CompositorConfig::default() });
        assert_eq!(errors, ["hotkey Super+q: run conflicts with the built-in quit, the built-in wins"]);
        assert!(table.rebuild_builtins(&CompositorConfig::default()).is_empty());
        assert_eq!(table.lookup(mods("w"), "q"), Some(Binding::Hub("run".into())), "the entry is back");
        assert_eq!(table.lookup(mods("a"), "F4"), Some(Binding::BuiltIn(Action::Close)));
    }

    #[test]
    fn rebuilding_the_built_ins_keeps_the_hub_table_and_reports_bad_keys() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        table.set_hub(&[("Ctrl+backslash".into(), "terminal".into())]);
        let config = CompositorConfig { close: "Super+q".into(), lower: "Hyper+".into(), ..CompositorConfig::default() };
        let errors = table.rebuild_builtins(&config);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("lower"), "{errors:?}");
        assert_eq!(table.lookup(mods("c"), "backslash"), Some(Binding::Hub("terminal".into())));
        let list = table.list();
        assert!(list.contains(&("close", "Super+q".to_string())), "{list:?}");
        assert!(list.contains(&("lower", "Alt+Escape".to_string())), "a bad key takes its default: {list:?}");
    }

    #[test]
    fn two_built_ins_on_the_same_key_are_reported_and_the_first_wins() {
        let mut table = HotkeyTable::new(&CompositorConfig::default());
        let config = CompositorConfig { lower: "Alt+F4".into(), show_desktop: "Alt+space".into(), ..CompositorConfig::default() };
        let errors = table.rebuild_builtins(&config);
        assert_eq!(
            errors,
            [
                "compositor.json show_desktop 'Alt+space': same key as window_menu, window_menu wins",
                "compositor.json lower 'Alt+F4': same key as close, close wins",
            ]
        );
        assert_eq!(table.lookup(mods("a"), "F4"), Some(Binding::BuiltIn(Action::Close)));
        assert_eq!(table.lookup(mods("a"), "space"), Some(Binding::WindowMenu));
    }

    #[test]
    fn a_bad_built_in_key_falls_back_to_its_default() {
        let config = CompositorConfig { window_menu: "Hyper+".into(), ..CompositorConfig::default() };
        let table = HotkeyTable::new(&config);
        assert_eq!(table.lookup(mods("a"), "space"), Some(Binding::WindowMenu));
    }
}
