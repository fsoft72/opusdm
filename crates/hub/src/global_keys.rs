//! System-global hotkeys (F-03), X11 only.
//!
//! A passive `XGrabKey` on the root window makes the X server deliver a
//! configured key combination to this process whatever window has the
//! focus. One connection, created lazily, owns the grabs; a thread blocks
//! on it and forwards each hit as a function id. Wayland has no equivalent
//! and is unsupported: if the X connection cannot be opened the hub logs it
//! once and global hotkeys stay inert.

use opusdm_ipc::bindings::{parse_key, Hotkey, Mods};
use opusdm_ipc::function::FunctionId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ConnectionExt as _, GrabMode, ModMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// X11 CapsLock modifier bit (`LockMask`).
pub const CAPS_LOCK_MASK: u16 = 1 << 1;

/// X11 NumLock modifier bit (`Mod2Mask` on a standard keyboard).
pub const NUM_LOCK_MASK: u16 = 1 << 4;

/// The modifier bits that identify a combination: Shift, Control, Mod1 (Alt)
/// and Mod4 (Super). Locks and pointer buttons are masked out.
pub const RELEVANT_MODS: u16 = 0x1 | 0x4 | 0x8 | 0x40;

/// One key grab: a keycode with a modifier mask, and the function it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grab {
    /// X keycode.
    pub keycode: u8,
    /// Modifier mask without lock bits.
    pub mods: u16,
    /// The function to run.
    pub function: FunctionId,
}

/// The X modifier mask of `mods` (no lock bits).
pub fn x_modifier_mask(mods: Mods) -> u16 {
    let mut mask = 0;
    if mods.shift {
        mask |= 0x1;
    }
    if mods.ctrl {
        mask |= 0x4;
    }
    if mods.alt {
        mask |= 0x8;
    }
    if mods.sup {
        mask |= 0x40;
    }
    mask
}

/// `base` combined with every NumLock/CapsLock state, because X matches a
/// grab against the exact modifier state including locks.
pub fn lock_variants(base: u16) -> [u16; 4] {
    [base, base | CAPS_LOCK_MASK, base | NUM_LOCK_MASK, base | CAPS_LOCK_MASK | NUM_LOCK_MASK]
}

/// `state` reduced to the modifier bits that identify a combination.
pub fn strip_locks(state: u16) -> u16 {
    state & RELEVANT_MODS
}

/// Reverse of the server's keyboard mapping: keysym to the lowest keycode
/// producing it (in any shift level). `keysyms` holds `keysyms_per_keycode`
/// entries per keycode, starting at `min_keycode`; `0` marks an unused slot.
pub fn keymap_from_mapping(min_keycode: u8, keysyms_per_keycode: u8, keysyms: &[u32]) -> HashMap<u32, u8> {
    let mut map = HashMap::new();
    let per = usize::from(keysyms_per_keycode).max(1);
    for (offset, slots) in keysyms.chunks(per).enumerate() {
        let keycode = min_keycode.saturating_add(offset as u8);
        for &keysym in slots.iter().filter(|&&s| s != 0) {
            map.entry(keysym).or_insert(keycode);
        }
    }
    map
}

/// The keysym for a GDK key name (GDK key values equal X keysyms), lower-cased
/// so a letter resolves to its unshifted keysym.
fn keysym_for(name: &str) -> Option<u32> {
    use gtk4::glib::translate::IntoGlib;
    gtk4::gdk::Key::from_name(name).map(|key| key.to_lower().into_glib())
}

/// Turn the global hotkeys into grabs. The second list holds the key texts
/// that could not be resolved (bad text, unknown key name, or a key this
/// keyboard does not have), for the log.
pub fn plan_grabs(globals: &[Hotkey], keymap: &HashMap<u32, u8>) -> (Vec<Grab>, Vec<String>) {
    let mut grabs = Vec::new();
    let mut unresolved = Vec::new();
    for hotkey in globals {
        let resolved = parse_key(&hotkey.key).ok().and_then(|combo| {
            let keycode = *keymap.get(&keysym_for(&combo.name)?)?;
            Some(Grab { keycode, mods: x_modifier_mask(combo.mods), function: hotkey.function.clone() })
        });
        match resolved {
            Some(grab) => grabs.push(grab),
            None => unresolved.push(hotkey.key.clone()),
        }
    }
    (grabs, unresolved)
}

/// The function bound to a key press, ignoring lock bits in `state`.
pub fn lookup(table: &HashMap<(u8, u16), FunctionId>, keycode: u8, state: u16) -> Option<&FunctionId> {
    table.get(&(keycode, strip_locks(state)))
}

/// The lazily created X connection, its keyboard map and the live grab table.
pub struct GlobalKeys {
    conn: Arc<RustConnection>,
    root: xproto::Window,
    keymap: HashMap<u32, u8>,
    table: Arc<Mutex<HashMap<(u8, u16), FunctionId>>>,
}

