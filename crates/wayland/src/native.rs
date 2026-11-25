//! Client of the private IPC of opusdm-compositor: a Unix socket whose
//! path the compositor exports to its children as `OPUSDM_COMPOSITOR_SOCKET`.
//! It lists the windows and the desktops, sends the window and desktop
//! requests, follows the changes ([`watch`]) and receives the global hotkeys
//! of the hub ([`watch_hotkeys`]).
//!
//! Wire format (the Wayfire framing, see `frame.rs`): a 4 byte little-endian
//! length, then JSON. A request is `{"method": ..., "data": {...}}`; the
//! reply is `{"ok": true, ...}` or `{"error": "..."}`. After `watch` the
//! connection also receives events, objects with an `"event"` key; replies
//! never have one, so the reader routes on that key.
//!
//! Nothing here is used unless [`under_native`] is true: the labwc, Wayfire
//! and X11 sessions never reach this module.

use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};

use crate::frame::{self, send};
pub use crate::wayfire::Rect;

/// Environment variable opusdm-compositor sets for its children: the path
/// of its IPC socket.
pub const ENV_SOCKET: &str = "OPUSDM_COMPOSITOR_SOCKET";

/// Wait between two attempts to reach the compositor again after the hotkey
/// connection was lost: the loop never spins.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Largest message accepted from the compositor: what it can queue for one
/// connection (`IPC_MAX_PENDING` of the server), a guard against garbage.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// Longest wait for one request to be written or answered: a stalled
/// compositor must not hang the caller forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Tag of the log lines of this crate.
const LOG_TAG: &str = "opusdm-wayland";

/// A window, as `windows/list` and the window events report it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeWindow {
    /// Stable id of the window for the life of the compositor.
    pub id: u32,
    /// `xdg_toplevel` app id, empty when unset.
    pub app_id: String,
    /// `xdg_toplevel` title, empty when unset.
    pub title: String,
    /// Pid of the client, when the compositor knows it.
    pub pid: Option<u32>,
    /// Position and size of the window geometry.
    pub geometry: Rect,
    /// 0-based desktop of the window.
    pub desktop: u32,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    /// The window has the keyboard focus.
    pub activated: bool,
}

/// Windows and desktops at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeState {
    /// Every window, in stacking order (bottom first).
    pub windows: Vec<NativeWindow>,
    /// Number of desktops.
    pub count: u32,
    /// 0-based desktop shown.
    pub current: u32,
}

impl NativeState {
    /// Read windows and desktops from the running compositor, on a new
    /// connection.
    pub fn read() -> Result<NativeState, String> {
        Client::connect()?.state()
    }

    /// Read windows and desktops from the compositor at `path`.
    fn _read_at(path: &Path) -> Result<NativeState, String> {
        Client::_connect_at(path)?.state()
    }

    /// Build a state from the replies of `windows/list` and `desktops/list`.
    /// A window entry without an id is left out, with a log line.
    fn _from_json(windows: &Value, desktops: &Value) -> Result<NativeState, String> {
        let list = windows.get("windows").and_then(Value::as_array).ok_or("windows/list: no 'windows' array")?;
        let count = _u32(desktops, "count").ok_or("desktops/list: no 'count'")?;
        let current = _u32(desktops, "current").ok_or("desktops/list: no 'current'")?;
        Ok(NativeState { windows: list.iter().filter_map(_parse_window).collect(), count, current })
    }

    /// The window that has the keyboard focus.
    pub fn active(&self) -> Option<&NativeWindow> {
        self.windows.iter().find(|w| w.activated)
    }
}

/// The unsigned 32-bit integer `key` of `v`.
fn _u32(v: &Value, key: &str) -> Option<u32> {
    v.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())
}

/// A `{x, y, width, height}` object.
fn _parse_rect(v: &Value) -> Rect {
    let n = |k: &str| v.get(k).and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()).unwrap_or(0);
    Rect { x: n("x"), y: n("y"), w: n("width"), h: n("height") }
}

