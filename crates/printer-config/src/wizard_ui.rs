//! The add printer dialog: one page per step of [`Wizard`], with Back,
//! Next (Add on the name page) and Cancel. The window's worker runs the
//! searches and the add; the window hands the reports to the open dialog.

use crate::pages::{MARGIN, SPACING};
use crate::wizard::{Step, Wizard};
use crate::worker::Job;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_printing::model::{Device, Driver, DriverChoice};
use opusdm_printing::parse::filter_drivers;
use opusdm_ui::{labeled_row, LabelWidth};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-printer-manager";
/// Size of the dialog.
const DIALOG_W: i32 = 640;
const DIALOG_H: i32 = 520;
/// Most drivers listed at once; the search narrows them.
const DRIVER_LIMIT: usize = 200;
/// Width of the labels of the name page, in characters.
const LABEL_CHARS: i32 = 12;
/// What the address field suggests.
const ADDRESS_HINT: &str = "ipp://host/ipp/print, socket://host:9100 or lpd://host/queue";

/// Sends a job to the worker of the window; returns whether it was sent.
pub type SendJob = Box<dyn Fn(Job) -> bool>;

/// The dialog and what the user chose.
pub struct WizardUi {
    dialog: gtk::Window,
    state: RefCell<Wizard>,
    send: SendJob,
    stack: gtk::Stack,
    back: gtk::Button,
    next: gtk::Button,
    cancel: gtk::Button,
    reason: gtk::Label,
    searching: gtk::Label,
    search_again: gtk::Button,
    device_list: gtk::ListBox,
    manual: gtk::Entry,
    driverless: gtk::CheckButton,
    from_list: gtk::CheckButton,
    from_file: gtk::CheckButton,
    driver_search: gtk::SearchEntry,
    driver_list: gtk::ListBox,
    driver_names: RefCell<Vec<String>>,
    drivers: RefCell<Option<Vec<Driver>>>,
    drivers_asked: Cell<bool>,
    choose_ppd: gtk::Button,
    ppd_label: gtk::Label,
    ppd_path: RefCell<Option<PathBuf>>,
    name: gtk::Entry,
    info: gtk::Entry,
    location: gtk::Entry,
    make_default: gtk::CheckButton,
    done_label: gtk::Label,
    test_page: gtk::Button,
    /// Set while the code fills widgets, so their signals change nothing.
    quiet: Cell<bool>,
    /// The add was sent and its answer is awaited.
    adding: Cell<bool>,
}

/// Open the dialog over `parent`. `taken` are the queue names in use and
/// `configured` the device addresses already added. The dialog keeps itself
/// alive until it is closed; the caller keeps only a weak reference.
pub fn open(parent: &gtk::Window, taken: Vec<String>, configured: Vec<String>, send: SendJob) -> Rc<WizardUi> {
    let dialog = gtk::Window::builder().title("Add printer").transient_for(parent).modal(true).default_width(DIALOG_W).default_height(DIALOG_H).build();
    let ui = Rc::new(WizardUi {
        dialog: dialog.clone(),
        state: RefCell::new(Wizard::new(taken, configured)),
        send,
        stack: gtk::Stack::new(),
        back: gtk::Button::with_label("Back"),
        next: gtk::Button::with_label("Next"),
        cancel: gtk::Button::with_label("Cancel"),
        reason: gtk::Label::new(None),
        searching: gtk::Label::new(Some("Searching the network...")),
        search_again: gtk::Button::with_label("Search again"),
        device_list: gtk::ListBox::new(),
        manual: gtk::Entry::new(),
        driverless: gtk::CheckButton::with_label("Driverless (IPP Everywhere)"),
        from_list: gtk::CheckButton::with_label("Driver from the list"),
        from_file: gtk::CheckButton::with_label("PPD file"),
        driver_search: gtk::SearchEntry::new(),
        driver_list: gtk::ListBox::new(),
        driver_names: RefCell::new(Vec::new()),
        drivers: RefCell::new(None),
        drivers_asked: Cell::new(false),
        choose_ppd: gtk::Button::with_label("Choose..."),
        ppd_label: gtk::Label::new(Some("No file chosen")),
        ppd_path: RefCell::new(None),
        name: gtk::Entry::new(),
        info: gtk::Entry::new(),
        location: gtk::Entry::new(),
        make_default: gtk::CheckButton::with_label("Set as the default printer"),
        done_label: gtk::Label::new(None),
        test_page: gtk::Button::with_label("Print test page"),
        quiet: Cell::new(false),
        adding: Cell::new(false),
    });
    dialog.set_child(Some(&ui.layout()));
    ui.wire();
    // The dialog owns its state until it is closed.
    let slot = RefCell::new(Some(ui.clone()));
    dialog.connect_close_request(move |_| {
        slot.borrow_mut().take();
        glib::Propagation::Proceed
    });
    ui.search();
    ui.show_step();
    dialog.present();
    ui
}

