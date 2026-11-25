//! Integration tests of the private IPC on the headless backend.

mod common;

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use common::ipc::{ipc_request, ipc_requests_then_half_close, ipc_send_raw, ipc_watch};
use common::{Compositor, HasSurface, TestClient};

/// Environment variable that turns `kill_helper_client` into a real client.
const HELPER_ENV: &str = "OPUSDM_COMPOSITOR_TEST_HELPER_SOCKET";

/// Line the helper prints once its window is mapped.
const HELPER_READY: &str = "HELPER_MAPPED";

/// Id of the only window in `windows/list`.
fn only_window_id(comp: &Compositor) -> u64 {
    let list = ipc_request(comp, "windows/list", json!({}));
    assert_eq!(list["windows"].as_array().map(Vec::len), Some(1), "one window expected: {list}");
    list["windows"][0]["id"].as_u64().expect("window id")
}

#[test]
fn the_ipc_lists_windows_and_minimizes_one() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let list = ipc_request(&comp, "windows/list", json!({}));
    let id = list["windows"][0]["id"].as_u64().unwrap();
    assert_eq!(list["windows"][0]["desktop"], 0);
    ipc_request(&comp, "window/minimize", json!({"id": id, "state": true}));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["windows"][0]["minimized"], true);
}

#[test]
fn a_window_moved_to_another_desktop_comes_back_when_activated() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 2}));
    ipc_request(&comp, "window/activate", json!({"id": id}));
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["current"], 2);
}

#[test]
fn shrinking_the_desktops_keeps_every_window() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = ipc_request(&comp, "windows/list", json!({}))["windows"][0]["id"].as_u64().unwrap();
    ipc_request(&comp, "window/move-to-desktop", json!({"id": id, "desktop": 3}));
    assert_eq!(ipc_request(&comp, "desktops/set-count", json!({"count": 2}))["count"], 2);
    assert_eq!(ipc_request(&comp, "windows/list", json!({}))["windows"][0]["desktop"], 1);
}

#[test]
fn a_watcher_sees_windows_come_and_go() {
    let comp = Compositor::start();
    let mut watch = ipc_watch(&comp);
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert_eq!(watch.next_event(Duration::from_secs(2)).unwrap()["event"], "window-added");
    drop(w);
    c.roundtrip();
    // window-changed events may come first; a window-removed must follow
    assert!(watch.wait_for("window-removed", Duration::from_secs(2)));
}

#[test]
fn a_connection_that_sends_garbage_is_dropped_and_the_server_goes_on() {
    let comp = Compositor::start();
    assert!(ipc_send_raw(&comp, &(u32::MAX).to_le_bytes()), "a length over IPC_MAX_MESSAGE closes the connection");
    assert!(ipc_send_raw(&comp, b"\x05\x00\x00\x00hello"), "a body that is not JSON closes the connection");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

#[test]
fn session_quit_ends_the_compositor_cleanly() {
    let mut comp = Compositor::start();
    assert!(comp.ipc_socket.exists());
    assert_eq!(ipc_request(&comp, "session/quit", json!({}))["ok"], true);
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
    assert!(!comp.ipc_socket.exists(), "the IPC socket file must go with the loop");
}

#[test]
fn sigterm_removes_the_ipc_socket() {
    let mut comp = Compositor::start();
    comp.signal(libc::SIGTERM);
    assert_eq!(comp.wait_exit(Duration::from_secs(2)), Some(0));
    assert!(!comp.ipc_socket.exists(), "the IPC socket file must go with the loop");
}

#[test]
fn a_window_on_another_desktop_is_hidden_and_comes_back_on_switch() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let a = c.map_toplevel((400, 300));
    let b = c.map_toplevel((300, 200));
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)));
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 2);
    // Stack order: the last mapped window is the last one.
    let a_id = windows[0]["id"].as_u64().unwrap();
    let b_id = windows[1]["id"].as_u64().unwrap();
    assert_eq!(windows[1]["activated"], true);
    assert_eq!(windows[0]["activated"], false);

    assert_eq!(ipc_request(&comp, "window/move-to-desktop", json!({"id": b_id, "desktop": 1}))["ok"], true);
    assert!(c.wait_keyboard_focus(&a, Duration::from_secs(2)), "the shown window gets the keyboard");
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    // Focusing `a` raised it: it is the last one of the stack now.
    assert_eq!(windows[1]["id"].as_u64(), Some(a_id));
    assert_eq!(windows[1]["activated"], true);
    let b_view = windows.iter().find(|w| w["id"] == b_id).expect("b is still listed");
    assert_eq!((b_view["desktop"].clone(), b_view["activated"].clone()), (json!(1), json!(false)));

    assert_eq!(ipc_request(&comp, "desktops/switch", json!({"index": 1}))["ok"], true);
    assert!(c.wait_keyboard_focus(&b, Duration::from_secs(2)), "the window of desktop 1 comes back with the keyboard");
    assert!(c.wait_frame(&b, Duration::from_secs(2)));

    assert_eq!(ipc_request(&comp, "desktops/switch", json!({"index": 0}))["ok"], true);
    assert!(c.wait_keyboard_focus(&a, Duration::from_secs(2)));
    assert_eq!(c.keyboard_focus(), Some(a.wl_surface().clone()));
}

