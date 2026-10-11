//! The error line a tray center shows for a few seconds after a failed
//! action (audio, devices, network, battery).

use gtk4::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Seconds a message stays up.
pub const MESSAGE_SECS: u32 = 5;

/// A message that clears itself after [`MESSAGE_SECS`], unless a newer one
/// replaced it in the meantime.
#[derive(Default)]
pub struct TransientMessage {
    text: RefCell<Option<String>>,
    generation: Cell<u32>,
}

impl TransientMessage {
    /// The message shown now, if any.
    pub fn get(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    /// Put up `text`; returns the token [`Self::expire`] needs.
    pub fn set(&self, text: String) -> u32 {
        *self.text.borrow_mut() = Some(text);
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        generation
    }

    /// Clear the message put up as `generation`. Returns false, and keeps the
    /// text, when a newer message replaced it.
    pub fn expire(&self, generation: u32) -> bool {
        if self.generation.get() != generation { return false; }
        *self.text.borrow_mut() = None;
        true
    }

    /// Show `text` in the message `field` of `owner`, call `changed` now and
    /// again when the message clears.
    pub fn show<T: 'static>(owner: &Rc<T>, field: fn(&T) -> &TransientMessage, changed: fn(&Rc<T>), text: String) {
        let generation = field(owner).set(text);
        changed(owner);
        let owner = owner.clone();
        glib::timeout_add_seconds_local_once(MESSAGE_SECS, move || {
            if field(&owner).expire(generation) {
                changed(&owner);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_expires_with_its_own_token_only() {
        let m = TransientMessage::default();
        assert_eq!(m.get(), None);
        let first = m.set("one".into());
        let second = m.set("two".into());
        assert!(!m.expire(first), "a newer message is not cleared by the old timer");
        assert_eq!(m.get().as_deref(), Some("two"));
        assert!(m.expire(second));
        assert_eq!(m.get(), None);
    }
}
