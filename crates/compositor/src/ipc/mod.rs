//! Private IPC of the compositor: a Unix socket at
//! `$XDG_RUNTIME_DIR/opusdm-compositor-<WAYLAND_DISPLAY>.sock` (exported to
//! the children as `OPUSDM_COMPOSITOR_SOCKET`), Wayfire framing (4-byte
//! little-endian length + JSON), one request `{"method", "data"}` and one
//! reply object at a time; `watch` makes a connection an event stream.
//!
//! Ownership: `IpcServer` lives in `State` (owned by `LoopData`, owned by
//! the backend `run` for the whole run). It owns every connection (its
//! stream, read buffer and pending writes); the calloop sources (listener,
//! one read source per connection, one write source while a connection has
//! pending bytes) hold only a duplicate fd and the connection id. Closing a
//! connection drops it from the table and shuts its socket down: its
//! sources wake up (hang-up), find no connection and remove themselves. A
//! dead or flooding client is dropped with an `err_log!` line, the server
//! keeps accepting. Dropping the server (the loop has ended: quit, signal,
//! error) removes the socket file.
//!
//! The hub is the connection that sent `hotkeys/set` and is a `watch`
//! stream (it sends `hotkeys/set`, then `watch`, on the same connection;
//! or `hotkeys/set` on its watch stream). The server keeps only its id
//! (`hub`), never the connection itself. A `hotkeys/set` from a connection
//! that is not watching replaces the table and makes that connection the
//! candidate (`hub_candidate`): it becomes the hub when it sends `watch`;
//! a candidate that closes first was a one-shot client (`debug_log!`, the
//! hub, if any, keeps its role). When the hub closes its id is cleared with
//! `err_log!("hub disconnected")`; the hotkey table (in `State::hotkeys`)
//! stays, and a matched hub hotkey is logged as "no hub connected" and
//! swallowed until a new hub connects. `hotkey` events go to the hub only,
//! never to the other watchers (the panel).

pub mod frame;
pub mod protocol;

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use serde_json::Value;
use smithay::reexports::calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction};

use crate::backend::LoopData;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;
use protocol::{After, Snapshot, WindowView};

/// Largest incoming message, in bytes.
pub const IPC_MAX_MESSAGE: usize = 1024 * 1024;

/// Most bytes queued for one connection that it has not read yet.
pub const IPC_MAX_PENDING: usize = 4 * 1024 * 1024;

/// Most connections open at the same time; more are refused.
pub const IPC_MAX_CONNECTIONS: usize = 64;

/// Bytes read from one connection per wake-up (the source is level
/// triggered: the rest is read on the next loop iteration, so one busy
/// client cannot hold the loop).
const READ_BUDGET: usize = 256 * 1024;

/// Size of one read.
const READ_CHUNK: usize = 16 * 1024;

/// Connections accepted per wake-up of the listener.
const ACCEPT_BUDGET: usize = 16;

/// One client connection.
struct Conn {
    /// The socket, non-blocking.
    stream: UnixStream,
    /// Bytes read and not yet decoded into a message.
    read_buf: Vec<u8>,
    /// Bytes to write, in order.
    pending: Vec<u8>,
    /// True once the connection asked for `watch`.
    watch: bool,
    /// True while a write source is registered for it.
    writing: bool,
}

/// The IPC server: the listening socket and every connection.
#[derive(Default)]
pub struct IpcServer {
    /// Handle of the event loop the sources live in; `None` for a server
    /// that was never bound (unit tests).
    handle: Option<LoopHandle<'static, LoopData>>,
    /// Path of the socket file, removed when the server is dropped.
    path: Option<PathBuf>,
    /// The open connections, by id.
    conns: HashMap<u64, Conn>,
    /// Id of the next connection.
    next_id: u64,
    /// What the `watch` connections were last told.
    snapshot: Snapshot,
    /// The hub: the connection that sent `hotkeys/set` and is a watcher.
    /// The only receiver of `hotkey` events.
    hub: Option<u64>,
    /// A connection that sent `hotkeys/set` and is not watching yet: it
    /// becomes the hub when it sends `watch`.
    hub_candidate: Option<u64>,
}