#[test]
fn a_watcher_sees_minimize_and_desktop_changes() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let id = only_window_id(&comp);
    let mut watch = ipc_watch(&comp);
    ipc_request(&comp, "window/minimize", json!({"id": id, "state": true}));
    let changed = watch.wait_for_event("window-changed", Duration::from_secs(2)).expect("window-changed");
    assert_eq!(changed["window"]["id"], id);
    assert_eq!(changed["window"]["minimized"], true);
    ipc_request(&comp, "desktops/switch", json!({"index": 3}));
    let desktops = watch.wait_for_event("desktops-changed", Duration::from_secs(2)).expect("desktops-changed");
    assert_eq!((desktops["count"].clone(), desktops["current"].clone()), (json!(4), json!(3)));
}

#[test]
fn the_window_view_has_the_documented_fields() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let _w = c.map_toplevel((400, 300));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert_eq!(list["ok"], true);
    let w = &list["windows"][0];
    assert_eq!(w["title"], "opusdm-compositor test");
    assert_eq!(w["app_id"], "");
    assert_eq!(w["pid"], u64::from(std::process::id()), "the test client is this process");
    assert_eq!((w["geometry"]["width"].clone(), w["geometry"]["height"].clone()), (json!(400), json!(300)));
    assert!(w["geometry"]["x"].is_i64() && w["geometry"]["y"].is_i64());
    for flag in ["minimized", "maximized", "fullscreen"] {
        assert_eq!(w[flag], false, "{flag}");
    }
}

#[test]
fn refusals_are_error_replies() {
    let comp = Compositor::start();
    let cases = [
        ("no/such-method", json!({})),
        ("window/activate", json!({"id": 99})),
        ("window/activate", json!({})),
        ("window/minimize", json!({"id": 1})),
        ("window/close", json!({"id": "x"})),
        ("window/kill", json!({"id": 99})),
        ("window/move-to-desktop", json!({"id": 99, "desktop": 0})),
        ("desktops/switch", json!({"index": 4})),
        ("desktops/set-count", json!({})),
        ("debug/key", json!({"key": "Ctrl+Bogus+u"})),
        ("debug/key", json!({"key": "Ctrl+NoSuchKeysym"})),
    ];
    for (method, data) in cases {
        let reply = ipc_request(&comp, method, data.clone());
        assert!(reply["error"].is_string(), "{method} {data} must be refused: {reply}");
        assert!(reply.get("ok").is_none(), "{method} {data}: {reply}");
    }
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}

#[test]
fn window_close_asks_the_client_to_close() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    let id = only_window_id(&comp);
    assert_eq!(ipc_request(&comp, "window/close", json!({"id": id}))["ok"], true);
    assert!(c.wait_close_request(&w, Duration::from_secs(2)), "the client got xdg_toplevel.close");
}

#[test]
fn closed_connections_give_back_their_fds_and_the_server_keeps_answering() {
    let comp = Compositor::start();
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    // Let the sources of that first connection go (a few loop iterations).
    std::thread::sleep(Duration::from_millis(200));
    let baseline = comp.open_fds();
    // More than IPC_MAX_CONNECTIONS (64): a leaked connection would make the server refuse the rest.
    for _ in 0..150 {
        assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    }
    for _ in 0..30 {
        assert!(ipc_send_raw(&comp, &(u32::MAX).to_le_bytes()));
        assert!(ipc_send_raw(&comp, b"\x02\x00\x00\x00{]"));
    }
    let watch = ipc_watch(&comp);
    drop(watch);
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
    // The sources of the last connection go away on the next loop iterations.
    let deadline = Instant::now() + Duration::from_secs(2);
    while comp.open_fds() > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(comp.open_fds(), baseline, "every closed connection gave its fds back");
}

