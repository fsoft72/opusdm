//! The panel's window list and pager on Wayland: the compositor's toplevels
//! and workspaces (`opusdm_wayland`) turned into the same [`EwmhEvent`]s the
//! X11 observer sends, so the widgets do not know which session they run in.
//!
//! What Wayland does not tell is left out: window icons (the app id names
//! the desktop file, whose icon the tasklist uses instead), pids (no Force
//! kill), window positions (the pager draws placeholder boxes) and the
//! workspace of each window (every window counts as on all of them).
//!
//! Under Wayfire its IPC (`opusdm_wayland::wayfire`) is used instead, which
//! tells all of that but the icons: the pager draws the real window boxes on
//! the right desktop, and windows can be moved to another desktop.

use std::sync::Mutex;

use opusdm_wayland::wayfire;
use opusdm_wayland::Snapshot;

use crate::ewmh::{ClientWin, EwmhEvent, WinRect};

/// Start forwarding the compositor's changes to `tx`. Nothing is sent when
/// the session or the compositor offers neither windows nor workspaces.
pub fn spawn(tx: async_channel::Sender<EwmhEvent>) {
    if wayfire::under_wayfire() && spawn_wayfire(tx.clone()) { return; }
    let Some(session) = opusdm_wayland::shared() else { return };
    let last: Mutex<Option<Snapshot>> = Mutex::new(None);
    session.subscribe(move |snapshot| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        for event in events(last.as_ref(), snapshot) {
            let _ = tx.send_blocking(event);
        }
        *last = Some(snapshot.clone());
    });
}

/// Forward Wayfire's changes to `tx` through its IPC. `false` when the IPC
/// cannot be watched (the caller then falls back to the Wayland protocols).
fn spawn_wayfire(tx: async_channel::Sender<EwmhEvent>) -> bool {
    let mut last: Option<wayfire::State> = None;
    let watched = wayfire::watch(move |state| {
        for event in wayfire_events(last.as_ref(), state) {
            let _ = tx.send_blocking(event);
        }
        last = Some(state.clone());
    });
    if let Err(e) = &watched {
        opusdm_ipc::err_log!("opusdm-panel", "Wayfire IPC not usable, window list through Wayland: {e}");
    }
    watched.is_ok()
}

/// The events that bring a panel showing Wayfire state `prev` (nothing yet
/// when `None`) up to `now`.
pub fn wayfire_events(prev: Option<&wayfire::State>, now: &wayfire::State) -> Vec<EwmhEvent> {
    let desktops = |s: &wayfire::State| (s.desktop_count(), s.current_desktop());
    let clients = |s: &wayfire::State| s.views.iter().map(|v| wayfire_client(s, v)).collect::<Vec<_>>();
    let mut out = Vec::new();
    if prev.map(desktops) != Some(desktops(now)) {
        let (count, current) = desktops(now);
        out.push(EwmhEvent::Desktops { count, current, names: (1..=count).map(|i| i.to_string()).collect() });
    }
    let now_clients = clients(now);
    if prev.map(clients).as_ref() != Some(&now_clients) {
        out.push(EwmhEvent::Clients(now_clients));
    }
    if prev.map(wayfire::State::active) != Some(now.active()) {
        out.push(EwmhEvent::ActiveWindow(now.active()));
    }
    out
}

/// One Wayfire window as the tasklist and the pager show it.
fn wayfire_client(state: &wayfire::State, v: &wayfire::View) -> ClientWin {
    let r = state.rect_of(v);
    ClientWin {
        xid: v.id,
        title: v.title.clone(),
        desktop: state.desktop_of(v).map_or(-1, i64::from),
        minimized: v.minimized,
        pid: v.pid,
        icon: None,
        app_id: (!v.app_id.is_empty()).then(|| v.app_id.clone()),
        rect: WinRect { x: r.x, y: r.y, w: r.w.max(0) as u32, h: r.h.max(0) as u32 },
    }
}

/// The events that bring a panel showing `prev` (nothing yet when `None`)
/// up to `now`.
pub fn events(prev: Option<&Snapshot>, now: &Snapshot) -> Vec<EwmhEvent> {
    let mut out = Vec::new();
    if prev.map(|p| &p.workspaces) != Some(&now.workspaces) {
        out.push(EwmhEvent::Desktops {
            count: now.workspaces.len() as u32,
            current: now.current_workspace().unwrap_or(0) as u32,
            names: now.workspaces.iter().map(|w| w.name.clone()).collect(),
        });
    }
    if prev.map(|p| &p.toplevels) != Some(&now.toplevels) {
        out.push(EwmhEvent::Clients(now.toplevels.iter().map(client).collect()));
    }
    if prev.map(Snapshot::active) != Some(now.active()) {
        out.push(EwmhEvent::ActiveWindow(now.active()));
    }
    out
}

/// One toplevel as the tasklist shows it.
fn client(t: &opusdm_wayland::Toplevel) -> ClientWin {
    ClientWin {
        xid: t.id,
        title: t.title.clone(),
        desktop: -1,
        minimized: t.minimized,
        pid: None,
        icon: None,
        app_id: (!t.app_id.is_empty()).then(|| t.app_id.clone()),
        rect: WinRect::default(),
    }
}

/// Switch to workspace `index`. `false` when not on Wayland (the caller then
/// takes the X11 path).
pub fn switch_desktop(index: u32) -> bool {
    if wayfire::under_wayfire() { return with_wayfire("switch desktop", || wayfire::switch_desktop(index)); }
    with_session(|s| s.activate_workspace(index as usize))
}