/// Path of the IPC socket for the Wayland socket `wayland_display` in
/// `runtime_dir`.
pub fn socket_path(runtime_dir: &Path, wayland_display: &str) -> PathBuf {
    runtime_dir.join(format!("opusdm-compositor-{wayland_display}.sock"))
}

impl IpcServer {
    /// Binds the socket at `path` (a stale file left by a dead compositor is
    /// removed first) and accepts its connections in the loop of `handle`.
    pub fn bind(handle: LoopHandle<'static, LoopData>, path: &Path) -> Result<IpcServer, String> {
        match std::fs::remove_file(path) {
            Ok(()) => opusdm_ipc::err_log!(LOG_TAG, "ipc: stale socket {} removed", path.display()),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(format!("ipc: cannot remove the stale socket {}: {e}", path.display())),
        }
        let listener = UnixListener::bind(path).map_err(|e| format!("ipc: bind {}: {e}", path.display()))?;
        // From here on the server owns the file: an error below drops it and removes the file.
        let server = IpcServer {
            handle: Some(handle.clone()),
            path: Some(path.to_path_buf()),
            conns: HashMap::new(),
            next_id: 0,
            snapshot: Snapshot::default(),
            hub: None,
            hub_candidate: None,
        };
        listener.set_nonblocking(true).map_err(|e| format!("ipc: non-blocking listener: {e}"))?;
        handle
            .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, data: &mut LoopData| {
                data.state.ipc._accept(listener);
                Ok(PostAction::Continue)
            })
            .map_err(|e| format!("ipc: listener source: {e}"))?;
        opusdm_ipc::debug_log!(LOG_TAG, "ipc: listening on {}", path.display());
        Ok(server)
    }

    /// Path of the socket, `None` when the server is not bound.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// True when at least one connection is a `watch` stream.
    pub fn has_watchers(&self) -> bool {
        self.conns.values().any(|c| c.watch)
    }

    /// Queues `event` on every `watch` connection (a connection over
    /// `IPC_MAX_PENDING` is dropped).
    pub fn broadcast(&mut self, event: &Value) {
        let watchers: Vec<u64> = self.conns.iter().filter(|(_, c)| c.watch).map(|(id, _)| *id).collect();
        for id in watchers {
            self._send(id, event);
        }
    }

    /// The events that bring the watchers from the last snapshot to
    /// `windows` / `desktops`; the snapshot becomes the new state.
    pub fn take_events(&mut self, windows: Vec<WindowView>, desktops: (u32, u32)) -> Vec<Value> {
        protocol::snapshot_events(&mut self.snapshot, windows, desktops)
    }

    /// Accepts the waiting connections (at most `ACCEPT_BUDGET`, the
    /// listener is level triggered) and registers a read source for each.
    fn _accept(&mut self, listener: &UnixListener) {
        for _ in 0..ACCEPT_BUDGET {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    opusdm_ipc::err_log!(LOG_TAG, "ipc: accept failed: {e}");
                    return;
                }
            };
            if self.conns.len() >= IPC_MAX_CONNECTIONS {
                opusdm_ipc::err_log!(LOG_TAG, "ipc: connection refused, {IPC_MAX_CONNECTIONS} already open");
                continue;
            }
            if let Err(e) = self._add(stream) {
                opusdm_ipc::err_log!(LOG_TAG, "ipc: connection dropped: {e}");
            }
        }
    }

    /// Registers the new connection `stream` with a read source.
    fn _add(&mut self, stream: UnixStream) -> Result<(), String> {
        let handle = self.handle.clone().ok_or("the server is not bound")?;
        stream.set_nonblocking(true).map_err(|e| format!("non-blocking stream: {e}"))?;
        let watched = stream.try_clone().map_err(|e| format!("dup stream: {e}"))?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        handle
            .insert_source(Generic::new(watched, Interest::READ, Mode::Level), move |_, _, data: &mut LoopData| {
                Ok(_on_readable(&mut data.state, id))
            })
            .map_err(|e| format!("read source: {e}"))?;
        self.conns.insert(id, Conn { stream, read_buf: Vec::new(), pending: Vec::new(), watch: false, writing: false });
        opusdm_ipc::debug_log!(LOG_TAG, "ipc: connection {id} open");
        Ok(())
    }

    /// Drops the connection `id` and shuts its socket down (its sources
    /// then remove themselves); `reason` is logged with `err_log!`.
    fn _close(&mut self, id: u64, reason: &str) {
        let Some(conn) = self.conns.remove(&id) else { return };
        opusdm_ipc::err_log!(LOG_TAG, "ipc: connection {id} dropped: {reason}");
        _shutdown(&conn.stream);
        self._forget_hub(id);
    }

    /// Sends `event` to the hub. False (nothing sent) when there is no hub.
    pub fn send_to_hub(&mut self, event: &Value) -> bool {
        let Some(id) = self.hub.filter(|id| self.conns.get(id).is_some_and(|c| c.watch)) else { return false };
        self._send(id, event);
        true
    }

    /// The connection `id` sent `hotkeys/set`: a watcher becomes the hub
    /// now, any other connection becomes the candidate (the hub once it
    /// sends `watch`).
    fn _hotkeys_sent(&mut self, id: u64) {
        if self.conns.get(&id).is_some_and(|c| c.watch) {
            self._make_hub(id);
            return;
        }
        self.hub_candidate = Some(id);
        opusdm_ipc::debug_log!(LOG_TAG, "connection {id} sent hotkeys/set: the hub once it watches");
    }

    /// The connection `id` is the hub (replacing the previous one).
    fn _make_hub(&mut self, id: u64) {
        if self.hub_candidate == Some(id) { self.hub_candidate = None; }
        if self.hub == Some(id) { return; }
        match self.hub.replace(id) {
            Some(old) => opusdm_ipc::debug_log!(LOG_TAG, "hub connected on connection {id} (replaces connection {old})"),
            None => opusdm_ipc::debug_log!(LOG_TAG, "hub connected on connection {id}"),
        }
    }

    /// The connection `id` is gone. The hub: `err_log!("hub disconnected")`,
    /// the hotkey table stays and hub hotkeys log "no hub connected" until a
    /// new hub connects. The candidate: a one-shot `hotkeys/set` client,
    /// only traced.
    fn _forget_hub(&mut self, id: u64) {
        if self.hub_candidate == Some(id) {
            self.hub_candidate = None;
            opusdm_ipc::debug_log!(LOG_TAG, "connection {id} sent hotkeys/set and closed without watching: one-shot, the table stays");
        }
        if self.hub != Some(id) { return; }
        self.hub = None;
        opusdm_ipc::err_log!(LOG_TAG, "hub disconnected");
        opusdm_ipc::debug_log!(LOG_TAG, "hub connection {id} closed, the hotkey table stays");
    }

    /// Serializes `value` and queues it on the connection `id`.
    fn _send(&mut self, id: u64, value: &Value) {
        match serde_json::to_vec(value) {
            Ok(body) => self._enqueue(id, frame::encode(&body)),
            Err(e) => self._close(id, &format!("reply not serialized: {e}")),
        }
    }

    /// Queues `bytes` on the connection `id` and writes what the socket
    /// takes now; a connection over `IPC_MAX_PENDING` is dropped.
    fn _enqueue(&mut self, id: u64, bytes: Vec<u8>) {
        let Some(conn) = self.conns.get_mut(&id) else { return };
        if conn.pending.len().saturating_add(bytes.len()) > IPC_MAX_PENDING {
            self._close(id, &format!("more than {IPC_MAX_PENDING} bytes pending, the client does not read"));
            return;
        }
        conn.pending.extend_from_slice(&bytes);
        self._flush(id);
    }

    /// Writes the pending bytes of the connection `id` until the socket is
    /// full; with bytes left, makes sure a write source waits for it.
    fn _flush(&mut self, id: u64) {
        let Some(conn) = self.conns.get_mut(&id) else { return };
        let mut written = 0;
        let result = loop {
            let Some(rest) = conn.pending.get(written..).filter(|r| !r.is_empty()) else { break Ok(()) };
            match (&conn.stream).write(rest) {
                Ok(0) => break Err("the socket takes no more bytes".to_string()),
                Ok(n) => written += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => break Err(format!("write: {e}")),
            }
        };
        conn.pending.drain(..written.min(conn.pending.len()));
        if let Err(e) = result {
            self._close(id, &e);
            return;
        }
        if conn.pending.is_empty() || conn.writing { return; }
        if let Err(e) = self._watch_writable(id) {
            self._close(id, &e);
        }
    }

    /// Registers a write source for the connection `id`; it flushes the
    /// connection when the socket takes bytes and goes away once nothing is
    /// pending.
    fn _watch_writable(&mut self, id: u64) -> Result<(), String> {
        let handle = self.handle.clone().ok_or("the server is not bound")?;
        let conn = self.conns.get_mut(&id).ok_or("unknown connection")?;
        let watched = conn.stream.try_clone().map_err(|e| format!("dup stream: {e}"))?;
        handle
            .insert_source(Generic::new(watched, Interest::WRITE, Mode::Level), move |_, _, data: &mut LoopData| {
                Ok(data.state.ipc._on_writable(id))
            })
            .map_err(|e| format!("write source: {e}"))?;
        conn.writing = true;
        Ok(())
    }

    /// The socket of the connection `id` takes bytes: flushes it. The
    /// source goes away when the connection is gone or has nothing pending.
    fn _on_writable(&mut self, id: u64) -> PostAction {
        self._flush(id);
        let Some(conn) = self.conns.get_mut(&id) else { return PostAction::Remove };
        if !conn.pending.is_empty() { return PostAction::Continue; }
        conn.writing = false;
        PostAction::Remove
    }

    /// Reads what the connection `id` has (at most `READ_BUDGET`) into its
    /// buffer. `Ok(true)` when the peer closed the connection.
    fn _read(&mut self, id: u64) -> Result<bool, String> {
        let conn = self.conns.get_mut(&id).ok_or("unknown connection")?;
        let mut chunk = [0u8; READ_CHUNK];
        let mut total = 0;
        while total < READ_BUDGET {
            match (&conn.stream).read(&mut chunk) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    conn.read_buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    total += n;
                    // A partial message never grows past one header + IPC_MAX_MESSAGE.
                    if conn.read_buf.len() > IPC_MAX_MESSAGE + frame::HEADER_LEN { return Ok(false); }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("read: {e}")),
            }
        }
        Ok(false)
    }

    /// The next whole message of the connection `id`: `Ok(None)` when none
    /// is complete (or the connection is gone), `Err` for one over
    /// `IPC_MAX_MESSAGE`.
    fn _next_message(&mut self, id: u64) -> Result<Option<Vec<u8>>, String> {
        let Some(conn) = self.conns.get_mut(&id) else { return Ok(None) };
        frame::decode(&mut conn.read_buf, IPC_MAX_MESSAGE)
    }

    /// Makes the connection `id` a `watch` stream. The first watcher takes
    /// the current windows and desktops as its starting point (it lists
    /// them itself): it gets events for the changes from now on.
    fn _start_watch(&mut self, id: u64, windows: Vec<WindowView>, desktops: (u32, u32)) {
        if !self.has_watchers() {
            self.snapshot = Snapshot { windows, desktops };
        }
        let Some(conn) = self.conns.get_mut(&id) else { return };
        conn.watch = true;
        opusdm_ipc::debug_log!(LOG_TAG, "ipc: connection {id} is a watcher");
        if self.hub_candidate == Some(id) {
            self._make_hub(id);
        }
    }
}

