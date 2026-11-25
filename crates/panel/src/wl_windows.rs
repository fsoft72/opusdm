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
//!
//! Under opusdm-compositor its private IPC (`opusdm_wayland::native`) comes
//! first and tells the same (desktop of each window, pid, geometry). Whether
//! the panel runs under it is decided once per process ([`under_native`]);
//! the actions use that IPC only when its watch started ([`native_active`]).

use std::sync::{mpsc, Mutex, OnceLock};

use opusdm_wayland::native;
use opusdm_wayland::wayfire;
use opusdm_wayland::Snapshot;

use crate::ewmh::{ClientWin, EwmhEvent, WinRect};

/// Start forwarding the compositor's changes to `tx`. Nothing is sent when
/// the session or the compositor offers neither windows nor workspaces.
pub fn spawn(tx: async_channel::Sender<EwmhEvent>) {
    if under_native() && _decide_native(spawn_native(tx.clone())) { return; }
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

/// Whether the panel runs under opusdm-compositor, decided once for the
/// whole process: the compositor removes its socket file when it quits,
/// and a check per call would then send the panel down the labwc /
/// foreign-toplevel paths in the middle of the shutdown.
static UNDER_NATIVE: OnceLock<bool> = OnceLock::new();

/// Whether the panel runs under opusdm-compositor (see [`UNDER_NATIVE`]).
pub(crate) fn under_native() -> bool {
    *UNDER_NATIVE.get_or_init(native::under_native)
}

/// Whether the window list comes from opusdm-compositor's IPC, decided once
/// by [`spawn`]: `false` for the rest of the process when its watch could
/// not start (the list then comes from foreign-toplevel, whose ids are not
/// the compositor's: actions must not go to the native IPC with them).
static NATIVE_ACTIVE: OnceLock<bool> = OnceLock::new();

/// Record the outcome of the native watch (`watched`) for the whole process
/// and return it. A second decision is ignored, with a log line.
fn _decide_native(watched: bool) -> bool {
    if NATIVE_ACTIVE.set(watched).is_err() {
        opusdm_ipc::err_log!("opusdm-panel", "native window list already decided, second decision ({watched}) ignored");
    }
    if !watched {
        opusdm_ipc::err_log!("opusdm-panel", "window actions go through Wayland for the rest of the session, not the compositor IPC");
    }
    native_active()
}

/// What [`native_active`] answers for `under` (running under the compositor)
/// and `decided` (the outcome of the native watch, `None` before `spawn`).
fn _native_decision(under: bool, decided: Option<bool>) -> bool {
    under && decided == Some(true)
}

/// Whether window actions and the tasklist "Move to desktop" menu use
/// opusdm-compositor's IPC: under it AND its watch started. Before `spawn`
/// decided it answers `false`, with a log line.
pub(crate) fn native_active() -> bool {
    if !under_native() { return false; }
    let decided = NATIVE_ACTIVE.get().copied();
    if decided.is_none() {
        opusdm_ipc::err_log!("opusdm-panel", "window action before the window list was set up: not sent to the compositor");
    }
    _native_decision(true, decided)
}

/// Forward opusdm-compositor's changes to `tx` through its private IPC.
/// `false` when it cannot be watched (the caller then falls back to the
/// Wayland protocols).
///
/// Ownership: the watch threads of `native::watch` own the closure, and
/// with it `tx` and the last state, for the whole run. When the compositor
/// goes away they stop with a log line each and are not restarted: the
/// panel keeps showing the last state it received.
fn spawn_native(tx: async_channel::Sender<EwmhEvent>) -> bool {
    let mut last: Option<native::NativeState> = None;
    let watched = native::watch(move |state| {
        for event in native_events(last.as_ref(), state) {
            if let Err(e) = tx.send_blocking(event) {
                opusdm_ipc::err_log!("opusdm-panel", "compositor window event dropped, the panel is gone: {e}");
            }
        }
        last = Some(state.clone());
    });
    if let Err(e) = &watched {
        opusdm_ipc::err_log!("opusdm-panel", "compositor IPC not usable, window list through Wayland: {e}");
    }
    watched.is_ok()
}

/// The events that bring a panel showing opusdm-compositor state `prev`
/// (nothing yet when `None`) up to `now`.
pub fn native_events(prev: Option<&native::NativeState>, now: &native::NativeState) -> Vec<EwmhEvent> {
    let desktops = |s: &native::NativeState| (s.count, s.current);
    let clients = |s: &native::NativeState| s.windows.iter().map(native_client).collect::<Vec<_>>();
    let active = |s: &native::NativeState| s.active().map(|w| w.id);
    let mut out = Vec::new();
    if prev.map(desktops) != Some(desktops(now)) {
        let (count, current) = desktops(now);
        out.push(EwmhEvent::Desktops { count, current, names: (1..=count).map(|i| i.to_string()).collect() });
    }
    let now_clients = clients(now);
    if prev.map(clients).as_ref() != Some(&now_clients) {
        out.push(EwmhEvent::Clients(now_clients));
    }
    if prev.map(active) != Some(active(now)) {
        out.push(EwmhEvent::ActiveWindow(active(now)));
    }
    out
}

/// One opusdm-compositor window as the tasklist and the pager show it.
fn native_client(w: &native::NativeWindow) -> ClientWin {
    let r = &w.geometry;
    ClientWin {
        xid: w.id,
        title: w.title.clone(),
        desktop: i64::from(w.desktop),
        minimized: w.minimized,
        pid: w.pid,
        icon: None,
        app_id: (!w.app_id.is_empty()).then(|| w.app_id.clone()),
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
    if native_active() { return with_native("switch desktop", move || native::switch_desktop(index)); }
    if wayfire::under_wayfire() { return with_wayfire("switch desktop", move || wayfire::switch_desktop(index)); }
    with_session(|s| s.activate_workspace(index as usize))
}

/// Raise and focus window `id`; `false` when not on Wayland.
pub fn activate_window(id: u32) -> bool {
    if native_active() { return with_native("activate window", move || native::activate(id)); }
    if wayfire::under_wayfire() { return with_wayfire("activate window", move || wayfire::focus(id)); }
    with_session(|s| s.activate(id))
}

/// Minimize window `id`; `false` when not on Wayland.
pub fn iconify_window(id: u32) -> bool {
    if native_active() { return with_native("minimize window", move || native::set_minimized(id, true)); }
    if wayfire::under_wayfire() { return with_wayfire("minimize window", move || wayfire::set_minimized(id, true)); }
    with_session(|s| s.minimize(id))
}

/// Ask window `id` to close, like its close button; `false` when not on
/// Wayland.
pub fn close_window(id: u32) -> bool {
    if native_active() { return with_native("close window", move || native::close(id)); }
    if wayfire::under_wayfire() { return with_wayfire("close window", move || wayfire::close(id)); }
    with_session(|s| s.close(id))
}

/// Move window `id` to desktop `index`; `false` when not on Wayland. Only
/// opusdm-compositor and Wayfire can do it: no Wayland protocol moves
/// another client's window.
pub fn move_window_to_desktop(id: u32, index: u32) -> bool {
    if native_active() { return with_native("move window to desktop", move || native::move_to_desktop(id, index)); }
    if wayfire::under_wayfire() { return with_wayfire("move window to desktop", move || wayfire::move_to_desktop(id, index)); }
    opusdm_ipc::session::is_wayland()
}

/// Kill the process of window `id` through opusdm-compositor, which does
/// the kill with its own pid guards. `false` when not under it (the caller
/// then kills the window's pid itself).
pub fn kill_window(id: u32) -> bool {
    if !native_active() { return false; }
    with_native("kill window", move || native::kill(id))
}

/// An opusdm-compositor request queued for the native worker thread.
type NativeJob = Box<dyn FnOnce() + Send>;

/// Queue of the worker thread that runs opusdm-compositor requests in click
/// order; `None` when the thread could not be started.
static NATIVE_JOBS: OnceLock<Option<mpsc::Sender<NativeJob>>> = OnceLock::new();

/// Start the worker thread behind [`NATIVE_JOBS`].
///
/// Ownership: the thread owns the receiving end and lives as long as the
/// process (the sending end is in a static and is never dropped). Each job
/// opens its own short request connection, so a compositor that went away
/// only makes the next jobs fail, each with a log line.
fn start_native_worker() -> Option<mpsc::Sender<NativeJob>> {
    let (tx, rx) = mpsc::channel::<NativeJob>();
    let spawned = std::thread::Builder::new().name("opusdm-native-call".into()).spawn(move || {
        while let Ok(job) = rx.recv() {
            job();
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-panel", "cannot start the compositor request thread: {e}");
            None
        }
    }
}

/// Run the opusdm-compositor request `f` on the native worker thread, so a
/// slow compositor never blocks the GTK thread, logging a failure. Runs `f`
/// in place when the worker is not available. Always `true`.
fn with_native(what: &'static str, f: impl FnOnce() -> Result<(), String> + Send + 'static) -> bool {
    let job: NativeJob = Box::new(move || {
        if let Err(e) = f() {
            opusdm_ipc::err_log!("opusdm-panel", "compositor: {what} failed: {e}");
        }
    });
    match NATIVE_JOBS.get_or_init(start_native_worker) {
        Some(tx) => {
            if let Err(mpsc::SendError(job)) = tx.send(job) {
                opusdm_ipc::err_log!("opusdm-panel", "compositor request thread gone, running {what} in place");
                job();
            }
        }
        None => job(),
    }
    true
}

/// A Wayfire request queued for the worker thread.
type WayfireJob = Box<dyn FnOnce() + Send>;

/// Queue of the worker thread that runs Wayfire requests in click order;
/// `None` when the thread could not be started.
static WAYFIRE_JOBS: OnceLock<Option<mpsc::Sender<WayfireJob>>> = OnceLock::new();

/// Start the worker thread behind [`WAYFIRE_JOBS`].
fn start_wayfire_worker() -> Option<mpsc::Sender<WayfireJob>> {
    let (tx, rx) = mpsc::channel::<WayfireJob>();
    let spawned = std::thread::Builder::new().name("opusdm-wayfire-call".into()).spawn(move || {
        while let Ok(job) = rx.recv() {
            job();
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-panel", "cannot start the Wayfire request thread: {e}");
            None
        }
    }
}

/// Run the Wayfire request `f` on the worker thread, so a slow compositor
/// never blocks the GTK thread, logging a failure. Runs `f` in place when
/// the worker is not available. Always `true`.
fn with_wayfire(what: &'static str, f: impl FnOnce() -> Result<(), String> + Send + 'static) -> bool {
    let job: WayfireJob = Box::new(move || {
        if let Err(e) = f() {
            opusdm_ipc::err_log!("opusdm-panel", "Wayfire: {what} failed: {e}");
        }
    });
    match WAYFIRE_JOBS.get_or_init(start_wayfire_worker) {
        Some(tx) => {
            if let Err(mpsc::SendError(job)) = tx.send(job) {
                opusdm_ipc::err_log!("opusdm-panel", "Wayfire request thread gone, running {what} in place");
                job();
            }
        }
        None => job(),
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

    fn native_state(current: u32, active: Option<u32>) -> native::NativeState {
        let window = |id: u32, desktop: u32| native::NativeWindow {
            id,
            app_id: "foot".into(),
            title: format!("w{id}"),
            pid: Some(100 + id),
            geometry: native::Rect { x: 10 * id as i32, y: 20, w: 300, h: 200 },
            desktop,
            activated: active == Some(id),
            ..Default::default()
        };
        native::NativeState { windows: vec![window(1, 0), window(2, 2)], count: 4, current }
    }

    #[test]
    fn native_actions_need_the_compositor_and_a_started_watch() {
        assert!(!_native_decision(false, None));
        assert!(!_native_decision(false, Some(true)));
        assert!(!_native_decision(true, None), "undecided is not native");
        assert!(!_native_decision(true, Some(false)), "a fallback is not native");
        assert!(_native_decision(true, Some(true)));
    }

    #[test]
    fn native_windows_carry_desktop_pid_and_rect() {
        let out = native_events(None, &native_state(1, Some(2)));
        assert_eq!(out.len(), 3);
        let EwmhEvent::Desktops { count, current, names } = &out[0] else { panic!("desktops first") };
        assert_eq!((*count, *current), (4, 1));
        assert_eq!(names, &["1", "2", "3", "4"]);
        let EwmhEvent::Clients(clients) = &out[1] else { panic!("then clients") };
        assert_eq!((clients[0].xid, clients[0].desktop, clients[0].pid), (1, 0, Some(101)));
        assert_eq!((clients[1].xid, clients[1].desktop, clients[1].pid), (2, 2, Some(102)));
        assert_eq!(clients[1].rect, WinRect { x: 20, y: 20, w: 300, h: 200 });
        assert_eq!(clients[0].app_id.as_deref(), Some("foot"));
        assert_eq!(clients[0].title, "w1");
        assert!(clients.iter().all(|c| c.icon.is_none() && !c.minimized));
        assert!(matches!(out[2], EwmhEvent::ActiveWindow(Some(2))));
    }

    #[test]
    fn a_native_window_on_desktop_two_is_never_sticky() {
        let out = native_events(None, &native_state(0, None));
        let EwmhEvent::Clients(clients) = &out[1] else { panic!("clients") };
        assert_eq!(clients[1].desktop, 2);
        assert!(clients.iter().all(|c| c.desktop >= 0));
    }

    #[test]
    fn native_only_what_changed_is_sent() {
        let before = native_state(0, Some(1));
        assert!(native_events(Some(&before), &before).is_empty());
        let out = native_events(Some(&before), &native_state(3, Some(1)));
        assert!(matches!(out.as_slice(), [EwmhEvent::Desktops { current: 3, .. }]));
        let out = native_events(Some(&before), &native_state(0, None));
        // Focus is not part of a ClientWin: only the active window moves.
        assert!(matches!(out.as_slice(), [EwmhEvent::ActiveWindow(None)]));
        let mut minimized = before.clone();
        minimized.windows[0].minimized = true;
        let out = native_events(Some(&before), &minimized);
        let [EwmhEvent::Clients(clients)] = out.as_slice() else { panic!("clients only") };
        assert!(clients[0].minimized);
    }

    #[test]
    fn native_empty_app_id_is_none_and_negative_size_is_zero() {
        let mut state = native_state(0, None);
        state.windows[0].app_id.clear();
        state.windows[0].geometry = native::Rect { x: -5, y: -6, w: -1, h: 0 };
        let out = native_events(None, &state);
        let EwmhEvent::Clients(clients) = &out[1] else { panic!("clients") };
        assert_eq!(clients[0].app_id, None);
        assert_eq!(clients[0].rect, WinRect { x: -5, y: -6, w: 0, h: 0 });
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
