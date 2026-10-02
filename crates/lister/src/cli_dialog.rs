//! The internal command line prompt (F-42): "Command Line..." in the palette.

use crate::window::Ui;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::cli::{self, CliCommand};
use opusdm_ipc::function;
use opusdm_ipc::ToHub;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// The last line that ran, offered again next time.
    static LAST_LINE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Parse `line` against the saved functions and turn it into the request to
/// send to the hub.
pub fn request_for(line: &str, functions: &[function::Function]) -> Result<ToHub, String> {
    let known: Vec<(String, String)> = functions.iter().map(|f| (f.id.clone(), f.name.clone())).collect();
    match cli::parse(line, &known)? {
        CliCommand::Function(id) => Ok(ToHub::RunFunctionById { id }),
        other => Ok(ToHub::RunFunction { steps: other.steps().expect("shell and internal commands have steps") }),
    }
}

/// Open the prompt.
pub fn open_command_line(ui: &Rc<Ui>) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Command Line").size(560, 170);
    let root = shell.content();
    let hint = gtk::Label::new(Some("Internal command (MakeDir backup, CopyAs new_name=*.bak), a saved function, or !shell command"));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    root.append(&hint);
    let entry = gtk::Entry::new();
    entry.set_text(&LAST_LINE.with(|l| l.borrow().clone()));
    entry.set_activates_default(true);
    root.append(&entry);
    let error = gtk::Label::new(None);
    error.set_xalign(0.0);
    error.add_css_class("error");
    root.append(&error);

    let run_btn = shell.button("Run");
    shell.button_spacer();
    shell.close_button("Close");
    let window = shell.present();
    window.set_default_widget(Some(&run_btn));
    entry.grab_focus();
    entry.select_region(0, -1);

    let ui = ui.clone();
    run_btn.connect_clicked(move |_| {
        let line = entry.text().to_string();
        match request_for(&line, &ui.hub.functions.borrow()) {
            Ok(request) => {
                LAST_LINE.with(|l| *l.borrow_mut() = line);
                ui.start_run(request);
                window.close();
            }
            Err(message) => error.set_text(&message),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(name: &str) -> function::Function {
        let mut f = function::Function::new(name);
        f.id = format!("id-{name}");
        f
    }

    #[test]
    fn lines_become_the_matching_hub_request() {
        let functions = [saved("Tidy")];
        assert!(matches!(request_for("tidy", &functions), Ok(ToHub::RunFunctionById { id }) if id == "id-Tidy"));
        assert!(matches!(request_for("!ls", &functions), Ok(ToHub::RunFunction { steps }) if steps.len() == 1));
        assert!(matches!(request_for("MakeDir x", &functions), Ok(ToHub::RunFunction { .. })));
        assert!(request_for("MakeDir", &functions).is_err());
        assert!(request_for("", &functions).is_err());
    }
}
