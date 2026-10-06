//! A control that edits one function reference: a saved Function, a bash
//! command, a program or an internal command (see `opusdm_ipc::funcref`), optionally also "none".
//! Shared by the hotkey, user menu, script event, button bank and file type
//! editors.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::funcref::{self, FuncRef};
use opusdm_ipc::function::{schema, Function, FunctionId, InternalCmd};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// What a function reference points at; the order of the kind drop-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// Nothing (optional slots only); the reference is empty.
    None,
    /// A saved Function.
    Function,
    /// A bash command.
    Bash,
    /// A program started directly.
    Exec,
    /// An internal command, with its arguments.
    Internal,
}

/// The kinds a picker offers, in drop-down order.
fn kinds(allow_none: bool) -> Vec<RefKind> {
    let mut all = Vec::new();
    if allow_none {
        all.push(RefKind::None);
    }
    all.extend([RefKind::Function, RefKind::Bash, RefKind::Exec, RefKind::Internal]);
    all
}

impl RefKind {
    /// The drop-down text; `none_label` names the "nothing" kind.
    fn label<'a>(self, none_label: &'a str) -> &'a str {
        match self {
            RefKind::None => none_label,
            RefKind::Function => "Function",
            RefKind::Bash => "Bash",
            RefKind::Exec => "Exec",
            RefKind::Internal => "Internal",
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
    /// The chosen internal command; `None` until one is picked (or when a
    /// stored reference names an unknown one).
    internal: Option<InternalCmd>,
    /// The arguments of the internal command, as typed on a command line.
    internal_args: String,
}

impl Draft {
    /// The draft for a stored reference string. An empty one is the `None`
    /// kind when `allow_none`, else an unchosen Function.
    fn from_reference(reference: &str, allow_none: bool) -> Draft {
        let mut draft = Draft { kind: RefKind::Function, function: String::new(), bash: String::new(), show_output: false, exec: String::new(), internal: None, internal_args: String::new() };
        if reference.is_empty() && allow_none {
            draft.kind = RefKind::None;
            return draft;
        }
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
            FuncRef::Internal { line } => {
                draft.kind = RefKind::Internal;
                let line = line.trim_start();
                let (head, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                draft.internal = opusdm_ipc::cli::find_command(head);
                draft.internal_args = if draft.internal.is_some() { rest.trim().to_string() } else { line.to_string() };
            }
        }
        draft
    }

    /// The reference string for the chosen kind (empty for `None`).
    fn reference(&self) -> String {
        match self.kind {
            RefKind::None => String::new(),
            RefKind::Function => self.function.clone(),
            RefKind::Bash => funcref::format(&FuncRef::Bash { cmd: self.bash.clone(), show_output: self.show_output }),
            RefKind::Exec => funcref::format(&FuncRef::Exec { line: self.exec.clone() }),
            RefKind::Internal => funcref::format(&FuncRef::Internal { line: self.internal_line() }),
        }
    }

    /// The command line of the internal kind: the command name and its
    /// arguments (empty when no command is chosen and nothing was typed).
    fn internal_line(&self) -> String {
        let name = self.internal.map(|cmd| format!("{cmd:?}")).unwrap_or_default();
        format!("{name} {}", self.internal_args).trim().to_string()
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

/// The position of `selected` (a function id) in `functions`, or the "nothing
/// selected" position when it is not there: a dangling reference.
fn function_position(functions: &[Function], selected: &str) -> u32 {
    functions.iter().position(|f| f.id == selected).map(|p| p as u32).unwrap_or(gtk::INVALID_LIST_POSITION)
}

/// Add a search field at the top of the popup of a drop-down built with
/// `DropDown::from_strings`: typing filters the entries.
fn enable_filter(dropdown: &gtk::DropDown) {
    dropdown.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), gtk::Expression::NONE, "string")));
    dropdown.set_enable_search(true);
}

/// A drop-down listing every function by name, with `selected` (a function
/// id) pre-selected. Nothing is selected when the id is not in the list: a
/// dangling reference, which the editors flag as an error.
pub fn function_dropdown(functions: &[Function], selected: &str) -> gtk::DropDown {
    let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&names);
    dropdown.set_selected(function_position(functions, selected));
    enable_filter(&dropdown);
    dropdown
}

/// The id of the function chosen in `dropdown`, or `None` when nothing is
/// selected.
pub fn selected_function(dropdown: &gtk::DropDown, functions: &[Function]) -> Option<FunctionId> {
    functions.get(dropdown.selected() as usize).map(|f| f.id.clone())
}

/// The widgets and state of one picker.
struct Inner {
    draft: RefCell<Draft>,
    /// True while the widgets are being filled from a reference, so the
    /// handlers do not report it back.
    loading: Cell<bool>,
    allow_none: bool,
    functions: Vec<Function>,
    kind_dd: gtk::DropDown,
    function_dd: gtk::DropDown,
    bash_entry: gtk::Entry,
    output: gtk::CheckButton,
    exec_entry: gtk::Entry,
    browse: gtk::Button,
    internal_dd: gtk::DropDown,
    internal_entry: gtk::Entry,
}

