//! X11 helpers for the GTK processes. The plumbing itself lives in
//! `opusdm-x11` (no GTK) and is re-exported here; only what needs the GTK main
//! loop is defined in this module.

pub use opusdm_x11::{with_x11, wrapped_desktop, x11_sync, X11Error, PLACEMENT_RETRY_MS, X11};

use gtk4::prelude::*;

/// The X11 window id backing a GTK window (or popover), if the session is X11
/// and the window is realized. `None` under Wayland or before realization.
pub fn x11_window_id(window: &impl IsA<gtk4::Native>) -> Option<u32> {
    let surface = window.as_ref().surface()?;
    let x11_surface = surface.downcast::<gdk4_x11::X11Surface>().ok()?;
    Some(x11_surface.xid() as u32)
}

/// Run `f` once after each of `delays_ms` milliseconds, on the GTK main loop.
/// Used to re-assert a window's placement once the window manager has
/// settled (see [`PLACEMENT_RETRY_MS`]).
pub fn retry_after_map(delays_ms: &[u64], f: impl Fn() + 'static) {
    let f = std::rc::Rc::new(f);
    for &delay in delays_ms {
        let f = f.clone();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || f());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrealized_window_has_no_x11_id() {
        if gtk4::init().is_err() {
            return;
        }
        assert_eq!(x11_window_id(&gtk4::Window::new()), None);
    }
}
