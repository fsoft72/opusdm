//! The Scripts window: every JavaScript script of `~/.config/opusdm/scripts`
//! (see `opusdm_ipc::scripts`) with New / Edit / Run / Rename / Duplicate /
//! Delete. It runs as its own process (`opusdm-lister --scripts`), linked to
//! the hub without being a Lister, like Manage Functions: the hub runs the
//! scripts (`InternalCmd::Script`) and sends their output back here. The
//! editor itself is `script_editor.rs`.

use crate::client::{Client, Inbound};
use crate::script_editor::{self, Editor};
use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use opusdm_ipc::function::{FnStep, InternalArgs, InternalCmd};
use opusdm_ipc::scripts::{self, ScriptInfo};
use opusdm_ipc::{Resolution, ToHub, ToLister};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// Command line flag that makes the Lister open only the Scripts window.
pub const SCRIPTS_FLAG: &str = "--scripts";

/// Window title.
const TITLE: &str = "Scripts";

/// Start the Scripts window as its own process.
pub fn launch() -> std::io::Result<()> {
    crate::function_editor::launch_tool(SCRIPTS_FLAG)
}

/// Where the output of the running script goes.
struct RunTarget {
    output: gtk::TextBuffer,
    /// The editor that started it, to show the line of an error.
    editor: Option<Weak<Editor>>,
}

/// What the Scripts window shares with its editors.
pub struct Host {
    app: gtk::Application,
    /// The hub link; `None` when the hub is not running (editing still works,
    /// running does not).
    client: Option<Client>,
    pub window: gtk::Window,
    list_box: gtk::ListBox,
    scripts: RefCell<Vec<ScriptInfo>>,
    filter: RefCell<String>,
    output: gtk::TextBuffer,
    output_box: gtk::Expander,
    run_buttons: RefCell<Vec<gtk::Button>>,
    running: RefCell<Option<RunTarget>>,
    editors: RefCell<Vec<Rc<Editor>>>,
    /// Watches the scripts folder, so files changed by hand show up.
    _monitor: RefCell<Option<gio::FileMonitor>>,
    reload_pending: Cell<bool>,
    /// False once the hub connection ended.
    hub_alive: Cell<bool>,
}

impl Host {
    /// Tell the user `text` went wrong.
    pub fn report(&self, parent: &gtk::Window, text: &str) {
        opusdm_ui::requester(parent, TITLE, text, None, &[("OK", ())], |_| {});
    }

    /// Whether a script can be run now (hub reachable, nothing running).
    fn can_run(&self) -> bool {
        self.client.is_some() && self.hub_alive.get() && self.running.borrow().is_none()
    }

    /// Enable the Run buttons of the window and of every editor.
    fn update_run_buttons(&self) {
        let on = self.can_run();
        let tip = if self.client.is_none() || !self.hub_alive.get() { Some("The OpusDM hub is not running") } else { None };
        for button in self.run_buttons.borrow().iter() {
            button.set_sensitive(on);
            button.set_tooltip_text(tip);
        }
        for editor in self.editors.borrow().iter() {
            editor.set_can_run(on, tip);
        }
    }

    /// Ask the hub to run script `name`; with `code` the editor's unsaved text
    /// runs instead of the file. The output goes to `output`.
    pub fn run(self: &Rc<Self>, name: &str, code: Option<String>, args: &str, output: gtk::TextBuffer, editor: Option<&Rc<Editor>>) {
        let Some(client) = &self.client else { return };
        if self.running.borrow().is_some() {
            return;
        }
        let mut step_args = InternalArgs::default();
        step_args.0.insert("script".to_string(), name.to_string());
        if let Some(code) = code {
            step_args.0.insert("code".to_string(), code);
        }
        if !args.trim().is_empty() {
            step_args.0.insert("args".to_string(), args.to_string());
        }
        let steps = vec![FnStep::Internal { cmd: InternalCmd::Script, args: step_args }];
        script_editor::append(&output, &format!("--- running {name} ---\n"), Some(script_editor::TAG_DIM));
        *self.running.borrow_mut() = Some(RunTarget { output, editor: editor.map(Rc::downgrade) });
        self.update_run_buttons();
        if let Err(e) = client.send(&ToHub::RunFunction { steps }) {
            self.run_ended(&format!("lost the connection to the hub: {e}\n"), Some(script_editor::TAG_ERROR));
        }
    }

