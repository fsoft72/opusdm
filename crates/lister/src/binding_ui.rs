//! Small widgets shared by the hotkey, user-menu and script-event editors.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::funcref::{self, FuncRef};
use opusdm_ipc::function::{Function, FunctionId};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// A drop-down listing every function by name, with `selected` (a function
/// id) pre-selected. Nothing is selected when the id is not in the list: a
/// dangling reference, which the editors flag as an error.
pub fn function_dropdown(functions: &[Function], selected: &str) -> gtk::DropDown {
    let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    let position = functions
        .iter()
        .position(|f| f.id == selected)
        .map(|p| p as u32)
        .unwrap_or(gtk::INVALID_LIST_POSITION);
    dropdown.set_selected(position);
    dropdown
}

/// The id of the function chosen in `dropdown`, or `None` when nothing is
/// selected.
pub fn selected_function(dropdown: &gtk::DropDown, functions: &[Function]) -> Option<FunctionId> {
    functions.get(dropdown.selected() as usize).map(|f| f.id.clone())
}

/// What a function reference points at; the order of the kind drop-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A saved Function.
    Function,
    /// A bash command.
    Bash,
    /// A program started directly.
    Exec,
}

impl RefKind {
    /// Every kind, in drop-down order.
    pub const ALL: [RefKind; 3] = [RefKind::Function, RefKind::Bash, RefKind::Exec];

    /// The drop-down text.
    pub fn label(self) -> &'static str {
        match self {
            RefKind::Function => "Function",
            RefKind::Bash => "Bash",
            RefKind::Exec => "Exec",
        }
    }
}

/// What a picker holds: the chosen kind plus the last value of every kind, so
/// switching kind and back loses nothing.
struct Draft {
    kind: RefKind,
    function: FunctionId,
    bash: String,
    show_output: bool,
    exec: String,
}

impl Draft {
    /// The draft for a stored reference string.
    fn from_reference(reference: &str) -> Draft {
        let mut draft = Draft { kind: RefKind::Function, function: String::new(), bash: String::new(), show_output: false, exec: String::new() };
        match funcref::parse(reference) {
            FuncRef::Saved(id) => draft.function = id,
            FuncRef::Bash { cmd, show_output } => {
                draft.kind = RefKind::Bash;
                draft.bash = cmd;
                draft.show_output = show_output;
            }
            FuncRef::Exec { line } => {
                draft.kind = RefKind::Exec;
                draft.exec = line;
            }
        }
        draft
    }

    /// The reference string for the chosen kind.
    fn reference(&self) -> String {
        match self.kind {
            RefKind::Function => self.function.clone(),
            RefKind::Bash => funcref::format(&FuncRef::Bash { cmd: self.bash.clone(), show_output: self.show_output }),
            RefKind::Exec => funcref::format(&FuncRef::Exec { line: self.exec.clone() }),
        }
    }
}

/// Whether `word` must be quoted to stay one word in an exec line.
fn needs_quote(word: &str) -> bool {
    word.is_empty() || word.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
}

/// `current` (an exec line) with its program replaced by `program`, keeping
/// the arguments. A line that does not parse loses its arguments.
pub fn browsed_line(current: &str, program: &Path) -> String {
    let program = program.to_string_lossy();
    let mut words = vec![if needs_quote(&program) { funcref::quote(&program) } else { program.to_string() }];
    if let Ok(old) = opusdm_ipc::cli::tokenize(current) {
        words.extend(old.iter().skip(1).map(|w| if needs_quote(w) { funcref::quote(w) } else { w.clone() }));
    }
    words.join(" ")
}

