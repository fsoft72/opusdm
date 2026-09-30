//! Hide and reveal: how a Lister window answers `ToLister::SetHidden`. The
//! decision is a pure function so it can be tested without a window. See
//! `docs/superpowers/specs/2026-09-30-hide-reveal-design.md`.

/// What to do with the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideAction {
    /// Leave the window as it is.
    Nothing,
    /// Minimize it.
    Minimize,
    /// Restore it and bring it to the front.
    Restore,
}

/// Decide the answer to `SetHidden(hidden)`. `hidden_by_hub` is whether this
/// window was minimized by an earlier hide, `minimized` whether it is
/// minimized now. Returns the action and the new `hidden_by_hub`.
///
/// A window the user had already minimized is left alone on hide and stays
/// minimized on reveal, so revealing never pops up what the user put away.
pub fn next_step(hidden_by_hub: bool, minimized: bool, hidden: bool) -> (HideAction, bool) {
    if hidden {
        if minimized {
            (HideAction::Nothing, hidden_by_hub)
        } else {
            (HideAction::Minimize, true)
        }
    } else if hidden_by_hub {
        (HideAction::Restore, false)
    } else {
        (HideAction::Nothing, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_a_visible_window_minimizes_it_and_remembers_it_did() {
        assert_eq!(next_step(false, false, true), (HideAction::Minimize, true));
    }

    #[test]
    fn hiding_a_window_the_user_already_minimized_leaves_it_alone_and_does_not_claim_it() {
        assert_eq!(next_step(false, true, true), (HideAction::Nothing, false));
    }

    #[test]
    fn hiding_twice_keeps_the_claim_on_a_window_the_hub_minimized() {
        assert_eq!(next_step(true, true, true), (HideAction::Nothing, true));
    }

    #[test]
    fn revealing_restores_only_what_the_hub_minimized() {
        assert_eq!(next_step(true, true, false), (HideAction::Restore, false));
        assert_eq!(next_step(false, true, false), (HideAction::Nothing, false));
        assert_eq!(next_step(false, false, false), (HideAction::Nothing, false));
    }

    #[test]
    fn revealing_a_window_the_hub_minimized_but_the_user_already_restored_still_presents_it() {
        assert_eq!(next_step(true, false, false), (HideAction::Restore, false));
    }
}