/// One window object; `None` (logged) when it has no id.
fn _parse_window(v: &Value) -> Option<NativeWindow> {
    let Some(id) = _u32(v, "id") else {
        opusdm_ipc::err_log!(LOG_TAG, "compositor window without an id left out: {v}");
        return None;
    };
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(NativeWindow {
        id,
        app_id: text("app_id"),
        title: text("title"),
        pid: _u32(v, "pid"),
        geometry: v.get("geometry").map(_parse_rect).unwrap_or_default(),
        desktop: _u32(v, "desktop").unwrap_or(0),
        minimized: flag("minimized"),
        maximized: flag("maximized"),
        fullscreen: flag("fullscreen"),
        activated: flag("activated"),
    })
}

/// The socket named by `value` (the content of `ENV_SOCKET`), when it is
/// set, not empty and the file exists.
fn _socket_from(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| p.exists())
}

/// The compositor's IPC socket, when this process runs under
/// opusdm-compositor.
pub fn socket_path() -> Option<PathBuf> {
    _socket_from(std::env::var_os(ENV_SOCKET))
}

/// Whether this process runs under opusdm-compositor with its IPC socket.
pub fn under_native() -> bool {
    socket_path().is_some()
}

/// The socket path, or the reason there is none.
fn _require_socket() -> Result<PathBuf, String> {
    socket_path().ok_or_else(|| format!("{ENV_SOCKET} is not set or the socket is missing"))
}

/// Open a new connection to the socket at `path`, with no timeout.
fn _open(path: &Path) -> Result<UnixStream, String> {
    UnixStream::connect(path).map_err(|e| format!("cannot connect to {}: {e}", path.display()))
}

/// Read one message from `stream`.
fn _receive(stream: &mut UnixStream) -> Result<Value, String> {
    frame::receive(stream, MAX_MESSAGE)
}

/// The reply as a result: `Err` with the compositor's message when it has
/// `error`.
fn _checked(method: &str, reply: Value) -> Result<Value, String> {
    match reply.get("error").and_then(Value::as_str) {
        Some(e) => Err(format!("{method}: {e}")),
        None => Ok(reply),
    }
}

/// Bound every read and write on `stream` by `timeout` (`None`: no bound).
fn _set_timeouts(stream: &UnixStream, timeout: Option<Duration>) -> Result<(), String> {
    stream.set_read_timeout(timeout).map_err(|e| format!("cannot set the read timeout: {e}"))?;
    stream.set_write_timeout(timeout).map_err(|e| format!("cannot set the write timeout: {e}"))
}

/// One request connection: any number of requests, one after the other,
/// each bounded by a timeout.
pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to the running compositor.
    pub fn connect() -> Result<Client, String> {
        Client::_connect_at(&_require_socket()?)
    }

    /// Connect to the compositor at `path`.
    fn _connect_at(path: &Path) -> Result<Client, String> {
        let stream = _open(path)?;
        _set_timeouts(&stream, Some(REQUEST_TIMEOUT))?;
        Ok(Client { stream })
    }

    /// Send request `method` with `data` and return the reply.
    pub fn request(&mut self, method: &str, data: Value) -> Result<Value, String> {
        send(&mut self.stream, method, data)?;
        let reply = _receive(&mut self.stream).map_err(|e| format!("{method}: {e}"))?;
        _checked(method, reply)
    }

    /// Read windows and desktops.
    pub fn state(&mut self) -> Result<NativeState, String> {
        let windows = self.request("windows/list", json!({}))?;
        let desktops = self.request("desktops/list", json!({}))?;
        NativeState::_from_json(&windows, &desktops)
    }
}

/// Send one request on a new connection and return the reply.
pub fn call(method: &str, data: Value) -> Result<Value, String> {
    Client::connect()?.request(method, data)
}

/// Raise and focus window `id`.
pub fn activate(id: u32) -> Result<(), String> {
    call("window/activate", json!({ "id": id })).map(drop)
}

