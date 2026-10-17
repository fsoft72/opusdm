//! Closing a Lister while a Function it started waits for a program (an
//! editor run with `wait`): ask whether to stop the program, leave it open or
//! keep the window. Without an answer (the window just goes away) the hub
//! leaves the program running.
//!
//! [`CloseGuard`] holds the decision logic, free of GTK; [`ask`] shows the
//! question.

use crate::window::Ui;
use gtk4 as gtk;
use opusdm_ipc::ToHub;
use std::collections::BTreeMap;
use std::rc::Rc;

/// What a close request of the window must do.
#[derive(Debug, PartialEq, Eq)]
pub enum CloseStep {
    /// Close now.
    Proceed,
    /// Ask the user first, naming these programs.
    Ask(Vec<String>),
    /// The question is already on screen: do not close, do not ask again.
    Wait,
}

/// The answer to the question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Close; the program keeps running and the rest of the Function is skipped.
    LeaveOpen,
    /// Close and kill the program.
    StopProgram,
    /// Keep the window open.
    Cancel,
}

/// The programs Functions of this Lister wait on, and the state of a close.
#[derive(Default)]
pub struct CloseGuard {
    /// Run id to the program it waits on.
    waiting: BTreeMap<u64, String>,
    /// The window may close without asking (an answer was given, or the
    /// close does not come from the user).
    decided: bool,
    /// The question is on screen.
    asking: bool,
}

impl CloseGuard {
    /// Run `run_id` waits for `program`.
    pub fn waiting(&mut self, run_id: u64, program: String) {
        self.waiting.insert(run_id, program);
    }

    /// Run `run_id` no longer waits.
    pub fn wait_ended(&mut self, run_id: u64) {
        self.waiting.remove(&run_id);
    }

    /// The next close request goes on without asking: the hub shuts the
    /// Lister down or is gone, or the user answered.
    pub fn allow_close(&mut self) {
        self.decided = true;
        self.asking = false;
    }

    /// The user chose to keep the window: a later close asks again.
    pub fn keep_open(&mut self) {
        self.asking = false;
    }

    /// A close request arrived: what to do with it.
    pub fn on_close_request(&mut self) -> CloseStep {
        if self.decided || self.waiting.is_empty() { return CloseStep::Proceed; }
        if self.asking { return CloseStep::Wait; }
        self.asking = true;
        CloseStep::Ask(self.waiting.values().cloned().collect())
    }
}

/// The question text for `programs`.
fn question(programs: &[String]) -> String {
    match programs {
        [one] => format!("\"{one}\", started by a function of this Lister, is still open."),
        many => {
            let names: Vec<String> = many.iter().map(|p| format!("\"{p}\"")).collect();
            format!("{}, started by functions of this Lister, are still open.", names.join(", "))
        }
    }
}

/// Ask what to do with `programs` before `ui` closes, and act on the answer.
/// The requester always answers once (closing it is Cancel).
pub fn ask(ui: &Rc<Ui>, programs: &[String]) {
    let weak = Rc::downgrade(ui);
    let detail = "Leave Open: the program keeps running and the rest of the function is skipped.\n\
                  Stop Program: the program is closed now; what is not saved in it is lost.";
    opusdm_ui::requester(
        &ui.window,
        "Close Lister",
        &question(programs),
        Some(detail),
        &[("Leave Open", Answer::LeaveOpen), ("Stop Program", Answer::StopProgram), ("Cancel", Answer::Cancel)],
        move |answer| {
            let Some(ui) = weak.upgrade() else {
                opusdm_ipc::err_log!("opusdm-lister", "close question answered after the Lister window was gone");
                return;
            };
            on_answer(&ui, answer);
        },
    );
}

/// Act on `answer`: stop the programs (through the hub) and close, close, or
/// keep the window.
fn on_answer(ui: &Rc<Ui>, answer: Answer) {
    use gtk::prelude::*;
    if answer == Answer::Cancel {
        ui.close_guard.borrow_mut().keep_open();
        return;
    }
    if answer == Answer::StopProgram {
        if let Err(e) = ui.hub.client.send(&ToHub::StopFunctions) {
            opusdm_ipc::err_log!("opusdm-lister", "cannot ask the hub to stop the programs, they stay open: {e}");
        }
    }
    ui.close_guard.borrow_mut().allow_close();
    ui.window.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_waiting_closes_at_once() {
        let mut g = CloseGuard::default();
        assert_eq!(g.on_close_request(), CloseStep::Proceed);
        g.waiting(3, "gedit".into());
        g.wait_ended(3);
        assert_eq!(g.on_close_request(), CloseStep::Proceed, "the program already ended");
    }

    #[test]
    fn a_waiting_program_is_asked_about_once_until_answered() {
        let mut g = CloseGuard::default();
        g.waiting(3, "gedit".into());
        g.waiting(5, "xterm".into());
        assert_eq!(g.on_close_request(), CloseStep::Ask(vec!["gedit".into(), "xterm".into()]));
        assert_eq!(g.on_close_request(), CloseStep::Wait, "a second X click does not open a second question");
        g.keep_open();
        assert!(matches!(g.on_close_request(), CloseStep::Ask(_)), "after Cancel the next close asks again");
        g.allow_close();
        assert_eq!(g.on_close_request(), CloseStep::Proceed, "after an answer the window closes");
    }

    #[test]
    fn a_close_not_from_the_user_never_asks() {
        let mut g = CloseGuard::default();
        g.waiting(3, "gedit".into());
        g.allow_close();
        assert_eq!(g.on_close_request(), CloseStep::Proceed);
    }

    #[test]
    fn the_question_names_the_programs() {
        assert_eq!(question(&["gedit".into()]), "\"gedit\", started by a function of this Lister, is still open.");
        assert!(question(&["a".into(), "b".into()]).starts_with("\"a\", \"b\", started by functions"));
    }
}