/// A page with the usual margins.
fn page() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
    b.set_margin_top(MARGIN);
    b.set_margin_bottom(MARGIN);
    b.set_margin_start(MARGIN);
    b.set_margin_end(MARGIN);
    b
}

/// A list in a framed scrolled window.
fn scrolled(list: &gtk::ListBox) -> gtk::ScrolledWindow {
    list.set_selection_mode(gtk::SelectionMode::Single);
    let scroll = gtk::ScrolledWindow::builder().child(list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    scroll.add_css_class("frame");
    scroll
}

/// A left-aligned label.
fn text(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}

/// The name of a step's page in the stack.
fn page_name(step: Step) -> &'static str {
    match step {
        Step::Device => "device",
        Step::Driver => "driver",
        Step::Name => "name",
        Step::Done => "done",
    }
}

impl WizardUi {
    /// The pages and the buttons.
    fn layout(&self) -> gtk::Box {
        let device = page();
        device.append(&text("Choose the printer:"));
        self.searching.set_xalign(0.0);
        self.searching.add_css_class("dim-label");
        device.append(&self.searching);
        device.append(&scrolled(&self.device_list));
        self.manual.set_placeholder_text(Some(ADDRESS_HINT));
        self.manual.set_hexpand(true);
        device.append(&labeled_row("Or its address", LabelWidth::Natural, SPACING, &self.manual));
        self.search_again.set_halign(gtk::Align::Start);
        device.append(&self.search_again);
        self.stack.add_named(&device, Some(page_name(Step::Device)));

        let driver = page();
        self.from_list.set_group(Some(&self.driverless));
        self.from_file.set_group(Some(&self.driverless));
        driver.append(&self.driverless);
        driver.append(&self.from_list);
        self.driver_search.set_placeholder_text(Some("Search the drivers"));
        driver.append(&self.driver_search);
        driver.append(&scrolled(&self.driver_list));
        driver.append(&self.from_file);
        let ppd_row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        ppd_row.append(&self.choose_ppd);
        self.ppd_label.add_css_class("dim-label");
        ppd_row.append(&self.ppd_label);
        driver.append(&ppd_row);
        self.stack.add_named(&driver, Some(page_name(Step::Driver)));

        let name = page();
        for (label, entry) in [("Name", &self.name), ("Description", &self.info), ("Location", &self.location)] {
            entry.set_hexpand(true);
            name.append(&labeled_row(label, LabelWidth::Chars(LABEL_CHARS), SPACING, entry));
        }
        name.append(&self.make_default);
        name.append(&text("The name is the CUPS queue name: letters, digits, - _ and . only."));
        self.stack.add_named(&name, Some(page_name(Step::Name)));

        let done = page();
        self.done_label.set_xalign(0.0);
        done.append(&self.done_label);
        self.test_page.set_halign(gtk::Align::Start);
        done.append(&self.test_page);
        self.stack.add_named(&done, Some(page_name(Step::Done)));
        self.stack.set_vexpand(true);

        let root = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        root.append(&self.stack);
        self.reason.set_xalign(0.0);
        self.reason.set_margin_start(MARGIN);
        self.reason.add_css_class("dim-label");
        root.append(&self.reason);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_end(MARGIN);
        buttons.set_margin_bottom(MARGIN);
        buttons.append(&self.cancel);
        buttons.append(&self.back);
        buttons.append(&self.next);
        self.next.add_css_class("suggested-action");
        root.append(&buttons);
        root
    }

