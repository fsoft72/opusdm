//! The thread that owns the live session, so the X tools (`xinput`,
//! `setxkbmap`, `xset`) never run on the GTK thread. The window sends
//! requests and gets replies; changes that queue up while a command runs are
//! merged, so only the newest one is applied.

use std::path::PathBuf;
use std::sync::mpsc;

use crate::backend::{Backend, Status};
use crate::session::Session;
use crate::{InputError, InputSettings};

/// What the window asks the worker.
#[derive(Debug)]
pub enum Request {
    /// Apply these settings live.
    Change(InputSettings),
    /// Write the current settings to this file.
    Commit(PathBuf),
    /// Put back what the system had when the session opened.
    Revert,
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Reply {
    /// The session is open: which groups can be edited and the settings the
    /// system has now. Always the first reply.
    Opened { status: Status, current: InputSettings },
    /// A change was applied; the errors of the commands that failed.
    Applied(Vec<InputError>),
    /// The result of a `Commit`.
    Committed(Result<(), InputError>),
    /// The snapshot was put back; the errors of the commands that failed.
    Reverted(Vec<InputError>),
}

/// Start the worker thread. `make` builds the backend on the thread (it may
/// run the X tools already). The thread ends when the request sender or the
/// reply receiver is dropped.
pub fn spawn<B, F>(make: F, replies: async_channel::Sender<Reply>) -> mpsc::Sender<Request>
where
    B: Backend + 'static,
    F: FnOnce() -> B + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::spawn(move || {
        let mut session = Session::open(make());
        let opened = Reply::Opened { status: session.status(), current: session.current().clone() };
        if replies.send_blocking(opened).is_err() { return; }
        run(&mut session, &rx, &replies);
    });
    tx
}

/// Serve the requests of `rx` on `session` until the senders are gone or
/// nobody listens to the replies.
pub fn run<B: Backend>(session: &mut Session<B>, rx: &mpsc::Receiver<Request>, replies: &async_channel::Sender<Reply>) {
    let mut held: Option<Request> = None;
    loop {
        let request = match held.take() {
            Some(r) => r,
            None => match rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            },
        };
        let request = match request {
            Request::Change(settings) => {
                let (newest, next) = newest_change(settings, rx);
                held = next;
                Request::Change(newest)
            }
            other => other,
        };
        if replies.send_blocking(handle(session, request)).is_err() { return; }
    }
}

/// The newest of `first` and the changes already queued in `rx`, and the
/// first queued request that is not a change, if one came.
fn newest_change(first: InputSettings, rx: &mpsc::Receiver<Request>) -> (InputSettings, Option<Request>) {
    let mut newest = first;
    while let Ok(request) = rx.try_recv() {
        match request {
            Request::Change(settings) => newest = settings,
            other => return (newest, Some(other)),
        }
    }
    (newest, None)
}

/// Run one request on `session`.
fn handle<B: Backend>(session: &mut Session<B>, request: Request) -> Reply {
    match request {
        Request::Change(settings) => Reply::Applied(session.change(settings)),
        Request::Commit(path) => Reply::Committed(session.commit(&path)),
        Request::Revert => Reply::Reverted(session.revert()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeBackend;

    fn with_speed(speed: f64) -> InputSettings {
        let mut s = InputSettings::default();
        s.mouse.speed = speed;
        s
    }

    /// Queue `requests`, run the worker loop to the end and return the
    /// replies and the session.
    fn serve(requests: Vec<Request>) -> (Vec<Reply>, Session<FakeBackend>) {
        let mut session = Session::open(FakeBackend::new(InputSettings::default()));
        let (tx, rx) = mpsc::channel();
        for r in requests {
            tx.send(r).unwrap();
        }
        drop(tx);
        let (reply_tx, reply_rx) = async_channel::unbounded();
        run(&mut session, &rx, &reply_tx);
        let mut replies = Vec::new();
        while let Ok(r) = reply_rx.try_recv() {
            replies.push(r);
        }
        (replies, session)
    }

    #[test]
    fn queued_changes_are_merged_into_the_newest() {
        let (replies, session) = serve(vec![Request::Change(with_speed(0.1)), Request::Change(with_speed(0.2)), Request::Change(with_speed(0.3))]);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert!(matches!(&replies[0], Reply::Applied(e) if e.is_empty()));
        assert_eq!(session.current().mouse.speed, 0.3);
    }

    #[test]
    fn a_commit_after_changes_saves_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let (replies, _) = serve(vec![Request::Change(with_speed(0.1)), Request::Change(with_speed(0.4)), Request::Commit(path.clone())]);
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert!(matches!(&replies[1], Reply::Committed(Ok(()))));
        assert_eq!(crate::store::load_from(&path).unwrap().mouse.speed, 0.4);
    }

    #[test]
    fn revert_after_a_change_puts_the_snapshot_back() {
        let (replies, session) = serve(vec![Request::Change(with_speed(0.5)), Request::Revert]);
        assert!(matches!(replies.last(), Some(Reply::Reverted(e)) if e.is_empty()));
        assert_eq!(session.current(), session.snapshot());
    }

    #[test]
    fn the_thread_reports_the_open_session_first() {
        let (reply_tx, reply_rx) = async_channel::unbounded();
        let requests = spawn(|| FakeBackend::new(with_speed(0.25)), reply_tx);
        requests.send(Request::Change(with_speed(0.5))).unwrap();
        match reply_rx.recv_blocking().unwrap() {
            Reply::Opened { current, status } => {
                assert_eq!(current.mouse.speed, 0.25);
                assert!(status.mouse.available);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(reply_rx.recv_blocking().unwrap(), Reply::Applied(_)));
    }
}
