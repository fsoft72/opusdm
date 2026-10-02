//! Live tracking of the X11 screen size.
//!
//! GTK's `GdkMonitor::notify::geometry` is not a reliable signal here: under
//! Xephyr's `-resizeable`, resizing the host window changes the screen size
//! without GDK ever refreshing its monitor list, so windows pinned to the
//! startup geometry stay pinned forever. The reason is that Xephyr updates
//! its RandR state only *after* announcing the resize, so the RandR query
//! GDK runs when the event lands still returns the old size - and GDK never
//! asks again.
//!
//! The X server also sends a plain `ConfigureNotify` on the root window for
//! every screen size change, and that one carries the new size itself. This
//! module listens for it on a dedicated connection and hands the size to the
//! GTK main loop.

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, MonitorInfo};
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, Window};
use x11rb::protocol::Event;

/// Call `on_resize(width, height)` on the GTK main thread with the primary
/// monitor's geometry every time the X11 screen changes size (a real mode
/// switch, or a Xephyr host-window resize). The initial size is not
/// delivered - callers already apply it at startup.
///
/// Must be called from the thread running the GTK main loop. Does nothing
/// useful when not running under X11: the watcher thread just fails to
/// connect and exits.
pub fn watch_screen_size<F>(on_resize: F)
where
    F: Fn(i32, i32) + 'static,
{
    let (tx, rx) = async_channel::unbounded::<(i32, i32)>();

    std::thread::spawn(move || {
        if let Err(e) = pump(&tx) {
            opusdm_ipc::err_log!("opusdm", "screen size watcher stopped: {e}");
        }
    });

    glib::spawn_future_local(async move {
        while let Ok((w, h)) = rx.recv().await {
            on_resize(w, h);
        }
    });
}

/// Blocking loop: forward every root-window resize to `tx` as the primary
/// monitor's new geometry. Returns when the connection or the channel dies.
fn pump(tx: &async_channel::Sender<(i32, i32)>) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let mut last = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);

    // Root ConfigureNotify is what a screen resize looks like to a plain X
    // client; it needs no extension and Xephyr always sends it.
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?;
    conn.flush()?;

    loop {
        let Event::ConfigureNotify(ev) = conn.wait_for_event()? else { continue };
        if ev.window != root {
            continue;
        }
        let root_size = (ev.width as i32, ev.height as i32);
        if root_size.0 <= 0 || root_size.1 <= 0 || root_size == last {
            continue;
        }
        last = root_size;
        tx.send_blocking(primary_geometry(&conn, root, root_size))?;
    }
}

/// Size of the primary monitor, given `root_size` as just reported by the
/// root window's `ConfigureNotify`. Falls back to `root_size` whenever RandR
/// is unavailable or reports nothing.
fn primary_geometry(conn: &impl Connection, root: Window, root_size: (i32, i32)) -> (i32, i32) {
    let Ok(cookie) = randr::get_monitors(conn, root, true) else { return root_size };
    let Ok(reply) = cookie.reply() else { return root_size };
    choose_monitor(&reply.monitors, root_size)
}

/// Pick the primary monitor out of `monitors`, or the first one.
///
/// On a single-monitor screen the monitor *is* the root window, so
/// `root_size` is used directly: RandR can still be a step behind at the
/// instant the resize event arrives (Xephyr updates its RandR state only
/// after sending the root `ConfigureNotify`), while the event itself always
/// carries the new size. Only a multi-head screen needs RandR to tell the
/// primary monitor apart from the whole root window, and there the size
/// changes come from RandR clients, which commit before the event is sent.
fn choose_monitor(monitors: &[MonitorInfo], root_size: (i32, i32)) -> (i32, i32) {
    if monitors.len() < 2 {
        return root_size;
    }
    monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .map(|m| (m.width as i32, m.height as i32))
        .filter(|(w, h)| *w > 0 && *h > 0)
        .unwrap_or(root_size)
}

/// Distinct on-screen monitor rectangles `(x, y, width, height)`, in root
/// window coordinates. Monitors reporting an identical rect (mirrored
/// outputs) collapse to one entry: X draws a single pixel buffer for them, so
/// a window per raw monitor would just stack duplicates at the same spot.
/// Works unchanged for horizontal, vertical or mixed layouts - callers place
/// one window per returned rect, using its own `(x, y)` as origin.
pub fn list_monitors(display: &gdk::Display) -> Vec<(i32, i32, i32, i32)> {
    rects_from_model(&display.monitors())
}

