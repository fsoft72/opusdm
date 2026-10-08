//! "Start in tray" for the Startup Apps: X11 has no standard way to ask an
//! app to start with no window, so after launching it the hub watches the
//! window manager's client list and closes the first normal window the app
//! opens (`WM_DELETE_WINDOW`, the same as clicking its close button). A tray
//! app answers by hiding itself in the tray; one with no tray quits.
//!
//! A window belongs to the launch when its `_NET_WM_PID` is the launched
//! process, one of its descendants, or a process of its process group (the
//! launch runs in its own group, see `startup_apps::spawn_detached`), so
//! apps started through a wrapper script that forks and exits still match.

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, MapState,
    PropMode, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// How long to wait for the app's first window. An app that opens nothing
/// in this time (it already starts in the tray, or is slow) is left alone,
/// so a window the user opens later from the tray is never closed.
pub const WATCH_TIME: Duration = Duration::from_secs(30);

/// Pause between the window becoming visible and the close request. The
/// window manager lists a window before the app has finished showing it,
/// and a close that arrives then is often ignored (measured with a GTK3
/// app under Openbox: 2 runs out of 3).
const SETTLE_TIME: Duration = Duration::from_millis(300);

/// How long a closed window gets to disappear before it is asked again.
const CLOSE_CHECK_TIME: Duration = Duration::from_secs(1);

/// Close requests sent to one window before giving up.
const MAX_CLOSE_TRIES: u32 = 3;

/// Pixels past the bottom right corner of the screen a window is moved to.
const OFF_SCREEN_GAP: i32 = 100;

/// Poll step while waiting for the window to become visible.
const MAP_POLL: Duration = Duration::from_millis(50);

/// Longest parent chain followed in `/proc` before giving up.
const MAX_ANCESTRY: usize = 64;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_CLIENT_LIST,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_MOVERESIZE_WINDOW,
        _NET_WM_WINDOW_OPACITY,
    }
}

/// Parent pid and process group of a `/proc/<pid>/stat` line. The command
/// name (field 2) is in parentheses and may hold spaces or `)`, so fields are
/// counted after the last `)`.
pub fn parse_stat(stat: &str) -> Option<(u32, u32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((ppid, pgrp))
}

/// Whether `pid` is `launched`, a descendant of it, or in its process group.
/// `stat_of` gives a pid's `(ppid, pgrp)`, `None` when it is gone.
pub fn is_from_launch(pid: u32, launched: u32, stat_of: impl Fn(u32) -> Option<(u32, u32)>) -> bool {
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        if current == launched {
            return true;
        }
        if current <= 1 {
            return false;
        }
        let Some((ppid, pgrp)) = stat_of(current) else { return false };
        if pgrp == launched {
            return true;
        }
        current = ppid;
    }
    false
}

/// Whether a window with these `_NET_WM_WINDOW_TYPE` atoms is a normal
/// window: no type at all, or `normal` first. Splash screens, dialogs and
/// the like are left alone.
pub fn is_normal_window(types: &[u32], normal: u32) -> bool {
    types.first().is_none_or(|t| *t == normal)
}

