//! "Add device": the devices a scan finds, updated live, and the pairing of
//! the one picked. The window owns the dialog while it is open and tells it
//! the state; the dialog tells the window what the user did through two
//! callbacks.

use crate::pages::{device_row, MARGIN, SPACING};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_bluetooth::BtState;
use std::cell::RefCell;
use std::rc::Rc;

/// Size of the dialog, in pixels.
const DIALOG_W: i32 = 420;
const DIALOG_H: i32 = 460;

/// What the dialog asks of the window.
pub struct PairCallbacks {
    /// Pair the device at this path.
    pub pair: Box<dyn Fn(String)>,
    /// The dialog closed; the device being paired, if any.
    pub closed: Box<dyn Fn(Option<String>)>,
}

/// The "Add device" dialog.
pub struct PairDialog {
    window: gtk::Window,
    list: gtk::ListBox,
    /// The device of each row of the list.
    paths: RefCell<Vec<String>>,
    status: gtk::Label,
    spinner: gtk::Spinner,
    pair: gtk::Button,
    /// The device a pairing is in progress for.
    pairing: RefCell<Option<String>>,
}

impl PairDialog {
    /// Build and show the dialog over `parent`.
    pub fn open(parent: &gtk::Window, callbacks: PairCallbacks) -> Rc<Self> {
        let window = gtk::Window::builder().title("Add Bluetooth device").modal(true).transient_for(parent).destroy_with_parent(true).default_width(DIALOG_W).default_height(DIALOG_H).build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, SPACING);
        body.set_margin_top(MARGIN);
        body.set_margin_bottom(MARGIN);
        body.set_margin_start(MARGIN);
        body.set_margin_end(MARGIN);
        let hint = gtk::Label::new(Some("Put the device in pairing mode, then pick it in the list."));
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        body.append(&hint);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        scroll.add_css_class("frame");
        body.append(&scroll);
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        let spinner = gtk::Spinner::new();
        let status = gtk::Label::new(Some("Looking for devices..."));
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_hexpand(true);
        status_row.append(&spinner);
        status_row.append(&status);
        body.append(&status_row);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, SPACING);
        buttons.set_halign(gtk::Align::End);
        let close = gtk::Button::with_label("Close");
        let pair = gtk::Button::with_label("Pair");
        pair.add_css_class("suggested-action");
        pair.set_sensitive(false);
        buttons.append(&close);
        buttons.append(&pair);
        body.append(&buttons);
        window.set_child(Some(&body));
        let dialog = Rc::new(Self { window, list, paths: RefCell::new(Vec::new()), status, spinner, pair, pairing: RefCell::new(None) });
        dialog.spinner.start();
        dialog.wire(callbacks, &close);
        dialog.window.present();
        dialog
    }

    /// Connect the buttons and the list. The handlers hold the dialog weakly:
    /// the window owns it.
    fn wire(self: &Rc<Self>, callbacks: PairCallbacks, close: &gtk::Button) {
        let callbacks = Rc::new(callbacks);
        let weak = Rc::downgrade(self);
        let start = {
            let (weak, callbacks) = (weak.clone(), callbacks.clone());
            move || {
                let Some(d) = weak.upgrade() else { return opusdm_ipc::err_log!("opusdm-bluetooth-manager", "Pair clicked after the pairing dialog was gone") };
                d.start_pairing(&callbacks);
            }
        };
        let start = Rc::new(start);
        {
            let start = start.clone();
            self.pair.connect_clicked(move |_| start());
        }
        self.list.connect_row_activated(move |_, _| start());
        {
            let weak = weak.clone();
            self.list.connect_selected_rows_changed(move |_| {
                if let Some(d) = weak.upgrade() { d.pair.set_sensitive(d.pairing.borrow().is_none() && d.selected().is_some()); }
            });
        }
        {
            let window = self.window.clone();
            close.connect_clicked(move |_| window.close());
        }
        self.window.connect_close_request(move |_| {
            match weak.upgrade() {
                Some(d) => (callbacks.closed)(d.pairing.borrow_mut().take()),
                None => opusdm_ipc::err_log!("opusdm-bluetooth-manager", "the pairing dialog closed after it was gone"),
            }
            glib::Propagation::Proceed
        });
    }

    /// The device of the selected row.
    fn selected(&self) -> Option<String> {
        let row = self.list.selected_row()?;
        self.paths.borrow().get(usize::try_from(row.index()).ok()?).cloned()
    }

    /// Pair the selected device.
    fn start_pairing(&self, callbacks: &PairCallbacks) {
        if self.pairing.borrow().is_some() { return; }
        let Some(device) = self.selected() else { return };
        *self.pairing.borrow_mut() = Some(device.clone());
        self.pair.set_sensitive(false);
        self.set_status("Pairing...", true);
        (callbacks.pair)(device);
    }

    /// Show the devices found and not paired, keeping the selection.
    pub fn refresh(&self, state: &BtState) {
        let keep = self.selected();
        let found = state.found();
        let paths: Vec<String> = found.iter().map(|d| d.path.clone()).collect();
        // Rebuilding the rows on every change would drop the selection and
        // flicker: only when the list really changed.
        let same = *self.paths.borrow() == paths;
        if !same {
            opusdm_ui::clear_children(&self.list);
            for d in &found { self.list.append(&device_row(d, false)); }
            *self.paths.borrow_mut() = paths.clone();
            if let Some(i) = keep.and_then(|k| paths.iter().position(|p| *p == k)) {
                if let Some(row) = self.list.row_at_index(i as i32) { self.list.select_row(Some(&row)); }
            }
        }
        if self.pairing.borrow().is_none() {
            let off = !state.is_on();
            self.set_status(if off { "Bluetooth is off" } else if found.is_empty() { "Looking for devices..." } else { "Pick the device to pair" }, !off);
        }
    }

    /// A pairing ended: `error` is `None` when it worked. The window closes
    /// the dialog after a good one.
    pub fn pairing_ended(&self, device: &str, error: Option<String>) {
        if self.pairing.borrow().as_deref() != Some(device) { return; }
        self.pairing.borrow_mut().take();
        self.pair.set_sensitive(self.selected().is_some());
        match error {
            Some(text) => {
                self.set_status(&text, false);
                self.status.add_css_class("error");
            }
            None => self.set_status("Paired", false),
        }
    }

    /// Show a status line; `busy` turns the spinner.
    fn set_status(&self, text: &str, busy: bool) {
        self.status.remove_css_class("error");
        self.status.set_text(text);
        self.spinner.set_spinning(busy);
    }

    /// Bring the dialog to the front.
    pub fn present(&self) {
        self.window.present();
    }

    /// Close the dialog (the closed callback runs).
    pub fn close(&self) {
        self.window.close();
    }

    /// The rows of the list (for the tests).
    #[cfg(test)]
    pub fn rows(&self) -> Vec<String> {
        self.paths.borrow().clone()
    }

    /// Select the row of `device` and press Pair, as the user does (for the tests).
    #[cfg(test)]
    pub fn pick_and_pair(&self, device: &str) {
        let i = self.paths.borrow().iter().position(|p| p == device).expect("the device is listed");
        self.list.select_row(self.list.row_at_index(i as i32).as_ref());
        self.pair.emit_clicked();
    }
}