/// Minimize (or restore) window `id`.
pub fn set_minimized(id: u32, minimized: bool) -> Result<(), String> {
    call("window/minimize", json!({ "id": id, "state": minimized })).map(drop)
}

/// Ask window `id` to close, like its close button.
pub fn close(id: u32) -> Result<(), String> {
    call("window/close", json!({ "id": id })).map(drop)
}

/// Kill the process of window `id`.
pub fn kill(id: u32) -> Result<(), String> {
    call("window/kill", json!({ "id": id })).map(drop)
}

/// Move window `id` to desktop `desktop`; the desktop shown does not change.
pub fn move_to_desktop(id: u32, desktop: u32) -> Result<(), String> {
    call("window/move-to-desktop", json!({ "id": id, "desktop": desktop })).map(drop)
}

/// Show desktop `index`.
pub fn switch_desktop(index: u32) -> Result<(), String> {
    call("desktops/switch", json!({ "index": index })).map(drop)
}

/// Set the number of desktops; returns the count the compositor kept (it
/// clamps it to its range).
pub fn set_desktop_count(count: u32) -> Result<u32, String> {
    let reply = call("desktops/set-count", json!({ "count": count }))?;
    _u32(&reply, "count").ok_or_else(|| format!("desktops/set-count: no 'count' in {reply}"))
}

/// Call `on_change` with the current state, then again after every burst of
/// events the compositor sends, from threads of their own. Returns once they
/// run; they stop (with a log line) when the compositor goes away.
///
/// Ownership: both threads own everything they use (the event stream, the
/// channel ends, the request connection, `on_change`) and are detached; they
/// live until the event stream ends (compositor gone: one `err_log!` line
/// each way) or the process exits.
///
/// One thread only reads the event stream; the other waits for it, drops
/// every event already queued (a window drag sends dozens), and re-reads the
/// state once on a request connection it keeps open.
pub fn watch(on_change: impl FnMut(&NativeState) + Send + 'static) -> Result<(), String> {
    _watch_at(_require_socket()?, on_change)
}

/// [`watch`] on the compositor at `path`.
fn _watch_at(path: PathBuf, mut on_change: impl FnMut(&NativeState) + Send + 'static) -> Result<(), String> {
    let mut stream = _open(&path)?;
    _set_timeouts(&stream, Some(REQUEST_TIMEOUT))?;
    send(&mut stream, "watch", json!({}))?;
    _checked("watch", _receive(&mut stream)?)?;
    _set_timeouts(&stream, None)?;
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("opusdm-native-events".into())
        .spawn(move || loop {
            let message = match _receive(&mut stream) {
                Ok(m) => m,
                Err(e) => {
                    opusdm_ipc::err_log!(LOG_TAG, "compositor event watch stopped: {e}");
                    return;
                }
            };
            if message.get("event").is_none() {
                opusdm_ipc::err_log!(LOG_TAG, "compositor watch: unexpected reply ignored: {message}");
                continue;
            }
            if tx.send(()).is_err() {
                opusdm_ipc::err_log!(LOG_TAG, "compositor event watch stopped: the state thread is gone");
                return;
            }
        })
        .map_err(|e| format!("cannot start the compositor event thread: {e}"))?;
    std::thread::Builder::new()
        .name("opusdm-native".into())
        .spawn(move || {
            let mut client: Option<Client> = None;
            let mut last: Option<NativeState> = None;
            loop {
                match _read_state(&path, &mut client) {
                    Ok(state) if last.as_ref() != Some(&state) => {
                        on_change(&state);
                        last = Some(state);
                    }
                    Ok(_) => {}
                    Err(e) => opusdm_ipc::err_log!(LOG_TAG, "compositor state not read: {e}"),
                }
                if !_wait_and_drain(&rx) {
                    opusdm_ipc::err_log!(LOG_TAG, "compositor state thread stopped: the event watch ended");
                    return;
                }
            }
        })
        .map_err(|e| format!("cannot start the compositor state thread: {e}"))?;
    Ok(())
}

