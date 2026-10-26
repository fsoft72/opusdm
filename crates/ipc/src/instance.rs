//! Single-instance socket and sibling-tool lookup shared by the hub and the
//! tools that must run once per session.

use crate::sockpath::ensure_sock_dir;
use std::io;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

/// Exit code of a single-instance tool (`opusdm-applets`, and the hub's applet
/// supervisor which reads it) that found a live owner of its socket.
pub const EXIT_ALREADY_RUNNING: i32 = 3;

/// Result of trying to become the owner of a single-instance socket.
#[derive(Debug)]
pub enum Acquired {
    /// We own the socket now.
    Bound(UnixListener),
    /// A live owner already runs; the socket file was not touched.
    Live,
    /// Could not bind and no live owner answered.
    Failed(String),
}

/// Ensure the socket dir, report a live owner, or bind the socket. `is_live`
/// probes the path; the hand-off to a live owner stays with the caller.
pub fn acquire_socket(path: &Path, is_live: impl Fn(&Path) -> bool) -> Acquired {
    if let Err(e) = ensure_sock_dir(path) {
        return Acquired::Failed(format!("cannot create socket dir: {e}"));
    }
    if is_live(path) {
        return Acquired::Live;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Acquired::Failed(format!("cannot remove stale socket: {e}")),
    }
    match UnixListener::bind(path) {
        Ok(l) => Acquired::Bound(l),
        Err(e) => Acquired::Failed(format!("cannot bind {}: {e}", path.display())),
    }
}

/// Path of the tool `name`: next to `dir` (the hub's own folder) when known,
/// otherwise the bare name for a `PATH` lookup.
pub fn sibling_bin_in(dir: Option<&Path>, name: &str) -> PathBuf {
    match dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_path_is_bound() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("x.sock");
        match acquire_socket(&path, |_| false) {
            Acquired::Bound(_l) => {}
            other => panic!("expected Bound, got {other:?}"),
        }
        assert!(path.exists());
    }

    #[test]
    fn a_live_owner_is_reported_and_the_file_is_kept() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("x.sock");
        let _owner = UnixListener::bind(&path).unwrap();
        match acquire_socket(&path, |_| true) {
            Acquired::Live => {}
            other => panic!("expected Live, got {other:?}"),
        }
        assert!(path.exists(), "the live owner's socket file must stay");
        assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
    }

    #[test]
    fn a_stale_socket_file_is_replaced() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("x.sock");
        std::fs::write(&path, b"").unwrap();
        match acquire_socket(&path, |_| false) {
            Acquired::Bound(_l) => {}
            other => panic!("expected Bound after clearing stale, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_parent_dir_is_created() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("missing").join("sub").join("x.sock");
        match acquire_socket(&path, |_| false) {
            Acquired::Bound(_l) => {}
            other => panic!("expected Bound, got {other:?}"),
        }
        assert!(path.exists());
    }

    #[test]
    fn a_tool_is_looked_up_next_to_the_hub_then_on_path() {
        let dir = Path::new("/opt/opusdm/bin");
        assert_eq!(sibling_bin_in(Some(dir), "opusdm-input-config"), dir.join("opusdm-input-config"));
        assert_eq!(sibling_bin_in(None, "opusdm-input-config"), std::path::PathBuf::from("opusdm-input-config"));
    }
}
