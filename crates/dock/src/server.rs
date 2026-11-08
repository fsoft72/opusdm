//! The dock socket server: a thread accepts the hub's requests and hands
//! each one to the GTK main loop, which answers it.
//!
//! Ownership: [`spawn`] starts the detached thread `opusdm-dock-ipc`, which
//! owns the listener and the sending end of the channel for the whole run.
//! The receiving end belongs to the main loop future (see `standalone.rs`).
//! If that future is gone, the thread logs it and ends: the dock then runs
//! on without Hide All.

use opusdm_ipc::dock::{DockReply, DockRequest};
use opusdm_ipc::frame::{recv, send};
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;

/// Name of the server thread.
const THREAD_NAME: &str = "opusdm-dock-ipc";

/// Log tag.
const TAG: &str = "opusdm-dock";

/// How long a connection waits for the request frame.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the server waits for the main loop to answer: below the
/// client timeout, so the client gets the "busy" error instead of silence.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(800);

/// One request on its way to the main loop, with the channel for its answer.
pub struct Incoming {
    /// What the client asked.
    pub request: DockRequest,
    /// Where the main loop sends the answer.
    pub reply: mpsc::SyncSender<DockReply>,
}

/// Start the server thread on `listener`; requests go to `to_main`.
pub fn spawn(listener: UnixListener, to_main: async_channel::Sender<Incoming>) -> io::Result<()> {
    std::thread::Builder::new().name(THREAD_NAME.into()).spawn(move || serve(listener, to_main)).map(|_| ())
}

/// The accept loop. A failed connection is logged and the loop goes on; a
/// closed channel to the main loop ends it (logged).
pub fn serve(listener: UnixListener, to_main: async_channel::Sender<Incoming>) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                opusdm_ipc::err_log!(TAG, "dock socket accept failed: {e}");
                continue;
            }
        };
        match handle(stream, &to_main) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe && to_main.is_closed() => {
                opusdm_ipc::err_log!(TAG, "the dock main loop no longer takes requests: the dock socket server stops, Hide All will not reach the dock");
                return;
            }
            Err(e) => opusdm_ipc::err_log!(TAG, "dock socket request failed: {e}"),
        }
    }
}

/// Read one request from `stream`, pass it to the main loop and write the
/// answer back.
fn handle(mut stream: UnixStream, to_main: &async_channel::Sender<Incoming>) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    let Some(request) = recv::<_, DockRequest>(&mut stream)? else {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "client closed before sending a request"));
    };
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    if to_main.send_blocking(Incoming { request, reply: reply_tx }).is_err() {
        let _ = send(&mut stream, &DockReply::Error("the dock main loop is gone".into()));
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the dock main loop is gone"));
    }
    let reply = match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(reply) => reply,
        Err(_) => {
            opusdm_ipc::err_log!(TAG, "the dock main loop did not answer {request:?} within {REPLY_TIMEOUT:?}");
            DockReply::Error("the dock is busy".into())
        }
    };
    send(&mut stream, &reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::dock::dock_request_at;

    /// Short temp dir under /tmp so socket paths stay below SUN_LEN.
    fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("ods").tempdir_in("/tmp").unwrap()
    }

    /// A fake main loop on its own thread: answers like the dock does and
    /// counts the requests.
    fn fake_main(rx: async_channel::Receiver<Incoming>) -> std::thread::JoinHandle<usize> {
        std::thread::spawn(move || {
            let mut hidden = false;
            let mut count = 0;
            while let Ok(inc) = rx.recv_blocking() {
                if let DockRequest::SetHidden(h) = inc.request {
                    hidden = h;
                }
                count += 1;
                inc.reply.send(DockReply::Hidden(hidden)).unwrap();
            }
            count
        })
    }

    #[test]
    fn set_hidden_is_answered_with_the_new_state_and_ping_reads_it_back() {
        let td = short_dir();
        let path = td.path().join("d.sock");
        let (tx, rx) = async_channel::unbounded();
        spawn(UnixListener::bind(&path).unwrap(), tx).unwrap();
        let main = fake_main(rx);
        assert_eq!(dock_request_at(&path, &DockRequest::SetHidden(true)).unwrap(), DockReply::Hidden(true));
        assert_eq!(dock_request_at(&path, &DockRequest::Ping).unwrap(), DockReply::Hidden(true));
        assert_eq!(dock_request_at(&path, &DockRequest::SetHidden(false)).unwrap(), DockReply::Hidden(false));
        drop(main);
    }

    #[test]
    fn the_server_still_answers_after_many_requests_and_a_bad_client() {
        use std::io::Write;
        let td = short_dir();
        let path = td.path().join("d.sock");
        let (tx, rx) = async_channel::unbounded();
        spawn(UnixListener::bind(&path).unwrap(), tx).unwrap();
        let _main = fake_main(rx);
        // A client that sends garbage, one that sends nothing.
        UnixStream::connect(&path).unwrap().write_all(b"\x00\x00\x00\x03abc").unwrap();
        drop(UnixStream::connect(&path).unwrap());
        for i in 0..50 {
            let want = i % 2 == 0;
            assert_eq!(dock_request_at(&path, &DockRequest::SetHidden(want)).unwrap(), DockReply::Hidden(want), "request {i}");
        }
    }

    #[test]
    fn a_main_loop_that_does_not_answer_gives_a_busy_error_not_silence() {
        let td = short_dir();
        let path = td.path().join("d.sock");
        let (tx, rx) = async_channel::unbounded::<Incoming>();
        spawn(UnixListener::bind(&path).unwrap(), tx).unwrap();
        // Keep the requests without answering them.
        let keep = std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok(inc) = rx.recv_blocking() {
                held.push(inc);
            }
        });
        assert!(matches!(dock_request_at(&path, &DockRequest::Ping).unwrap(), DockReply::Error(_)));
        drop(keep);
    }

    #[test]
    fn a_gone_main_loop_is_reported_to_the_client() {
        let td = short_dir();
        let path = td.path().join("d.sock");
        let (tx, rx) = async_channel::unbounded::<Incoming>();
        drop(rx);
        spawn(UnixListener::bind(&path).unwrap(), tx).unwrap();
        assert!(matches!(dock_request_at(&path, &DockRequest::Ping).unwrap(), DockReply::Error(_)));
    }
}