/// Read the state on `client`, connecting to `path` first when there is
/// none. A failed read drops the connection, so the next one starts afresh.
fn _read_state(path: &Path, client: &mut Option<Client>) -> Result<NativeState, String> {
    if client.is_none() { *client = Some(Client::_connect_at(path)?); }
    let Some(c) = client.as_mut() else { return Err("no compositor connection".into()) };
    let state = c.state();
    if state.is_err() { *client = None; }
    state
}

/// Block until at least one event arrived on `rx`, then drop all the others
/// already queued. `false` once the event thread has stopped.
fn _wait_and_drain(rx: &mpsc::Receiver<()>) -> bool {
    if rx.recv().is_err() { return false; }
    while rx.try_recv().is_ok() {}
    true
}

/// What the hotkey thread and its [`HotkeyWatch`] handles share.
struct HotkeyShared {
    /// The global hotkey table, as (key, function) pairs: sent on every
    /// (re)connection.
    table: Vec<(String, String)>,
    /// A second handle on the live hub connection, for [`HotkeyWatch::set`];
    /// `None` while there is none.
    writer: Option<UnixStream>,
}

/// Lock `shared`, going on after a panic of another holder (the data is a
/// table and a socket: always consistent).
fn _lock(shared: &Mutex<HotkeyShared>) -> MutexGuard<'_, HotkeyShared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handle on the thread of [`watch_hotkeys`], to change its table. Dropping
/// it does not stop the thread.
#[derive(Clone)]
pub struct HotkeyWatch {
    shared: Arc<Mutex<HotkeyShared>>,
}

impl HotkeyWatch {
    /// Replace the hotkey table: kept for every reconnection and sent now
    /// (`hotkeys/set`) on the live connection, if any. The reply comes on
    /// the event stream; its errors are logged by the hotkey thread. `Err`
    /// when the write failed: the thread then reconnects and sends this
    /// table.
    pub fn set(&self, table: Vec<(String, String)>) -> Result<(), String> {
        let mut shared = _lock(&self.shared);
        shared.table = table;
        let data = _hotkeys_data(&shared.table);
        let Some(writer) = shared.writer.as_mut() else { return Ok(()) };
        send(writer, "hotkeys/set", data).map_err(|e| format!("{e} (sent again on reconnection)"))
    }
}

/// The `data` of a `hotkeys/set` request for `table`.
fn _hotkeys_data(table: &[(String, String)]) -> Value {
    let list: Vec<Value> = table.iter().map(|(key, function)| json!({ "key": key, "function": function })).collect();
    json!({ "hotkeys": list })
}

/// Make this process the hub of the compositor: send `table` (key,
/// function) with `hotkeys/set`, then `watch` on the same connection, and
/// call `on_hit(function)` for every `hotkey` event, from a thread of its
/// own. When the connection is lost it logs, waits [`RECONNECT_DELAY`] and
/// connects again, sending the table again; it never stops. `Err` only when
/// this process does not run under the compositor or the thread cannot
/// start. The returned [`HotkeyWatch`] changes the table later.
///
/// Ownership: the thread is detached and owns `on_hit`, its connection and
/// one `Arc` of the shared table; it runs for the whole life of the
/// process. When the compositor goes away every attempt logs a line and the
/// next one comes `RECONNECT_DELAY` later.
pub fn watch_hotkeys(table: Vec<(String, String)>, on_hit: impl Fn(String) + Send + 'static) -> Result<HotkeyWatch, String> {
    _watch_hotkeys_at(_require_socket()?, table, on_hit, RECONNECT_DELAY)
}

