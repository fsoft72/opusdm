//! Spawning Lister child processes and reaping them on SIGCHLD, plus the
//! single-instance socket acquisition.

use crate::server::HubCmd;
use opusdm_ipc::frame::send;
use opusdm_ipc::sockpath::{ensure_sock_dir, probe_live_hub};
use opusdm_ipc::ToHub;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;

/// Result of trying to become the hub for a given socket path.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// We own the socket now.
    Bound(UnixListener),
    /// A live hub already runs; we asked it to raise its toolbar. Exit 0.
    HandedOff,
    /// Could not bind and no live hub answered.
    Failed(String),
}

/// Ensure the socket dir, hand off to a live hub, or bind the socket.
pub fn acquire_socket(path: &Path) -> AcquireOutcome {
    if let Err(e) = ensure_sock_dir(path) {
        return AcquireOutcome::Failed(format!("cannot create socket dir: {e}"));
    }
    if probe_live_hub(path) {
        if let Ok(mut s) = UnixStream::connect(path) {
            let _ = send(&mut s, &ToHub::RaiseToolbar);
        }
        return AcquireOutcome::HandedOff;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return AcquireOutcome::Failed(format!("cannot remove stale socket: {e}")),
    }
    match UnixListener::bind(path) {
        Ok(l) => AcquireOutcome::Bound(l),
        Err(e) => AcquireOutcome::Failed(format!("cannot bind {}: {e}", path.display())),
    }
}

/// Launch one `opusdm-lister`, found next to the running hub binary.
pub fn spawn_lister(sock_path: &Path, start_path: Option<&Path>) -> io::Result<u32> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| io::Error::other("no exe dir"))?;
    spawn_lister_from_dir(dir, sock_path, start_path)
}

/// Testable core of [`spawn_lister`].
pub fn spawn_lister_from_dir(
    dir: &Path,
    sock_path: &Path,
    start_path: Option<&Path>,
) -> io::Result<u32> {
    let bin = dir.join("opusdm-lister");
    if !bin.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("opusdm-lister not found at {}", bin.display()),
        ));
    }
    let mut cmd = Command::new(bin);
    cmd.env(opusdm_ipc::sockpath::ENV_SOCK, sock_path);
    if let Some(p) = start_path {
        cmd.arg(p);
    }
    Ok(cmd.spawn()?.id())
}

/// Reap exited children and notify the owner loop by pid.
pub fn install_sigchld(cmd_tx: Sender<HubCmd>) {
    use signal_hook::consts::SIGCHLD;
    use signal_hook::iterator::Signals;
    std::thread::Builder::new()
        .name("opusdm-sigchld".into())
        .spawn(move || {
            let mut signals = match Signals::new([SIGCHLD]) {
                Ok(s) => s,
                Err(_) => return,
            };
            for _ in &mut signals {
                loop {
                    let mut status: libc::c_int = 0;
                    // SAFETY: waitpid with WNOHANG on any child; status is a valid out-ptr.
                    let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                    if pid <= 0 {
                        break;
                    }
                    opusdm_ipc::reaped::record(pid as u32, status);
                    if cmd_tx.send(HubCmd::ChildExited(pid as u32)).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn sigchld thread");
}

/// `path` (a `PATH` value) with `dir` in front, unchanged when `dir` is
/// already one of its entries.
pub fn path_with_dir(path: Option<std::ffi::OsString>, dir: &Path) -> std::ffi::OsString {
    let path = path.unwrap_or_default();
    // An empty PATH has no entries: splitting it would yield one empty entry,
    // which means "the current directory".
    if path.is_empty() {
        return dir.as_os_str().to_os_string();
    }
    if std::env::split_paths(&path).any(|entry| entry == dir) {
        return path;
    }
    let entries = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path));
    std::env::join_paths(entries).unwrap_or(path)
}

/// Put the directory of this executable on `PATH`, so the OpusDM binaries
/// resolve by name (`exec:opusdm-run` in a hotkey) for the hub, for the
/// Listers and editors it starts and for the commands they run, whatever
/// the session script did. Call it once, before any thread starts.
pub fn add_own_dir_to_path() {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) else { return };
    std::env::set_var("PATH", path_with_dir(std::env::var_os("PATH"), &dir));
}

/// Path of the tool `name`: next to `dir` (the hub's own folder) when known,
/// otherwise the bare name for a `PATH` lookup.
pub fn sibling_bin_in(dir: Option<&Path>, name: &str) -> PathBuf {
    match dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Start the OpusDM tool `name` (a sibling of the running hub), detached.
pub fn spawn_tool(name: &str) -> io::Result<()> {
    let exe = std::env::current_exe().ok();
    let bin = sibling_bin_in(exe.as_deref().and_then(Path::parent), name);
    Command::new(bin).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_tool_is_looked_up_next_to_the_hub_then_on_path() {
        let dir = Path::new("/opt/opusdm/bin");
        assert_eq!(sibling_bin_in(Some(dir), "opusdm-input-config"), dir.join("opusdm-input-config"));
        assert_eq!(sibling_bin_in(None, "opusdm-input-config"), std::path::PathBuf::from("opusdm-input-config"));
    }

    #[test]
    fn path_with_dir_puts_the_directory_first_once() {
        let dir = Path::new("/opt/opusdm/bin");
        let added = path_with_dir(Some("/usr/bin:/bin".into()), dir);
        assert_eq!(added, std::ffi::OsString::from("/opt/opusdm/bin:/usr/bin:/bin"));
        assert_eq!(path_with_dir(Some(added.clone()), dir), added, "already there: unchanged");
        assert_eq!(path_with_dir(None, dir), std::ffi::OsString::from("/opt/opusdm/bin"));
    }


    #[test]
    fn acquire_binds_when_no_hub_present() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        match acquire_socket(&path) {
            AcquireOutcome::Bound(_l) => {}
            other => panic!("expected Bound, got {other:?}"),
        }
        assert!(path.exists());
    }

    #[test]
    fn acquire_replaces_a_stale_socket_file() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let _stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(_stale);
        std::fs::write(&path, b"").ok();
        match acquire_socket(&path) {
            AcquireOutcome::Bound(_l) => {}
            other => panic!("expected Bound after clearing stale, got {other:?}"),
        }
    }

    #[test]
    fn acquire_hands_off_to_a_live_hub() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("hub.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let _h = crate::server::spawn_server(listener, Box::new(|_p| {}));
        match acquire_socket(&path) {
            AcquireOutcome::HandedOff => {}
            other => panic!("expected HandedOff, got {other:?}"),
        }
    }

    #[test]
    fn spawn_lister_reports_missing_binary_cleanly() {
        let td = tempfile::tempdir().unwrap();
        let sock = td.path().join("hub.sock");
        let res = spawn_lister_from_dir(td.path(), &sock, None);
        assert!(res.is_err());
    }
}
