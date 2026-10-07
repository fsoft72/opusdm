//! X11 helpers for the GTK processes. The plumbing itself lives in
//! `opusdm-x11` (no GTK) and is re-exported here; only what needs the GTK main
//! loop is defined in this module.

pub use opusdm_x11::{with_x11, wrapped_desktop, x11_sync, X11Error, PLACEMENT_RETRY_MS, X11};

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
