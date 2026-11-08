//! Single instance of `opusdm-dock`: own the dock socket or report why not.

use opusdm_ipc::dock::{dock_request_at, dock_socket_path, DockReply, DockRequest};
use opusdm_ipc::instance::{acquire_socket, Acquired};
use std::os::unix::net::UnixListener;
use std::path::Path;

/// Why this process cannot own the dock socket.
#[derive(Debug)]
pub enum Exit {
    /// A live dock owns the socket.
    AlreadyRunning,
    /// The socket could not be bound.
    Failed(String),
}

/// Become the owner of the dock socket.
pub fn acquire() -> Result<UnixListener, Exit> {
    acquire_at(&dock_socket_path())
}

/// [`acquire`] on an explicit path.
pub fn acquire_at(path: &Path) -> Result<UnixListener, Exit> {
    match acquire_socket(path, probe) {
        Acquired::Bound(l) => Ok(l),
        Acquired::Live => Err(Exit::AlreadyRunning),
        Acquired::Failed(msg) => Err(Exit::Failed(msg)),
    }
}

/// Live only if the socket answers `Ping` with `Hidden(_)` within the client
/// timeout; silence or garbage means a stale file.
fn probe(path: &Path) -> bool {
    matches!(dock_request_at(path, &DockRequest::Ping), Ok(DockReply::Hidden(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};

    /// Short temp dir under /tmp so socket paths stay below SUN_LEN.
    fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("odi").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn a_second_instance_sees_the_live_one() {
        let td = short_dir();
        let path = td.path().join("d.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _: Option<DockRequest> = recv(&mut s).unwrap();
            send(&mut s, &DockReply::Hidden(false)).unwrap();
        });
        assert!(matches!(acquire_at(&path), Err(Exit::AlreadyRunning)));
        h.join().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_socket_that_does_not_answer_is_stale() {
        let td = short_dir();
        let path = td.path().join("d.sock");
        let silent = UnixListener::bind(&path).unwrap();
        let got = acquire_at(&path);
        assert!(got.is_ok(), "expected Bound, got {got:?}");
        drop(silent);
    }

    #[test]
    fn a_free_path_is_bound() {
        let td = short_dir();
        assert!(acquire_at(&td.path().join("d.sock")).is_ok());
    }
}
