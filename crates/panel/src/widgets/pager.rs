//! Virtual-desktop pager: one miniature per desktop showing a rectangle for
//! every window on it, the current desktop highlighted. A click switches
//! desktop, dragging a rectangle onto another miniature moves the window
//! there. Names from `_NET_DESKTOP_NAMES` become tooltips.

use gtk4 as gtk;
use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::ewmh::{self, ClientWin};

/// Height of one miniature, in pixels.
const MINI_HEIGHT: i32 = 26;
/// Width used when the screen size is not known yet.
const MINI_FALLBACK_WIDTH: i32 = 42;
/// Smallest rectangle side drawn, so tiny windows stay visible.
const MIN_RECT_SIDE: f64 = 2.0;

/// What the miniatures draw, shared with every draw and drag callback.
#[derive(Default)]
struct PagerState {
    current: u32,
    clients: Vec<ClientWin>,
    active: Option<u32>,
    /// Bounding box of every monitor: `(x, y, width, height)`.
    screen: (i32, i32, i32, i32),
}

/// The pager widget and its live miniature list.
pub struct Pager {
    root: gtk::Box,
    minis: RefCell<Vec<gtk::DrawingArea>>,
    count: Cell<u32>,
    width: Cell<i32>,
    state: Rc<RefCell<PagerState>>,
}

impl Pager {
    /// Build an empty pager. Call [`Pager::update`] with the first snapshot.
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        root.add_css_class("opusdm-panel-pager");
        Pager {
            root,
            minis: RefCell::new(Vec::new()),
            count: Cell::new(0),
            width: Cell::new(0),
            state: Rc::new(RefCell::new(PagerState::default())),
        }
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Apply a snapshot: rebuild the miniatures when the desktop count or
    /// the screen shape changed, then redraw them and refresh the tooltips.
    pub fn update(
        &self,
        count: u32,
        current: u32,
        names: &[String],
        clients: &[ClientWin],
        active: Option<u32>,
    ) {
        let screen = screen_bounds();
        {
            let mut st = self.state.borrow_mut();
            st.current = current;
            st.clients = clients.to_vec();
            st.active = active;
            st.screen = screen;
        }
        let width = mini_width(screen);
        if count != self.count.get() || width != self.width.get() {
            self.rebuild(count, width);
        }
        for (i, mini) in self.minis.borrow().iter().enumerate() {
            mini.set_tooltip_text(names.get(i).map(|s| s.as_str()));
            mini.queue_draw();
        }
    }

    /// Replace every miniature with a fresh `0..count` row.
    fn rebuild(&self, count: u32, width: i32) {
        let mut minis = self.minis.borrow_mut();
        for m in minis.drain(..) {
            self.root.remove(&m);
        }
        for i in 0..count {
            let mini = self.build_mini(i, width);
            self.root.append(&mini);
            minis.push(mini);
        }
        self.count.set(count);
        self.width.set(width);
    }

    /// One miniature: drawing, click to switch, drag source and drop target.
    fn build_mini(&self, index: u32, width: i32) -> gtk::DrawingArea {
        let mini = gtk::DrawingArea::new();
        mini.set_content_width(width);
        mini.set_content_height(MINI_HEIGHT);
        mini.set_valign(gtk::Align::Center);

        let state = self.state.clone();
        mini.set_draw_func(move |_, cr, w, h| draw_mini(&state.borrow(), index, cr, w, h));

        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| ewmh::switch_desktop(index));
        mini.add_controller(click);

        // Dragging a rectangle carries the window id of the one under the pointer.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let state = self.state.clone();
        let area = mini.clone();
        drag.connect_prepare(move |_, x, y| {
            let xid = hit_window(&state.borrow(), index, x, y, area.width(), area.height())?;
            Some(gdk::ContentProvider::for_value(&xid.to_value()))
        });
        mini.add_controller(drag);

        let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        drop.connect_drop(move |_, value, _, _| {
            let Ok(xid) = value.get::<u32>() else { return false };
            ewmh::move_window_to_desktop(xid, index);
            true
        });
        mini.add_controller(drop);
        mini
    }
}

impl Default for Pager {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounding box of every monitor, or an empty box before a display exists.
fn screen_bounds() -> (i32, i32, i32, i32) {
    let Some(display) = gdk::Display::default() else { return (0, 0, 0, 0) };
    let monitors = display.monitors();
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for i in 0..monitors.n_items() {
        let Some(m) = monitors.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()) else { continue };
        let g = m.geometry();
        x0 = x0.min(g.x());
        y0 = y0.min(g.y());
        x1 = x1.max(g.x() + g.width());
        y1 = y1.max(g.y() + g.height());
    }
    if x0 >= x1 || y0 >= y1 {
        return (0, 0, 0, 0);
    }
    (x0, y0, x1 - x0, y1 - y0)
}

/// Miniature width that keeps the screen's aspect ratio at `MINI_HEIGHT`.
fn mini_width(screen: (i32, i32, i32, i32)) -> i32 {
    let (_, _, w, h) = screen;
    if w <= 0 || h <= 0 {
        return MINI_FALLBACK_WIDTH;
    }
    (MINI_HEIGHT * w / h).clamp(24, 80)
}