impl Drop for IpcServer {
    /// Removes the socket file: the loop has ended.
    fn drop(&mut self) {
        let Some(path) = self.path.take() else { return };
        for conn in self.conns.values() {
            _shutdown(&conn.stream);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => opusdm_ipc::debug_log!(LOG_TAG, "ipc: socket {} removed", path.display()),
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "ipc: socket {} not removed: {e}", path.display()),
        }
    }
}

/// Shuts both directions of `stream` down; a failure (already closed by the
/// peer) is only traced.
fn _shutdown(stream: &UnixStream) {
    if let Err(e) = stream.shutdown(std::net::Shutdown::Both) {
        opusdm_ipc::debug_log!(LOG_TAG, "ipc: shutdown: {e}");
    }
}

/// The connection `id` is readable: reads it, runs each whole request and
/// queues its reply, then tells the watchers what changed. A peer that
/// closed, a read error, a message over `IPC_MAX_MESSAGE` or a body that is
/// not a request drops the connection. The source goes away with the
/// connection.
fn _on_readable(state: &mut State, id: u64) -> PostAction {
    if !state.ipc.conns.contains_key(&id) { return PostAction::Remove; }
    let closed = match state.ipc._read(id) {
        Ok(closed) => closed,
        Err(e) => {
            state.ipc._close(id, &e);
            return PostAction::Remove;
        }
    };
    loop {
        let body = match state.ipc._next_message(id) {
            Ok(Some(body)) => body,
            Ok(None) => break,
            Err(e) => {
                state.ipc._close(id, &format!("protocol error: {e}"));
                break;
            }
        };
        let request = match protocol::parse_request(&body) {
            Ok(request) => request,
            Err(e) => {
                state.ipc._close(id, &format!("protocol error: {e}"));
                break;
            }
        };
        let (reply, after) = protocol::handle(state, &request);
        state.ipc._send(id, &reply);
        match after {
            After::Watch => {
                let views = state.window_views();
                let desktops = (state.desktops.count(), state.desktops.current());
                state.ipc._start_watch(id, views, desktops);
            }
            After::Hub => state.ipc._hotkeys_sent(id),
            After::Nothing => {}
        }
    }
    state.notify_watchers();
    if closed {
        if let Some(conn) = state.ipc.conns.remove(&id) {
            opusdm_ipc::debug_log!(LOG_TAG, "ipc: connection {id} closed by the client");
            // Wakes a write source of this connection so it removes itself.
            _shutdown(&conn.stream);
            state.ipc._forget_hub(id);
        }
    }
    if state.ipc.conns.contains_key(&id) { PostAction::Continue } else { PostAction::Remove }
}

