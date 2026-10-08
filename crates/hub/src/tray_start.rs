//! "Start in tray" for the Startup Apps: X11 has no standard way to ask an
//! app to start with no window, so after launching it the hub watches the
//! window manager's client list for the first normal window the app opens.
//! The window opens normally, where the app wants it; the hub minimizes it
//! (`WM_CHANGE_STATE`) and then closes it (`WM_DELETE_WINDOW`, the same as
//! clicking its close button). A tray app answers by hiding itself in the
//! tray; one with no tray quits. The window is never moved, so it keeps its
//! real place when it is opened again from the tray icon.
//!
//! Toolkits remember the minimize: Qt, told by the window manager that the
//! window is hidden, maps it minimized again on its next plain `show()`.
//! Once the app has hidden the window, its `_NET_WM_STATE` from before the
//! minimize is written back without `_NET_WM_STATE_HIDDEN`; the toolkit
//! reads it and forgets the minimize (and keeps a maximize).
//!
//! A window belongs to the launch when its `_NET_WM_PID` is the launched
//! process, one of its descendants, or a process of its process group (the
//! launch runs in its own group, see `startup_apps::spawn_detached`), so
//! apps started through a wrapper script that forks and exits still match.

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt, EventMask, MapState, PropMode, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// How long to wait for the app's first window. An app that opens nothing
/// in this time (it already starts in the tray, or is slow) is left alone,
/// so a window the user opens later from the tray is never closed.
pub const WATCH_TIME: Duration = Duration::from_secs(30);

/// Pause between the window becoming visible and the minimize and close
/// requests. The window manager lists a window before the app has finished
/// showing it, and a close that arrives then is often ignored (measured
/// with a GTK3 app under Openbox: 2 runs out of 3).
const SETTLE_TIME: Duration = Duration::from_millis(300);

/// How long a closed window gets to disappear before it is asked again.
const CLOSE_CHECK_TIME: Duration = Duration::from_secs(1);

/// Close requests sent to one window before giving up.
const MAX_CLOSE_TRIES: u32 = 3;

/// ICCCM `IconicState`: the `WM_CHANGE_STATE` value that minimizes a window.
const ICONIC_STATE: u32 = 3;

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
        WM_CHANGE_STATE,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
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

/// Data words of the `WM_CHANGE_STATE` message that minimizes a window.
pub fn iconify_data() -> [u32; 5] {
    [ICONIC_STATE, 0, 0, 0, 0]
}

/// `states` (a `_NET_WM_STATE` atom list) without `unwanted`.
pub fn without_state(states: &[u32], unwanted: u32) -> Vec<u32> {
    states.iter().copied().filter(|s| *s != unwanted).collect()
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

    /// Wait until `window` is visible, then minimize it and ask it to close
    /// until the app has hidden it (it left the window manager's client list)
    /// or it is gone, up to `MAX_CLOSE_TRIES` times. A window that is still
    /// visible before a try (the app or the window manager ignored the first
    /// minimize request) is minimized again. The window is never moved, so it
    /// shows at its own place when the app shows it again from its tray icon;
    /// once hidden, its state from before the minimize is given back (see the
    /// module doc). One that refuses to close stays minimized. Returns
    /// `window` once it is hidden or gone, `None` when it stayed or could not
    /// be asked.
    fn close_until_hidden(&self, window: Window, deadline: Instant) -> Result<Option<Window>, opusdm_x11::X11Error> {
        loop {
            match self.is_viewable(window)? {
                None => return Ok(Some(window)),
                Some(true) => break,
                Some(false) if Instant::now() >= deadline => return Ok(None),
                Some(false) => std::thread::sleep(MAP_POLL),
            }
        }
        let mut states = None;
        for _ in 0..MAX_CLOSE_TRIES {
            std::thread::sleep(SETTLE_TIME);
            if self.is_viewable(window)? == Some(true) {
                if states.is_none() {
                    states = Some(self.cardinals(window, self.atoms._NET_WM_STATE, AtomEnum::ATOM)?);
                }
                self.iconify(window)?;
            }
            if !self.close(window)? {
                return Ok(None);
            }
            std::thread::sleep(CLOSE_CHECK_TIME);
            if !self.is_managed(window)? {
                self.unminimize_hidden(window, &states.unwrap_or_default())?;
                return Ok(Some(window));
            }
        }
        opusdm_ipc::err_log!("opusdm-hub", "start in tray: window {window:#x} still open after {MAX_CLOSE_TRIES} close requests, left minimized");
        Ok(None)
    }

    /// Ask the window manager to minimize `window` (ICCCM `WM_CHANGE_STATE`
    /// with `IconicState`).
    fn iconify(&self, window: Window) -> Result<(), opusdm_x11::X11Error> {
        opusdm_x11::send_to_root(&self.conn, self.root, window, self.atoms.WM_CHANGE_STATE, iconify_data())?;
        self.conn.flush()?;
        Ok(())
    }

    /// Write `states` (the window's `_NET_WM_STATE` before the minimize)
    /// back on the hidden `window`, without `_NET_WM_STATE_HIDDEN`. The
    /// window manager erased the property when the app hid the window; the
    /// new value tells the toolkit the window is no longer minimized. A
    /// window that is gone is left alone.
    fn unminimize_hidden(&self, window: Window, states: &[u32]) -> Result<(), opusdm_x11::X11Error> {
        if self.is_viewable(window)?.is_none() {
            return Ok(());
        }
        let kept = without_state(states, self.atoms._NET_WM_STATE_HIDDEN);
        self.conn.change_property32(PropMode::REPLACE, window, self.atoms._NET_WM_STATE, AtomEnum::ATOM, &kept)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Whether the window manager still manages `window`: it is in
    /// `_NET_CLIENT_LIST`. A minimized window still is; one the app hid
    /// (withdrew) or destroyed is not.
    fn is_managed(&self, window: Window) -> Result<bool, opusdm_x11::X11Error> {
        Ok(self.client_list()?.contains(&window))
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
            opusdm_ipc::err_log!("opusdm-hub", "start in tray: window {window:#x} cannot be closed politely, left minimized");
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
    fn the_minimize_request_asks_for_the_iconic_state() {
        assert_eq!(iconify_data(), [3, 0, 0, 0, 0]);
    }

    #[test]
    fn the_restored_state_drops_only_hidden() {
        let (hidden, max_v, max_h) = (10, 11, 12);
        assert_eq!(without_state(&[max_v, hidden, max_h], hidden), vec![max_v, max_h]);
        assert!(without_state(&[hidden], hidden).is_empty());
        assert!(without_state(&[], hidden).is_empty());
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