/// [`watch_hotkeys`] on the compositor at `path`, waiting `delay` between
/// two attempts.
fn _watch_hotkeys_at(
    path: PathBuf,
    table: Vec<(String, String)>,
    on_hit: impl Fn(String) + Send + 'static,
    delay: Duration,
) -> Result<HotkeyWatch, String> {
    let shared = Arc::new(Mutex::new(HotkeyShared { table, writer: None }));
    let ours = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("opusdm-native-hotkeys".into())
        .spawn(move || loop {
            let e = _hotkey_session(&path, &ours, &on_hit);
            _lock(&ours).writer = None;
            opusdm_ipc::err_log!(LOG_TAG, "compositor IPC lost: {e}, retrying");
            std::thread::sleep(delay);
        })
        .map_err(|e| format!("cannot start the compositor hotkey thread: {e}"))?;
    Ok(HotkeyWatch { shared })
}

/// One hub connection: connect, send the table and `watch`, then report the
/// hotkey events until the connection fails. Returns why it ended.
fn _hotkey_session(path: &Path, shared: &Mutex<HotkeyShared>, on_hit: &impl Fn(String)) -> String {
    let mut stream = match _hub_handshake(path, shared, on_hit) {
        Ok(s) => s,
        Err(e) => return e,
    };
    loop {
        let message = match _receive(&mut stream) {
            Ok(m) => m,
            Err(e) => return e,
        };
        if let Some(reply) = _dispatch(message, on_hit) {
            _log_hotkeys_reply(reply);
        }
    }
}

/// Connect to `path`, send `hotkeys/set` with the shared table and `watch`,
/// and publish a writer for [`HotkeyWatch::set`]. The lock is held from the
/// table read to the publication, so a new table is never lost nor mixed
/// with the replies of the handshake. Returns the stream, with no read
/// timeout (events may stay away for hours).
fn _hub_handshake(path: &Path, shared: &Mutex<HotkeyShared>, on_hit: &impl Fn(String)) -> Result<UnixStream, String> {
    let mut stream = _open(path)?;
    _set_timeouts(&stream, Some(REQUEST_TIMEOUT))?;
    let mut shared = _lock(shared);
    send(&mut stream, "hotkeys/set", _hotkeys_data(&shared.table))?;
    _log_hotkeys_reply(_await_reply(&mut stream, on_hit).map_err(|e| format!("hotkeys/set: {e}"))?);
    send(&mut stream, "watch", json!({}))?;
    _checked("watch", _await_reply(&mut stream, on_hit).map_err(|e| format!("watch: {e}"))?)?;
    stream.set_read_timeout(None).map_err(|e| format!("cannot clear the read timeout: {e}"))?;
    shared.writer = Some(stream.try_clone().map_err(|e| format!("cannot duplicate the hub connection: {e}"))?);
    Ok(stream)
}

/// Read messages until one is a reply (no `event` key), reporting the
/// events met on the way.
fn _await_reply(stream: &mut UnixStream, on_hit: &impl Fn(String)) -> Result<Value, String> {
    loop {
        if let Some(reply) = _dispatch(_receive(stream)?, on_hit) { return Ok(reply); }
    }
}

/// Handle one message of the hub connection: a `hotkey` event calls
/// `on_hit`, the other events are not for the hub; a reply is returned.
fn _dispatch(message: Value, on_hit: &impl Fn(String)) -> Option<Value> {
    let Some(event) = message.get("event") else { return Some(message) };
    if event.as_str() != Some("hotkey") { return None; }
    match message.get("function").and_then(Value::as_str) {
        Some(function) => on_hit(function.to_string()),
        None => opusdm_ipc::err_log!(LOG_TAG, "compositor hotkey event without a function: {message}"),
    }
    None
}