/// `window/kill`: SIGKILL to the process of the client of the window `id`
/// (its pid read from the socket credentials when the toplevel was
/// created, `WindowInfo::pid`). Refused for an unknown window, a window
/// without a pid, a pid `<= 1` or the compositor's own pid. Returns the
/// pid killed.
pub fn kill_window(state: &State, id: WindowId) -> Result<i32, String> {
    let (_, info) = state.window_by_id(id).ok_or_else(|| format!("kill: unknown window {}", id.0))?;
    let pid = info.pid.ok_or_else(|| format!("kill: window {} has no client pid", id.0))?;
    let pid = i32::try_from(pid).map_err(|_| format!("kill: pid {pid} of window {} is out of range", id.0))?;
    let pid = check_kill_pid(pid, std::process::id())?;
    // SAFETY: kill(2) on a pid checked by `check_kill_pid`, no memory involved.
    let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
    if rc != 0 {
        return Err(format!("kill: pid {pid} of window {}: {}", id.0, std::io::Error::last_os_error()));
    }
    opusdm_ipc::err_log!(LOG_TAG, "window/kill: SIGKILL sent to pid {pid} (window {})", id.0);
    Ok(pid)
}

/// `pid` when SIGKILL may be sent to it: never to a pid `<= 1` (init, or
/// a process group) nor to `own_pid`, the compositor itself.
pub fn check_kill_pid(pid: i32, own_pid: u32) -> Result<i32, String> {
    if pid <= 1 { return Err(format!("kill refused: pid {pid}")); }
    if u32::try_from(pid).is_ok_and(|p| p == own_pid) {
        return Err(format!("kill refused: pid {pid} is the compositor itself"));
    }
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_is_refused_for_init_groups_and_the_compositor() {
        assert!(check_kill_pid(0, 500).is_err());
        assert!(check_kill_pid(1, 500).is_err());
        assert!(check_kill_pid(-1, 500).is_err());
        assert!(check_kill_pid(-42, 500).is_err());
        assert!(check_kill_pid(500, 500).is_err());
        assert_eq!(check_kill_pid(501, 500), Ok(501));
    }

    #[test]
    fn the_socket_path_follows_the_wayland_display() {
        let path = socket_path(Path::new("/run/user/1000"), "wayland-2");
        assert_eq!(path, PathBuf::from("/run/user/1000/opusdm-compositor-wayland-2.sock"));
    }

    /// A server holding one connection `1` (a socket pair, the peer
    /// returned) with `pending` bytes already queued and a write source
    /// marked as registered.
    fn server_with_conn(pending: usize) -> (IpcServer, UnixStream) {
        let (ours, peer) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let mut server = IpcServer::default();
        let conn = Conn { stream: ours, read_buf: Vec::new(), pending: vec![0; pending], watch: true, writing: pending > 0 };
        server.conns.insert(1, conn);
        (server, peer)
    }

    #[test]
    fn a_connection_over_the_pending_limit_is_dropped() {
        let (mut server, mut peer) = server_with_conn(IPC_MAX_PENDING - 10);
        server._enqueue(1, vec![1; 11]);
        assert!(!server.conns.contains_key(&1), "the flooded connection is dropped");
        assert!(!server.has_watchers());
        peer.set_nonblocking(true).unwrap();
        let mut sink = Vec::new();
        assert!(peer.read_to_end(&mut sink).is_ok(), "the peer sees the connection closed");
    }

    #[test]
    fn a_small_reply_is_written_at_once() {
        let (mut server, mut peer) = server_with_conn(0);
        server._send(1, &serde_json::json!({"ok": true}));
        assert!(server.conns.get(&1).is_some_and(|c| c.pending.is_empty() && !c.writing));
        let mut buf = [0u8; 15];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"\x0b\x00\x00\x00{\"ok\":true}");
    }

    #[test]
    fn hotkey_events_go_to_the_hub_only_once_it_watches() {
        let (mut server, mut peer) = server_with_conn(0);
        server.conns.get_mut(&1).unwrap().watch = false;
        assert!(!server.send_to_hub(&serde_json::json!({"event": "hotkey"})), "no hub");
        server._hotkeys_sent(1);
        assert_eq!((server.hub, server.hub_candidate), (None, Some(1)));
        assert!(!server.send_to_hub(&serde_json::json!({"event": "hotkey"})), "a candidate is not the hub");
        server._start_watch(1, Vec::new(), (4, 0));
        assert_eq!((server.hub, server.hub_candidate), (Some(1), None));
        assert!(server.send_to_hub(&serde_json::json!({"event": "hotkey"})));
        let mut buf = [0u8; 4];
        peer.read_exact(&mut buf).unwrap();
        server._close(1, "test");
        assert_eq!(server.hub, None);
        assert!(!server.send_to_hub(&serde_json::json!({"event": "hotkey"})));
    }

    #[test]
    fn a_one_shot_hotkeys_set_keeps_the_watching_hub() {
        let (mut server, _peer) = server_with_conn(0);
        server._hotkeys_sent(1);
        assert_eq!(server.hub, Some(1), "a watcher that sends hotkeys/set is the hub at once");
        let (ours, _peer2) = UnixStream::pair().unwrap();
        server.conns.insert(2, Conn { stream: ours, read_buf: Vec::new(), pending: Vec::new(), watch: false, writing: false });
        server._hotkeys_sent(2);
        assert_eq!((server.hub, server.hub_candidate), (Some(1), Some(2)));
        server._close(2, "one-shot done");
        assert_eq!((server.hub, server.hub_candidate), (Some(1), None));
    }

    #[test]
    fn an_unbound_server_has_no_watchers_and_removes_nothing() {
        let mut server = IpcServer::default();
        assert!(!server.has_watchers());
        server.broadcast(&serde_json::json!({"event": "x"}));
        assert!(server.path().is_none());
    }
}
