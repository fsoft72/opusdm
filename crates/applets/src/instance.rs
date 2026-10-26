//! Single instance of `opusdm-applets`: own the socket or report why not.

use crate::protocol::{request_at, socket_path, Reply, Request};
use opusdm_ipc::instance::{acquire_socket, Acquired};
use std::os::unix::net::UnixListener;
use std::path::Path;

/// Why this process must not run.
#[derive(Debug)]
pub enum Exit {
    /// A live instance owns the socket.
    AlreadyRunning,
    /// The socket could not be bound.
    Failed(String),
}

/// Become the owner of the applets socket.
pub fn acquire() -> Result<UnixListener, Exit> {
    acquire_at(&socket_path())
}

/// `acquire` on an explicit path.
pub fn acquire_at(path: &Path) -> Result<UnixListener, Exit> {
    match acquire_socket(path, probe) {
        Acquired::Bound(l) => Ok(l),
        Acquired::Live => Err(Exit::AlreadyRunning),
        Acquired::Failed(msg) => Err(Exit::Failed(msg)),
    }
}

/// Live only if the socket answers `GetEdit` with `Edit(_)` within the client
/// timeout; silence or garbage means a stale file.
fn probe(path: &Path) -> bool {
    matches!(request_at(path, &Request::GetEdit), Ok(Reply::Edit(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::frame::{recv, send};

    fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("oa8b").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn a_second_instance_sees_the_live_one() {
        let td = short_dir();
        let path = td.path().join("a.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _: Option<Request> = recv(&mut s).unwrap();
            send(&mut s, &Reply::Edit(false)).unwrap();
        });
        assert!(matches!(acquire_at(&path), Err(Exit::AlreadyRunning)));
        h.join().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_socket_that_does_not_answer_the_protocol_is_stale() {
        let td = short_dir();
        let path = td.path().join("a.sock");
        let silent = UnixListener::bind(&path).unwrap();
        // The silent listener accepts in the kernel backlog and never replies.
        let got = acquire_at(&path);
        assert!(got.is_ok(), "expected Bound, got {got:?}");
        drop(silent);
    }

    #[test]
    fn a_socket_that_answers_garbage_is_stale() {
        use std::io::Write;
        let td = short_dir();
        let path = td.path().join("a.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _ = s.write_all(b"\x00\x00\x00\x03abc");
        });
        assert!(acquire_at(&path).is_ok());
        h.join().unwrap();
    }

    #[test]
    fn a_free_path_is_bound() {
        let td = short_dir();
        assert!(acquire_at(&td.path().join("a.sock")).is_ok());
    }
}
