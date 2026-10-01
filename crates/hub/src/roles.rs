//! The hybrid SOURCE/DEST role machine. Pure: no sockets, no GTK.
//!
//! At most one Lister holds SOURCE. Zero or more hold DEST: a locked DEST
//! persists, the single unlocked ("transient") DEST is replaced when focus or
//! an explicit button assigns a new one. A Copy/Move fans out to every DEST.
//! Lock is only meaningful for a Lister that currently holds a role.

use opusdm_ipc::{ListerId, Role};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    role: Role,
    locked: bool,
}

/// Current per-Lister role assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleState {
    slots: BTreeMap<ListerId, Slot>,
}

impl RoleState {
    /// The SOURCE Lister, if any.
    pub fn source(&self) -> Option<ListerId> {
        self.slots
            .iter()
            .find(|(_, s)| s.role == Role::Source)
            .map(|(id, _)| *id)
    }

    /// Every DEST Lister, ascending.
    pub fn dests(&self) -> Vec<ListerId> {
        self.slots
            .iter()
            .filter(|(_, s)| s.role == Role::Dest)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Whether `id` currently holds SOURCE or DEST.
    pub fn has_role(&self, id: ListerId) -> bool {
        self.slots.get(&id).is_some_and(|s| s.role != Role::None)
    }

    fn role_of(&self, id: ListerId) -> Role {
        self.slots.get(&id).map(|s| s.role).unwrap_or(Role::None)
    }

    fn is_locked(&self, id: ListerId) -> bool {
        self.slots.get(&id).is_some_and(|s| s.locked)
    }

    fn set(&mut self, id: ListerId, role: Role, locked: bool) {
        if role == Role::None {
            self.slots.remove(&id);
        } else {
            self.slots.insert(id, Slot { role, locked });
        }
    }

    fn locked_source(&self) -> Option<ListerId> {
        self.slots
            .iter()
            .find(|(_, s)| s.role == Role::Source && s.locked)
            .map(|(id, _)| *id)
    }

    /// Whether SOURCE is currently pinned - a `SetRole(_, Source)` from any
    /// other Lister will be rejected while this holds. Sent to Listers so
    /// they don't keep retrying a doomed request (see badge cycle in the
    /// Lister UI).
    pub fn source_locked(&self) -> bool {
        self.locked_source().is_some()
    }

    /// Clear every unlocked DEST (the transient slot).
    fn clear_transient_dests(&mut self, except: Option<ListerId>) {
        let victims: Vec<ListerId> = self
            .slots
            .iter()
            .filter(|(id, s)| {
                s.role == Role::Dest && !s.locked && Some(**id) != except
            })
            .map(|(id, _)| *id)
            .collect();
        for id in victims {
            self.slots.remove(&id);
        }
    }
}

/// Something that can change the role assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleEvent {
    /// A Lister window received focus.
    FocusGained(ListerId),
    /// A Lister pressed an explicit role button.
    SetRole(ListerId, Role),
    /// A Lister pinned its current role.
    Lock(ListerId),
    /// A Lister released its pin.
    Unlock(ListerId),
    /// A Lister disconnected.
    Dropped(ListerId),
}

/// What the caller must do after applying an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleEffect {
    /// Roles changed; send `RoleChanged` to every Lister.
    Broadcast,
    /// The request was refused; tell this Lister to re-snap its buttons.
    Reject(ListerId),
}

