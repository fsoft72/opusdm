//! The hybrid SOURCE/DEST role machine. Pure: no sockets, no GTK.
//!
//! Roles are global singletons - at most one SOURCE and one DEST across all
//! Listers, either possibly unset. Focus auto-assigns; explicit buttons force;
//! a per-Lister lock pins a role against focus changes.

use opusdm_ipc::{ListerId, Role};
use std::collections::BTreeSet;

/// Current role assignment plus the set of Listers whose role is pinned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleState {
    /// The Lister that Copy/Move read from.
    pub source: Option<ListerId>,
    /// The Lister that Copy/Move write into.
    pub dest: Option<ListerId>,
    /// Listers that pinned their current role.
    pub locked: BTreeSet<ListerId>,
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
    let before = (s.source, s.dest);

    match ev {
        RoleEvent::Lock(id) => {
            s.locked.insert(id);
            return (s, Vec::new());
        }
        RoleEvent::Unlock(id) => {
            s.locked.remove(&id);
            return (s, Vec::new());
        }
        RoleEvent::Dropped(id) => {
            s.locked.remove(&id);
            if s.source == Some(id) {
                s.source = None;
            }
            if s.dest == Some(id) {
                s.dest = None;
            }
        }
        RoleEvent::FocusGained(id) => {
            if s.locked.contains(&id) || s.source == Some(id) {
                return (s, Vec::new());
            }
            // A locked SOURCE cannot be displaced by focus.
            if s.source.is_some_and(|cur| s.locked.contains(&cur)) {
                return (s, Vec::new());
            }
            let old_source = s.source;
            s.source = Some(id);
            if s.dest == Some(id) {
                s.dest = None;
            }
            if let Some(os) = old_source {
                if !s.locked.contains(&os) {
                    s.dest = Some(os);
                }
            }
        }
        RoleEvent::SetRole(id, role) => {
            let holder = match role {
                Role::Source => s.source,
                Role::Dest => s.dest,
                Role::None => None,
            };
            if let Some(h) = holder {
                if h != id && s.locked.contains(&h) {
                    return (s, vec![RoleEffect::Reject(id)]);
                }
            }
            if s.source == Some(id) {
                s.source = None;
            }
            if s.dest == Some(id) {
                s.dest = None;
            }
            match role {
                Role::Source => s.source = Some(id),
                Role::Dest => s.dest = Some(id),
                Role::None => {}
            }
        }
    }

    if (s.source, s.dest) == before {
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

    #[test]
    fn first_focus_becomes_source() {
        let (a, _, _) = ids();
        let (s, fx) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        assert_eq!(s.source, Some(a));
        assert_eq!(s.dest, None);
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn second_focus_demotes_previous_source_to_dest() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source, Some(b));
        assert_eq!(s.dest, Some(a));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn refocusing_current_source_is_a_noop() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::FocusGained(b));
        assert_eq!(s2, s);
        assert!(fx.is_empty());
    }

    #[test]
    fn locked_holder_is_not_demoted_by_focus() {
        let (a, b, c) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::Lock(a));
        assert!(fx.is_empty());
        let (s, _fx) = apply_role_event(s, RoleEvent::FocusGained(c));
        assert_eq!(s.source, Some(a), "locked source keeps its role");
        let (s, _fx) = apply_role_event(s, RoleEvent::FocusGained(b));
        assert_eq!(s.source, Some(a));
    }

    #[test]
    fn set_role_forces_dest() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, fx) = apply_role_event(s, RoleEvent::SetRole(b, Role::Dest));
        assert_eq!(s.source, Some(a));
        assert_eq!(s.dest, Some(b));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn set_role_colliding_with_locked_holder_is_rejected() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::Lock(a));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::SetRole(b, Role::Source));
        assert_eq!(s2, s, "state unchanged");
        assert_eq!(fx, vec![RoleEffect::Reject(b)]);
    }

    #[test]
    fn set_role_none_clears_only_that_lister() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b));
        let (s, fx) = apply_role_event(s, RoleEvent::SetRole(b, Role::None));
        assert_eq!(s.source, None);
        assert_eq!(s.dest, Some(a));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn dropping_a_lister_clears_its_roles_and_lock() {
        let (a, b, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::FocusGained(b));
        let (s, _) = apply_role_event(s, RoleEvent::Lock(b));
        let (s, fx) = apply_role_event(s, RoleEvent::Dropped(b));
        assert_eq!(s.source, None);
        assert_eq!(s.dest, Some(a));
        assert!(!s.locked.contains(&b));
        assert_eq!(fx, vec![RoleEffect::Broadcast]);
    }

    #[test]
    fn unlock_alone_does_not_move_roles() {
        let (a, _, _) = ids();
        let (s, _) = apply_role_event(RoleState::default(), RoleEvent::FocusGained(a));
        let (s, _) = apply_role_event(s, RoleEvent::Lock(a));
        let (s2, fx) = apply_role_event(s.clone(), RoleEvent::Unlock(a));
        assert_eq!(s2.source, s.source);
        assert_eq!(s2.dest, s.dest);
        assert!(fx.is_empty());
    }
}
