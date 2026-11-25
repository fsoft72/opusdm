//! Environment and lifetime of the startup command started by the compositor.

use std::process::{Child, Command};

/// Environment changes for the children: `Some` sets a variable, `None` removes it.
///
/// Children see only this compositor: `DISPLAY` is removed because stage 1 has
/// no XWayland (an X11 app must fail, not open on the host desktop), and the
/// labwc / Wayfire variables are removed so the hub does not take their paths.
pub fn child_env(socket_name: &str) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("WAYLAND_DISPLAY", Some(socket_name.to_string())),
        ("XDG_SESSION_TYPE", Some("wayland".to_string())),
        ("DISPLAY", None),
        ("GDK_BACKEND", None),
        ("LABWC_PID", None),
        ("WAYFIRE_SOCKET", None),
    ]
}

/// Starts `sh -c command` with the `child_env` changes applied.
pub fn spawn_startup(command: &str, socket_name: &str) -> std::io::Result<Child> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    for (key, value) in child_env(socket_name) {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
    cmd.spawn()
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
        for gone in ["DISPLAY", "GDK_BACKEND", "LABWC_PID", "WAYFIRE_SOCKET"] {
            assert!(env.contains(&(gone, None)), "{gone} must be removed");
        }
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
