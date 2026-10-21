//! The contract of a configuration tool that lives inside another window (the
//! Advanced tab of `opusdm-settings`) and the question asked before the user
//! leaves it with changes that are not saved yet.

use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// A configuration tool shown in the area of a host window. The tool keeps
/// its own Save and Cancel buttons; the host only needs to know whether there
/// is something to lose.
pub trait EmbeddedTool {
    /// Name of the tool, used in the question about unsaved changes.
    fn title(&self) -> &str;

    /// The widget to put in the host window.
    fn widget(&self) -> gtk::Widget;

    /// Whether the tool holds changes that were not saved.
    fn is_dirty(&self) -> bool;

    /// Save and apply the changes. `done` gets `true` when they are saved and
    /// `false` when saving failed (the tool shows why and stays dirty).
    fn save(&self, done: Box<dyn FnOnce(bool)>);

    /// Drop the unsaved changes and put the previous values back.
    fn revert(&self);
}

/// What the user picked in the question about unsaved changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeaveChoice {
    Save,
    Discard,
    Stay,
}

/// What to do after the choice.
#[derive(Debug, PartialEq, Eq)]
enum LeaveStep {
    /// Leave now.
    Proceed,
    /// Save first, leave only if it worked.
    SaveThenProceed,
    /// Revert first, then leave.
    RevertThenProceed,
    /// Stay where we are.
    Stay,
}

/// The step for a tool that is `dirty` (or not) and a `choice` (`None` while
/// nothing was asked, which is the case when nothing is dirty).
fn _leave_step(dirty: bool, choice: Option<LeaveChoice>) -> LeaveStep {
    if !dirty { return LeaveStep::Proceed; }
    match choice {
        Some(LeaveChoice::Save) => LeaveStep::SaveThenProceed,
        Some(LeaveChoice::Discard) => LeaveStep::RevertThenProceed,
        Some(LeaveChoice::Stay) | None => LeaveStep::Stay,
    }
}

/// Run `proceed` at once when `tool` has nothing unsaved. Otherwise ask
/// whether to save and apply (Save), put the previous values back (Discard) or
/// stay (Cancel, Esc, or a failed save). `proceed` runs once the tool is
/// saved or reverted; `cancel` runs when the user stays.
pub fn confirm_leave(parent: &impl IsA<gtk::Window>, tool: &Rc<dyn EmbeddedTool>, proceed: impl Fn() + 'static, cancel: impl Fn() + 'static) {
    if _leave_step(tool.is_dirty(), None) == LeaveStep::Proceed { return proceed(); }
    let message = format!("{} has changes that are not saved. Save and apply them?", tool.title());
    let tool = tool.clone();
    let (proceed, cancel) = (Rc::new(proceed), Rc::new(cancel));
    crate::requester(parent, "Unsaved Changes", &message, None, &[("Save", LeaveChoice::Save), ("Discard", LeaveChoice::Discard), ("Cancel", LeaveChoice::Stay)], move |choice| {
        match _leave_step(true, Some(choice)) {
            LeaveStep::Proceed => proceed(),
            LeaveStep::SaveThenProceed => {
                let (proceed, cancel) = (proceed.clone(), cancel.clone());
                tool.save(Box::new(move |saved| if saved { proceed() } else { cancel() }));
            }
            LeaveStep::RevertThenProceed => {
                tool.revert();
                proceed();
            }
            LeaveStep::Stay => cancel(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_with_nothing_unsaved_is_left_without_asking() {
        assert_eq!(_leave_step(false, None), LeaveStep::Proceed);
    }

    #[test]
    fn save_discard_and_cancel_lead_to_their_steps() {
        assert_eq!(_leave_step(true, Some(LeaveChoice::Save)), LeaveStep::SaveThenProceed);
        assert_eq!(_leave_step(true, Some(LeaveChoice::Discard)), LeaveStep::RevertThenProceed);
        assert_eq!(_leave_step(true, Some(LeaveChoice::Stay)), LeaveStep::Stay);
    }

    #[test]
    fn a_dirty_tool_never_leaves_without_an_answer() {
        assert_eq!(_leave_step(true, None), LeaveStep::Stay);
    }
}
