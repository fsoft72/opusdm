//! One bank window: a top-level window with the grid of buttons.

use crate::{grid, layer, x11};
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
    /// position at first, then wherever it was when it was last hidden. For a
    /// layer surface (see [`layer`]) it is always where the window is.
    place: Rc<Cell<Option<(i32, i32)>>>,
    /// The window is a Wayland layer surface placed by the bank itself.
    layered: bool,
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
        let saved = bank.x.zip(bank.y).and_then(|p| position_on_screen(p, &current_monitors()));
        let place = Rc::new(Cell::new(saved));
        if layer::wants_layer(bank) {
            init_layer(&window, &place);
            return BankWindow { window, bank: shared, place, layered: true };
        }
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
        BankWindow { window, bank: shared, place, layered: false }
    }

    /// Whether `bank` needs a new window: on Wayland a borderless bank is a
    /// layer surface and a framed one a normal window, and a window cannot
    /// turn from one into the other.
    pub fn needs_rebuild(&self, bank: &ButtonBank) -> bool {
        layer::wants_layer(bank) != self.layered
    }

    /// The application the window belongs to.
    pub fn application(&self) -> Option<gtk::Application> {
        self.window.application()
    }

    /// Rebuild the grid, title and decoration from `bank`. A change of the
    /// all-desktops setting applies at once to a mapped window.
    pub fn update(&self, bank: &ButtonBank, hooks: &WindowHooks) {
        let was_all = self.bank.borrow().all_desktops;
        fill(&self.window, &self.bank, bank, hooks);
        // Layer surfaces show on every desktop anyway.
        if !self.layered && self.window.is_mapped() && was_all != bank.all_desktops {
            x11::set_all_desktops(&self.window, bank.all_desktops);
        }
    }

    /// Show or hide the window without destroying it.
    pub fn set_visible(&self, visible: bool) {
        if !visible && !self.layered {
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
        if self.layered { self.place.get() } else { x11::position(&self.window) }
    }

    /// Close the window (runs the `connect_close` callback).
    pub fn close(&self) {
        self.window.close();
    }

    /// Call `f` with the window position when the window is closed, whether
    /// by the user or by [`BankWindow::close`].
    pub fn connect_close(&self, f: impl Fn(Option<(i32, i32)>) + 'static) {
        let this = self.clone();
        self.window.connect_close_request(move |_| {
            f(this.position());
            glib::Propagation::Proceed
        });
    }
}

/// Make `window` a layer surface at the position in `place`, centered on the
/// first monitor once mapped when there is none, and moved by Alt+drag.
fn init_layer(window: &gtk::Window, place: &Rc<Cell<Option<(i32, i32)>>>) {
    let monitors = current_monitors();
    let Some(first) = monitors.first().copied() else { return };
    let pos = place.get().unwrap_or((first.0, first.1));
    let monitor = layer::monitor_of(pos, &monitors).unwrap_or(first);
    if !opusdm_ui::layer::init(window, &layer::spec_at(pos, monitor)) { return; }
    let move_to = {
        let (window, place) = (window.clone(), place.clone());
        move |pos: (i32, i32)| {
            let monitors = current_monitors();
            let Some(monitor) = layer::monitor_of(pos, &monitors) else { return };
            opusdm_ui::layer::update(&window, &layer::spec_at(pos, monitor));
            place.set(Some(pos));
        }
    };
    let move_to = Rc::new(move_to);
    {
        let (place, move_to) = (place.clone(), move_to.clone());
        window.connect_map(move |w| {
            if place.get().is_some() { return; }
            move_to(layer::centered(first, (w.width(), w.height())));
        });
    }
    let place = place.clone();
    layer::connect_alt_drag(window, move |dx, dy| {
        let Some(pos) = place.get() else { return };
        move_to(layer::moved(pos, (dx, dy), &current_monitors()));
    });
}
