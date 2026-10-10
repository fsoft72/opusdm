//! The panel's window list and pager on Wayland: the compositor's toplevels
//! and workspaces (`opusdm_wayland`) turned into the same [`EwmhEvent`]s the
//! X11 observer sends, so the widgets do not know which session they run in.
//!
//! What Wayland does not tell is left out: window icons (the app id names
//! the desktop file, whose icon the tasklist uses instead), pids (no Force
//! kill), window positions (the pager shows no window boxes) and the
//! workspace of each window (every window counts as on all of them).

use std::sync::Mutex;

use opusdm_wayland::Snapshot;

use crate::ewmh::{ClientWin, EwmhEvent, WinRect};

/// Start forwarding the compositor's changes to `tx`. Nothing is sent when
/// the session or the compositor offers neither windows nor workspaces.
pub fn spawn(tx: async_channel::Sender<EwmhEvent>) {
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
    with_session(|s| s.activate_workspace(index as usize))
}

/// Raise and focus window `id`; `false` when not on Wayland.
pub fn activate_window(id: u32) -> bool {
    with_session(|s| s.activate(id))
}

/// Minimize window `id`; `false` when not on Wayland.
pub fn iconify_window(id: u32) -> bool {
    with_session(|s| s.minimize(id))
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