impl Inner {
    /// Show only the widgets of `kind`.
    fn show_kind(&self, kind: RefKind) {
        self.function_dd.set_visible(kind == RefKind::Function);
        self.bash_entry.set_visible(kind == RefKind::Bash);
        self.output.set_visible(kind == RefKind::Bash);
        self.exec_entry.set_visible(kind == RefKind::Exec);
        self.browse.set_visible(kind == RefKind::Exec);
        self.internal_dd.set_visible(kind == RefKind::Internal);
        self.show_internal_args();
    }

    /// Show the arguments entry only for an internal command that has
    /// parameters.
    fn show_internal_args(&self) {
        let draft = self.draft.borrow();
        let takes_args = draft.internal.map(|cmd| !schema::params_for(cmd).is_empty()).unwrap_or(true);
        self.internal_entry.set_visible(draft.kind == RefKind::Internal && takes_args);
    }

    /// Fill every widget from the draft, without reporting any change.
    fn load(&self) {
        self.loading.set(true);
        let draft = self.draft.borrow();
        let position = kinds(self.allow_none).iter().position(|k| *k == draft.kind).unwrap_or(0);
        self.kind_dd.set_selected(position as u32);
        self.function_dd.set_selected(function_position(&self.functions, &draft.function));
        self.bash_entry.set_text(&draft.bash);
        self.output.set_active(draft.show_output);
        self.exec_entry.set_text(&draft.exec);
        let internal_position = draft.internal.and_then(|cmd| InternalCmd::ALL.iter().position(|c| *c == cmd));
        self.internal_dd.set_selected(internal_position.map(|p| p as u32).unwrap_or(gtk::INVALID_LIST_POSITION));
        self.internal_entry.set_text(&draft.internal_args);
        let kind = draft.kind;
        drop(draft);
        self.show_kind(kind);
        self.loading.set(false);
    }
}

/// A control that edits one function reference: a kind drop-down (Function,
/// Bash, Exec, and "none" for an optional slot) followed by the matching
/// fields. It reports the new reference string after every edit.
pub struct RefPicker {
    root: gtk::Box,
    inner: Rc<Inner>,
}