/// A control that edits one function reference: a kind drop-down (Function,
/// Bash, Exec) followed by the matching fields. `current` is the stored
/// string; `on_change` gets the new string after every edit. `parent` owns
/// the Browse dialog.
pub fn reference_picker(functions: &[Function], current: &str, parent: &gtk::Window, on_change: impl Fn(String) + 'static) -> gtk::Box {
    let draft = Rc::new(RefCell::new(Draft::from_reference(current)));
    let on_change: Rc<dyn Fn(String)> = Rc::new(on_change);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let kinds: Vec<&str> = RefKind::ALL.iter().map(|k| k.label()).collect();
    let kind_dd = gtk::DropDown::from_strings(&kinds);
    kind_dd.set_selected(RefKind::ALL.iter().position(|k| *k == draft.borrow().kind).unwrap_or(0) as u32);
    line.append(&kind_dd);

    let function_dd = function_dropdown(functions, &draft.borrow().function);
    function_dd.set_hexpand(true);
    line.append(&function_dd);

    let bash_entry = gtk::Entry::new();
    bash_entry.set_hexpand(true);
    bash_entry.set_placeholder_text(Some("bash command"));
    bash_entry.set_text(&draft.borrow().bash);
    let output = gtk::CheckButton::with_label("Show output");
    output.set_active(draft.borrow().show_output);
    line.append(&bash_entry);
    line.append(&output);

    let exec_entry = gtk::Entry::new();
    exec_entry.set_hexpand(true);
    exec_entry.set_placeholder_text(Some("program and arguments"));
    exec_entry.set_text(&draft.borrow().exec);
    let browse = gtk::Button::with_label("Browse...");
    line.append(&exec_entry);
    line.append(&browse);

    // Show only the widgets of the chosen kind.
    let show_kind = {
        let (function_dd, bash_entry, output, exec_entry, browse) =
            (function_dd.clone(), bash_entry.clone(), output.clone(), exec_entry.clone(), browse.clone());
        Rc::new(move |kind: RefKind| {
            function_dd.set_visible(kind == RefKind::Function);
            bash_entry.set_visible(kind == RefKind::Bash);
            output.set_visible(kind == RefKind::Bash);
            exec_entry.set_visible(kind == RefKind::Exec);
            browse.set_visible(kind == RefKind::Exec);
        })
    };
    show_kind(draft.borrow().kind);

    {
        let (draft, on_change, show_kind) = (draft.clone(), on_change.clone(), show_kind.clone());
        kind_dd.connect_selected_notify(move |d| {
            let Some(kind) = RefKind::ALL.get(d.selected() as usize).copied() else { return };
            draft.borrow_mut().kind = kind;
            show_kind(kind);
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change, functions) = (draft.clone(), on_change.clone(), functions.to_vec());
        function_dd.connect_selected_notify(move |d| {
            let Some(id) = selected_function(d, &functions) else { return };
            draft.borrow_mut().function = id;
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        bash_entry.connect_changed(move |e| {
            draft.borrow_mut().bash = e.text().to_string();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        output.connect_toggled(move |c| {
            draft.borrow_mut().show_output = c.is_active();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (draft, on_change) = (draft.clone(), on_change.clone());
        exec_entry.connect_changed(move |e| {
            draft.borrow_mut().exec = e.text().to_string();
            on_change(draft.borrow().reference());
        });
    }
    {
        let (entry, parent) = (exec_entry.clone(), parent.clone());
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Choose a program").build();
            let entry = entry.clone();
            dialog.open(Some(&parent), gio::Cancellable::NONE, move |res| {
                let Some(path) = res.ok().and_then(|file| file.path()) else { return };
                // Setting the text fires `changed`, which updates the draft.
                entry.set_text(&browsed_line(&entry.text(), &path));
            });
        });
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_reference_selects_the_matching_kind_and_keeps_every_draft() {
        let d = Draft::from_reference("bash+out:ls -l");
        assert_eq!((d.kind, d.bash.as_str(), d.show_output), (RefKind::Bash, "ls -l", true));
        assert_eq!(d.reference(), "bash+out:ls -l");
        let d = Draft::from_reference("exec:kitty --x");
        assert_eq!((d.kind, d.exec.as_str()), (RefKind::Exec, "kitty --x"));
        let d = Draft::from_reference("fn-1-2");
        assert_eq!((d.kind, d.function.as_str()), (RefKind::Function, "fn-1-2"));
    }

    #[test]
    fn switching_kind_keeps_what_was_typed_for_the_others() {
        let mut d = Draft::from_reference("bash:ls");
        d.kind = RefKind::Exec;
        d.exec = "kitty".into();
        assert_eq!(d.reference(), "exec:kitty");
        d.kind = RefKind::Bash;
        assert_eq!(d.reference(), "bash:ls");
        d.kind = RefKind::Function;
        assert_eq!(d.reference(), "", "no function chosen yet");
    }

    #[test]
    fn browsed_line_replaces_the_program_and_keeps_the_arguments() {
        let prog = Path::new("/usr/bin/kitty");
        assert_eq!(browsed_line("", prog), "/usr/bin/kitty");
        assert_eq!(browsed_line("old --class 'my term'", prog), "/usr/bin/kitty --class 'my term'");
        assert_eq!(browsed_line("old", Path::new("/opt/my app/run")), "'/opt/my app/run'");
        assert_eq!(browsed_line("old 'unclosed", prog), "/usr/bin/kitty", "an unparsable line loses its arguments");
    }
}
