//! The socket side of `opusdm-applets`: the accept thread and one thread per
//! connection.
//!
//! `GetEdit` (also the liveness probe of `instance`) and `SetEdit` are
//! answered in the connection thread from a shared edit flag, never through
//! the GTK main loop, so a busy main loop cannot make a live instance look
//! stale. Every other request is sent to the host on the main loop
//! ([`HostMessage::Request`]) and its reply comes back on a one-shot channel;
//! the connection thread waits at most the reply timeout and then answers
//! [`BUSY_MESSAGE`].

use crate::protocol::{Reply, Request};
use opusdm_ipc::err_log;
use opusdm_ipc::frame::{recv, send};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::Duration;

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Read and write timeout of an accepted stream, so a stuck client cannot pin its thread.
pub const CONN_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest wait for the main loop to answer one request.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The answer when the main loop did not reply within the reply timeout.
pub const BUSY_MESSAGE: &str = "the applets process is busy";

/// The answer when the main-loop side of the request channel is gone.
pub const HOST_GONE_MESSAGE: &str = "the applets host no longer answers requests";

/// Name of the thread that accepts connections.
const ACCEPT_THREAD: &str = "opusdm-applets-accept";

/// Name of the thread that serves one connection.
const CONN_THREAD: &str = "opusdm-applets-conn";

/// What a connection thread sends to the host on the GTK main loop.
#[derive(Debug)]
pub enum HostMessage {
    /// A request to answer; the reply goes into the one-shot sender.
    Request(Request, SyncSender<Reply>),
    /// The edit mode was turned on or off (already stored in the flag).
    Edit(bool),
}

/// What every connection thread shares: the edit flag and the way to the host.
#[derive(Clone)]
pub struct Shared {
    edit: Arc<AtomicBool>,
    to_host: async_channel::Sender<HostMessage>,
    reply_timeout: Duration,
}

impl Shared {
    /// Connection state over `edit` and the host channel `to_host`, waiting
    /// [`REPLY_TIMEOUT`] for each reply.
    pub fn new(edit: Arc<AtomicBool>, to_host: async_channel::Sender<HostMessage>) -> Shared {
        Shared { edit, to_host, reply_timeout: REPLY_TIMEOUT }
    }

    /// The same, waiting `timeout` for each reply (tests use a short one).
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Shared {
        self.reply_timeout = timeout;
        self
    }
}

/// Answer one request in the connection thread (see the module doc).
pub fn answer(req: &Request, shared: &Shared) -> Reply {
    match req {
        Request::GetEdit => Reply::Edit(shared.edit.load(Ordering::SeqCst)),
        Request::SetEdit(on) => {
            shared.edit.store(*on, Ordering::SeqCst);
            if let Err(e) = shared.to_host.try_send(HostMessage::Edit(*on)) {
                err_log!(APP_TAG, "edit mode {on} stored, but the host was not told: {e}");
            }
            Reply::Edit(*on)
        }
        _ => ask_host(req, shared),
    }
}

/// Send `req` to the host on the main loop and wait for its reply, at most
/// the reply timeout. Every failure is logged and answered with an `Error`.
fn ask_host(req: &Request, shared: &Shared) -> Reply {
    let (reply_tx, reply_rx) = sync_channel(1);
    if let Err(e) = shared.to_host.try_send(HostMessage::Request(req.clone(), reply_tx)) {
        err_log!(APP_TAG, "request {req:?} not delivered to the host: {e}");
        return Reply::Error(HOST_GONE_MESSAGE.into());
    }
    match reply_rx.recv_timeout(shared.reply_timeout) {
        Ok(reply) => reply,
        Err(RecvTimeoutError::Timeout) => {
            err_log!(APP_TAG, "request {req:?}: no reply from the main loop within {:?}, answered busy", shared.reply_timeout);
            Reply::Error(BUSY_MESSAGE.into())
        }
        Err(RecvTimeoutError::Disconnected) => {
            err_log!(APP_TAG, "request {req:?}: the host dropped it without a reply");
            Reply::Error(HOST_GONE_MESSAGE.into())
        }
    }
}

/// Serve one connection: answer every request until a clean EOF.
pub fn handle_conn(stream: UnixStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(CONN_READ_TIMEOUT))?;
    stream.set_write_timeout(Some(CONN_READ_TIMEOUT))?;
    let mut stream = stream;
    while let Some(req) = recv::<_, Request>(&mut stream)? {
        send(&mut stream, &answer(&req, shared))?;
    }
    Ok(())
}

/// Start the accept thread, which owns `listener` for the whole run and
/// serves each connection on its own thread.
pub fn spawn_accept(listener: UnixListener, shared: Shared) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(ACCEPT_THREAD.into()).spawn(move || {
        for conn in listener.incoming() {
            let stream = match conn {
                Ok(s) => s,
                Err(e) => {
                    err_log!(APP_TAG, "accept failed, exiting: {e}");
                    std::process::exit(1);
                }
            };
            spawn_conn(stream, shared.clone());
        }
    })
}

/// Serve `stream` on its own thread; a thread that cannot start is logged
/// and the connection dropped.
fn spawn_conn(stream: UnixStream, shared: Shared) {
    let spawned = std::thread::Builder::new().name(CONN_THREAD.into()).spawn(move || {
        if let Err(e) = handle_conn(stream, &shared) {
            err_log!(APP_TAG, "connection dropped: {e}");
        }
    });
    if let Err(e) = spawned {
        err_log!(APP_TAG, "cannot start a connection thread, connection dropped: {e}");
    }
}

#[cfg(test)]
mod tests;