/// Apply one event. Returns the new state and the effects to carry out.
pub fn apply_role_event(mut s: RoleState, ev: RoleEvent) -> (RoleState, Vec<RoleEffect>) {
    let before = (s.source(), s.dests());

    match ev {
        RoleEvent::Lock(id) => {
            if !s.has_role(id) {
                return (s, vec![RoleEffect::Reject(id)]);
            }
            if let Some(slot) = s.slots.get_mut(&id) {
                slot.locked = true;
            }
            return (s, Vec::new());
        }
        RoleEvent::Unlock(id) => {
            if let Some(slot) = s.slots.get_mut(&id) {
                slot.locked = false;
            }
            return (s, Vec::new());
        }
        RoleEvent::Dropped(id) => {
            s.slots.remove(&id);
        }
        RoleEvent::FocusGained(id) => {
            if s.role_of(id) == Role::Source || s.is_locked(id) {
                return (s, Vec::new());
            }
            if s.locked_source().is_some() {
                return (s, Vec::new());
            }
            let old_source = s.source();
            s.set(id, Role::Source, false);
            s.clear_transient_dests(None);
            if let Some(os) = old_source {
                if os != id && !s.is_locked(os) {
                    s.set(os, Role::Dest, false);
                }
            }
        }
        RoleEvent::SetRole(id, Role::Source) => {
            if let Some(ls) = s.locked_source() {
                if ls != id {
                    return (s, vec![RoleEffect::Reject(id)]);
                }
            }
            if let Some(os) = s.source() {
                if os != id && !s.is_locked(os) {
                    s.set(os, Role::None, false);
                }
            }
            let keep_lock = s.is_locked(id);
            s.set(id, Role::Source, keep_lock);
        }
        RoleEvent::SetRole(id, Role::Dest) => {
            let keep_lock = s.is_locked(id);
            s.set(id, Role::Dest, keep_lock);
            if !keep_lock {
                s.clear_transient_dests(Some(id));
            }
        }
        RoleEvent::SetRole(id, Role::None) => {
            s.slots.remove(&id);
        }
    }

    if (s.source(), s.dests()) == before {
        (s, Vec::new())
    } else {
        (s, vec![RoleEffect::Broadcast])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::Role;

    fn ids() -> (ListerId, ListerId, ListerId) {
        (ListerId(1), ListerId(2), ListerId(3))
    }

    fn ev(s: RoleState, e: RoleEvent) -> (RoleState, Vec<RoleEffect>) {
        apply_role_event(s, e)
    }

    #[test]
    fn first_focus_becomes_source() {
        let (a, _, _) = ids();
        let (s, fx) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        assert_eq!(s.source(), Some(a));
        assert!(s.dests().is_empty());
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn second_focus_demotes_previous_source_to_dest() {
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = ev(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source(), Some(b));
        assert_eq!(s.dests(), vec![a]);
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn refocusing_current_source_is_a_noop() {
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::FocusGained(b));
        let (s2, fx) = ev(s.clone(), RoleEvent::FocusGained(b));
        assert_eq!(s2, s);
        assert!(fx.is_empty());
    }

    #[test]
    fn cannot_lock_without_a_role() {
        let (a, _, _) = ids();
        let (s, fx) = ev(RoleState::default(), RoleEvent::Lock(a));
        assert_eq!(fx, vec![RoleEffect::Reject(a)]);
        assert!(!s.has_role(a));
    }

    #[test]
    fn locked_source_not_displaced_by_focus() {
        let (a, b, c) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = ev(s, RoleEvent::Lock(a));
        assert!(fx.is_empty());
        let (s, _) = ev(s, RoleEvent::FocusGained(c));
        assert_eq!(s.source(), Some(a));
        let (s, _) = ev(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source(), Some(a));
    }

    #[test]
    fn multiple_locked_dests_accumulate() {
        let (a, b, c) = ids();
        // a = source
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        // b = dest, locked
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest));
        let (s, _) = ev(s, RoleEvent::Lock(b));
        // c = dest, locked
        let (s, _) = ev(s, RoleEvent::SetRole(c, Role::Dest));
        let (s, fx) = ev(s, RoleEvent::Lock(c));
        assert!(fx.is_empty()); // lock does not broadcast
        assert_eq!(s.source(), Some(a));
        assert_eq!(s.dests(), vec![b, c]);
    }

    #[test]
    fn new_source_focus_clears_transient_dest_but_keeps_locked_dests() {
        let (a, b, c) = ids();
        let d = ListerId(4);
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a)); // a source
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest));
        let (s, _) = ev(s, RoleEvent::Lock(b)); // b locked dest
        let (s, _) = ev(s, RoleEvent::SetRole(c, Role::Dest)); // c transient dest
        assert_eq!(s.dests(), vec![b, c]);
        // Focusing a brand-new Lister d: d becomes source, a demotes to the new
        // transient dest, old transient dest c is cleared, locked b stays.
        let (s, _) = ev(s, RoleEvent::FocusGained(d));
        assert_eq!(s.source(), Some(d));
        assert_eq!(s.dests(), vec![a, b]);
    }

    #[test]
    fn explicit_transient_dest_replaces_previous_transient() {
        let (a, b, c) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest)); // b transient
        let (s, _) = ev(s, RoleEvent::SetRole(c, Role::Dest)); // c transient, b cleared
        assert_eq!(s.dests(), vec![c]);
    }

    #[test]
    fn set_role_source_colliding_with_locked_source_is_rejected() {
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::Lock(a));
        let (s2, fx) = ev(s.clone(), RoleEvent::SetRole(b, Role::Source));
        assert_eq!(s2, s);
        assert_eq!(fx, vec![RoleEffect::Reject(b)]);
    }

    #[test]
    fn source_locked_reflects_pin_state() {
        let (a, _, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        assert!(!s.source_locked(), "source held but not pinned yet");
        let (s, _) = ev(s, RoleEvent::Lock(a));
        assert!(s.source_locked());
        let (s, _) = ev(s, RoleEvent::Unlock(a));
        assert!(!s.source_locked());
    }

    #[test]
    fn focus_gain_after_set_role_dest_makes_the_lister_source_again() {
        // Why the Lister must send a palette "Set Role" only after its window
        // has regained the focus: a focus event arriving later wins.
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest));
        let (s, _) = ev(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source(), Some(b));
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest));
        assert_eq!(s.dests(), vec![b], "sent after the focus, the request sticks");
    }

    #[test]
    fn set_role_none_clears_and_unlocks() {
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::SetRole(b, Role::Dest));
        let (s, _) = ev(s, RoleEvent::Lock(b));
        let (s, fx) = ev(s, RoleEvent::SetRole(b, Role::None));
        assert!(s.dests().is_empty());
        assert!(!s.has_role(b));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn dropping_a_lister_clears_its_role() {
        let (a, b, _) = ids();
        let (s, _) = ev(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = ev(s, RoleEvent::FocusGained(b)); // b source, a dest
        let (s, fx) = ev(s, RoleEvent::Dropped(b));
        assert_eq!(s.source(), None);
        assert_eq!(s.dests(), vec![a]);
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }
}
