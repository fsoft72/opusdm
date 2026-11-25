//! Environment and lifetime of the startup command started by the compositor.

use std::process::{Child, Command};

/// Environment changes for the children: `Some` sets a variable, `None` removes it.
///
/// Children see only this compositor: `DISPLAY` is removed because stage 1 has
/// no XWayland (an X11 app must fail, not open on the host desktop), and the
/// labwc / Wayfire variables are removed so the hub does not take their paths.
/// The OpusDM overrides of a live session are removed too: with
/// `OPUSDM_PICOM_BASE` a hub started here would rewrite the user's picom.conf
/// and signal the live picom, the socket overrides would point it at the live
/// hub, dock and applets. That list must stay the same as
/// `LIVE_SESSION_VARS` in `run-native.sh`.
pub fn child_env(socket_name: &str) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("WAYLAND_DISPLAY", Some(socket_name.to_string())),
        ("XDG_SESSION_TYPE", Some("wayland".to_string())),
        ("DISPLAY", None),
        ("GDK_BACKEND", None),
        ("LABWC_PID", None),
        ("WAYFIRE_SOCKET", None),
        // Keep in sync with LIVE_SESSION_VARS in run-native.sh.
        ("OPUSDM_PICOM_BASE", None),
        ("OPUSDM_HUB_SOCK", None),
        ("OPUSDM_DOCK_SOCK", None),
        ("OPUSDM_APPLETS_SOCK", None),
        ("OPUSDM_LABWC_RC", None),
    ]
}

/// Starts `sh -c command` with the `child_env` changes applied.
pub fn spawn_startup(command: &str, socket_name: &str) -> std::io::Result<Child> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    _apply_child_env(&mut cmd, socket_name);
    cmd.spawn()
}

/// Applies the `child_env` changes to `cmd`, after any variable `cmd`
/// already sets (so a test can give it a parent environment of its own).
fn _apply_child_env(cmd: &mut Command, socket_name: &str) {
    for (key, value) in child_env(socket_name) {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
}

/// Non-blocking check of the startup child: when it has exited, logs the
/// status and clears the slot; a failed check is logged and clears it too.
pub fn reap(child: &mut Option<Child>) {
    let Some(c) = child.as_mut() else { return };
    match c.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => {
            opusdm_ipc::err_log!("opusdm-compositor", "startup command exited: {status}");
            *child = None;
        }
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-compositor", "startup command wait failed: {e}");
            *child = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_see_only_this_compositor() {
        let env = child_env("wayland-3");
        assert!(env.contains(&("WAYLAND_DISPLAY", Some("wayland-3".into()))));
        assert!(env.contains(&("XDG_SESSION_TYPE", Some("wayland".into()))));
        let live_session = ["OPUSDM_PICOM_BASE", "OPUSDM_HUB_SOCK", "OPUSDM_DOCK_SOCK", "OPUSDM_APPLETS_SOCK", "OPUSDM_LABWC_RC"];
        for gone in ["DISPLAY", "GDK_BACKEND", "LABWC_PID", "WAYFIRE_SOCKET"].into_iter().chain(live_session) {
            assert!(env.contains(&(gone, None)), "{gone} must be removed");
        }
    }

    #[test]
    fn the_live_session_list_matches_run_native_sh() {
        let script = include_str!("../../../run-native.sh");
        let line = script.lines().find_map(|l| l.strip_prefix("LIVE_SESSION_VARS=(")).unwrap();
        let script_vars: Vec<&str> = line.trim_end_matches(')').split_whitespace().collect();
        let env = child_env("wayland-3");
        let removed: Vec<&str> = env.iter().filter(|(k, v)| v.is_none() && k.starts_with("OPUSDM_")).map(|(k, _)| *k).collect();
        assert_eq!(removed, script_vars);
    }

    /// Shell test run by the children below: exit 0 only when the child
    /// sees this compositor and nothing of the host session.
    const CHECK_ENV: &str = "test -z \"${DISPLAY+set}\" && test -z \"${OPUSDM_PICOM_BASE+set}\" \
        && test -z \"${OPUSDM_HUB_SOCK+set}\" && test \"$WAYLAND_DISPLAY\" = wayland-9 \
        && test \"$XDG_SESSION_TYPE\" = wayland";

    #[test]
    fn a_child_does_not_inherit_the_host_display_or_the_live_overrides() {
        // The parent environment is given to this child only: the tests run
        // in parallel and must not change the process environment.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(CHECK_ENV);
        cmd.env("DISPLAY", ":99").env("OPUSDM_PICOM_BASE", "/opt/opusdm/config/picom.conf");
        cmd.env("OPUSDM_HUB_SOCK", "/tmp/live-hub.sock");
        _apply_child_env(&mut cmd, "wayland-9");
        let status = cmd.status().unwrap();
        assert!(status.success(), "the child saw the host session: {status}");
    }

    #[test]
    fn spawn_startup_applies_the_child_environment() {
        let status = spawn_startup(CHECK_ENV, "wayland-9").unwrap().wait().unwrap();
        assert!(status.success(), "the startup command saw the host session: {status}");
    }

    #[test]
    fn reap_clears_the_slot_once_the_child_has_exited() {
        let mut child = Some(spawn_startup("exit 3", "wayland-9").unwrap());
        for _ in 0..200 {
            reap(&mut child);
            if child.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("child was never reaped");
    }
}