/// Raise and focus window `id`; `false` when not on Wayland.
pub fn activate_window(id: u32) -> bool {
    if wayfire::under_wayfire() { return with_wayfire("activate window", || wayfire::focus(id)); }
    with_session(|s| s.activate(id))
}

/// Minimize window `id`; `false` when not on Wayland.
pub fn iconify_window(id: u32) -> bool {
    if wayfire::under_wayfire() { return with_wayfire("minimize window", || wayfire::set_minimized(id, true)); }
    with_session(|s| s.minimize(id))
}

/// Move window `id` to desktop `index`; `false` when not on Wayland. Only
/// Wayfire can do it: no Wayland protocol moves another client's window.
pub fn move_window_to_desktop(id: u32, index: u32) -> bool {
    if wayfire::under_wayfire() { return with_wayfire("move window to desktop", || wayfire::move_to_desktop(id, index)); }
    opusdm_ipc::session::is_wayland()
}

/// Run the Wayfire request `f`, logging a failure. Always `true`.
fn with_wayfire(what: &str, f: impl FnOnce() -> Result<(), String>) -> bool {
    if let Err(e) = f() {
        opusdm_ipc::err_log!("opusdm-panel", "Wayfire: {what} failed: {e}");
    }
    true
}

/// Run `f` on the Wayland session. `false` outside a Wayland session; on
/// Wayland always `true`, even when the compositor cannot be reached.
fn with_session(f: impl FnOnce(&opusdm_wayland::Session)) -> bool {
    if !opusdm_ipc::session::is_wayland() { return false; }
    if let Some(session) = opusdm_wayland::shared() { f(&session); }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_wayland::{Toplevel, Workspace};

    fn snapshot(active_ws: usize, active_win: Option<u32>) -> Snapshot {
        Snapshot {
            toplevels: vec![
                Toplevel { id: 1, title: "one".into(), app_id: "foot".into(), activated: active_win == Some(1), ..Default::default() },
                Toplevel { id: 2, title: "two".into(), activated: active_win == Some(2), ..Default::default() },
            ],
            workspaces: (0..3).map(|i| Workspace { name: format!("W{i}"), active: i == active_ws, ..Default::default() }).collect(),
        }
    }

    #[test]
    fn the_first_snapshot_sends_everything() {
        let out = events(None, &snapshot(1, Some(2)));
        assert_eq!(out.len(), 3);
        let EwmhEvent::Desktops { count, current, names } = &out[0] else { panic!("desktops first") };
        assert_eq!((*count, *current), (3, 1));
        assert_eq!(names, &["W0", "W1", "W2"]);
        let EwmhEvent::Clients(clients) = &out[1] else { panic!("then clients") };
        assert_eq!(clients[0].app_id.as_deref(), Some("foot"));
        assert_eq!(clients[1].app_id, None);
        assert!(clients.iter().all(|c| c.desktop == -1));
        assert!(matches!(out[2], EwmhEvent::ActiveWindow(Some(2))));
    }

    fn wayfire_state(current: i64) -> wayfire::State {
        let outputs = serde_json::json!([{ "id": 1, "geometry": { "x": 0, "y": 0, "width": 1000, "height": 500 },
            "workspace": { "grid_width": 4, "grid_height": 1, "x": current, "y": 0 } }]);
        let views = serde_json::json!([{ "id": 7, "role": "toplevel", "mapped": true, "title": "w", "app-id": "foot",
            "pid": 99, "activated": true, "output-id": 1, "geometry": { "x": 10, "y": 20, "width": 300, "height": 200 } }]);
        wayfire::State::from_json(&views, &outputs)
    }

    #[test]
    fn wayfire_windows_carry_rect_desktop_and_pid() {
        let out = wayfire_events(None, &wayfire_state(2));
        let EwmhEvent::Desktops { count, current, names } = &out[0] else { panic!("desktops first") };
        assert_eq!((*count, *current), (4, 2));
        assert_eq!(names, &["1", "2", "3", "4"]);
        let EwmhEvent::Clients(clients) = &out[1] else { panic!("then clients") };
        assert_eq!((clients[0].desktop, clients[0].pid), (2, Some(99)));
        assert_eq!(clients[0].rect, WinRect { x: 10, y: 20, w: 300, h: 200 });
        assert!(matches!(out[2], EwmhEvent::ActiveWindow(Some(7))));
    }

    #[test]
    fn a_wayfire_desktop_switch_moves_the_window_off_screen() {
        // Wayfire shifts the geometry when the shown workspace changes: the
        // window stays on desktop 2 at the same place.
        let before = wayfire_state(2);
        let mut after = wayfire_state(3);
        after.views[0].geometry.x -= 1000;
        let out = wayfire_events(Some(&before), &after);
        assert!(matches!(out.as_slice(), [EwmhEvent::Desktops { current: 3, .. }]));
    }

    #[test]
    fn only_what_changed_is_sent() {
        let before = snapshot(0, Some(1));
        assert!(events(Some(&before), &before).is_empty());
        let out = events(Some(&before), &snapshot(2, Some(1)));
        assert!(matches!(out.as_slice(), [EwmhEvent::Desktops { current: 2, .. }]));
        let out = events(Some(&before), &snapshot(0, Some(2)));
        assert!(matches!(out.as_slice(), [EwmhEvent::Clients(_), EwmhEvent::ActiveWindow(Some(2))]));
    }
}
