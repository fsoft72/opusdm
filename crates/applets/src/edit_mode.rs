//! The edit mode of the applets on the GTK main loop: on or off, plus the
//! listeners (one per applet window) told when it changes.
//!
//! The connection threads keep their own `Arc<AtomicBool>` (they answer
//! `GetEdit` / `SetEdit` without the main loop, see [`crate::server`]); a
//! `SetEdit` also reaches the host on the main loop, which calls
//! [`EditMode::set`] here, so both agree. The edit mode is not saved: after a
//! restart it is off.
//!
//! Ownership: the host owns an `EditMode` for the whole run and gives a clone
//! (the same shared cell and list) to every applet window. Each window
//! subscribes one listener when it opens and unsubscribes it when it is
//! dropped; a listener holds only weak references to its widgets and logs
//! when they are gone.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What a listener is called with: the new edit mode.
pub type Listener = Rc<dyn Fn(bool)>;

/// The handle of a subscribed listener, to unsubscribe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// The shared edit mode and its listeners (see the module doc). Clones
/// share the same state.
#[derive(Clone, Default)]
pub struct EditMode {
    on: Rc<Cell<bool>>,
    listeners: Rc<RefCell<Vec<(ListenerId, Listener)>>>,
    next_id: Rc<Cell<u64>>,
}

impl EditMode {
    /// A new edit mode, off, with no listener.
    pub fn new() -> EditMode {
        EditMode::default()
    }

    /// True while the edit mode is on.
    pub fn get(&self) -> bool {
        self.on.get()
    }

    /// Turn the edit mode on or off. When that changes it, every listener is
    /// called with `on` (a listener may subscribe or unsubscribe while it
    /// runs). Returns true when it changed.
    pub fn set(&self, on: bool) -> bool {
        if self.on.get() == on { return false; }
        self.on.set(on);
        // A copy, so a listener can subscribe or unsubscribe while it runs.
        let listeners: Vec<Listener> = self.listeners.borrow().iter().map(|(_, l)| l.clone()).collect();
        for listener in listeners {
            listener(on);
        }
        true
    }

    /// Call `listener` on every change from now on.
    pub fn subscribe(&self, listener: impl Fn(bool) + 'static) -> ListenerId {
        let id = ListenerId(self.next_id.get());
        self.next_id.set(id.0 + 1);
        self.listeners.borrow_mut().push((id, Rc::new(listener)));
        id
    }

    /// Stop calling the listener `id`. Returns false when it was not
    /// subscribed.
    pub fn unsubscribe(&self, id: ListenerId) -> bool {
        let mut listeners = self.listeners.borrow_mut();
        let before = listeners.len();
        listeners.retain(|(lid, _)| *lid != id);
        listeners.len() != before
    }

    /// How many listeners are subscribed.
    pub fn listener_count(&self) -> usize {
        self.listeners.borrow().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_mode_starts_off() {
        assert!(!EditMode::new().get());
    }

    #[test]
    fn a_change_calls_every_listener_once() {
        let edit = EditMode::new();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (a, b) = (seen.clone(), seen.clone());
        edit.subscribe(move |on| a.borrow_mut().push(("a", on)));
        edit.subscribe(move |on| b.borrow_mut().push(("b", on)));
        assert!(edit.set(true));
        assert!(edit.get());
        assert_eq!(*seen.borrow(), [("a", true), ("b", true)]);
        assert!(!edit.set(true), "the same value is not a change");
        assert_eq!(seen.borrow().len(), 2, "no call without a change");
        assert!(edit.set(false));
        assert_eq!(seen.borrow()[2..], [("a", false), ("b", false)]);
    }

    #[test]
    fn clones_share_the_state_and_the_listeners() {
        let edit = EditMode::new();
        let other = edit.clone();
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        other.subscribe(move |_| counter.set(counter.get() + 1));
        edit.set(true);
        assert!(other.get());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn an_unsubscribed_listener_is_not_called() {
        let edit = EditMode::new();
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        let id = edit.subscribe(move |_| counter.set(counter.get() + 1));
        assert_eq!(edit.listener_count(), 1);
        assert!(edit.unsubscribe(id));
        assert!(!edit.unsubscribe(id), "a second unsubscribe finds nothing");
        assert_eq!(edit.listener_count(), 0);
        edit.set(true);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn a_listener_may_unsubscribe_while_it_runs() {
        let edit = EditMode::new();
        let slot: Rc<Cell<Option<ListenerId>>> = Rc::default();
        let (inner, inner_slot) = (edit.clone(), slot.clone());
        let id = edit.subscribe(move |_| {
            if let Some(id) = inner_slot.get() { inner.unsubscribe(id); }
        });
        slot.set(Some(id));
        assert!(edit.set(true), "no panic on a borrowed list");
        assert_eq!(edit.listener_count(), 0);
    }
}
