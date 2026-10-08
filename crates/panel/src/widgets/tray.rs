//! The docked-icon placeholder strip: one fixed-size square per currently
//! docked tray icon, in dock order. Each square reserves screen space; the
//! real pixels are the client's own X window, drawn directly on top by the
//! X server once `systray.rs` places its container there - see `update`
//! and `reposition_now`. The squares move whenever anything else in the bar
//! changes width (an SNI icon appears, the clock text grows), so after every
//! painted frame the tray re-reads their rects and moves the icon windows
//! that are no longer under their square (`follow_layout`).

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
    /// The last rect sent for each docked xid, so a repaint that moved
    /// nothing sends nothing.
    placed: RefCell<HashMap<u32, (i32, i32, i32, i32)>>,
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
            placed: RefCell::new(HashMap::new()),
        })
    }

    /// Re-place moved icons after every frame the bar paints. Frames are
    /// only painted when something changed, so an idle bar costs nothing;
    /// a layout change always paints, so it is never missed. Hooked to the
    /// bar window's frame clock once the tray is realized (a rebuilt bar
    /// builds a new `Tray`, so the hook never outlives its window).
    pub fn follow_layout(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.root.connect_realize(move |root| {
            let Some(clock) = root.frame_clock() else { return };
            let weak = weak.clone();
            clock.connect_after_paint(move |_| {
                if let Some(tray) = weak.upgrade() {
                    tray.place_icons(false);
                }
            });
        });
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
            placeholders.entry(xid).or_insert_with(|| {
                let square = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                square.set_size_request(self.icon_size, self.icon_size);
                self.root.append(&square);
                square
            });
        }
        drop(placeholders);
        self.placed.borrow_mut().retain(|xid, _| order.contains(xid));

        *self.order.borrow_mut() = order.to_vec();

        let this = self.clone();
        gtk::glib::idle_add_local_once(move || this.reposition_now());
    }

    /// Recompute every docked icon's absolute screen rect from its
    /// placeholder's current layout (relative to the bar window) and tell
    /// `systray.rs` to move (and raise) the real window there, even when it
    /// did not move: callers use this after re-pinning or raising the bar,
    /// which can bury the icons. Safe to call any time; a placeholder with
    /// no allocation yet is skipped until a later pass.
    pub fn reposition_now(&self) {
        self.place_icons(true);
    }

    /// Send a `PlaceIcon` for every placeholder whose rect differs from the
    /// last one sent, or for all of them when `force` is set.
    fn place_icons(&self, force: bool) {
        let (origin_x, origin_y) = self.origin.get();
        // Bounds must be relative to the bar window (not to the tray box), or
        // the tray's own offset inside the bar is lost and every icon lands at
        // the top-left corner of the bar.
        let Some(window) = self.root.root() else { return };
        let placeholders = self.placeholders.borrow();
        let mut placed = self.placed.borrow_mut();
        for (&xid, square) in placeholders.iter() {
            let Some(bounds) = square.compute_bounds(window.upcast_ref::<gtk::Widget>()) else { continue };
            let rect = (
                origin_x + bounds.x() as i32,
                origin_y + bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            );
            if !force && placed.get(&xid) == Some(&rect) {
                continue;
            }
            let (x, y, w, h) = rect;
            if self.cmd_tx.send(TrayCmd::PlaceIcon { xid, x, y, w, h }).is_ok() {
                placed.insert(xid, rect);
            }
        }
    }
}
