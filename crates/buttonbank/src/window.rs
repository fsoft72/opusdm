//! One bank window: a top-level window with the grid of buttons.

use crate::{grid, x11};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ButtonBank;
use std::cell::RefCell;
use std::rc::Rc;

/// What a bank window asks of its owner.
#[derive(Clone)]
pub struct WindowHooks {
    /// Run the Function with this id.
    pub run: Rc<dyn Fn(&str)>,
    /// Whether a Function id is still saved.
    pub function_exists: Rc<dyn Fn(&str) -> bool>,
}

/// A bank window. Cloning shares the same window.
#[derive(Clone)]
pub struct BankWindow {
    window: gtk::Window,
    /// The bank the grid was last built from, so a click maps to its button.
    bank: Rc<RefCell<ButtonBank>>,
}

/// Fill `window` with the grid for `bank` and apply its appearance.
fn fill(window: &gtk::Window, shared: &Rc<RefCell<ButtonBank>>, bank: &ButtonBank, hooks: &WindowHooks) {
    *shared.borrow_mut() = bank.clone();
    window.set_title(Some(&bank.name));
    window.set_decorated(!bank.borderless);
    let on_click = {
        let shared = shared.clone();
        let run = hooks.run.clone();
        Rc::new(move |index: usize, mouse| {
            let id = shared.borrow().buttons.get(index).and_then(|b| b.function_for(mouse).map(str::to_string));
            if let Some(id) = id {
                run(&id);
            }
        })
    };
    let exists = hooks.function_exists.clone();
    window.set_child(Some(&grid::build_grid(bank, on_click, &|id| exists(id))));
}

impl BankWindow {
    /// Create (but do not show) the window of `bank`. Once mapped it is kept
    /// out of the taskbar and pager and moved to the bank's saved position.
    pub fn new(app: &gtk::Application, bank: &ButtonBank, hooks: &WindowHooks) -> BankWindow {
        let window = gtk::Window::builder().application(app).resizable(false).build();
        let shared = Rc::new(RefCell::new(ButtonBank::default()));
        fill(&window, &shared, bank, hooks);
        let saved = bank.x.zip(bank.y);
        window.connect_map(move |w| {
            x11::set_skip_taskbar(w);
            if let Some((x, y)) = saved {
                x11::move_to(w, x, y);
            }
        });
        BankWindow { window, bank: shared }
    }

    /// Rebuild the grid, title and decoration from `bank`.
    pub fn update(&self, bank: &ButtonBank, hooks: &WindowHooks) {
        fill(&self.window, &self.bank, bank, hooks);
    }

    /// Show or hide the window without destroying it.
    pub fn set_visible(&self, visible: bool) {
        self.window.set_visible(visible);
    }

    /// Show the window and bring it to the front.
    pub fn present(&self) {
        self.window.present();
    }

    /// The position of the window on the screen, if it can be read.
    pub fn position(&self) -> Option<(i32, i32)> {
        x11::position(&self.window)
    }

    /// Close the window (runs the `connect_close` callback).
    pub fn close(&self) {
        self.window.close();
    }

    /// Call `f` with the window position when the window is closed, whether
    /// by the user or by [`BankWindow::close`].
    pub fn connect_close(&self, f: impl Fn(Option<(i32, i32)>) + 'static) {
        self.window.connect_close_request(move |w| {
            f(x11::position(w));
            glib::Propagation::Proceed
        });
    }
}