/// Log the failures a `hotkeys/set` reply reports: a refusal (`error`) or
/// the keys it could not bind (`errors`).
fn _log_hotkeys_reply(reply: Value) {
    if let Some(e) = reply.get("error").and_then(Value::as_str) {
        opusdm_ipc::err_log!(LOG_TAG, "compositor refused the hotkeys: {e}");
        return;
    }
    let errors = reply.get("errors").and_then(Value::as_array).cloned().unwrap_or_default();
    for e in errors {
        opusdm_ipc::err_log!(LOG_TAG, "compositor hotkey not bound: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{frame, receive};
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// A directory removed when dropped, for the socket of a fake server.
    struct TempDir(PathBuf);

    impl TempDir {
        /// A new empty directory under the system temp dir.
        fn new() -> TempDir {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("odm-native-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        /// Path of the socket inside it.
        fn socket(&self) -> PathBuf {
            self.0.join("c.sock")
        }
    }

    impl Drop for TempDir {
        /// Remove the directory and its socket.
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Longest wait of the fake server for a client or a message.
    const TEST_WAIT: Duration = Duration::from_secs(5);

    /// Accept one connection on `listener`, panicking after `TEST_WAIT`
    /// (a client that never comes fails the test instead of hanging it).
    fn accept(listener: &UnixListener) -> UnixStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + TEST_WAIT;
        loop {
            match listener.accept() {
                Ok((s, _)) => {
                    s.set_nonblocking(false).unwrap();
                    s.set_read_timeout(Some(TEST_WAIT)).unwrap();
                    return s;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("no client: {e}"),
            }
        }
    }

    /// Write `value` as one framed message on `stream`.
    fn reply(stream: &mut UnixStream, value: Value) {
        stream.write_all(&frame(value.to_string().as_bytes())).unwrap();
    }

    /// A `windows/list` entry.
    fn window(id: u32, activated: bool) -> Value {
        json!({ "id": id, "app_id": "foot", "title": "t", "pid": if id == 1 { json!(42) } else { Value::Null },
                "geometry": { "x": 10, "y": 20, "width": 300, "height": 200 }, "desktop": 1,
                "minimized": false, "maximized": true, "fullscreen": false, "activated": activated })
    }

    #[test]
    fn state_is_read_from_windows_and_desktops() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let server = std::thread::spawn(move || {
            let mut s = accept(&listener);
            let req = receive(&mut s, 1 << 20).unwrap();
            assert_eq!(req["method"], "windows/list");
            reply(&mut s, json!({ "ok": true, "windows": [window(1, false), window(2, true)] }));
            let req = receive(&mut s, 1 << 20).unwrap();
            assert_eq!(req["method"], "desktops/list");
            reply(&mut s, json!({ "ok": true, "count": 4, "current": 2 }));
        });
        let st = NativeState::_read_at(&dir.socket()).unwrap();
        server.join().unwrap();
        assert_eq!((st.count, st.current, st.windows.len()), (4, 2, 2));
        let w = &st.windows[0];
        assert_eq!((w.id, w.app_id.as_str(), w.pid, w.desktop, w.maximized), (1, "foot", Some(42), 1, true));
        assert_eq!(w.geometry, Rect { x: 10, y: 20, w: 300, h: 200 });
        assert_eq!(st.windows[1].pid, None);
        assert_eq!(st.active().map(|w| w.id), Some(2));
    }

    #[test]
    fn an_error_reply_is_an_error() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let server = std::thread::spawn(move || {
            let mut s = accept(&listener);
            receive(&mut s, 1 << 20).unwrap();
            reply(&mut s, json!({ "error": "boom" }));
        });
        let err = NativeState::_read_at(&dir.socket()).unwrap_err();
        assert!(err.contains("boom"), "{err}");
        server.join().unwrap();
    }

    /// Accept one connection on `listener`, check it sends `hotkeys/set`
    /// with `table` and then `watch`, answer both; return the stream.
    fn accept_hub(listener: &UnixListener, table: &Value) -> UnixStream {
        let mut s = accept(listener);
        let req = receive(&mut s, 1 << 20).unwrap();
        assert_eq!(req["method"], "hotkeys/set");
        assert_eq!(&req["data"]["hotkeys"], table);
        reply(&mut s, json!({ "ok": true, "errors": [] }));
        let req = receive(&mut s, 1 << 20).unwrap();
        assert_eq!(req["method"], "watch");
        reply(&mut s, json!({ "ok": true }));
        s
    }

    #[test]
    fn hotkeys_are_set_then_watched_and_hits_are_reported_after_a_reconnect() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let table = vec![("Ctrl+backslash".to_string(), "terminal".to_string())];
        let wire = json!([{ "key": "Ctrl+backslash", "function": "terminal" }]);
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let _watch = _watch_hotkeys_at(dir.socket(), table, move |f| tx.lock().unwrap().send(f).unwrap(), Duration::from_millis(50)).unwrap();

        let mut s = accept_hub(&listener, &wire);
        reply(&mut s, json!({ "event": "window-added", "window": window(1, true) }));
        reply(&mut s, json!({ "event": "hotkey", "function": "terminal" }));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "terminal");
        drop(s);

        // The compositor went away and came back: the table is sent again.
        let mut s = accept_hub(&listener, &wire);
        reply(&mut s, json!({ "event": "hotkey", "function": "run" }));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "run");
    }

    #[test]
    fn a_new_table_is_sent_on_the_live_connection_and_on_reconnect() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let first = vec![("Ctrl+backslash".to_string(), "terminal".to_string())];
        let watch = _watch_hotkeys_at(dir.socket(), first, |_| {}, Duration::from_millis(50)).unwrap();
        let mut s = accept_hub(&listener, &json!([{ "key": "Ctrl+backslash", "function": "terminal" }]));

        watch.set(vec![("Alt+F2".to_string(), "run".to_string())]).unwrap();
        let req = receive(&mut s, 1 << 20).unwrap();
        assert_eq!(req["method"], "hotkeys/set");
        assert_eq!(req["data"]["hotkeys"], json!([{ "key": "Alt+F2", "function": "run" }]));
        reply(&mut s, json!({ "ok": true, "errors": ["Alt+F2: taken"] }));
        drop(s);

        accept_hub(&listener, &json!([{ "key": "Alt+F2", "function": "run" }]));
    }

    /// Answer one `windows/list` and one `desktops/list` on `s`.
    fn serve_state(s: &mut UnixStream, windows: Value, current: u32) {
        assert_eq!(receive(s, 1 << 20).unwrap()["method"], "windows/list");
        reply(s, json!({ "ok": true, "windows": windows }));
        assert_eq!(receive(s, 1 << 20).unwrap()["method"], "desktops/list");
        reply(s, json!({ "ok": true, "count": 4, "current": current }));
    }

    #[test]
    fn watch_reports_the_state_then_every_change() {
        let dir = TempDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let (tx, rx) = mpsc::channel();
        let path = dir.socket();
        let watcher = std::thread::spawn(move || {
            _watch_at(path, move |st: &NativeState| tx.send(st.clone()).unwrap()).unwrap();
        });
        let mut events = accept(&listener);
        assert_eq!(receive(&mut events, 1 << 20).unwrap()["method"], "watch");
        reply(&mut events, json!({ "ok": true }));
        watcher.join().unwrap();

        let mut requests = accept(&listener);
        serve_state(&mut requests, json!([window(1, true)]), 0);
        let st = rx.recv_timeout(TEST_WAIT).unwrap();
        assert_eq!((st.windows.len(), st.current), (1, 0));

        reply(&mut events, json!({ "event": "desktops-changed", "count": 4, "current": 3 }));
        serve_state(&mut requests, json!([window(1, true)]), 3);
        assert_eq!(rx.recv_timeout(TEST_WAIT).unwrap().current, 3);
    }

    #[test]
    fn the_socket_must_be_named_and_exist() {
        let dir = TempDir::new();
        assert_eq!(_socket_from(None), None);
        assert_eq!(_socket_from(Some("".into())), None);
        assert_eq!(_socket_from(Some(dir.socket().into_os_string())), None);
        let _listener = UnixListener::bind(dir.socket()).unwrap();
        assert_eq!(_socket_from(Some(dir.socket().into_os_string())), Some(dir.socket()));
    }
}
