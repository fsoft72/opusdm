//! Harness helpers for the private IPC of the compositor: requests, raw
//! writes and `watch` event streams over its Unix socket (4-byte
//! little-endian length + JSON, as Wayfire).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::Compositor;

/// How long `ipc_request` waits for its reply.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest message the harness accepts from the compositor.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Connects to the IPC socket of `comp`.
fn _connect(comp: &Compositor) -> UnixStream {
    UnixStream::connect(&comp.ipc_socket).expect("connect to the compositor IPC socket")
}

/// Writes one framed `{"method", "data"}` request on `stream`.
fn _send(stream: &mut UnixStream, method: &str, data: Value) {
    let body = serde_json::to_vec(&json!({"method": method, "data": data})).expect("request json");
    let len = u32::try_from(body.len()).expect("request length");
    stream.write_all(&len.to_le_bytes()).expect("write request length");
    stream.write_all(&body).expect("write request body");
}

/// Reads one framed message from `stream`, waiting at most until `deadline`.
/// `None` on timeout or when the compositor closed the connection.
fn _read_message(stream: &mut UnixStream, deadline: Instant) -> Option<Value> {
    let mut header = [0u8; 4];
    _read_exact(stream, &mut header, deadline)?;
    let len = usize::try_from(u32::from_le_bytes(header)).expect("length fits");
    assert!(len <= MAX_MESSAGE, "the compositor sent a message of {len} bytes");
    let mut body = vec![0u8; len];
    _read_exact(stream, &mut body, deadline)?;
    Some(serde_json::from_slice(&body).expect("the compositor sent valid JSON"))
}

/// Fills `buf` from `stream` before `deadline`; `None` on timeout or EOF.
fn _read_exact(stream: &mut UnixStream, buf: &mut [u8], deadline: Instant) -> Option<()> {
    let mut done = 0;
    while done < buf.len() {
        let left = deadline.checked_duration_since(Instant::now())?;
        stream.set_read_timeout(Some(left.max(Duration::from_millis(1)))).expect("read timeout");
        match stream.read(&mut buf[done..]) {
            Ok(0) => return None,
            Ok(n) => done += n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return None,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => panic!("read from the IPC socket: {e}"),
        }
    }
    Some(())
}

/// Sends one request on a new connection and returns its reply; panics when
/// no reply comes within `REQUEST_TIMEOUT`.
pub fn ipc_request(comp: &Compositor, method: &str, data: Value) -> Value {
    let mut stream = _connect(comp);
    _send(&mut stream, method, data);
    _read_message(&mut stream, Instant::now() + REQUEST_TIMEOUT)
        .unwrap_or_else(|| panic!("no reply to {method} within {REQUEST_TIMEOUT:?}"))
}

/// Writes `bytes` as they are on a new connection, then waits (at most
/// `REQUEST_TIMEOUT`) until the compositor closes it. True when it did.
pub fn ipc_send_raw(comp: &Compositor, bytes: &[u8]) -> bool {
    let mut stream = _connect(comp);
    stream.write_all(bytes).expect("write raw bytes");
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut buf = [0u8; 64];
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
        stream.set_read_timeout(Some(left.max(Duration::from_millis(1)))).expect("read timeout");
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return true,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return false,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => panic!("read from the IPC socket: {e}"),
        }
    }
}

/// A `watch` connection: the events the compositor sends on it.
pub struct IpcWatch {
    stream: UnixStream,
}

/// Opens a connection, sends `watch` and checks its `{"ok": true}` reply.
pub fn ipc_watch(comp: &Compositor) -> IpcWatch {
    let mut stream = _connect(comp);
    _send(&mut stream, "watch", json!({}));
    let reply = _read_message(&mut stream, Instant::now() + REQUEST_TIMEOUT).expect("reply to watch");
    assert_eq!(reply["ok"], true, "watch refused: {reply}");
    IpcWatch { stream }
}

impl IpcWatch {
    /// The next event, waiting at most `timeout`; `None` on timeout or when
    /// the compositor closed the connection.
    pub fn next_event(&mut self, timeout: Duration) -> Option<Value> {
        _read_message(&mut self.stream, Instant::now() + timeout)
    }

    /// Reads events until one named `event` comes (true) or `timeout` passes.
    pub fn wait_for(&mut self, event: &str, timeout: Duration) -> bool {
        self.wait_for_event(event, timeout).is_some()
    }

    /// Reads events until one named `event` comes and returns it, or `None`
    /// once `timeout` passes.
    pub fn wait_for_event(&mut self, event: &str, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let value = self.next_event(left)?;
            if value["event"] == event {
                return Some(value);
            }
        }
    }
}
