//! The "Find" dialog (F-19, F-20): name and content search with streaming
//! results. The search runs on a thread; this file only shows what it sends.

use crate::find_search::{result_label, run_search, status_text, FindQuery, Found, RunState, SearchEvent};
use crate::viewer::ViewerOptions;
use crate::viewer_mode::Mode;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::NameFilter;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// One open Find dialog.
struct Dialog {
    ui: Rc<Ui>,
    name: gtk::Entry,
    contains: gtk::Entry,
    folder: gtk::Entry,
    case_sensitive: gtk::CheckButton,
    include_hidden: gtk::CheckButton,
    include_binary: gtk::CheckButton,
    list: gtk::ListBox,
    status: gtk::Label,
    /// Results of the current search, in the order of the list rows.
    results: RefCell<Vec<Found>>,
    /// The folder the current search started from.
    root: RefCell<PathBuf>,
    /// The Contains text of the current search, if it is a content search.
    searched_text: RefCell<Option<String>>,
    /// Cancel flag of the running (or last) search.
    cancel: RefCell<Arc<AtomicBool>>,
    /// Bumped by every search; events of an older one are ignored.
    generation: Cell<u64>,
    scanned: Cell<usize>,
}

/// Open the Find dialog. `focus_contains` puts the cursor on the Contains
/// field ("Search in Files...") instead of Name ("Find Files...").
pub fn open(ui: &Rc<Ui>, focus_contains: bool) {
    let shell = opusdm_ui::DialogShell::new(&ui.window, "Find").modal(false).size(720, 560);
    let content = shell.content().clone();
    let name = crate::advanced_select::add_field(&content, "Name", "*.log, *.rs, !test*");
    let contains = crate::advanced_select::add_field(&content, "Contains", "text to look for inside files");
    let folder = crate::advanced_select::add_field(&content, "Folder", "");
    folder.set_text(&ui.cwd_path().to_string_lossy());
    let case_sensitive = gtk::CheckButton::with_label("Case sensitive");
    let include_hidden = gtk::CheckButton::with_label("Include hidden files and folders");
    content.append(&case_sensitive);
    content.append(&include_hidden);
    let include_binary = gtk::CheckButton::with_label("Include binary files (slower)");
    content.append(&include_binary);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    content.append(&status);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    // A click selects a result for the buttons; only a double click acts.
    list.set_activate_on_single_click(false);
    let scroller = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    content.append(&scroller);

    let search_btn = shell.button("Search");
    let stop_btn = shell.button("Stop");
    let show_btn = shell.button("Show in folder");
    let view_btn = shell.button("View");
    shell.close_button("Close");
    let window = shell.window().clone();

    let dialog = Rc::new(Dialog {
        ui: ui.clone(),
        name: name.clone(),
        contains: contains.clone(),
        folder: folder.clone(),
        case_sensitive,
        include_hidden,
        include_binary,
        list: list.clone(),
        status,
        results: RefCell::new(Vec::new()),
        root: RefCell::new(PathBuf::new()),
        searched_text: RefCell::new(None),
        cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        generation: Cell::new(0),
        scanned: Cell::new(0),
    });

    // Handlers on widgets the Dialog also owns hold only a weak reference, so
    // the Dialog can be freed once its window is gone.
    let weak = |dialog: &Rc<Dialog>| Rc::downgrade(dialog);
    for entry in [&name, &contains, &folder] {
        let d = weak(&dialog);
        entry.connect_activate(move |_| {
            if let Some(d) = d.upgrade() {
                d.start();
            }
        });
    }
    {
        let d = weak(&dialog);
        search_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.start();
            }
        });
    }
    {
        let d = weak(&dialog);
        stop_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.stop();
            }
        });
    }
    {
        let d = weak(&dialog);
        show_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.show_selected();
            }
        });
    }
    {
        let d = weak(&dialog);
        view_btn.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.view_selected();
            }
        });
    }
    {
        let d = weak(&dialog);
        list.connect_row_activated(move |_, row| {
            let Some(d) = d.upgrade() else { return };
            if d.searched_text.borrow().is_some() {
                d.view_index(row.index() as usize);
            } else {
                d.show_index(row.index() as usize);
            }
        });
    }
    {
        // The one strong reference: the window owns it through this handler,
        // so the Dialog lives exactly as long as the window and the weak
        // handlers above can upgrade. The Dialog does not own the window, so
        // there is no cycle.
        let d = dialog.clone();
        window.connect_close_request(move |_| {
            d.stop();
            glib::Propagation::Proceed
        });
    }
    shell.present();
    if focus_contains {
        contains.grab_focus();
    } else {
        name.grab_focus();
    }
}