/// `(ppid, pgrp)` of a live process, read from `/proc`.
fn proc_stat(pid: u32) -> Option<(u32, u32)> {
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Watches the client list for the first window of one launch. Built
/// *before* the launch, so a window that appears at once is not missed and
/// the windows already open are never touched.
pub struct Watch {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    known: Vec<Window>,
}

impl Watch {
    /// Connect to the X server and remember the windows open now.
    pub fn new() -> Result<Self, opusdm_x11::X11Error> {
        let opusdm_x11::Session { conn, root, .. } = opusdm_x11::connect()?;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
        conn.flush()?;
        let mut watch = Self { conn, root, atoms, known: Vec::new() };
        watch.known = watch.client_list()?;
        Ok(watch)
    }

    /// Close the first normal window of the process `launched` that the
    /// window manager takes in within `WATCH_TIME`, then stop. Returns the
    /// closed window, or `None` when none came in time or it would not close.
    pub fn close_first_window(mut self, launched: u32) -> Result<Option<Window>, opusdm_x11::X11Error> {
        let deadline = Instant::now() + WATCH_TIME;
        loop {
            while let Some(event) = self.conn.poll_for_event()? {
                let Event::PropertyNotify(e) = event else { continue };
                if e.window != self.root || e.atom != self.atoms._NET_CLIENT_LIST {
                    continue;
                }
                if let Some(window) = self.check_new_windows(launched)? {
                    return self.close_until_hidden(window, deadline);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.wait_for_event(left);
        }
    }

    /// Look at the windows that joined the client list since the last look;
    /// return the first one that belongs to `launched`.
    fn check_new_windows(&mut self, launched: u32) -> Result<Option<Window>, opusdm_x11::X11Error> {
        let list = self.client_list()?;
        let fresh: Vec<Window> = list.iter().copied().filter(|w| !self.known.contains(w)).collect();
        self.known = list;
        for window in fresh {
            if self.is_launched_window(window, launched)? {
                return Ok(Some(window));
            }
        }
        Ok(None)
    }

    /// Whether `window` is a normal window of the launch. A window that is
    /// already gone, or has no `_NET_WM_PID`, does not match.
    fn is_launched_window(&self, window: Window, launched: u32) -> Result<bool, opusdm_x11::X11Error> {
        let Some(pid) = self.cardinals(window, self.atoms._NET_WM_PID, AtomEnum::CARDINAL)?.first().copied() else {
            return Ok(false);
        };
        if !is_from_launch(pid, launched, proc_stat) {
            return Ok(false);
        }
        let types = self.cardinals(window, self.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM)?;
        Ok(is_normal_window(&types, self.atoms._NET_WM_WINDOW_TYPE_NORMAL))
    }

    /// Move `window` off screen at once, wait until it is visible, then ask
    /// it to close until it is no longer visible, up to `MAX_CLOSE_TRIES`
    /// times. Whatever happens, the window goes back to where it was, so it
    /// shows in the right place when the app shows it again (from its tray
    /// icon) or when it refused to close. Returns `window` once it is hidden
    /// or gone, `None` when it stayed open or could not be asked.
    fn close_until_hidden(&self, window: Window, deadline: Instant) -> Result<Option<Window>, opusdm_x11::X11Error> {
        let home = self.move_off_screen(window)?;
        let closed = self.close_when_visible(window, deadline);
        if let Some((x, y)) = home {
            self.move_back(window, x, y)?;
        }
        closed
    }

    /// The close loop of `close_until_hidden`, with the window off screen.
    fn close_when_visible(&self, window: Window, deadline: Instant) -> Result<Option<Window>, opusdm_x11::X11Error> {
        loop {
            match self.is_viewable(window)? {
                None => return Ok(Some(window)),
                Some(true) => break,
                Some(false) if Instant::now() >= deadline => return Ok(None),
                Some(false) => std::thread::sleep(MAP_POLL),
            }
        }
        for _ in 0..MAX_CLOSE_TRIES {
            std::thread::sleep(SETTLE_TIME);
            if !self.close(window)? {
                return Ok(None);
            }
            std::thread::sleep(CLOSE_CHECK_TIME);
            if self.is_viewable(window)? != Some(true) {
                return Ok(Some(window));
            }
        }
        opusdm_ipc::err_log!("opusdm-hub", "start in tray: window {window:#x} still open after {MAX_CLOSE_TRIES} close requests");
        Ok(None)
    }

    /// Hide `window` while the app finishes showing it: make it fully
    /// transparent (a compositor such as picom honours
    /// `_NET_WM_WINDOW_OPACITY`) and ask the window manager to put it past
    /// the bottom right corner of the screen. Openbox keeps a few pixels of
    /// a window on screen whatever a client asks, so the move alone leaves a
    /// small corner visible. Returns its position before the move (root
    /// coordinates of the client area), `None` when it is gone.
    fn move_off_screen(&self, window: Window) -> Result<Option<(i32, i32)>, opusdm_x11::X11Error> {
        let Some(pos) = opusdm_x11::gone(self.conn.translate_coordinates(window, self.root, 0, 0)?.reply())? else {
            return Ok(None);
        };
        let (width, height) = self
            .conn
            .setup()
            .roots
            .iter()
            .find(|s| s.root == self.root)
            .map_or((0, 0), |s| (s.width_in_pixels, s.height_in_pixels));
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            self.atoms._NET_WM_WINDOW_OPACITY,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        self.request_move(window, i32::from(width) + OFF_SCREEN_GAP, i32::from(height) + OFF_SCREEN_GAP)?;
        Ok(Some((i32::from(pos.dst_x), i32::from(pos.dst_y))))
    }

    /// Make `window` opaque again and put it back at `x`, `y`. A hidden (withdrawn) window is no
    /// longer managed, so it is moved directly; the window manager starts
    /// from there when the app maps it again. A visible one goes through
    /// the window manager.
    fn move_back(&self, window: Window, x: i32, y: i32) -> Result<(), opusdm_x11::X11Error> {
        self.conn.delete_property(window, self.atoms._NET_WM_WINDOW_OPACITY)?;
        match self.is_viewable(window)? {
            None => {}
            Some(true) => self.request_move(window, x, y)?,
            Some(false) => {
                self.conn.configure_window(window, &ConfigureWindowAux::new().x(x).y(y))?;
                self.conn.flush()?;
            }
        }
        Ok(())
    }

    /// `_NET_MOVERESIZE_WINDOW` with only x and y given.
    fn request_move(&self, window: Window, x: i32, y: i32) -> Result<(), opusdm_x11::X11Error> {
        let flags = (1 << 8) | (1 << 9) | (opusdm_x11::SOURCE_APPLICATION << 12);
        let data = [flags, x as u32, y as u32, 0, 0];
        opusdm_x11::send_to_root(&self.conn, self.root, window, self.atoms._NET_MOVERESIZE_WINDOW, data)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Whether `window` is mapped and visible; `None` when it is gone.
    fn is_viewable(&self, window: Window) -> Result<Option<bool>, opusdm_x11::X11Error> {
        let attrs = opusdm_x11::gone(self.conn.get_window_attributes(window)?.reply())?;
        Ok(attrs.map(|a| a.map_state == MapState::VIEWABLE))
    }

    /// Ask `window` to close with `WM_DELETE_WINDOW`. A window that does not
    /// take part in that protocol is not closed (the only other way is to
    /// kill the app) and `false` is returned.
    fn close(&self, window: Window) -> Result<bool, opusdm_x11::X11Error> {
        let protocols = self.cardinals(window, self.atoms.WM_PROTOCOLS, AtomEnum::ATOM)?;
        if !protocols.contains(&self.atoms.WM_DELETE_WINDOW) {
            opusdm_ipc::err_log!("opusdm-hub", "start in tray: window {window:#x} cannot be closed politely, left open");
            return Ok(false);
        }
        let data = [self.atoms.WM_DELETE_WINDOW, x11rb::CURRENT_TIME, 0, 0, 0];
        let event = ClientMessageEvent::new(32, window, self.atoms.WM_PROTOCOLS, data);
        self.conn.send_event(false, window, EventMask::NO_EVENT, event)?;
        self.conn.flush()?;
        Ok(true)
    }

    /// The window manager's `_NET_CLIENT_LIST`.
    fn client_list(&self) -> Result<Vec<Window>, opusdm_x11::X11Error> {
        self.cardinals(self.root, self.atoms._NET_CLIENT_LIST, AtomEnum::WINDOW)
    }

    /// A 32 bit list property of `window`; empty when absent or when the
    /// window is gone.
    fn cardinals(&self, window: Window, property: u32, kind: AtomEnum) -> Result<Vec<u32>, opusdm_x11::X11Error> {
        let reply = opusdm_x11::gone(self.conn.get_property(false, window, property, kind, 0, 1024)?.reply())?;
        Ok(reply.and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default())
    }

    /// Sleep until the X connection has data or `timeout` passes.
    fn wait_for_event(&self, timeout: Duration) {
        let mut fd = libc::pollfd { fd: self.conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // EINTR or a timeout just means "look again", which the caller does.
        unsafe { libc::poll(&mut fd, 1, ms) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_stat_reads_ppid_and_pgrp_after_the_command_name() {
        let stat = "4911 (megasync) S 3547 4911 4911 0 -1 4194560 12 0";
        assert_eq!(parse_stat(stat), Some((3547, 4911)));
    }

    #[test]
    fn parse_stat_survives_spaces_and_parens_in_the_name() {
        let stat = "77 (Web Content (x)) R 70 60 60 0";
        assert_eq!(parse_stat(stat), Some((70, 60)));
        assert_eq!(parse_stat("garbage"), None);
    }

    /// pid -> (ppid, pgrp) of a fake process table.
    fn table(rows: &[(u32, u32, u32)]) -> impl Fn(u32) -> Option<(u32, u32)> {
        let map: HashMap<u32, (u32, u32)> = rows.iter().map(|&(p, pp, g)| (p, (pp, g))).collect();
        move |pid| map.get(&pid).copied()
    }

    #[test]
    fn the_launched_process_and_its_descendants_match() {
        // 100 launched (group 100) -> 101 -> 102 that moved to group 102.
        let stat = table(&[(100, 1, 100), (101, 100, 100), (102, 101, 102)]);
        assert!(is_from_launch(100, 100, &stat));
        assert!(is_from_launch(101, 100, &stat));
        assert!(is_from_launch(102, 100, &stat));
    }

    #[test]
    fn an_orphan_of_the_launch_group_matches() {
        // The wrapper 100 exited; 105 was reparented to init but kept group 100.
        let stat = table(&[(105, 1, 100)]);
        assert!(is_from_launch(105, 100, &stat));
    }

    #[test]
    fn unrelated_and_vanished_processes_do_not_match() {
        let stat = table(&[(200, 1, 200), (300, 200, 200)]);
        assert!(!is_from_launch(300, 100, &stat));
        assert!(!is_from_launch(999, 100, &stat), "a pid with no /proc entry");
        assert!(!is_from_launch(1, 100, &stat));
    }

    #[test]
    fn a_parent_loop_does_not_hang() {
        let stat = table(&[(10, 11, 10), (11, 10, 11)]);
        assert!(!is_from_launch(10, 100, &stat));
    }

    #[test]
    fn only_untyped_and_normal_windows_are_closed() {
        let (normal, splash, dialog) = (1, 2, 3);
        assert!(is_normal_window(&[], normal));
        assert!(is_normal_window(&[normal], normal));
        assert!(!is_normal_window(&[splash], normal));
        assert!(!is_normal_window(&[dialog, normal], normal), "the first type is the preferred one");
    }
}
