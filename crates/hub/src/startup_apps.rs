//! Starts the applications listed in Settings, Desktop, Startup Apps.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use opusdm_ipc::StartupApp;

/// The command line of `app`, split like an `exec:` hotkey (quotes group).
/// An empty command or an unclosed quote is an error.
pub fn argv_for(app: &StartupApp) -> Result<Vec<String>, String> {
    let argv = opusdm_ipc::cli::tokenize(&app.command)?;
    if argv.is_empty() {
        return Err("no program given".to_string());
    }
    Ok(argv)
}

/// Start `argv` detached: own process group, no stdio, so it outlives the
/// hub's terminal. Returns the new process's pid, which is also its process
/// group id. Fails when the program cannot be started.
pub fn spawn_detached(argv: &[String]) -> Result<u32, String> {
    let Some((program, args)) = argv.split_first() else { return Err("nothing to run".to_string()) };
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|child| child.id())
        .map_err(|e| format!("cannot run '{program}': {e}"))
}

/// Longest wait for the panel's tray before the wait times start anyway (a
/// panel that never gets its tray must not hold the apps back forever).
pub const TRAY_WAIT_LIMIT: Duration = Duration::from_secs(20);

/// Wait for the app's own time, then launch it; for a `start_in_tray` app,
/// close the first window it opens (see `tray_start`). A failure is logged
/// and never stops the other apps.
fn launch_after_wait(app: &StartupApp) {
    std::thread::sleep(Duration::from_secs(u64::from(app.wait_secs)));
    // Watch before launching, so a window that appears at once is not missed.
    let watch = if app.start_in_tray {
        crate::tray_start::Watch::new()
            .inspect_err(|e| opusdm_ipc::err_log!("opusdm-hub", "startup app '{}': cannot watch its windows: {e}", app.command))
            .ok()
    } else {
        None
    };
    let pid = match argv_for(app).and_then(|argv| spawn_detached(&argv)) {
        Ok(pid) => pid,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-hub", "startup app '{}': {e}", app.command);
            return;
        }
    };
    let Some(watch) = watch else { return };
    match watch.close_first_window(pid) {
        Ok(Some(_)) => {}
        Ok(None) => opusdm_ipc::err_log!("opusdm-hub", "startup app '{}': no window to send to the tray", app.command),
        Err(e) => opusdm_ipc::err_log!("opusdm-hub", "startup app '{}': start in tray failed: {e}", app.command),
    }
}

/// Start every app after its own wait, counted from the moment the panel's
/// tray exists (at most `TRAY_WAIT_LIMIT` from this call), so a tray app
/// with a wait of 0 finds the tray. Each app has its own thread, so the
/// waits run side by side and never delay each other.
pub fn start(apps: &[StartupApp]) {
    start_after(apps, || {
        if !opusdm_panel::tray_ready::wait_until_ready(TRAY_WAIT_LIMIT) {
            opusdm_ipc::err_log!("opusdm-hub", "the panel tray is not ready, starting the startup apps anyway");
        }
    });
}

/// Start every app after `ready` returns, then after its own wait. `ready`
/// runs once, on a thread of its own, so the caller never blocks.
fn start_after(apps: &[StartupApp], ready: impl FnOnce() + Send + 'static) {
    let apps = apps.to_vec();
    let spawned = std::thread::Builder::new().name("startup-apps".to_string()).spawn(move || {
        ready();
        launch_all(&apps);
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-hub", "cannot start a thread for the startup apps: {e}");
    }
}

/// Launch every app after its own wait, each on its own thread.
fn launch_all(apps: &[StartupApp]) {
    for app in apps {
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("startup-app".to_string())
            .spawn(move || launch_after_wait(&app));
        if let Err(e) = spawned {
            opusdm_ipc::err_log!("opusdm-hub", "cannot start a thread for a startup app: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::StartupApp;
    use std::time::{Duration, Instant};

    fn app(command: &str, wait_secs: u32) -> StartupApp {
        StartupApp { command: command.to_string(), wait_secs, start_in_tray: false }
    }

    #[test]
    fn argv_splits_like_an_exec_hotkey() {
        let argv = argv_for(&app("sh -c 'echo hi there'", 0)).unwrap();
        assert_eq!(argv, vec!["sh", "-c", "echo hi there"]);
    }

    #[test]
    fn argv_reports_an_unclosed_quote_and_an_empty_command() {
        assert!(argv_for(&app("sh -c 'oops", 0)).is_err());
        assert!(argv_for(&app("   ", 0)).is_err());
    }

    #[test]
    fn spawn_reports_a_missing_program() {
        let argv = vec!["opusdm-no-such-program-xyz".to_string()];
        assert!(spawn_detached(&argv).unwrap_err().contains("opusdm-no-such-program-xyz"));
    }

    /// Wait until `path` exists, up to a few seconds.
    fn appears(path: &std::path::Path) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn start_runs_an_app_with_no_wait_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("now");
        start_after(&[app(&format!("touch {}", marker.display()), 0)], || {});
        assert!(appears(&marker));
    }

    #[test]
    fn start_holds_an_app_back_for_its_wait_time() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("later");
        let begun = Instant::now();
        start_after(&[app(&format!("touch {}", marker.display()), 1)], || {});
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "launched before its wait time");
        assert!(appears(&marker));
        assert!(begun.elapsed() >= Duration::from_secs(1));
    }

    #[test]
    fn the_wait_times_start_when_the_tray_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("after-tray");
        let (tray_tx, tray_rx) = std::sync::mpsc::channel::<()>();
        start_after(&[app(&format!("touch {}", marker.display()), 0)], move || {
            let _ = tray_rx.recv();
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "launched before the tray was ready");
        tray_tx.send(()).unwrap();
        assert!(appears(&marker));
    }

    #[test]
    fn a_failing_app_does_not_stop_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("after-failure");
        start_after(&[app("opusdm-no-such-program-xyz", 0), app(&format!("touch {}", marker.display()), 0)], || {});
        assert!(appears(&marker));
    }
}
