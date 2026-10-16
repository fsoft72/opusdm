//! Where the hub socket lives, and how to tell a live hub from a stale file.
//!
//! These tests mutate process environment; run single-threaded.

use crate::frame::{recv, send};
use crate::proto::{ToHub, ToLister};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable that, when set, overrides the socket path.
pub const ENV_SOCK: &str = "OPUSDM_HUB_SOCK";

/// Resolve the hub socket path.
///
/// Order: `$OPUSDM_HUB_SOCK`, then `$XDG_RUNTIME_DIR/opusdm/hub.sock`,
/// then `/tmp/opusdm-$UID/hub.sock`.
pub fn hub_sock_path() -> PathBuf {
    if let Some(p) = std::env::var_os(ENV_SOCK) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("opusdm").join("hub.sock");
        }
    }
    // SAFETY: getuid is always safe; it cannot fail and touches no memory.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/opusdm-{uid}/hub.sock"))
}

/// Create the directory that will contain `path`, mode 0700. No-op if present.
pub fn ensure_sock_dir(path: &Path) -> io::Result<()> {
    let Some(dir) = path.parent() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"));
    };
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true).mode(0o700);
    b.create(dir)
}

/// Connect to the hub socket at `sock`, send `msgs` in order and hang up,
/// without waiting for an answer: the one-shot client of the tools that only
/// tell the hub something (open a Lister, apply settings, run a function).
pub fn send_oneshot(sock: &Path, msgs: &[ToHub]) -> io::Result<()> {
    let mut stream = UnixStream::connect(sock)?;
    for msg in msgs {
        send(&mut stream, msg)?;
    }
    Ok(())
}

/// Connect to `path` and probe with `Ping`. `true` only if a live hub replies `Pong`.
pub fn probe_live_hub(path: &Path) -> bool {
    let Ok(mut s) = UnixStream::connect(path) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
    if send(&mut s, &ToHub::Ping).is_err() {
        return false;
    }
    matches!(recv::<_, ToLister>(&mut s), Ok(Some(ToLister::Pong)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        temp_env_var(ENV_SOCK, Some("/run/custom.sock"), || {
            assert_eq!(hub_sock_path(), PathBuf::from("/run/custom.sock"));
        });
    }

    #[test]
    fn xdg_runtime_dir_is_used_when_no_override() {
        temp_env_var(ENV_SOCK, None, || {
            temp_env_var("XDG_RUNTIME_DIR", Some("/run/user/1000"), || {
                assert_eq!(
                    hub_sock_path(),
                    PathBuf::from("/run/user/1000/opusdm/hub.sock")
                );
            });
        });
    }

    #[test]
    fn tmp_fallback_shape() {
        temp_env_var(ENV_SOCK, None, || {
            temp_env_var("XDG_RUNTIME_DIR", Some(""), || {
                let p = hub_sock_path();
                let s = p.to_str().unwrap();
                assert!(s.starts_with("/tmp/opusdm-"), "{s}");
                assert!(s.ends_with("/hub.sock"), "{s}");
            });
        });
    }

    #[test]
    fn send_oneshot_delivers_every_message_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("hub.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        send_oneshot(&sock, &[ToHub::Ping, ToHub::RaiseToolbar]).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        assert!(matches!(recv::<_, ToHub>(&mut peer).unwrap(), Some(ToHub::Ping)));
        assert!(matches!(recv::<_, ToHub>(&mut peer).unwrap(), Some(ToHub::RaiseToolbar)));
        assert!(recv::<_, ToHub>(&mut peer).unwrap().is_none(), "the client hung up");
    }

    #[test]
    fn send_oneshot_fails_when_no_hub_listens() {
        let dir = tempfile::tempdir().unwrap();
        assert!(send_oneshot(&dir.path().join("nope.sock"), &[ToHub::Ping]).is_err());
    }

    #[test]
    fn probe_returns_false_for_missing_socket() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!probe_live_hub(&dir.path().join("nope.sock")));
    }

    /// Minimal scoped env-var setter/restorer for single-threaded tests.
    fn temp_env_var(key: &str, val: Option<&str>, f: impl FnOnce()) {
        let prev = std::env::var_os(key);
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        f();
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