    /// Connect the widgets.
    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let on = |action: fn(&Rc<WizardUi>)| {
            let weak = weak.clone();
            move || match weak.upgrade() {
                Some(w) => action(&w),
                None => opusdm_ipc::err_log!(LOG_TAG, "the add printer dialog is gone"),
            }
        };
        let f = on(WizardUi::press_next);
        self.next.connect_clicked(move |_| f());
        let f = on(WizardUi::press_back);
        self.back.connect_clicked(move |_| f());
        let f = on(WizardUi::close);
        self.cancel.connect_clicked(move |_| f());
        let f = on(WizardUi::search);
        self.search_again.connect_clicked(move |_| f());
        let f = on(WizardUi::pick_ppd);
        self.choose_ppd.connect_clicked(move |_| f());
        let f = on(WizardUi::print_test_page);
        self.test_page.connect_clicked(move |_| f());
        self.device_list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(w), Some(row)) = (weak.upgrade(), row) else { return };
                if w.quiet.get() { return; }
                w.pick_device(row.index() as usize);
            }
        });
        self.manual.connect_changed({
            let weak = weak.clone();
            move |e| {
                let Some(w) = weak.upgrade() else { return };
                if w.quiet.get() || e.text().is_empty() { return; }
                w.quiet.set(true);
                w.device_list.unselect_all();
                w.quiet.set(false);
                w.state.borrow_mut().choose_manual(&e.text());
                w.update();
            }
        });
        for check in [&self.driverless, &self.from_list, &self.from_file] {
            let weak = weak.clone();
            check.connect_toggled(move |c| {
                let Some(w) = weak.upgrade() else { return };
                if w.quiet.get() || !c.is_active() { return; }
                w.driver_kind_changed();
            });
        }
        self.driver_search.connect_search_changed({
            let weak = weak.clone();
            move |_| if let Some(w) = weak.upgrade() { w.fill_drivers() }
        });
        self.driver_list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                let (Some(w), Some(row)) = (weak.upgrade(), row) else { return };
                if w.quiet.get() { return; }
                let name = w.driver_names.borrow().get(row.index() as usize).cloned();
                if let Some(name) = name {
                    w.from_list.set_active(true);
                    w.state.borrow_mut().driver = Some(DriverChoice::Model(name));
                    w.update();
                }
            }
        });
        for (entry, field) in [(&self.name, 0), (&self.info, 1), (&self.location, 2)] {
            let weak = weak.clone();
            entry.connect_changed(move |e| {
                let Some(w) = weak.upgrade() else { return };
                if w.quiet.get() { return; }
                let value = e.text().to_string();
                {
                    let mut s = w.state.borrow_mut();
                    match field {
                        0 => s.name = value,
                        1 => s.info = value,
                        _ => s.location = value,
                    }
                }
                w.update();
            });
        }
        self.make_default.connect_toggled({
            let weak = weak.clone();
            move |c| if let Some(w) = weak.upgrade() { w.state.borrow_mut().make_default = c.is_active() }
        });
    }

    /// The current step.
    pub fn step(&self) -> Step {
        self.state.borrow().step
    }

    /// How many devices the list shows (for the tests).
    #[cfg(test)]
    pub fn device_count(&self) -> usize {
        self.state.borrow().devices.len()
    }

    /// Close the dialog.
    pub fn close(self: &Rc<Self>) {
        self.dialog.close();
    }

    /// Bring the dialog to the front.
    pub fn present(&self) {
        self.dialog.present();
    }

    /// Ask the worker for the devices.
    fn search(self: &Rc<Self>) {
        self.searching.set_text("Searching the network...");
        if !(self.send)(Job::Devices) { self.searching.set_text("The search could not start"); }
    }

    /// The devices found arrived.
    pub fn on_devices(&self, devices: Vec<Device>) {
        self.quiet.set(true);
        opusdm_ui::clear_children(&self.device_list);
        for d in &devices {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            row.set_margin_start(SPACING);
            row.set_margin_top(SPACING / 2);
            row.set_margin_bottom(SPACING / 2);
            let mut title = d.info.clone();
            if self.state.borrow().is_configured(&d.uri) { title.push_str("  (already added)"); }
            row.append(&text(&title));
            let uri = text(&d.uri);
            uri.add_css_class("dim-label");
            uri.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row.append(&uri);
            self.device_list.append(&row);
        }
        self.quiet.set(false);
        let found = match devices.len() {
            0 => "No printer found on the network: type its address below.".to_string(),
            1 => "1 printer found.".to_string(),
            n => format!("{n} printers found."),
        };
        self.searching.set_text(&found);
        self.state.borrow_mut().devices = devices;
    }

    /// The driver list arrived.
    pub fn on_drivers(&self, drivers: Vec<Driver>) {
        *self.drivers.borrow_mut() = Some(drivers);
        self.fill_drivers();
    }

    /// The printer was added.
    pub fn on_added(&self, name: &str) {
        self.adding.set(false);
        self.state.borrow_mut().next();
        self.done_label.set_text(&format!("{name} was added."));
        self.show_step();
    }

    /// The add failed: stay on the name page and say why.
    pub fn on_add_failed(&self, why: &str) {
        self.adding.set(false);
        self.update();
        self.reason.set_text(why);
        self.reason.add_css_class("error");
    }

    /// The user picked the device at `index` of the list.
    pub fn pick_device(&self, index: usize) {
        let Some(device) = self.state.borrow().devices.get(index).cloned() else { return };
        self.quiet.set(true);
        self.manual.set_text("");
        if let Some(row) = self.device_list.row_at_index(index as i32) { self.device_list.select_row(Some(&row)); }
        self.quiet.set(false);
        self.state.borrow_mut().choose_device(device);
        self.update();
    }

    /// Next, or Add on the name page, or Close on the last page.
    pub fn press_next(self: &Rc<Self>) {
        let step = self.step();
        if step == Step::Done { return self.close(); }
        if step == Step::Name {
            let Some(request) = self.state.borrow().request() else { return self.update() };
            self.adding.set(true);
            self.update();
            self.reason.set_text("Adding the printer...");
            if !(self.send)(Job::Add(Box::new(request))) { self.on_add_failed("The worker is gone: the printer was not added"); }
            return;
        }
        self.state.borrow_mut().next();
        self.show_step();
    }

    /// Back one page.
    fn press_back(self: &Rc<Self>) {
        self.state.borrow_mut().back();
        self.show_step();
    }

    /// Show the page of the current step, filled from the state.
    fn show_step(&self) {
        let step = self.step();
        self.stack.set_visible_child_name(page_name(step));
        match step {
            Step::Driver => self.enter_driver_page(),
            Step::Name => {
                let s = self.state.borrow().clone();
                self.quiet.set(true);
                self.name.set_text(&s.name);
                self.info.set_text(&s.info);
                self.location.set_text(&s.location);
                self.make_default.set_active(s.make_default);
                self.quiet.set(false);
            }
            _ => {}
        }
        self.update();
    }

    /// Fill the driver page: the driverless choice when the device has it,
    /// the search prefilled with the maker.
    fn enter_driver_page(&self) {
        let (driverless, driver, maker) = {
            let s = self.state.borrow();
            let maker = s.device.as_ref().map(|d| d.make_model.split_whitespace().next().unwrap_or_default().to_string()).unwrap_or_default();
            (s.driverless, s.driver.clone(), maker)
        };
        self.quiet.set(true);
        self.driverless.set_sensitive(driverless);
        match driver {
            Some(DriverChoice::Driverless) => self.driverless.set_active(true),
            Some(DriverChoice::PpdFile(_)) => self.from_file.set_active(true),
            _ => self.from_list.set_active(true),
        }
        if self.driver_search.text().is_empty() && maker != "Unknown" { self.driver_search.set_text(&maker); }
        self.quiet.set(false);
        if !driverless { self.ask_drivers(); }
        self.fill_drivers();
    }

    /// Ask the worker for the driver list, once.
    fn ask_drivers(&self) {
        if self.drivers_asked.replace(true) { return; }
        if !(self.send)(Job::Drivers) { self.drivers_asked.set(false); }
    }

    /// The radio buttons of the driver page changed.
    fn driver_kind_changed(&self) {
        let choice = if self.driverless.is_active() {
            Some(DriverChoice::Driverless)
        } else if self.from_file.is_active() {
            self.ppd_path.borrow().clone().map(DriverChoice::PpdFile)
        } else {
            self.ask_drivers();
            let selected = self.driver_list.selected_row().and_then(|r| self.driver_names.borrow().get(r.index() as usize).cloned());
            selected.map(DriverChoice::Model)
        };
        self.state.borrow_mut().driver = choice;
        self.update();
    }

    /// Fill the driver list from the search text.
    fn fill_drivers(&self) {
        self.quiet.set(true);
        opusdm_ui::clear_children(&self.driver_list);
        let mut names = Vec::new();
        match self.drivers.borrow().as_ref() {
            None => {
                let loading = text(if self.drivers_asked.get() { "Loading drivers..." } else { "Choose \"Driver from the list\" to load the drivers." });
                loading.add_css_class("dim-label");
                self.driver_list.append(&loading);
                names.push(String::new());
            }
            Some(all) => {
                let current = match &self.state.borrow().driver {
                    Some(DriverChoice::Model(n)) => Some(n.clone()),
                    _ => None,
                };
                for d in filter_drivers(all, &self.driver_search.text(), DRIVER_LIMIT) {
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&text(&d.description)));
                    row.set_tooltip_text(Some(&d.name));
                    self.driver_list.append(&row);
                    if current.as_deref() == Some(d.name.as_str()) { self.driver_list.select_row(Some(&row)); }
                    names.push(d.name.clone());
                }
            }
        }
        *self.driver_names.borrow_mut() = names;
        self.quiet.set(false);
    }

    /// Choose a PPD file.
    fn pick_ppd(self: &Rc<Self>) {
        let pick = opusdm_ui::FilePick {
            title: "Choose a PPD file",
            filter: Some(opusdm_ui::FileFilterSpec { name: "PPD files (*.ppd, *.ppd.gz)", patterns: &["*.ppd", "*.PPD", "*.ppd.gz"], ..Default::default() }),
            ..Default::default()
        };
        let weak = Rc::downgrade(self);
        opusdm_ui::pick_file_or_error(Some(&self.dialog), &pick, move |result| {
            let Some(w) = weak.upgrade() else {
                opusdm_ipc::err_log!(LOG_TAG, "a PPD file was chosen after the add printer dialog closed");
                return;
            };
            match result {
                Ok(path) => {
                    w.ppd_label.set_text(&path.display().to_string());
                    *w.ppd_path.borrow_mut() = Some(path);
                    w.from_file.set_active(true);
                    w.driver_kind_changed();
                }
                Err(message) => w.reason.set_text(&message),
            }
        });
    }

    /// Print the test page on the new printer.
    fn print_test_page(self: &Rc<Self>) {
        let name = self.state.borrow().name.clone();
        if (self.send)(Job::TestPage(name.clone())) { self.done_label.set_text(&format!("{name} was added. Test page sent.")); }
    }

    /// Update the buttons and the reason line.
    fn update(&self) {
        let (step, check) = {
            let s = self.state.borrow();
            (s.step, s.can_next())
        };
        let adding = self.adding.get();
        self.reason.remove_css_class("error");
        self.reason.set_text(check.as_ref().err().map(String::as_str).unwrap_or(""));
        self.next.set_label(match step {
            Step::Name => "Add",
            Step::Done => "Close",
            _ => "Next",
        });
        self.next.set_sensitive(check.is_ok() && !adding);
        self.back.set_sensitive(matches!(step, Step::Driver | Step::Name) && !adding);
        self.cancel.set_visible(step != Step::Done);
        self.back.set_visible(step != Step::Done);
    }
}
