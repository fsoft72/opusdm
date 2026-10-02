//! The open button bank windows. The hub owns them (like the group windows)
//! so they survive Lister restarts; the store is `Banks` on disk. A click
//! runs the button's Function headless. See
//! `docs/superpowers/specs/2026-09-30-button-banks-design.md`.

use crate::server::HubCmd;
use gtk4 as gtk;
use opusdm_buttonbank::window::{BankWindow, WindowHooks};
use opusdm_ipc::buttonbank::{BankAction, BankView};
use opusdm_ipc::function::Function;
use opusdm_ipc::Banks;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// One open bank window and whether a `HideBank` hid it.
struct Open {
    window: BankWindow,
    own_hidden: Cell<bool>,
}

/// Every open bank window, keyed by bank id.
pub struct BankWindows {
    banks: Rc<RefCell<Banks>>,
    open: Rc<RefCell<HashMap<String, Open>>>,
    hooks: WindowHooks,
    /// Whether the whole desktop is hidden (`Hide`): every bank window hides.
    desktop_hidden: Cell<bool>,
    /// Set once the hub is quitting, so closing the windows does not clear
    /// their saved `open` state.
    shutting_down: Rc<Cell<bool>>,
}

impl BankWindows {
    /// New, with no window open. A button click sends
    /// `HubCmd::RunFunctionHeadless` on `cmd_tx`.
    pub fn new(banks: Rc<RefCell<Banks>>, functions: Rc<RefCell<Vec<Function>>>, cmd_tx: Sender<HubCmd>) -> BankWindows {
        let run: Rc<dyn Fn(&str)> = Rc::new(move |id| {
            let _ = cmd_tx.send(HubCmd::RunFunctionHeadless(id.to_string()));
        });
        let function_exists: Rc<dyn Fn(&str) -> bool> = Rc::new(move |id| functions.borrow().iter().any(|f| f.id == id));
        BankWindows {
            banks,
            open: Rc::new(RefCell::new(HashMap::new())),
            hooks: WindowHooks { run, function_exists },
            desktop_hidden: Cell::new(false),
            shutting_down: Rc::new(Cell::new(false)),
        }
    }

    /// Open every bank that was open when the hub last ran.
    pub fn restore(&self, app: &gtk::Application) {
        let ids: Vec<String> = self.banks.borrow().banks.iter().filter(|b| b.open).map(|b| b.id.clone()).collect();
        for id in ids {
            self.open_id(app, &id);
        }
    }

    /// The current view of bank `id`.
    fn view(&self, id: &str) -> BankView {
        match self.open.borrow().get(id) {
            None => BankView::Closed,
            Some(o) if o.own_hidden.get() => BankView::Hidden,
            Some(_) => BankView::Shown,
        }
    }

    /// Apply a bank command to the bank called `name`. An unknown name is
    /// logged (the Function step already checked it, so this means the bank
    /// was deleted in between).
    pub fn apply(&self, app: &gtk::Application, action: BankAction, name: &str) {
        let Some(id) = self.banks.borrow().get_by_name(name).map(|b| b.id.clone()) else {
            opusdm_ipc::err_log!("hub", "bank command {action:?}: no bank called \"{name}\"");
            return;
        };
        match action.apply(self.view(&id)) {
            BankView::Shown => self.open_id(app, &id),
            BankView::Closed => self.close_id(&id),
            BankView::Hidden => self.hide_id(&id),
        }
    }

    /// Open bank `id`, or show it again when it is open but hidden.
    pub fn open_id(&self, app: &gtk::Application, id: &str) {
        if let Some(open) = self.open.borrow().get(id) {
            open.own_hidden.set(false);
            open.window.set_visible(!self.desktop_hidden.get());
            open.window.present();
            self.remember(id, true);
            return;
        }
        let Some(bank) = self.banks.borrow().get(id).cloned() else { return };
        let window = BankWindow::new(app, &bank, &self.hooks);
        {
            let (open, banks, shutting_down, id) = (self.open.clone(), self.banks.clone(), self.shutting_down.clone(), id.to_string());
            window.connect_close(move |pos| {
                open.borrow_mut().remove(&id);
                if shutting_down.get() {
                    return;
                }
                banks.borrow_mut().set_runtime(&id, false, pos);
                if let Err(e) = banks.borrow().save_bank(&id) {
                    opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
                }
            });
        }
        if !self.desktop_hidden.get() {
            window.present();
        }
        self.open.borrow_mut().insert(id.to_string(), Open { window, own_hidden: Cell::new(false) });
        self.remember(id, true);
    }

    /// Close bank `id`, saving its position and `open: false`.
    pub fn close_id(&self, id: &str) {
        let window = self.open.borrow().get(id).map(|o| o.window.clone());
        match window {
            Some(window) => window.close(),
            None => self.remember(id, false),
        }
    }

    /// Hide the window of bank `id` without closing it.
    fn hide_id(&self, id: &str) {
        if let Some(open) = self.open.borrow().get(id) {
            open.own_hidden.set(true);
            open.window.set_visible(false);
        }
    }

    /// Bring the window of bank `id` in line with the store (after a save),
    /// or close it when the bank is gone.
    pub fn refresh(&self, id: &str) {
        let bank = self.banks.borrow().get(id).cloned();
        let window = self.open.borrow().get(id).map(|o| o.window.clone());
        match (window, bank) {
            (Some(window), Some(bank)) => window.update(&bank, &self.hooks),
            (Some(window), None) => window.close(),
            _ => {}
        }
    }

    /// Hide (`true`) or show (`false`) every bank window with the desktop.
    /// A window a `HideBank` hid stays hidden.
    pub fn set_desktop_hidden(&self, hidden: bool) {
        self.desktop_hidden.set(hidden);
        for open in self.open.borrow().values() {
            open.window.set_visible(!hidden && !open.own_hidden.get());
        }
    }

    /// Save the position of every open window and keep `open: true`, then
    /// stop the close handlers from clearing it. Call before the hub quits.
    pub fn begin_shutdown(&self) {
        self.shutting_down.set(true);
        let positions: Vec<(String, Option<(i32, i32)>)> =
            self.open.borrow().iter().map(|(id, o)| (id.clone(), o.window.position())).collect();
        for (id, pos) in positions {
            self.banks.borrow_mut().set_runtime(&id, true, pos);
            if let Err(e) = self.banks.borrow().save_bank(&id) {
                opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
            }
        }
    }

    /// Record `open` for bank `id` and save it.
    fn remember(&self, id: &str, open: bool) {
        self.banks.borrow_mut().set_runtime(id, open, None);
        if let Err(e) = self.banks.borrow().save_bank(id) {
            opusdm_ipc::err_log!("hub", "cannot save bank {id}: {e}");
        }
    }
}