fn rects_from_model(monitors: &gio::ListModel) -> Vec<(i32, i32, i32, i32)> {
    let rects = (0..monitors.n_items())
        .filter_map(|i| monitors.item(i))
        .filter_map(|o| o.downcast::<gdk::Monitor>().ok())
        .map(|m| {
            let g = m.geometry();
            (g.x(), g.y(), g.width(), g.height())
        })
        .filter(|(_, _, w, h)| *w > 0 && *h > 0)
        .collect();
    dedupe_rects(rects)
}

/// Collapse duplicate rects (mirrored monitors), keeping first-seen order.
fn dedupe_rects(rects: Vec<(i32, i32, i32, i32)>) -> Vec<(i32, i32, i32, i32)> {
    let mut out: Vec<(i32, i32, i32, i32)> = Vec::new();
    for r in rects {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Call `on_change` with the current `list_monitors` result every time the
/// monitor list changes (hotplug) or any monitor's geometry changes
/// (resolution switch, or a mirror layout toggled at runtime). Not called
/// immediately - callers already apply the initial layout at startup.
pub fn watch_monitors<F>(display: &gdk::Display, on_change: F)
where
    F: Fn(Vec<(i32, i32, i32, i32)>) + 'static,
{
    let on_change: Rc<dyn Fn(Vec<(i32, i32, i32, i32)>)> = Rc::new(on_change);
    let monitors = display.monitors();
    let handlers: Rc<RefCell<Vec<(gdk::Monitor, glib::SignalHandlerId)>>> =
        Rc::new(RefCell::new(Vec::new()));

    fn rewire(
        monitors: &gio::ListModel,
        on_change: &Rc<dyn Fn(Vec<(i32, i32, i32, i32)>)>,
        handlers: &Rc<RefCell<Vec<(gdk::Monitor, glib::SignalHandlerId)>>>,
    ) {
        for (m, id) in handlers.borrow_mut().drain(..) {
            m.disconnect(id);
        }
        for i in 0..monitors.n_items() {
            let Some(m) = monitors.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()) else {
                continue;
            };
            let monitors = monitors.clone();
            let on_change = on_change.clone();
            let id = m.connect_notify_local(Some("geometry"), move |_, _| {
                on_change(rects_from_model(&monitors));
            });
            handlers.borrow_mut().push((m, id));
        }
    }

    rewire(&monitors, &on_change, &handlers);
    {
        let handlers = handlers.clone();
        let on_change = on_change.clone();
        monitors.connect_items_changed(move |monitors, _, _, _| {
            rewire(monitors, &on_change, &handlers);
            on_change(rects_from_model(monitors));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_rects_pass_through_unchanged() {
        let rects = vec![(0, 0, 1920, 1080), (1920, 0, 1280, 1024)];
        assert_eq!(dedupe_rects(rects.clone()), rects);
    }

    #[test]
    fn identical_rects_collapse_to_one_mirror_case() {
        let rects = vec![(0, 0, 1920, 1080), (0, 0, 1920, 1080)];
        assert_eq!(dedupe_rects(rects), vec![(0, 0, 1920, 1080)]);
    }

    #[test]
    fn vertical_layout_keeps_both_distinct_rects() {
        let rects = vec![(0, 0, 1920, 1080), (0, 1080, 1920, 1080)];
        assert_eq!(dedupe_rects(rects.clone()), rects);
    }

    /// Build a `MonitorInfo` carrying only the fields `choose_monitor` reads.
    fn monitor(primary: bool, width: u16, height: u16) -> MonitorInfo {
        MonitorInfo {
            name: 0,
            primary,
            automatic: true,
            x: 0,
            y: 0,
            width,
            height,
            width_in_millimeters: 0,
            height_in_millimeters: 0,
            outputs: Vec::new(),
        }
    }

    #[test]
    fn the_primary_monitor_wins_over_the_first_one() {
        let monitors = [monitor(false, 1280, 1024), monitor(true, 1920, 1080)];
        assert_eq!(choose_monitor(&monitors, (3200, 1080)), (1920, 1080));
    }

    #[test]
    fn the_first_monitor_is_used_when_none_is_primary() {
        let monitors = [monitor(false, 1280, 1024), monitor(false, 1920, 1080)];
        assert_eq!(choose_monitor(&monitors, (3200, 1080)), (1280, 1024));
    }

    #[test]
    fn a_single_monitor_takes_the_root_size_even_when_randr_is_stale() {
        let monitors = [monitor(true, 1024, 768)];
        assert_eq!(choose_monitor(&monitors, (1280, 900)), (1280, 900));
    }

    #[test]
    fn an_empty_or_degenerate_list_falls_back_to_the_root_size() {
        assert_eq!(choose_monitor(&[], (1100, 850)), (1100, 850));
        let degenerate = [monitor(true, 0, 0), monitor(false, 0, 0)];
        assert_eq!(choose_monitor(&degenerate, (1100, 850)), (1100, 850));
    }
}