    /// Write `text` to the output of the running script.
    fn run_output(&self, text: &str, tag: Option<&str>) {
        if let Some(target) = self.running.borrow().as_ref() {
            script_editor::append(&target.output, text, tag);
        }
    }

    /// The running script ended: write `text` and enable Run again.
    fn run_ended(&self, text: &str, tag: Option<&str>) {
        let target = self.running.borrow_mut().take();
        if let Some(target) = target {
            script_editor::append(&target.output, text, tag);
            if tag == Some(script_editor::TAG_ERROR) {
                if let Some(editor) = target.editor.and_then(|e| e.upgrade()) {
                    editor.show_error(text);
                }
            }
        }
        self.update_run_buttons();
    }

    /// The window questions of a running script are asked over: its editor,
    /// else the Scripts window.
    fn prompt_parent(&self) -> gtk::Window {
        let editor = self.running.borrow().as_ref().and_then(|t| t.editor.as_ref().and_then(Weak::upgrade));
        editor.map(|e| e.window.clone()).unwrap_or_else(|| self.window.clone())
    }

    /// Handle one message from the hub: the output and end of a run, and the
    /// questions a running script asks.
    fn handle(self: &Rc<Self>, msg: ToLister) {
        match msg {
            ToLister::FunctionOutput { line, .. } => self.run_output(&format!("{line}\n"), None),
            ToLister::OutputLine { line, .. } => self.run_output(&format!("{line}\n"), None),
            ToLister::FunctionFailed { error, .. } => self.run_ended(&format!("{error}\n"), Some(script_editor::TAG_ERROR)),
            ToLister::FunctionDone => self.run_ended("--- finished ---\n", Some(script_editor::TAG_DIM)),
            ToLister::ConfirmPrompt { req_id, message } => {
                let host = self.clone();
                opusdm_ui::requester(&self.prompt_parent(), "Confirm", &message, None, &[("Yes", true), ("No", false)], move |ok| {
                    host.send(&ToHub::ConfirmReply { req_id, ok });
                });
            }
            ToLister::AlertPrompt { req_id, message } => {
                let host = self.clone();
                opusdm_ui::requester(&self.prompt_parent(), "Alert", &message, None, &[("OK", true)], move |ok| {
                    host.send(&ToHub::ConfirmReply { req_id, ok });
                });
            }
            ToLister::ConflictPrompt { req_id, path, src, dst, .. } => {
                let host = self.clone();
                let detail = format!("source: {} bytes\ndestination: {} bytes", src.size, dst.size);
                opusdm_ui::requester(
                    &self.prompt_parent(),
                    "Request",
                    &format!("{} already exists", path.display()),
                    Some(&detail),
                    &[
                        ("Overwrite", Resolution::Overwrite),
                        ("Skip", Resolution::Skip),
                        ("Overwrite All", Resolution::OverwriteAll),
                        ("Skip All", Resolution::SkipAll),
                        ("Cancel", Resolution::Cancel),
                    ],
                    move |resolution| host.send(&ToHub::ConflictReply { req_id, resolution }),
                );
            }
            _ => {}
        }
    }

    /// Send `msg` to the hub, reporting a failure.
    fn send(&self, msg: &ToHub) {
        let Some(client) = &self.client else { return };
        if let Err(e) = client.send(msg) {
            self.report(&self.window, &format!("lost the connection to the hub: {e}"));
        }
    }

    /// The editor open on script `name`, if any.
    fn editor_of(&self, name: &str) -> Option<Rc<Editor>> {
        self.editors.borrow().iter().find(|e| e.name() == name).cloned()
    }