/// Rectangle (x, y, w, h) of `c` scaled into a `width` x `height` miniature.
fn scaled_rect(st: &PagerState, c: &ClientWin, width: i32, height: i32) -> Option<(f64, f64, f64, f64)> {
    let (sx, sy, sw, sh) = st.screen;
    if sw <= 0 || sh <= 0 {
        return None;
    }
    let fx = f64::from(width) / f64::from(sw);
    let fy = f64::from(height) / f64::from(sh);
    let x = f64::from(c.rect.x - sx) * fx;
    let y = f64::from(c.rect.y - sy) * fy;
    let w = (f64::from(c.rect.w) * fx).max(MIN_RECT_SIDE);
    let h = (f64::from(c.rect.h) * fy).max(MIN_RECT_SIDE);
    Some((x, y, w, h))
}

/// True when `c` is drawn on desktop `index` (minimized windows are hidden).
fn shown_on(c: &ClientWin, index: u32) -> bool {
    !c.minimized && (c.desktop < 0 || c.desktop == i64::from(index))
}

/// The topmost window rectangle of desktop `index` under `(x, y)`.
fn hit_window(st: &PagerState, index: u32, x: f64, y: f64, width: i32, height: i32) -> Option<u32> {
    st.clients.iter().rev().filter(|c| shown_on(c, index)).find_map(|c| {
        let (rx, ry, rw, rh) = scaled_rect(st, c, width, height)?;
        (x >= rx && x <= rx + rw && y >= ry && y <= ry + rh).then_some(c.xid)
    })
}

/// Paint desktop `index`: background, window rectangles and its number.
fn draw_mini(st: &PagerState, index: u32, cr: &gtk::cairo::Context, width: i32, height: i32) {
    let is_current = index == st.current;
    let (r, g, b) = if is_current { (0.29, 0.56, 0.85) } else { (0.24, 0.25, 0.26) };
    cr.set_source_rgba(r, g, b, if is_current { 0.45 } else { 1.0 });
    cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
    let _ = cr.fill();

    for c in st.clients.iter().filter(|c| shown_on(c, index)) {
        let Some((x, y, w, h)) = scaled_rect(st, c, width, height) else { continue };
        let focused = st.active == Some(c.xid);
        cr.rectangle(x, y, w, h);
        cr.set_source_rgba(0.83, 0.84, 0.81, if focused { 0.85 } else { 0.45 });
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.08, 0.09, 0.10, 0.9);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }

    let border = if is_current { (0.29, 0.56, 0.85) } else { (0.45, 0.46, 0.47) };
    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.rectangle(0.5, 0.5, f64::from(width) - 1.0, f64::from(height) - 1.0);
    let _ = cr.stroke();

    cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
    cr.set_font_size(8.0);
    cr.move_to(2.0, 9.0);
    let _ = cr.show_text(&format!("{}", index + 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ewmh::WinRect;

    fn client(xid: u32, desktop: i64, minimized: bool, rect: WinRect) -> ClientWin {
        ClientWin { xid, title: String::new(), desktop, minimized, pid: None, icon: None, rect }
    }

    fn state(clients: Vec<ClientWin>) -> PagerState {
        PagerState { current: 0, clients, active: None, screen: (0, 0, 1000, 500) }
    }

    #[test]
    fn minimized_hidden_and_sticky_shown_everywhere() {
        let r = WinRect { x: 0, y: 0, w: 100, h: 100 };
        assert!(shown_on(&client(1, 2, false, r), 2));
        assert!(!shown_on(&client(1, 2, false, r), 1));
        assert!(shown_on(&client(1, -1, false, r), 3));
        assert!(!shown_on(&client(1, 2, true, r), 2));
    }

    #[test]
    fn rect_is_scaled_to_the_miniature() {
        let st = state(vec![]);
        let c = client(1, 0, false, WinRect { x: 500, y: 250, w: 500, h: 250 });
        let (x, y, w, h) = scaled_rect(&st, &c, 100, 50).unwrap();
        assert_eq!((x, y, w, h), (50.0, 25.0, 50.0, 25.0));
    }

    #[test]
    fn hit_picks_topmost_window_on_that_desktop() {
        let big = WinRect { x: 0, y: 0, w: 1000, h: 500 };
        let small = WinRect { x: 0, y: 0, w: 500, h: 250 };
        let st = state(vec![client(1, 0, false, big), client(2, 0, false, small), client(3, 1, false, small)]);
        assert_eq!(hit_window(&st, 0, 10.0, 5.0, 100, 50), Some(2));
        assert_eq!(hit_window(&st, 0, 90.0, 45.0, 100, 50), Some(1));
        assert_eq!(hit_window(&st, 2, 10.0, 5.0, 100, 50), None);
    }

    #[test]
    fn width_follows_the_screen_aspect_ratio() {
        assert_eq!(mini_width((0, 0, 0, 0)), MINI_FALLBACK_WIDTH);
        assert_eq!(mini_width((0, 0, 1920, 1080)), 46);
    }
}
