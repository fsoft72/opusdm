//! "Start in tray" on Wayland: the counterpart of `tray_start` through the
//! compositor's window list (`opusdm_wayland`). The first new window of the
//! launched program is minimized and asked to close, as its close button
//! would; a tray app hides itself in the tray, one with no tray quits.
//!
//! Wayland does not tell which process owns a window, so the window is
//! matched by its app id against the program name instead (see
//! [`app_id_matches`]). Toolkits are not told about a minimize on Wayland,
//! so there is no minimized state to undo once the window is hidden.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use opusdm_wayland::{Session, Snapshot, ToplevelId};

use crate::tray_start::{CLOSE_CHECK_TIME, MAX_CLOSE_TRIES, SETTLE_TIME, WATCH_TIME};

/// Pause between two looks at the window list.
const POLL: Duration = Duration::from_millis(100);

/// Parts of an app id that say nothing about the program
/// (`org.telegram.desktop`, `io.github.foo.Bar`).
const GENERIC_PARTS: [&str; 9] = ["org", "com", "io", "net", "github", "gitlab", "desktop", "app", "kde"];

/// Whether the window with `app_id` belongs to `program` (a path or a name):
/// the app id equals the program name, or one of its dot separated parts
/// equals the program name or the part of it before the first `-` or `_`
/// (`telegram-desktop` matches `org.telegram.desktop`). Case is ignored.
pub fn app_id_matches(app_id: &str, program: &str) -> bool {
    let app_id = app_id.to_lowercase();
    let bin = Path::new(program).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if bin.is_empty() || app_id.is_empty() { return false; }
    let stem = bin.split(['-', '_']).next().unwrap_or(&bin).to_string();
    if app_id == bin { return true; }
    app_id.split('.').filter(|part| part.len() >= 3 && !GENERIC_PARTS.contains(part)).any(|part| part == bin || part == stem)
}

/// The id of the first window of `snapshot` that is not in `known` and
/// belongs to `program`. Windows that do not match are added to `known`, so
/// they are looked at once; one without an app id yet is looked at again.
pub fn new_window(snapshot: &Snapshot, known: &mut HashSet<ToplevelId>, program: &str) -> Option<ToplevelId> {
    for t in &snapshot.toplevels {
        if known.contains(&t.id) || t.app_id.is_empty() { continue; }
        if app_id_matches(&t.app_id, program) { return Some(t.id); }
        known.insert(t.id);
    }
    None
}

/// Watches the window list for the first window of one launch. Built
/// *before* the launch, so the windows already open are never touched.
pub struct Watch {
    session: Arc<Session>,
    known: HashSet<ToplevelId>,
}

impl Watch {
    /// Remember the windows open now. `None` when the compositor does not
    /// list windows.
    pub fn new() -> Option<Self> {
        let session = opusdm_wayland::shared().filter(|s| s.has_toplevels())?;
        let known = session.snapshot().toplevels.iter().map(|t| t.id).collect();
        Some(Watch { session, known })
    }

    /// Minimize and close the first window of `program` that opens within
    /// `WATCH_TIME`. Returns it once it is gone from the window list (the app
    /// hid it), `None` when none came in time or it would not close.
    pub fn close_first_window(mut self, program: &str) -> Option<ToplevelId> {
        let deadline = Instant::now() + WATCH_TIME;
        let id = loop {
            if let Some(id) = new_window(&self.session.snapshot(), &mut self.known, program) { break id; }
            if Instant::now() >= deadline { return None; }
            std::thread::sleep(POLL);
        };
        for _ in 0..MAX_CLOSE_TRIES {
            std::thread::sleep(SETTLE_TIME);
            let Some(window) = self.session.snapshot().toplevels.into_iter().find(|t| t.id == id) else { return Some(id) };
            if !window.minimized { self.session.minimize(id); }
            self.session.close(id);
            std::thread::sleep(CLOSE_CHECK_TIME);
            if !self.session.snapshot().toplevels.iter().any(|t| t.id == id) { return Some(id); }
        }
        opusdm_ipc::err_log!("opusdm-hub", "start in tray: the window of '{program}' is still open after {MAX_CLOSE_TRIES} close requests, left minimized");
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_wayland::Toplevel;

    #[test]
    fn app_ids_match_program_names() {
        assert!(app_id_matches("org.telegram.desktop", "/usr/bin/telegram-desktop"));
        assert!(app_id_matches("Enpass", "/opt/enpass/Enpass"));
        assert!(app_id_matches("nz.mega.MEGAsync", "megasync"));
        assert!(app_id_matches("megasync", "megasync"));
        assert!(!app_id_matches("org.kde.konsole", "telegram-desktop"));
        assert!(!app_id_matches("org.gnome.desktop", "desktop-thing"), "generic parts never match");
        assert!(!app_id_matches("", "foo"));
    }

    #[test]
    fn only_new_matching_windows_are_picked() {
        let t = |id, app_id: &str| Toplevel { id, app_id: app_id.into(), ..Default::default() };
        let snapshot = Snapshot { toplevels: vec![t(1, "megasync"), t(2, "org.kde.konsole"), t(3, ""), t(4, "megasync")], workspaces: vec![] };
        let mut known: HashSet<ToplevelId> = [1].into();
        assert_eq!(new_window(&snapshot, &mut known, "megasync"), Some(4));
        assert!(known.contains(&2), "a window of another app is looked at once");
        assert!(!known.contains(&3), "a window with no app id yet is looked at again");
    }
}
