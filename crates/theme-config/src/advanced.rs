//! The Advanced tab: a list of tools on the left and, on the right, the tool
//! the user picked. A tool is built the first time it is picked and kept, so
//! its state survives a trip to another entry. Leaving a tool with unsaved
//! changes (picking another entry, closing Settings) asks first, see
//! `opusdm_ui::confirm_leave`.

use crate::tools;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ui::EmbeddedTool;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Width of the list on the left.
const LIST_WIDTH: i32 = 190;

/// Gap around the list and the tool area.
const MARGIN: i32 = 12;

/// Name of the stack page shown while no entry is picked.
const EMPTY_PAGE: &str = "empty";

/// Builds the tool of an entry for the window that hosts it.
type BuildTool = fn(&gtk::Window) -> Rc<dyn EmbeddedTool>;

/// One entry of the list.
struct Entry {
    label: &'static str,
    build: BuildTool,
}

/// The entries of the list, in order.
const ENTRIES: [Entry; 5] = [
    Entry { label: "Hotkeys", build: build_hotkeys },
    Entry { label: "File Types", build: build_file_types },
    Entry { label: "Input", build: build_input },
    Entry { label: "Network", build: build_network },
    Entry { label: "Printers", build: build_printers },
];

/// The Hotkeys editor.
fn build_hotkeys(window: &gtk::Window) -> Rc<dyn EmbeddedTool> {
    opusdm_lister::hotkey_editor::embedded(window)
}

/// The File Types editor.
fn build_file_types(window: &gtk::Window) -> Rc<dyn EmbeddedTool> {
    opusdm_lister::filetypes_ui::embedded(window)
}

/// The Input tabs (mouse, touchpad, keyboard).
fn build_input(window: &gtk::Window) -> Rc<dyn EmbeddedTool> {
    opusdm_input_config::ui::embedded(window)
}

/// The Network connections.
fn build_network(window: &gtk::Window) -> Rc<dyn EmbeddedTool> {
    opusdm_network_config::window::embedded(window)
}

/// The CUPS printers and their queues.
fn build_printers(window: &gtk::Window) -> Rc<dyn EmbeddedTool> {
    opusdm_printer_config::window::embedded(window)
}

/// What the callbacks of the tab share.
pub struct Advanced {
    list: gtk::ListBox,
    stack: gtk::Stack,
    /// The tool of each entry, built on first use.
    tools: RefCell<Vec<Option<Rc<dyn EmbeddedTool>>>>,
    /// The entry shown, `None` before the first pick.
    current: Cell<Option<usize>>,
    /// Set while the code changes the list selection.
    quiet: Cell<bool>,
}

/// Build the tab. The returned widget is the tab page; keep the `Advanced`
/// to ask it before Settings closes.
pub fn build() -> (gtk::Box, Rc<Advanced>) {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for entry in &ENTRIES {
        let label = gtk::Label::new(Some(entry.label));
        label.set_xalign(0.0);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(6);
        list.append(&label);
    }
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    let hint = gtk::Label::new(Some("Pick a tool on the left."));
    stack.add_named(&hint, Some(EMPTY_PAGE));

    let advanced = Rc::new(Advanced { list: list.clone(), stack: stack.clone(), tools: RefCell::new(vec![None; ENTRIES.len()]), current: Cell::new(None), quiet: Cell::new(false) });

    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.set_size_request(LIST_WIDTH, -1);
    left.set_margin_top(MARGIN);
    left.set_margin_bottom(MARGIN);
    left.set_margin_start(MARGIN);
    let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    scroll.add_css_class("frame");
    left.append(&scroll);
    left.append(&tools::build_tool_buttons());

    let page = gtk::Box::new(gtk::Orientation::Horizontal, MARGIN);
    page.append(&left);
    page.append(&stack);

    // The row callback holds the tab weakly: `page` owns the widgets, the
    // caller owns the `Advanced`.
    let weak = Rc::downgrade(&advanced);
    list.connect_row_selected(move |_, row| {
        let Some(advanced) = weak.upgrade() else {
            opusdm_ipc::err_log!("theme-config", "the Advanced tab is gone: ignoring the pick of an entry");
            return;
        };
        let Some(row) = row else { return };
        advanced.on_selected(row.index() as usize);
    });
    (page, advanced)
}

impl Advanced {
    /// A row was picked: leave the current tool (asking when it has unsaved
    /// changes), then show the new one.
    fn on_selected(self: &Rc<Self>, index: usize) {
        if self.quiet.get() || self.current.get() == Some(index) { return; }
        let this = self.clone();
        self.leave_current(move || this.show(index), {
            let this = self.clone();
            move || this.restore_selection()
        });
    }

    /// Run `proceed` once the tool on screen can be left, `cancel` when the
    /// user stays.
    fn leave_current(&self, proceed: impl Fn() + 'static, cancel: impl Fn() + 'static) {
        let Some(tool) = self.current_tool() else { return proceed() };
        let Some(window) = self.window() else {
            opusdm_ipc::err_log!("theme-config", "the Advanced tab is not in a window: cannot ask about unsaved changes");
            return cancel();
        };
        opusdm_ui::confirm_leave(&window, &tool, proceed, cancel);
    }

    /// Whether the tool on screen has changes that are not saved.
    pub fn is_dirty(&self) -> bool {
        self.current_tool().is_some_and(|tool| tool.is_dirty())
    }

    /// Ask before Settings closes: `proceed` runs when no tool is dirty or the
    /// user saved or discarded; `cancel` when they stay.
    pub fn confirm_close(&self, proceed: impl Fn() + 'static, cancel: impl Fn() + 'static) {
        self.leave_current(proceed, cancel);
    }

    /// The tool on screen, if it is an embedded one.
    fn current_tool(&self) -> Option<Rc<dyn EmbeddedTool>> {
        let index = self.current.get()?;
        self.tools.borrow().get(index).cloned().flatten()
    }

    /// The window the tab is in.
    fn window(&self) -> Option<gtk::Window> {
        self.stack.root().and_then(|root| root.downcast::<gtk::Window>().ok())
    }

    /// Put the list selection back on the entry on screen.
    fn restore_selection(&self) {
        self.quiet.set(true);
        match self.current.get().and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
        self.quiet.set(false);
    }

    /// Show the entry `index`, building its tool the first time.
    fn show(&self, index: usize) {
        let Some(window) = self.window() else {
            opusdm_ipc::err_log!("theme-config", "the Advanced tab is not in a window: cannot show an entry");
            return;
        };
        let entry = &ENTRIES[index];
        let name = format!("entry-{index}");
        if self.stack.child_by_name(&name).is_none() {
            let tool = (entry.build)(&window);
            self.stack.add_named(&tool.widget(), Some(&name));
            self.tools.borrow_mut()[index] = Some(tool);
        }
        self.stack.set_visible_child_name(&name);
        self.current.set(Some(index));
    }
}