impl RefPicker {
    /// A picker showing `current`. `none_label` is `Some(text)` for an
    /// optional slot: the first kind is then `text`, and the empty reference
    /// means nothing. `parent` owns the Browse dialog. `on_change` gets the
    /// new reference after every user edit (not after `set_reference`).
    pub fn new(functions: &[Function], current: &str, parent: &gtk::Window, none_label: Option<&str>, on_change: impl Fn(String) + 'static) -> RefPicker {
        let allow_none = none_label.is_some();
        let none_label = none_label.unwrap_or("").to_string();
        let on_change: Rc<dyn Fn(String)> = Rc::new(on_change);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let kind_list = kinds(allow_none);
        let labels: Vec<&str> = kind_list.iter().map(|k| k.label(&none_label)).collect();
        let kind_dd = gtk::DropDown::from_strings(&labels);
        root.append(&kind_dd);
        let function_dd = function_dropdown(functions, "");
        function_dd.set_hexpand(true);
        root.append(&function_dd);
        let bash_entry = gtk::Entry::new();
        bash_entry.set_hexpand(true);
        bash_entry.set_placeholder_text(Some("bash command"));
        let output = gtk::CheckButton::with_label("Show output");
        root.append(&bash_entry);
        root.append(&output);
        let exec_entry = gtk::Entry::new();
        exec_entry.set_hexpand(true);
        exec_entry.set_placeholder_text(Some("program and arguments"));
        let browse = gtk::Button::with_label("Browse...");
        root.append(&exec_entry);
        root.append(&browse);
        let command_names: Vec<&str> = InternalCmd::ALL.iter().map(|c| c.label()).collect();
        let internal_dd = gtk::DropDown::from_strings(&command_names);
        enable_filter(&internal_dd);
        root.append(&internal_dd);
        let internal_entry = gtk::Entry::new();
        internal_entry.set_hexpand(true);
        internal_entry.set_placeholder_text(Some("arguments, e.g. name=value"));
        root.append(&internal_entry);

        let inner = Rc::new(Inner {
            draft: RefCell::new(Draft::from_reference(current, allow_none)),
            loading: Cell::new(false),
            allow_none,
            functions: functions.to_vec(),
            kind_dd,
            function_dd,
            bash_entry,
            output,
            exec_entry,
            browse,
            internal_dd,
            internal_entry,
        });
        inner.load();

        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            let kind_list = kind_list.clone();
            inner.clone().kind_dd.connect_selected_notify(move |d| {
                let Some(kind) = kind_list.get(d.selected() as usize).copied() else { return };
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().kind = kind;
                inner.show_kind(kind);
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().function_dd.connect_selected_notify(move |d| {
                let Some(id) = selected_function(d, &inner.functions) else { return };
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().function = id;
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().bash_entry.connect_changed(move |e| {
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().bash = e.text().to_string();
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().output.connect_toggled(move |c| {
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().show_output = c.is_active();
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().exec_entry.connect_changed(move |e| {
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().exec = e.text().to_string();
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().internal_dd.connect_selected_notify(move |d| {
                let Some(cmd) = InternalCmd::ALL.get(d.selected() as usize).copied() else { return };
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().internal = Some(cmd);
                inner.show_internal_args();
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, on_change) = (inner.clone(), on_change.clone());
            inner.clone().internal_entry.connect_changed(move |e| {
                if inner.loading.get() {
                    return;
                }
                inner.draft.borrow_mut().internal_args = e.text().to_string();
                on_change(inner.draft.borrow().reference());
            });
        }
        {
            let (inner, parent) = (inner.clone(), parent.clone());
            inner.clone().browse.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::builder().title("Choose a program").build();
                let inner = inner.clone();
                dialog.open(Some(&parent), gio::Cancellable::NONE, move |res| {
                    let Some(path) = res.ok().and_then(|file| file.path()) else { return };
                    // Setting the text fires `changed`, which updates the draft.
                    inner.exec_entry.set_text(&browsed_line(&inner.exec_entry.text(), &path));
                });
            });
        }
        RefPicker { root, inner }
    }

    /// The widget to put in a form.
    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Show `reference`, replacing every kind's draft. Does not call
    /// `on_change`.
    pub fn set_reference(&self, reference: &str) {
        *self.inner.draft.borrow_mut() = Draft::from_reference(reference, self.inner.allow_none);
        self.inner.load();
    }

    /// Enable or disable the whole picker.
    pub fn set_sensitive(&self, on: bool) {
        self.root.set_sensitive(on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_reference_selects_the_matching_kind_and_keeps_every_draft() {
        let d = Draft::from_reference("bash+out:ls -l", false);
        assert_eq!((d.kind, d.bash.as_str(), d.show_output), (RefKind::Bash, "ls -l", true));
        assert_eq!(d.reference(), "bash+out:ls -l");
        let d = Draft::from_reference("exec:kitty --x", false);
        assert_eq!((d.kind, d.exec.as_str()), (RefKind::Exec, "kitty --x"));
        let d = Draft::from_reference("fn-1-2", false);
        assert_eq!((d.kind, d.function.as_str()), (RefKind::Function, "fn-1-2"));
    }

    #[test]
    fn switching_kind_keeps_what_was_typed_for_the_others() {
        let mut d = Draft::from_reference("bash:ls", false);
        d.kind = RefKind::Exec;
        d.exec = "kitty".into();
        assert_eq!(d.reference(), "exec:kitty");
        d.kind = RefKind::Bash;
        assert_eq!(d.reference(), "bash:ls");
        d.kind = RefKind::Function;
        assert_eq!(d.reference(), "", "no function chosen yet");
    }

    #[test]
    fn an_empty_reference_is_none_only_when_none_is_allowed() {
        let d = Draft::from_reference("", true);
        assert_eq!((d.kind, d.reference().as_str()), (RefKind::None, ""));
        let d = Draft::from_reference("", false);
        assert_eq!(d.kind, RefKind::Function, "a required slot has no none kind");
    }

    #[test]
    fn none_keeps_the_other_drafts_and_gives_an_empty_reference() {
        let mut d = Draft::from_reference("bash:ls", true);
        d.kind = RefKind::None;
        assert_eq!(d.reference(), "");
        d.kind = RefKind::Bash;
        assert_eq!(d.reference(), "bash:ls");
    }

    #[test]
    fn kinds_start_with_none_only_when_allowed() {
        assert_eq!(kinds(false), vec![RefKind::Function, RefKind::Bash, RefKind::Exec, RefKind::Internal]);
        assert_eq!(kinds(true), vec![RefKind::None, RefKind::Function, RefKind::Bash, RefKind::Exec, RefKind::Internal]);
    }

    #[test]
    fn an_internal_reference_splits_into_command_and_arguments() {
        let d = Draft::from_reference("internal:CopyAs new_name=*.bak", false);
        assert_eq!((d.kind, d.internal, d.internal_args.as_str()), (RefKind::Internal, Some(InternalCmd::CopyAs), "new_name=*.bak"));
        assert_eq!(d.reference(), "internal:CopyAs new_name=*.bak");
        let d = Draft::from_reference("internal:Hide", false);
        assert_eq!((d.internal, d.internal_args.as_str()), (Some(InternalCmd::Hide), ""));
        assert_eq!(d.reference(), "internal:Hide");
        let d = Draft::from_reference("internal:frobnicate x", false);
        assert_eq!(d.internal, None, "an unknown command is kept as typed");
        assert_eq!(d.reference(), "internal:frobnicate x");
        assert_eq!(Draft::from_reference("internal:", false).reference(), "internal:");
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