    /// Open (or raise) the editor of script `name`.
    pub fn edit(self: &Rc<Self>, name: &str) {
        if let Some(editor) = self.editor_of(name) {
            editor.window.present();
            return;
        }
        let code = match scripts::load(name) {
            Ok(code) => code,
            Err(e) => {
                self.report(&self.window, &format!("cannot read script \"{name}\": {e}"));
                return;
            }
        };
        let editor = Editor::open(self, &self.app, name, &code);
        editor.set_can_run(self.can_run(), if self.client.is_none() || !self.hub_alive.get() { Some("The OpusDM hub is not running") } else { None });
        self.editors.borrow_mut().push(editor);
    }

    /// An editor was closed.
    pub fn editor_closed(&self, editor: &Editor) {
        self.editors.borrow_mut().retain(|e| !std::ptr::eq(Rc::as_ptr(e), editor));
    }

    /// Read the scripts folder again and repaint the list, keeping the
    /// selected script selected.
    pub fn reload(&self) {
        let selected = self.selected();
        *self.scripts.borrow_mut() = scripts::list();
        self.rebuild(selected.as_deref());
    }

    /// Reload a moment from now, once for a burst of file changes.
    fn reload_soon(self: &Rc<Self>) {
        if self.reload_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        gtk::glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if let Some(host) = weak.upgrade() {
                host.reload_pending.set(false);
                host.reload();
            }
        });
    }

    /// The scripts the filter lets through.
    fn shown(&self) -> Vec<ScriptInfo> {
        let filter = self.filter.borrow().to_lowercase();
        self.scripts
            .borrow()
            .iter()
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter) || s.description.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    }

    /// Repaint the list and select `select`.
    fn rebuild(&self, select: Option<&str>) {
        while let Some(row) = self.list_box.first_child() {
            self.list_box.remove(&row);
        }
        for script in self.shown() {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&script.name);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
            col.set_margin_start(8);
            col.set_margin_end(8);
            col.set_margin_top(4);
            col.set_margin_bottom(4);
            let name = gtk::Label::new(None);
            name.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(&script.name)));
            name.set_xalign(0.0);
            col.append(&name);
            let description = if script.description.is_empty() { "(no description)".to_string() } else { script.description.clone() };
            let desc = gtk::Label::new(Some(&description));
            desc.set_xalign(0.0);
            desc.set_ellipsize(gtk::pango::EllipsizeMode::End);
            desc.add_css_class("dim-label");
            col.append(&desc);
            row.set_child(Some(&col));
            self.list_box.append(&row);
            if select == Some(script.name.as_str()) {
                self.list_box.select_row(Some(&row));
            }
        }
    }

    /// The name of the selected script.
    fn selected(&self) -> Option<String> {
        self.list_box.selected_row().map(|row| row.widget_name().to_string()).filter(|n| !n.is_empty())
    }

    /// Ask for a new script name (`title`, starting at `initial`) and hand a
    /// valid, unused one to `then`.
    fn ask_name(self: &Rc<Self>, title: &str, initial: &str, then: impl Fn(&Rc<Host>, String) + 'static) {
        let host = self.clone();
        let then = Rc::new(then);
        opusdm_ui::requester_entry(&self.window, title, initial, &[("OK", true), ("Cancel", false)], move |ok, name| {
            if !ok {
                return;
            }
            let name = name.trim().to_string();
            if let Err(e) = scripts::validate_name(&name) {
                host.report(&host.window, &e);
                return;
            }
            if host.scripts.borrow().iter().any(|s| s.name == name) {
                host.report(&host.window, &format!("a script called \"{name}\" already exists"));
                return;
            }
            then(&host, name);
        });
    }

    fn new_script(self: &Rc<Self>) {
        self.ask_name("New Script", "", |host, name| {
            if let Err(e) = scripts::save(&name, &scripts::template(&name)) {
                host.report(&host.window, &format!("cannot create the script: {e}"));
                return;
            }
            host.reload();
            host.rebuild(Some(&name));
            host.edit(&name);
        });
    }

    fn rename_script(self: &Rc<Self>) {
        let Some(old) = self.selected() else { return };
        if self.editor_of(&old).is_some_and(|e| e.is_dirty()) {
            self.report(&self.window, &format!("\"{old}\" has unsaved changes in its editor: save or close them first"));
            return;
        }
        self.ask_name("Rename Script", &old.clone(), move |host, new| {
            if let Err(e) = scripts::rename(&old, &new) {
                host.report(&host.window, &format!("cannot rename the script: {e}"));
                return;
            }
            if let Some(editor) = host.editor_of(&old) {
                editor.set_name(&new);
            }
            host.reload();
            host.rebuild(Some(&new));
        });
    }

    fn duplicate_script(self: &Rc<Self>) {
        let Some(name) = self.selected() else { return };
        let taken: Vec<String> = self.scripts.borrow().iter().map(|s| s.name.clone()).collect();
        let copy = (1..)
            .map(|n| if n == 1 { format!("{name} copy") } else { format!("{name} copy {n}") })
            .find(|c| !taken.contains(c))
            .expect("some copy name is free");
        let result = scripts::load(&name).and_then(|code| scripts::save(&copy, &code));
        match result {
            Ok(()) => {
                self.reload();
                self.rebuild(Some(&copy));
            }
            Err(e) => self.report(&self.window, &format!("cannot duplicate the script: {e}")),
        }
    }

    fn delete_script(self: &Rc<Self>) {
        let Some(name) = self.selected() else { return };
        let host = self.clone();
        let text = format!("Delete the script \"{name}\"?");
        opusdm_ui::requester(&self.window, "Delete Script", &text, Some("The file is removed from the scripts folder."), &[("Delete", true), ("Cancel", false)], move |ok| {
            if !ok {
                return;
            }
            if let Err(e) = scripts::delete(&name) {
                host.report(&host.window, &format!("cannot delete the script: {e}"));
                return;
            }
            if let Some(editor) = host.editor_of(&name) {
                editor.close_discarding();
            }
            host.reload();
        });
    }

    fn run_selected(self: &Rc<Self>) {
        let Some(name) = self.selected() else { return };
        // A script open with unsaved changes runs as it is in its editor.
        let code = self.editor_of(&name).filter(|e| e.is_dirty()).map(|e| e.text());
        self.output_box.set_expanded(true);
        self.run(&name, code, "", self.output.clone(), None);
    }
}