impl Dialog {
    /// Stop the running search, if any.
    fn stop(&self) {
        self.cancel.borrow().store(true, Ordering::Relaxed);
    }

    /// Start a search from the fields, replacing the one that is running.
    fn start(self: &Rc<Self>) {
        // Check the fields before touching the running search, so a typo does
        // not kill it and its message is not overwritten.
        let root = PathBuf::from(self.folder.text().trim());
        if !root.is_dir() {
            self.status.set_text(&format!("Folder not found: {}", root.display()));
            return;
        }
        self.stop();
        let contains = self.contains.text().to_string();
        let contains = (!contains.is_empty()).then_some(contains);
        let query = FindQuery {
            name: NameFilter::new(self.name.text().as_str(), ""),
            contains: contains.clone(),
            case_sensitive: self.case_sensitive.is_active(),
            include_hidden: self.include_hidden.is_active(),
            include_binary: self.include_binary.is_active(),
            root: root.clone(),
        };

        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        self.results.borrow_mut().clear();
        *self.root.borrow_mut() = root;
        *self.searched_text.borrow_mut() = contains;
        self.scanned.set(0);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.status.set_text(&status_text(RunState::Running, 0, 0, 0, false));

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = cancel.clone();
        let (tx, rx) = async_channel::unbounded::<SearchEvent>();
        let thread_cancel = cancel.clone();
        std::thread::spawn(move || {
            run_search(&query, &thread_cancel, |event| {
                let _ = tx.send_blocking(event);
            });
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                if this.generation.get() != generation {
                    break;
                }
                this.handle(event, &cancel);
            }
        });
    }

    /// Show one event of the running search.
    fn handle(&self, event: SearchEvent, cancel: &AtomicBool) {
        match event {
            SearchEvent::Found(found) => {
                let label = gtk::Label::new(Some(&result_label(&self.root.borrow(), &found)));
                label.set_xalign(0.0);
                label.set_selectable(false);
                self.list.append(&label);
                self.results.borrow_mut().push(found);
                self.status.set_text(&status_text(RunState::Running, self.scanned.get(), self.results.borrow().len(), 0, false));
            }
            SearchEvent::Scanned(n) => {
                self.scanned.set(n);
                self.status.set_text(&status_text(RunState::Running, n, self.results.borrow().len(), 0, false));
            }
            SearchEvent::Done { scanned, skipped, truncated } => {
                let state = if cancel.load(Ordering::Relaxed) { RunState::Cancelled } else { RunState::Done };
                self.status.set_text(&status_text(state, scanned, self.results.borrow().len(), skipped, truncated));
            }
        }
    }

    /// The result behind the selected row.
    fn selected(&self) -> Option<usize> {
        self.list.selected_row().map(|r| r.index() as usize)
    }

    fn show_selected(&self) {
        match self.selected() {
            Some(i) => self.show_index(i),
            None => self.status.set_text("Select a result first"),
        }
    }

    fn view_selected(&self) {
        match self.selected() {
            Some(i) => self.view_index(i),
            None => self.status.set_text("Select a result first"),
        }
    }

    /// Navigate the Lister to the folder holding result `index` and select it.
    fn show_index(&self, index: usize) {
        let Some(found) = self.results.borrow().get(index).cloned() else { return };
        self.ui.show_in_folder(&found.path);
    }

    /// Open result `index` in the Viewer, on the first match of the text.
    fn view_index(&self, index: usize) {
        let Some(found) = self.results.borrow().get(index).cloned() else { return };
        if found.is_dir {
            self.status.set_text("A folder cannot be viewed; use Show in folder");
            return;
        }
        let opts = ViewerOptions { initial_search: self.searched_text.borrow().clone() };
        crate::viewer::open(&self.ui, vec![found.path], Mode::Smart, false, opts);
    }
}

impl Ui {
    /// Go to the folder that holds `path` and select and show its entry.
    pub fn show_in_folder(self: &Rc<Self>, path: &Path) {
        let Some(parent) = path.parent() else { return };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        let this = self.clone();
        self.navigate(parent, true, move || {
            for i in 0..this.view.selection.n_items() {
                if this.row_at(i).is_some_and(|row| row.name == name) {
                    this.jump_to_row(i);
                    return;
                }
            }
            crate::actions::flash(&this, "not shown in the list (hidden)");
        });
    }
}