impl GlobalKeys {
    /// Connect to X11, read the keyboard map and start the thread that calls
    /// `on_key` with the function id of every grabbed key press. `None` (after
    /// one log line) when there is no X connection.
    pub fn start(on_key: Box<dyn Fn(FunctionId) + Send>) -> Option<GlobalKeys> {
        let (conn, screen_num) = match x11rb::connect(None) {
            Ok(pair) => pair,
            Err(e) => {
                opusdm_ipc::err_log!("hub", "global hotkeys unavailable (no X11 connection): {e}");
                return None;
            }
        };
        let conn = Arc::new(conn);
        let setup = conn.setup();
        let root = setup.roots[screen_num].root;
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let mapping = match conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()))
        {
            Ok(mapping) => mapping,
            Err(e) => {
                opusdm_ipc::err_log!("hub", "global hotkeys unavailable (cannot read the keyboard map): {e}");
                return None;
            }
        };
        let keymap = keymap_from_mapping(min, mapping.keysyms_per_keycode, &mapping.keysyms);
        let table: Arc<Mutex<HashMap<(u8, u16), FunctionId>>> = Arc::new(Mutex::new(HashMap::new()));

        let event_conn = conn.clone();
        let event_table = table.clone();
        let spawned = std::thread::Builder::new().name("opusdm-global-keys".into()).spawn(move || loop {
            match event_conn.wait_for_event() {
                Ok(Event::KeyPress(ev)) => {
                    let id = {
                        let table = event_table.lock().unwrap();
                        lookup(&table, ev.detail, u16::from(ev.state)).cloned()
                    };
                    if let Some(id) = id {
                        on_key(id);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    opusdm_ipc::err_log!("hub", "global hotkey thread stopping: {e}");
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("hub", "cannot spawn the global hotkey thread: {e}");
            return None;
        }
        Some(GlobalKeys { conn, root, keymap, table })
    }

    /// Replace every grab with the ones `globals` asks for. A key that cannot
    /// be resolved, or that another client already grabbed (`BadAccess`), is
    /// logged and skipped; the other grabs stay active.
    pub fn set(&self, globals: &[Hotkey]) {
        // Keycode 0 is AnyKey: drop every grab this connection made.
        let _ = self.conn.ungrab_key(0u8, self.root, ModMask::ANY);
        let (grabs, unresolved) = plan_grabs(globals, &self.keymap);
        for key in unresolved {
            opusdm_ipc::err_log!("hub", "global hotkey '{key}' does not match a key on this keyboard");
        }
        let mut table = HashMap::new();
        for grab in grabs {
            let mut grabbed = false;
            for mods in lock_variants(grab.mods) {
                let result = self
                    .conn
                    .grab_key(false, self.root, ModMask::from(mods), grab.keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                    .map_err(|e| e.to_string())
                    .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
                match result {
                    Ok(()) => grabbed = true,
                    Err(e) => opusdm_ipc::err_log!(
                        "hub",
                        "cannot grab keycode {} with modifiers {mods:#x} (already taken?): {e}",
                        grab.keycode
                    ),
                }
            }
            if grabbed {
                table.insert((grab.keycode, grab.mods), grab.function);
            }
        }
        *self.table.lock().unwrap() = table;
        let _ = self.conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::bindings::Hotkey;

    fn hk(key: &str, function: &str) -> Hotkey {
        Hotkey { key: key.into(), function: function.into(), global: true }
    }

    #[test]
    fn keymap_maps_each_keysym_to_its_lowest_keycode() {
        // Keycodes 8 and 9, two keysyms each (unshifted, shifted).
        let map = keymap_from_mapping(8, 2, &[0x75, 0x55, 0x75, 0x2b]);
        assert_eq!(map.get(&0x75), Some(&8), "the lowest keycode wins for a repeated keysym");
        assert_eq!(map.get(&0x55), Some(&8));
        assert_eq!(map.get(&0x2b), Some(&9));
    }

    #[test]
    fn keymap_skips_unused_slots() {
        let map = keymap_from_mapping(8, 2, &[0x75, 0, 0, 0]);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn modifier_masks_follow_the_x11_bits() {
        let combo = opusdm_ipc::bindings::parse_key("Ctrl+Alt+Shift+Super+u").unwrap();
        assert_eq!(x_modifier_mask(combo.mods), 0x1 | 0x4 | 0x8 | 0x40);
        let plain = opusdm_ipc::bindings::parse_key("Ctrl+u").unwrap();
        assert_eq!(x_modifier_mask(plain.mods), 0x4);
    }

    #[test]
    fn a_grab_is_made_for_every_lock_combination() {
        let variants = lock_variants(0x4);
        assert_eq!(variants.len(), 4);
        assert!(variants.contains(&0x4));
        assert!(variants.contains(&(0x4 | CAPS_LOCK_MASK)));
        assert!(variants.contains(&(0x4 | NUM_LOCK_MASK)));
        assert!(variants.contains(&(0x4 | CAPS_LOCK_MASK | NUM_LOCK_MASK)));
    }

    #[test]
    fn locks_and_buttons_are_stripped_from_an_event_state() {
        let state = 0x4 | CAPS_LOCK_MASK | NUM_LOCK_MASK | 0x100; // Ctrl + locks + button 1
        assert_eq!(strip_locks(state), 0x4);
    }

    #[test]
    fn plan_grabs_resolves_keys_and_reports_the_rest() {
        let keymap = keymap_from_mapping(30, 1, &[0x75]); // keycode 30 is `u`
        let globals = [hk("Ctrl+Alt+u", "f1"), hk("Ctrl+Alt+zzzz", "f2"), hk("Ctrl+Alt+F7", "f3"), hk("bad+", "f4")];
        let (grabs, unresolved) = plan_grabs(&globals, &keymap);
        assert_eq!(grabs, vec![Grab { keycode: 30, mods: 0x4 | 0x8, function: "f1".into() }]);
        assert_eq!(unresolved, vec!["Ctrl+Alt+zzzz".to_string(), "Ctrl+Alt+F7".to_string(), "bad+".to_string()]);
    }

    #[test]
    fn lookup_ignores_locks_and_needs_an_exact_modifier_match() {
        let mut table = std::collections::HashMap::new();
        table.insert((30u8, 0x4u16 | 0x8), "f1".to_string());
        assert_eq!(lookup(&table, 30, 0x4 | 0x8 | NUM_LOCK_MASK), Some(&"f1".to_string()));
        assert_eq!(lookup(&table, 30, 0x4), None);
        assert_eq!(lookup(&table, 31, 0x4 | 0x8), None);
    }
}
