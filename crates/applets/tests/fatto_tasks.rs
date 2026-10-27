//! Checks of the `fatto-tasks` applet against a fake Fatto MCP endpoint on a
//! local port: it lists the open cards (not DONE, archived or deleted), opens
//! a detail window once per card, and a refused key is shown without ever
//! printing the key. Nothing reaches the Internet or shows a window.

use gtk4::glib::MainContext;
use opusdm_applets::manifest;
use opusdm_applets::runtime::{Event, Runtime, Sink};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest a test waits for something to happen on the main loop.
const WAIT: Duration = Duration::from_secs(10);

/// The API key the tests give the applet.
const KEY: &str = "fatto_test_key_123";

/// What the fake server saw: (tool name, Authorization header) per request.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// The folder of the applet.
fn applet_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../applets/fatto-tasks")
}

/// The tool answer of the fake server for tool `name` with `args`.
fn tool_answer(name: &str, args: &Value) -> Value {
    match name {
        "kanban_workspace_list" => json!([{ "id": "ws1", "name": "Work" }, { "id": "ws2", "name": "Home" }]),
        "kanban_board_list" => json!([{ "id": "b1", "id_workspace": args["id_workspace"], "name": "Main" }]),
        "kanban_board_get_full" => json!({
            "board": { "id": "b1", "name": "Main" },
            "columns": [
                { "id": "c3", "name": "DONE", "position": 2 },
                { "id": "c1", "name": "TO-DO", "position": 0 },
                { "id": "c2", "name": "In corso", "position": 1 },
            ],
            "cards": [
                { "id": "k3", "id_board": "b1", "id_column": "c2", "code": "T-3", "title": "Second", "position": 0 },
                { "id": "k1", "id_board": "b1", "id_column": "c1", "code": "T-1", "title": "First task", "position": 0 },
                { "id": "k4", "id_board": "b1", "id_column": "c3", "code": "T-4", "title": "Finished", "position": 0, "done_at": 5 },
                { "id": "k5", "id_board": "b1", "id_column": "c1", "code": "T-5", "title": "Archived", "position": 1, "is_archived": true },
            ],
        }),
        "kanban_card_get" => json!({
            "id": args["id_card"], "id_board": "b1", "id_column": "c1", "code": "T-1", "title": "First task",
            "description": "Do the thing", "due_date": 1793003107116u64, "assignees": [{ "name": "Ada" }], "tags": [{ "name": "urgent" }],
            "checklists": [{ "title": "Steps", "items": [{ "text": "one", "done": true }, { "text": "two", "done": false }] }],
        }),
        other => panic!("the applet called an unexpected tool {other}"),
    }
}

/// Answer one connection: read the request, record it, reply with the tool
/// answer (or HTTP 401 when `refuse`).
fn serve_connection(mut stream: TcpStream, seen: &Seen, refuse: bool) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let (head_end, length) = loop {
        let Ok(n) = stream.read(&mut buf) else { return };
        if n == 0 { return }
        data.extend_from_slice(&buf[..n]);
        let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
        let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
        let length = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
        if data.len() >= end + 4 + length { break (end, length) }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let auth = head.lines().find_map(|l| l.to_lowercase().starts_with("authorization:").then(|| l.split_once(':').unwrap().1.trim().to_string())).unwrap_or_default();
    let body: Value = serde_json::from_slice(&data[head_end + 4..head_end + 4 + length]).unwrap();
    let name = body["params"]["name"].as_str().unwrap_or_default().to_string();
    seen.lock().unwrap().push((name.clone(), auth));
    let (status, text) = if refuse {
        ("401 Unauthorized", json!({ "error": { "code": -32001, "message": "Unauthorized" } }).to_string())
    } else {
        let answer = tool_answer(&name, &body["params"]["arguments"]);
        ("200 OK", json!({ "jsonrpc": "2.0", "id": body["id"], "result": { "content": [{ "type": "text", "text": answer.to_string() }] } }).to_string())
    };
    let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
    let _ = stream.write_all(reply.as_bytes());
}

/// Serve the fake endpoint on a local port in the background; returns the port.
fn serve(seen: &Seen, refuse: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let seen = seen.clone();
            std::thread::spawn(move || serve_connection(stream, &seen, refuse));
        }
    });
    port
}

/// Every `Event::Ui` value the applet sent.
type Ui = Rc<RefCell<Vec<Value>>>;

/// Start the real applet against `port` with `key`, collecting its Ui events.
fn start(port: u16, key: &str, ui: &Ui) -> Runtime {
    let dir = applet_dir();
    let manifest = manifest::parse(&std::fs::read_to_string(dir.join("applet.json")).unwrap()).unwrap();
    let mut settings = Map::new();
    settings.insert("baseUrl".into(), json!(format!("http://127.0.0.1:{port}")));
    settings.insert("apiKey".into(), json!(key));
    let ui = ui.clone();
    let sink: Sink = Rc::new(move |e| {
        if let Event::Ui(v) = e { ui.borrow_mut().push(v); }
    });
    Runtime::start("fatto-tasks", &dir, &manifest, settings, sink).unwrap()
}

/// Iterate the thread-default main context until `done` or [`WAIT`] passes.
fn pump_until(done: impl Fn() -> bool) -> bool {
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if done() { return true; }
        if !ctx.iteration(false) { std::thread::sleep(Duration::from_millis(1)); }
    }
    done()
}