/// Open the Scripts window on its own, owned by `app` (which exits when the
/// last window closes). A second launch only raises the window already open.
pub fn open_standalone(app: &gtk::Application) {
    if let Some(window) = app.windows().first() {
        window.present();
        return;
    }
    let shell = opusdm_ui::DialogShell::standalone(app, TITLE).size(520, 520);
    let mut client = Client::connect_tool(&opusdm_ipc::sockpath::hub_sock_path()).ok();
    let inbox = client.as_mut().map(Client::take_inbox);
    let root = shell.content();

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filter scripts"));
    root.append(&search);
    let list_box = gtk::ListBox::new();
    list_box.set_activate_on_single_click(false);
    let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).min_content_height(200).build();
    root.append(&scroller);

    let (output_view, output) = script_editor::output_view();
    let output_scroller = gtk::ScrolledWindow::builder().child(&output_view).min_content_height(120).build();
    let output_box = gtk::Expander::new(Some("Output"));
    output_box.set_child(Some(&output_scroller));
    root.append(&output_box);

    let new_btn = shell.button("New...");
    let edit_btn = shell.button("Edit...");
    let run_btn = shell.button("Run");
    let rename_btn = shell.button("Rename...");
    let duplicate_btn = shell.button("Duplicate");
    let delete_btn = shell.button("Delete");
    shell.button_spacer();
    let folder_btn = shell.button("Show Folder");
    shell.close_button("Close");

    let host = Rc::new(Host {
        app: app.clone(),
        client,
        window: shell.window().clone(),
        list_box: list_box.clone(),
        scripts: RefCell::new(Vec::new()),
        filter: RefCell::new(String::new()),
        output,
        output_box,
        run_buttons: RefCell::new(vec![run_btn.clone()]),
        running: RefCell::new(None),
        editors: RefCell::new(Vec::new()),
        _monitor: RefCell::new(None),
        reload_pending: Cell::new(false),
        hub_alive: Cell::new(true),
    });
    // The callbacks hold weak references: the window keeps the host.
    {
        let keep = host.clone();
        shell.window().connect_destroy(move |_| {
            let _ = &keep;
        });
    }
    host.reload();
    host.update_run_buttons();

    // Changes made outside (an editor, a copy) show up on their own.
    let dir = scripts::scripts_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(monitor) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
        let weak = Rc::downgrade(&host);
        monitor.connect_changed(move |_, _, _, _| {
            if let Some(host) = weak.upgrade() {
                host.reload_soon();
            }
        });
        *host._monitor.borrow_mut() = Some(monitor);
    }

    let selection_buttons = [edit_btn.clone(), rename_btn.clone(), duplicate_btn.clone(), delete_btn.clone()];
    let sync_buttons = {
        let host = Rc::downgrade(&host);
        let run_btn = run_btn.clone();
        move || {
            let Some(host) = host.upgrade() else { return };
            let any = host.selected().is_some();
            for b in &selection_buttons {
                b.set_sensitive(any);
            }
            run_btn.set_sensitive(any && host.can_run());
        }
    };
    sync_buttons();
    {
        let sync = sync_buttons.clone();
        list_box.connect_selected_rows_changed(move |_| sync());
    }
    {
        let host = Rc::downgrade(&host);
        list_box.connect_row_activated(move |_, row| {
            if let Some(host) = host.upgrade() {
                host.edit(&row.widget_name());
            }
        });
    }
    {
        let host = Rc::downgrade(&host);
        search.connect_search_changed(move |entry| {
            let Some(host) = host.upgrade() else { return };
            *host.filter.borrow_mut() = entry.text().to_string();
            let selected = host.selected();
            host.rebuild(selected.as_deref());
        });
    }
    let on = |button: &gtk::Button, action: fn(&Rc<Host>)| {
        let host = Rc::downgrade(&host);
        button.connect_clicked(move |_| {
            if let Some(host) = host.upgrade() {
                action(&host);
            }
        });
    };
    on(&new_btn, Host::new_script);
    on(&edit_btn, |host| {
        if let Some(name) = host.selected() {
            host.edit(&name);
        }
    });
    on(&run_btn, Host::run_selected);
    on(&rename_btn, Host::rename_script);
    on(&duplicate_btn, Host::duplicate_script);
    on(&delete_btn, Host::delete_script);
    on(&folder_btn, |host| {
        let dir = scripts::scripts_dir();
        match &host.client {
            Some(_) => host.send(&ToHub::SpawnLister { start_path: Some(dir) }),
            None => host.report(&host.window, &format!("The scripts are in {}", dir.display())),
        }
    });

    // Closing the Scripts window closes the editors too, each asking about
    // its unsaved changes first.
    {
        let host = Rc::downgrade(&host);
        shell.window().connect_close_request(move |_| {
            if let Some(host) = host.upgrade() {
                let editors: Vec<Rc<Editor>> = host.editors.borrow().clone();
                for editor in editors {
                    editor.window.close();
                }
            }
            gtk::glib::Propagation::Proceed
        });
    }

    shell.present();
    search.grab_focus();

    let Some(inbox) = inbox else { return };
    let weak = Rc::downgrade(&host);
    gtk::glib::spawn_future_local(async move {
        while let Ok(inbound) = inbox.recv().await {
            let Some(host) = weak.upgrade() else { return };
            match inbound {
                Inbound::Msg(msg) => host.handle(msg),
                Inbound::HubGone => {
                    host.hub_alive.set(false);
                    host.run_ended("the hub quit\n", Some(script_editor::TAG_ERROR));
                    return;
                }
            }
        }
    });
}
