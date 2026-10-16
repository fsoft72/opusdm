//! Asking before unsaved changes are lost: when a window closes
//! ([`DirtyGuard`]) or before something replaces them ([`confirm_discard`]).

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Title of the question about unsaved changes.
const UNSAVED_TITLE: &str = "Unsaved Changes";

/// Whether a close may go on: the user already agreed, or nothing is unsaved.
fn may_close(allowed: bool, dirty: bool) -> bool {
    allowed || !dirty
}

/// Asks before a window with unsaved changes closes.
pub struct DirtyGuard {
    /// Set once the next close may go on without asking.
    allowed: Rc<Cell<bool>>,
}

impl DirtyGuard {
    /// Guard `window`: while `is_dirty()` is true a close asks `question`
    /// (with `detail` below it) and goes on only after Discard. `on_close`
    /// runs when the close goes on.
    pub fn attach(
        window: &impl IsA<gtk::Window>,
        question: &str,
        detail: Option<&str>,
        is_dirty: impl Fn() -> bool + 'static,
        on_close: impl Fn() + 'static,
    ) -> Self {
        let allowed = Rc::new(Cell::new(false));
        let (question, detail) = (question.to_string(), detail.map(str::to_string));
        let flag = allowed.clone();
        window.as_ref().connect_close_request(move |w| {
            if may_close(flag.get(), is_dirty()) {
                on_close();
                return glib::Propagation::Proceed;
            }
            let (flag, w2) = (flag.clone(), w.clone());
            crate::requester(w, UNSAVED_TITLE, &question, detail.as_deref(), &[("Discard", true), ("Cancel", false)], move |discard| {
                if !discard { return; }
                flag.set(true);
                w2.close();
            });
            glib::Propagation::Stop
        });
        Self { allowed }
    }

    /// Let the next close go on without asking.
    pub fn allow_close(&self) {
        self.allowed.set(true);
    }
}

/// Run `proceed` at once when `dirty` is false, otherwise after the user
/// agrees to discard the unsaved changes (`question` says which); `cancel`
/// runs when they decline.
pub fn confirm_discard(parent: &impl IsA<gtk::Window>, dirty: bool, question: &str, proceed: impl Fn() + 'static, cancel: impl Fn() + 'static) {
    if !dirty { return proceed(); }
    crate::requester(parent, UNSAVED_TITLE, question, None, &[("Discard", true), ("Cancel", false)], move |discard| {
        if discard { proceed() } else { cancel() }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_dirty_window_not_yet_allowed_asks() {
        assert!(may_close(false, false));
        assert!(may_close(true, true));
        assert!(!may_close(false, true));
    }
}
