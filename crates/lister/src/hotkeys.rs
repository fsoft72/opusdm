//! Local user hotkeys (F-03): the lookup table a Lister consults before its
//! built-in key table, and the helpers that turn a GDK key event into the
//! canonical [`KeyCombo`] the bindings file stores.

use crate::keys::Focus;
use gtk4::gdk;
use opusdm_ipc::bindings::{parse_key, Hotkey, KeyCombo, Mods};
use opusdm_ipc::function::FunctionId;
use std::collections::HashMap;

/// The Ctrl/Alt/Shift/Super flags held in `state`. NumLock and CapsLock are
/// ignored on purpose.
pub fn mods_from_state(state: gdk::ModifierType) -> Mods {
    Mods {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        sup: state.contains(gdk::ModifierType::SUPER_MASK),
    }
}

/// Whether `key` is a modifier (or lock) key pressed on its own.
fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
            | gdk::Key::Shift_Lock
    )
}

/// Whether `key` is `F1`..`F35`, a key that keeps working while typing.
fn is_function_key(key: gdk::Key) -> bool {
    let Some(name) = key.name() else { return false };
    let Some(digits) = name.strip_prefix('F') else { return false };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// The combination a key event stands for, with the key lower-cased the same
/// way the lookup does. `None` for a modifier-only press.
pub fn combo_from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<KeyCombo> {
    if is_modifier_key(key) {
        return None;
    }
    let name = key.to_lower().name()?.to_string();
    Some(KeyCombo { mods: mods_from_state(state), name })
}

/// Local (non-global) user hotkeys, keyed by (modifiers, lower-cased key).
#[derive(Default)]
pub struct HotkeyMap {
    map: HashMap<(Mods, gdk::Key), FunctionId>,
}

impl HotkeyMap {
    /// Build the table from a hotkey list. Global hotkeys are grabbed by the
    /// hub and never reach the Lister, so they are left out; entries whose key
    /// text does not parse or name a known key are skipped; the first of two
    /// entries on the same key wins.
    pub fn from_hotkeys(hotkeys: &[Hotkey]) -> Self {
        let mut map = HashMap::new();
        for hotkey in hotkeys.iter().filter(|h| !h.global) {
            let Ok(combo) = parse_key(&hotkey.key) else { continue };
            let Some(key) = gdk::Key::from_name(&combo.name) else { continue };
            map.entry((combo.mods, key.to_lower())).or_insert_with(|| hotkey.function.clone());
        }
        Self { map }
    }

    /// The function bound to this key event, if any. While a text field has
    /// the focus only a key with Ctrl, Alt or Super, or a function key, counts:
    /// a bare letter must stay a character.
    pub fn lookup(&self, key: gdk::Key, state: gdk::ModifierType, focus: Focus) -> Option<&FunctionId> {
        let mods = mods_from_state(state);
        let key = key.to_lower();
        if focus == Focus::Editing && !(mods.has_command_modifier() || is_function_key(key)) {
            return None;
        }
        self.map.get(&(mods, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(key: &str, function: &str, global: bool) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global }
    }

    fn ctrl_alt() -> gdk::ModifierType {
        gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
    }

    #[test]
    fn a_local_hotkey_is_found_by_key_and_modifiers() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+Alt+u", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::u, ctrl_alt(), Focus::List), Some(&"f1".to_string()));
        assert_eq!(map.lookup(gdk::Key::u, gdk::ModifierType::CONTROL_MASK, Focus::List), None);
        assert_eq!(map.lookup(gdk::Key::i, ctrl_alt(), Focus::List), None);
    }

    #[test]
    fn the_shifted_keyval_of_a_letter_still_matches() {
        // Shift+u arrives as keyval `U` with the shift mask set.
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+Shift+u", "f1", false)]);
        let state = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
        assert_eq!(map.lookup(gdk::Key::U, state, Focus::List), Some(&"f1".to_string()));
    }

    #[test]
    fn global_and_unparsable_hotkeys_are_left_out_and_the_first_duplicate_wins() {
        let map = HotkeyMap::from_hotkeys(&[
            hk("Ctrl+Alt+g", "global", true),
            hk("Ctrl+Alt+???", "bad", false),
            hk("Ctrl+Alt+u", "first", false),
            hk("ctrl+alt+U", "second", false),
        ]);
        assert_eq!(map.lookup(gdk::Key::g, ctrl_alt(), Focus::List), None);
        assert_eq!(map.lookup(gdk::Key::u, ctrl_alt(), Focus::List), Some(&"first".to_string()));
    }

    #[test]
    fn a_hand_written_uppercase_letter_still_matches() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+U", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::u, gdk::ModifierType::CONTROL_MASK, Focus::List), Some(&"f1".to_string()));
    }

    #[test]
    fn a_plain_letter_never_fires_while_typing() {
        let map = HotkeyMap::from_hotkeys(&[hk("j", "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::empty(), Focus::List), Some(&"f1".to_string()));
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::empty(), Focus::Editing), None);
    }

    #[test]
    fn a_command_modifier_or_a_function_key_fires_while_typing() {
        let map = HotkeyMap::from_hotkeys(&[hk("Ctrl+j", "ctrl", false), hk("F5", "fkey", false), hk("Shift+j", "shift", false)]);
        assert_eq!(map.lookup(gdk::Key::j, gdk::ModifierType::CONTROL_MASK, Focus::Editing), Some(&"ctrl".to_string()));
        assert_eq!(map.lookup(gdk::Key::F5, gdk::ModifierType::empty(), Focus::Editing), Some(&"fkey".to_string()));
        assert_eq!(map.lookup(gdk::Key::J, gdk::ModifierType::SHIFT_MASK, Focus::Editing), None, "Shift alone is still typing");
    }

    #[test]
    fn combo_from_event_lowercases_letters_and_reads_modifiers() {
        let combo = combo_from_event(gdk::Key::U, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK).unwrap();
        assert_eq!(combo.name, "u");
        assert!(combo.mods.ctrl && combo.mods.shift && !combo.mods.alt && !combo.mods.sup);
    }

    #[test]
    fn combo_from_event_ignores_modifier_only_presses() {
        for key in [gdk::Key::Control_L, gdk::Key::Alt_R, gdk::Key::Shift_L, gdk::Key::Super_L, gdk::Key::Caps_Lock, gdk::Key::Num_Lock] {
            assert_eq!(combo_from_event(key, gdk::ModifierType::empty()), None, "{key:?} is only a modifier");
        }
    }

    #[test]
    fn combo_text_roundtrips_through_the_map() {
        let combo = combo_from_event(gdk::Key::F7, gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK).unwrap();
        let text = opusdm_ipc::bindings::format_key(&combo);
        assert_eq!(text, "Alt+Shift+F7");
        let map = HotkeyMap::from_hotkeys(&[hk(&text, "f1", false)]);
        assert_eq!(map.lookup(gdk::Key::F7, gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK, Focus::List), Some(&"f1".to_string()));
    }
}
