//! One bank window: a top-level window with the grid of buttons.

use crate::{grid, x11};
use opusdm_ipc::buttonbank::position_on_screen;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ButtonBank;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How often, and at most how many times, the taskbar and desktop hints and
/// the saved position are (re)applied after the window is mapped.
const MAP_RETRY_MS: u64 = 250;
const MAP_RETRY_LIMIT: u32 = 60;
/// Tries when there is no position to restore (only the taskbar hint).
const MAP_HINT_ONLY_TRIES: u32 = 4;

/// What a bank window asks of its owner.
#[derive(Clone)]
pub struct WindowHooks {
    /// Run the Function with this id.
    pub run: Rc<dyn Fn(&str)>,
    /// Whether a Function id is still saved.
    pub function_exists: Rc<dyn Fn(&str) -> bool>,
}

/// The rectangles of the monitors connected now, empty when there is no
/// display.
fn current_monitors() -> Vec<(i32, i32, i32, i32)> {
    gtk::gdk::Display::default().map(|d| opusdm_ui::list_monitors(&d)).unwrap_or_default()
}

/// A bank window. Cloning shares the same window.
#[derive(Clone)]
pub struct BankWindow {
    window: gtk::Window,
    /// The bank the grid was last built from, so a click maps to its button.
    bank: Rc<RefCell<ButtonBank>>,
    /// Where the window should be placed when it is mapped: the saved
    /// position at first, then wherever it was when it was last hidden.
    place: Rc<Cell<Option<(i32, i32)>>>,
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
    /// out of the taskbar and pager, put on every virtual desktop when the
    /// bank asks for it, and moved to the bank's saved position.
    pub fn new(app: &gtk::Application, bank: &ButtonBank, hooks: &WindowHooks) -> BankWindow {
        let window = gtk::Window::builder().application(app).resizable(false).build();
        let shared = Rc::new(RefCell::new(ButtonBank::default()));
        fill(&window, &shared, bank, hooks);
        let place = Rc::new(Cell::new(bank.x.zip(bank.y)));
        let place_for_map = place.clone();
        let bank_for_map = shared.clone();
        window.connect_map(move |w| {
            // The window manager ignores state and position requests made
            // while it is still adopting the window, and how long that takes
            // depends on how busy the hub is at startup. So keep asking, a
            // few times a second, until the window sits where it should (or
            // gives up after MAP_RETRY_LIMIT tries).
            let window = w.clone();
            let place = place_for_map.clone();
            let bank = bank_for_map.clone();
            let tries = Cell::new(0_u32);
            glib::timeout_add_local(Duration::from_millis(MAP_RETRY_MS), move || {
                tries.set(tries.get() + 1);
                x11::set_skip_taskbar(&window);
                // A new window already sits on the current desktop only, so
                // only the all-desktops case needs asking for.
                if bank.borrow().all_desktops {
                    x11::set_all_desktops(&window, true);
                }
                let target = place.get().and_then(|p| position_on_screen(p, &current_monitors()));
                // Without a saved position only the taskbar hint is retried,
                // for a shorter while.
                let Some((x, y)) = target else {
                    return if tries.get() >= MAP_HINT_ONLY_TRIES { glib::ControlFlow::Break } else { glib::ControlFlow::Continue };
                };
                if x11::position(&window) == Some((x, y)) || tries.get() >= MAP_RETRY_LIMIT {
                    return glib::ControlFlow::Break;
                }
                x11::move_to(&window, x, y);
                glib::ControlFlow::Continue
            });
        });
        BankWindow { window, bank: shared, place }
    }

    /// Rebuild the grid, title and decoration from `bank`. A change of the
    /// all-desktops setting applies at once to a mapped window.
    pub fn update(&self, bank: &ButtonBank, hooks: &WindowHooks) {
        let was_all = self.bank.borrow().all_desktops;
        fill(&self.window, &self.bank, bank, hooks);
        if self.window.is_mapped() && was_all != bank.all_desktops {
            x11::set_all_desktops(&self.window, bank.all_desktops);
        }
    }

    /// Show or hide the window without destroying it.
    pub fn set_visible(&self, visible: bool) {
        if !visible {
            if let Some(pos) = x11::position(&self.window) {
                self.place.set(Some(pos));
            }
        }
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
