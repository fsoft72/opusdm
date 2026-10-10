//! The compositor process itself: ending the Wayland session. The session
//! scripts `exec` the compositor, so when it quits the session is over.
//!
//! The compositor is told to quit with `SIGTERM`, what `labwc --exit` does
//! and what Wayfire and the other wlroots compositors handle by shutting
//! down cleanly. Its pid comes from `LABWC_PID` under labwc, else from the
//! peer credentials of the Wayland socket, which works for any compositor.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Ask the compositor this process runs under to quit, ending the session.
pub fn exit() -> Result<(), String> {
    let pid = pid()?;
    // SAFETY: `kill` takes plain integers and has no memory-safety contract.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(format!("cannot ask the compositor ({pid}) to quit: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The compositor's pid: `LABWC_PID`, else the owner of the Wayland socket.
fn pid() -> Result<libc::pid_t, String> {
    if let Some(pid) = std::env::var("LABWC_PID").ok().and_then(|p| p.parse::<libc::pid_t>().ok()) {
        return Ok(pid);
    }
    let path = socket_path().ok_or("not in a Wayland session (no WAYLAND_DISPLAY)")?;
    let stream = UnixStream::connect(&path).map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
    peer_pid(&stream)
}

/// The Wayland socket: `WAYLAND_DISPLAY` as is when absolute, else inside
/// `XDG_RUNTIME_DIR`.
fn socket_path() -> Option<PathBuf> {
    let display = std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty())?;
    let display = PathBuf::from(display);
    if display.is_absolute() { return Some(display); }
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(display))
}

/// The pid of the process at the other end of `stream` (`SO_PEERCRED`).
fn peer_pid(stream: &UnixStream) -> Result<libc::pid_t, String> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes and `len` holds the size
    // of `cred`, as `getsockopt` requires.
    let rc = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    if rc != 0 { return Err(format!("cannot read the compositor's pid: {}", std::io::Error::last_os_error())); }
    if cred.pid <= 0 { return Err("the Wayland socket has no peer pid".to_string()); }
    Ok(cred.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peer_of_a_local_pair_is_this_process() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_pid(&a).unwrap(), std::process::id() as libc::pid_t);
    }
}
