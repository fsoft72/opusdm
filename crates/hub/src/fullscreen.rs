//! Is the desktop hidden behind full screen windows? The background rotation
//! and the background videos pause while it is: nobody sees them.

use opusdm_ui::X11Error;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

/// A rectangle on the screen, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Area of the overlap with `other`, in square pixels.
    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let h = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        if w <= 0 || h <= 0 { 0 } else { i64::from(w) * i64::from(h) }
    }
}

/// Share of a monitor, in percent, a window must cover to hide it.
const COVERED_PERCENT: i64 = 90;

/// Whether every monitor in `monitors` is hidden by one of `windows`. No
/// monitor at all counts as not hidden.
pub fn covers_all(monitors: &[Rect], windows: &[Rect]) -> bool {
    if monitors.is_empty() {
        return false;
    }
    monitors.iter().all(|m| {
        let area = i64::from(m.w) * i64::from(m.h);
        windows.iter().any(|w| m.overlap(w) * 100 >= area * COVERED_PERCENT)
    })
}

/// `_NET_WM_DESKTOP` value meaning "on all virtual desktops".
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

/// The geometry of every visible full screen window of the current virtual
/// desktop (minimized ones are left out). Reads the window manager's EWMH
/// properties on the shared X connection, or the compositor's window list
/// on Wayland.
pub fn fullscreen_windows() -> Result<Vec<Rect>, X11Error> {
    if opusdm_ipc::session::is_wayland() { return Ok(wayland_fullscreen_windows()); }
    opusdm_ui::with_x11(|x11| {
        let conn = x11.conn();
        let root = x11.root();
        let (fullscreen, hidden) = (x11.atom("_NET_WM_STATE_FULLSCREEN")?, x11.atom("_NET_WM_STATE_HIDDEN")?);
        let (state, desktop) = (x11.atom("_NET_WM_STATE")?, x11.atom("_NET_WM_DESKTOP")?);
        let read = |window: u32, atom: u32| -> Result<Vec<u32>, X11Error> {
            let reply = conn.get_property(false, window, atom, AtomEnum::ANY, 0, 4096)?.reply()?;
            Ok(reply.value32().map(|v| v.collect()).unwrap_or_default())
        };
        let current = read(root, x11.atom("_NET_CURRENT_DESKTOP")?)?.first().copied();
        let mut found = Vec::new();
        for window in read(root, x11.atom("_NET_CLIENT_LIST")?)? {
            let states = read(window, state)?;
            if !states.contains(&fullscreen) || states.contains(&hidden) {
                continue;
            }
            let on = read(window, desktop)?.first().copied();
            if on != Some(ALL_DESKTOPS) && on != current {
                continue;
            }
            let geometry = conn.get_geometry(window)?.reply()?;
            let origin = conn.translate_coordinates(window, root, 0, 0)?.reply()?;
            found.push(Rect {
                x: i32::from(origin.dst_x),
                y: i32::from(origin.dst_y),
                w: i32::from(geometry.width),
                h: i32::from(geometry.height),
            });
        }
        Ok(found)
    })
}

/// [`fullscreen_windows`] on Wayland. A full screen window covers the
/// monitors it is on; the window list does not tell the workspace of a
/// window, so one on another workspace counts too.
fn wayland_fullscreen_windows() -> Vec<Rect> {
    let Some(session) = opusdm_wayland::shared() else { return Vec::new() };
    fullscreen_rects(&session.snapshot(), &monitor_rects())
}

/// The rectangles of the monitors named in `outputs` (connector name and
/// rect) that a visible full screen window of `snapshot` is on.
pub fn fullscreen_rects(snapshot: &opusdm_wayland::Snapshot, outputs: &[(String, Rect)]) -> Vec<Rect> {
    snapshot
        .toplevels
        .iter()
        .filter(|t| t.fullscreen && !t.minimized)
        .flat_map(|t| t.outputs.iter())
        .filter_map(|name| outputs.iter().find(|(n, _)| n == name).map(|(_, r)| *r))
        .collect()
}

/// Every GDK monitor by connector name, with its rectangle.
fn monitor_rects() -> Vec<(String, Rect)> {
    use gtk4::prelude::*;
    let Some(display) = gtk4::gdk::Display::default() else { return Vec::new() };
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_then(|o| o.downcast::<gtk4::gdk::Monitor>().ok()))
        .filter_map(|m| {
            let g = m.geometry();
            Some((m.connector()?.to_string(), Rect { x: g.x(), y: g.y(), w: g.width(), h: g.height() }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn wayland_full_screen_windows_cover_their_outputs() {
        use opusdm_wayland::{Snapshot, Toplevel};
        let right = Rect { x: 1920, y: 0, w: 1280, h: 1024 };
        let outputs = vec![("eDP-1".to_string(), SCREEN), ("HDMI-A-1".to_string(), right)];
        let t = |fullscreen, minimized, output: &str| Toplevel { fullscreen, minimized, outputs: vec![output.to_string()], ..Default::default() };
        let snapshot = Snapshot { toplevels: vec![t(true, false, "HDMI-A-1"), t(false, false, "eDP-1"), t(true, true, "eDP-1"), t(true, false, "gone")], workspaces: vec![] };
        assert_eq!(fullscreen_rects(&snapshot, &outputs), vec![right]);
    }

    #[test]
    fn a_full_screen_window_hides_the_monitor() {
        assert!(covers_all(&[SCREEN], &[SCREEN]));
    }

    #[test]
    fn a_window_smaller_than_the_monitor_does_not() {
        let window = Rect { x: 100, y: 100, w: 800, h: 600 };
        assert!(!covers_all(&[SCREEN], &[window]));
    }

    #[test]
    fn nothing_is_hidden_without_windows_or_monitors() {
        assert!(!covers_all(&[SCREEN], &[]));
        assert!(!covers_all(&[], &[SCREEN]));
    }

    #[test]
    fn every_monitor_needs_its_own_cover() {
        let left = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let right = Rect { x: 1920, y: 0, w: 1280, h: 1024 };
        assert!(!covers_all(&[left, right], &[left]));
        assert!(covers_all(&[left, right], &[right, left]));
        // One window spanning both monitors hides both.
        let both = Rect { x: 0, y: 0, w: 3200, h: 1080 };
        assert!(covers_all(&[left, right], &[both]));
    }

    #[test]
    fn a_window_a_few_pixels_off_still_covers() {
        let almost = Rect { x: 0, y: 0, w: 1920, h: 1060 };
        assert!(covers_all(&[SCREEN], &[almost]));
        let too_short = Rect { x: 0, y: 0, w: 1920, h: 900 };
        assert!(!covers_all(&[SCREEN], &[too_short]));
    }
}
