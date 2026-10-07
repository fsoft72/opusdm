//! The docked-icon placeholder strip: one fixed-size square per currently
//! docked tray icon, in dock order. Each square reserves screen space; the
//! real pixels are the client's own X window, drawn directly on top by the
//! X server once `systray.rs` places its container there - see `update`
//! and `reposition_now`.

use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use crate::systray::{TrayCmd, TraySender};

pub struct Tray {
    root: gtk::Box,
    icon_size: i32,
    cmd_tx: TraySender,
    /// Dock order, mirrors what `update` was last called with.
    order: RefCell<Vec<u32>>,
    /// One placeholder square per docked xid, keyed the same way.
    placeholders: RefCell<HashMap<u32, gtk::Box>>,
    /// The owning bar's root-screen position, set once at construction by
    /// `build_bar_window` (see `lib.rs`) and never changed after (the panel
    /// doesn't move without a full rebuild, which rebuilds the Tray too).
    origin: Cell<(i32, i32)>,
}

impl Tray {
    /// Build an empty tray strip. `icon_size` is `systray::icon_square_size`
    /// applied to the panel height; `cmd_tx` is the sender half returned by
    /// `systray::spawn`.
    pub fn new(icon_size: i32, cmd_tx: TraySender) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        root.set_valign(gtk::Align::Center);
        Rc::new(Tray {
            root,
            icon_size,
            cmd_tx,
            order: RefCell::new(Vec::new()),
            placeholders: RefCell::new(HashMap::new()),
            origin: Cell::new((0, 0)),
        })
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Record the bar's root-screen origin, used to translate placeholder
    /// bounds into absolute coordinates when placing real icon windows.
    pub fn set_origin(&self, x: i32, y: i32) {
        self.origin.set((x, y));
    }

    /// Reconcile the displayed placeholders with `order` (the current dock
    /// order from `TrayEvent`s folded into `PanelState`). Adds a square for
    /// every xid not yet shown, removes one for every xid no longer docked,
    /// preserving relative order for the rest.
    pub fn update(self: &Rc<Self>, order: &[u32]) {
        if *self.order.borrow() == order {
            return;
        }

        let mut placeholders = self.placeholders.borrow_mut();
        let previous: Vec<u32> = self.order.borrow().clone();

        for xid in &previous {
            if !order.contains(xid) {
                if let Some(square) = placeholders.remove(xid) {
                    self.root.remove(&square);
                }
            }
        }
        for &xid in order {
            if !placeholders.contains_key(&xid) {
                let square = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                square.set_size_request(self.icon_size, self.icon_size);
                self.root.append(&square);
                placeholders.insert(xid, square);
            }
        }
        drop(placeholders);

        *self.order.borrow_mut() = order.to_vec();

        let this = self.clone();
        gtk::glib::idle_add_local_once(move || this.reposition_now());
    }

    /// Recompute every docked icon's absolute screen rect from its
    /// placeholder's current layout (relative to the bar window) and tell `systray.rs` to move the real
    /// window there. Safe to call any time the placeholders are realized;
    /// a placeholder with no allocation yet (bounds unavailable) is skipped -
    /// the next reposition pass (next `update`, or the panel's settle-delay
    /// timers) will place it once layout has run.
    pub fn reposition_now(&self) {
        let (origin_x, origin_y) = self.origin.get();
        // Bounds must be relative to the bar window (not to the tray box), or
        // the tray's own offset inside the bar is lost and every icon lands at
        // the top-left corner of the bar.
        let Some(window) = self.root.root() else { return };
        let placeholders = self.placeholders.borrow();
        for (&xid, square) in placeholders.iter() {
            let Some(bounds) = square.compute_bounds(window.upcast_ref::<gtk::Widget>()) else { continue };
            let x = origin_x + bounds.x() as i32;
            let y = origin_y + bounds.y() as i32;
            let w = bounds.width() as i32;
            let h = bounds.height() as i32;
            let _ = self.cmd_tx.send(TrayCmd::PlaceIcon { xid, x, y, w, h });
        }
    }
}