/// Iterate the main context for `time`.
fn pump_for(time: Duration) {
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + time;
    while Instant::now() < end {
        if !ctx.iteration(false) { std::thread::sleep(Duration::from_millis(1)); }
    }
}

/// The latest `update` of widget `id` whose props contain `key`.
fn last_update(ui: &Ui, id: &str, key: &str) -> Option<Value> {
    ui.borrow().iter().rev().find(|v| v["op"] == "update" && v["id"] == id && v["props"].get(key).is_some()).map(|v| v["props"].clone())
}

/// The `openWindow` events so far.
fn windows(ui: &Ui) -> Vec<Value> {
    ui.borrow().iter().filter(|v| v["op"] == "openWindow").cloned().collect()
}

/// Every text (`text` or `label`) of the nodes of `tree`.
fn texts(tree: &Value, out: &mut Vec<String>) {
    for key in ["text", "label"] {
        if let Some(t) = tree[key].as_str() { out.push(t.to_string()); }
    }
    for child in tree["children"].as_array().into_iter().flatten() { texts(child, out); }
}

#[test]
fn the_manifest_parses() {
    let m = manifest::parse(&std::fs::read_to_string(applet_dir().join("applet.json")).unwrap()).unwrap();
    assert_eq!(m.name, "Fatto Tasks");
    assert!(applet_dir().join(&m.entry).is_file());
}

#[test]
fn it_lists_the_open_cards_and_opens_each_detail_once() {
    MainContext::new()
        .with_thread_default(|| {
            let seen = Seen::default();
            let port = serve(&seen, false);
            let ui = Ui::default();
            let rt = start(port, KEY, &ui);
            assert!(pump_until(|| last_update(&ui, "list", "items").is_some_and(|p| !p["items"].as_array().unwrap().is_empty())), "no list: {:?}", ui.borrow());
            // The dropdowns got the workspace and board names, first selected.
            assert_eq!(last_update(&ui, "workspaces", "items").unwrap(), json!({ "items": ["Work", "Home"], "selected": 0 }));
            assert_eq!(last_update(&ui, "boards", "items").unwrap(), json!({ "items": ["Main"], "selected": 0 }));
            // Open cards only, in column order (TO-DO, then In corso).
            let items = last_update(&ui, "list", "items").unwrap()["items"].clone();
            let list: Vec<&str> = items.as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
            assert_eq!(list, ["T-1  First task  [TO-DO]", "T-3  Second  [In corso]"]);
            // The board of the first workspace was asked for.
            let calls: Vec<String> = seen.lock().unwrap().iter().map(|(n, _)| n.clone()).collect();
            assert_eq!(calls, ["kanban_workspace_list", "kanban_board_list", "kanban_board_get_full"]);
            assert!(seen.lock().unwrap().iter().all(|(_, auth)| auth == &format!("Bearer {KEY}")));

            // A click opens the detail window, read only, with the card fields.
            let click = items[0]["onClick"]["handler"].as_u64().unwrap() as u32;
            rt.dispatch(click, Value::Null);
            assert!(pump_until(|| windows(&ui).len() == 1), "no detail window: {:?}", ui.borrow());
            let window = windows(&ui)[0].clone();
            assert_eq!(window["title"], "T-1 - First task");
            let mut all = Vec::new();
            texts(&window["tree"], &mut all);
            for want in ["T-1  First task", "Column: TO-DO", "Assignees: Ada", "Due: 2026-10-26", "Tags: urgent", "Do the thing", "Open in Fatto"] {
                assert!(all.iter().any(|t| t == want), "{want} missing in {all:?}");
            }
            assert!(all.iter().any(|t| t.contains("[x] one") && t.contains("[ ] two")), "{all:?}");
            // The same card again while its window is open: no second window.
            rt.dispatch(click, Value::Null);
            pump_for(Duration::from_millis(400));
            assert_eq!(windows(&ui).len(), 1);
            // After it is closed the card opens again.
            let on_close = window["onClose"]["handler"].as_u64().unwrap() as u32;
            rt.dispatch(on_close, Value::Null);
            rt.dispatch(click, Value::Null);
            assert!(pump_until(|| windows(&ui).len() == 2), "the card did not open again");
            // The window ids are never reused.
            assert_ne!(windows(&ui)[0]["window"], windows(&ui)[1]["window"]);
        })
        .unwrap();
}

#[test]
fn a_refused_key_is_shown_without_the_key() {
    MainContext::new()
        .with_thread_default(|| {
            let seen = Seen::default();
            let port = serve(&seen, true);
            let ui = Ui::default();
            let _rt = start(port, KEY, &ui);
            assert!(pump_until(|| last_update(&ui, "status", "text").is_some_and(|p| p["text"].as_str().unwrap().contains("refused"))), "{:?}", ui.borrow());
            assert!(!format!("{:?}", ui.borrow()).contains(KEY), "the key leaked into the UI events");
        })
        .unwrap();
}

#[test]
fn without_a_key_nothing_is_sent() {
    MainContext::new()
        .with_thread_default(|| {
            let seen = Seen::default();
            let port = serve(&seen, false);
            let ui = Ui::default();
            let _rt = start(port, "", &ui);
            assert!(pump_until(|| last_update(&ui, "status", "text").is_some_and(|p| p["text"].as_str().unwrap().contains("API key"))), "{:?}", ui.borrow());
            pump_for(Duration::from_millis(200));
            assert!(seen.lock().unwrap().is_empty(), "a request was sent without a key");
        })
        .unwrap();
}