#[test]
fn debug_key_reaches_the_focused_window_through_the_keyboard() {
    let comp = Compositor::start();
    let mut c = TestClient::new(&comp);
    let w = c.map_toplevel((400, 300));
    assert!(c.wait_keyboard_focus(&w, Duration::from_secs(2)));
    assert_eq!(ipc_request(&comp, "debug/key", json!({"key": "Ctrl+Alt+u"}))["ok"], true);
    assert!(c.wait_keys(2, Duration::from_secs(2)), "press and release of u, got {}", c.keys_received());
    c.roundtrip();
    assert_eq!(c.keys_received(), 2, "modifiers are not counted, nothing else is sent");
}

/// Kills (SIGKILL) and reaps the helper process this test spawned, when the
/// test ends early.
struct HelperGuard(Child);

impl Drop for HelperGuard {
    /// Kills and reaps the helper (a no-op error when it is already gone).
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn window_kill_kills_the_client_process_and_nothing_else() {
    let comp = Compositor::start();
    let exe = std::env::current_exe().expect("test binary path");
    let child = Command::new(exe)
        .args(["--exact", "kill_helper_client", "--ignored", "--nocapture", "--test-threads=1"])
        .env(HELPER_ENV, &comp.socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the helper client");
    let mut helper = HelperGuard(child);
    let stdout = helper.0.stdout.take().expect("helper stdout");
    let mut lines = BufReader::new(stdout).lines();
    // libtest prints "test kill_helper_client ... " on the same line first.
    let ready = lines.any(|l| l.is_ok_and(|l| l.trim_end().ends_with(HELPER_READY)));
    assert!(ready, "the helper never mapped its window");
    let helper_pid = helper.0.id();
    assert_ne!(helper_pid, std::process::id());
    let list = ipc_request(&comp, "windows/list", json!({}));
    let windows = list["windows"].as_array().expect("windows");
    let target = windows.iter().find(|w| w["pid"] == u64::from(helper_pid)).expect("the helper window, by pid");
    assert_eq!(ipc_request(&comp, "window/kill", json!({"id": target["id"]}))["ok"], true);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = helper.0.try_wait().expect("wait helper") { break status; }
        assert!(Instant::now() < deadline, "the helper was not killed");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let list = ipc_request(&comp, "windows/list", json!({}));
    assert!(list["windows"].as_array().is_some_and(|w| w.iter().all(|w| w["pid"] != u64::from(helper_pid))));
}

/// Not a test of its own: the client process of
/// `window_kill_kills_the_client_process_and_nothing_else`. With the helper
/// variable set it maps a window, prints `HELPER_READY` and waits to be
/// killed (at most 30 s); without it it returns at once.
#[test]
#[ignore = "helper process of window_kill_kills_the_client_process_and_nothing_else"]
fn kill_helper_client() {
    let Some(socket) = std::env::var_os(HELPER_ENV) else { return };
    let mut c = TestClient::connect_to(&PathBuf::from(socket));
    let _w = c.map_toplevel((200, 100));
    println!("{HELPER_READY}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_client_that_half_closes_after_its_request_gets_the_reply() {
    let comp = Compositor::start();
    let replies = ipc_requests_then_half_close(&comp, "desktops/list", 1, Duration::from_secs(2));
    assert_eq!(replies.len(), 1, "one reply after shutdown(Write)");
    assert_eq!(replies[0]["count"], 4);
}

#[test]
fn a_client_that_half_closes_gets_every_reply_even_past_the_socket_buffer() {
    let comp = Compositor::start();
    std::thread::sleep(Duration::from_millis(200));
    let baseline = comp.open_fds();
    // About 3 MiB of replies (under IPC_MAX_PENDING): far more than the socket buffer, so some are still pending at EOF.
    let count = 100_000;
    let replies = ipc_requests_then_half_close(&comp, "desktops/list", count, Duration::from_secs(10));
    assert_eq!(replies.len(), count, "every reply reaches a client that half-closed");
    assert!(replies.iter().all(|r| r["count"] == 4));
    // Once flushed the connection is removed and its fds go back.
    let deadline = Instant::now() + Duration::from_secs(2);
    while comp.open_fds() > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(comp.open_fds(), baseline, "the half-closed connection gave its fds back");
    assert_eq!(ipc_request(&comp, "desktops/list", json!({}))["count"], 4);
}
