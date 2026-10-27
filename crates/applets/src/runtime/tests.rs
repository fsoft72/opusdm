use super::*;
use crate::manifest;
use gtk4::glib::MainContext;
use serde_json::json;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest a test waits for something to happen on the main loop.
const WAIT: Duration = Duration::from_secs(5);
/// Longest a runaway test waits for the cut: a callback burns its whole CPU
/// budget first (about 3 s in a debug build), much more under load.
const RUNAWAY_WAIT: Duration = Duration::from_secs(20);

/// The events an applet sent, in order.
type Events = Rc<RefCell<Vec<Event>>>;

/// A temporary applet folder with `main.js` holding `code`.
fn applet(code: &str) -> (tempfile::TempDir, Manifest) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), code).unwrap();
    let manifest = manifest::parse(r#"{ "name": "Test Applet" }"#).unwrap();
    (dir, manifest)
}

/// A sink that stores every event in `events`.
fn collect(events: &Events) -> Sink {
    let events = events.clone();
    Rc::new(move |e| events.borrow_mut().push(e))
}

/// A notifier that never reaches D-Bus: it records the payload and answers 42.
fn fake_notifier(seen: &Arc<Mutex<Vec<Value>>>) -> Notifier {
    let seen = seen.clone();
    Arc::new(move |payload: &Value| {
        seen.lock().unwrap().push(payload.clone());
        Ok(json!(42))
    })
}

/// Start `code` as applet `test` with `settings`, collecting its events.
fn start_code(code: &str, settings: Map<String, Value>, events: &Events) -> (tempfile::TempDir, Result<Runtime, String>) {
    let (dir, manifest) = applet(code);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, settings, collect(events), fake_notifier(&seen));
    (dir, rt)
}

/// Iterate the thread-default main context for `time`.
fn pump(time: Duration) {
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + time;
    while Instant::now() < end {
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Iterate the thread-default main context until `done` or [`WAIT`] passes.
fn pump_until(done: impl Fn() -> bool) -> bool {
    pump_until_within(WAIT, done)
}

/// Iterate the thread-default main context until `done` or `limit` passes.
fn pump_until_within(limit: Duration, done: impl Fn() -> bool) -> bool {
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if done() {
            return true;
        }
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    done()
}

/// Run `test` with a fresh main context as the thread default.
fn on_main_loop(test: impl FnOnce()) {
    MainContext::new().with_thread_default(test).unwrap();
}

/// The text of every `Log` event.
fn logs(events: &Events) -> Vec<String> {
    events.borrow().iter().filter_map(|e| if let Event::Log(t) = e { Some(t.clone()) } else { None }).collect()
}

/// The text of every `Error` event.
fn errors(events: &Events) -> Vec<String> {
    events.borrow().iter().filter_map(|e| if let Event::Error(t) = e { Some(t.clone()) } else { None }).collect()
}

/// Whether some log line equals `line`.
fn has_log(events: &Events, line: &str) -> bool {
    logs(events).iter().any(|l| l == line)
}

/// Whether some error contains `part`.
fn has_error(events: &Events, part: &str) -> bool {
    errors(events).iter().any(|e| e.contains(part))
}

/// Serve one HTTP answer with `body` on a local port; returns the port.
fn serve_once(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(reply.as_bytes());
    });
    port
}

#[test]
fn a_script_can_log() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"opus.log("hi")"#, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["hi".to_string()]);
    });
}

#[test]
fn the_applet_fields_are_set() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.log(opus.applet.id + "|" + opus.applet.name + "|" + (opus.applet.dir.length > 0) + "|" + typeof opus.version)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["test|Test Applet|true|string".to_string()]);
    });
}

#[test]
fn console_maps_to_log_and_error_events() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"console.log("a", 1); console.warn("w"); console.error("e")"#, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["a 1".to_string(), "warn: w".to_string()]);
        assert_eq!(errors(&events), vec!["e".to_string()]);
    });
}

#[test]
fn set_timeout_fires_on_the_main_loop() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"setTimeout((a) => opus.log("fired " + a), 10, "x"); opus.log("started")"#, Map::new(), &events);
        let _rt = rt.unwrap();
        assert_eq!(logs(&events), vec!["started".to_string()]);
        assert!(pump_until(|| has_log(&events, "fired x")), "{:?}", events.borrow());
    });
}

#[test]
fn clear_timeout_cancels_a_timer() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"const t = setTimeout(() => opus.log("no"), 10); clearTimeout(t); setTimeout(() => opus.log("yes"), 30)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "yes")));
        assert!(!has_log(&events, "no"), "{:?}", events.borrow());
    });
}

#[test]
fn set_interval_repeats_until_cleared() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"let n = 0; const id = setInterval(() => { n++; opus.log("tick " + n); if (n === 3) clearInterval(id); }, 5)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "tick 3")));
        pump(Duration::from_millis(100));
        assert_eq!(logs(&events), vec!["tick 1", "tick 2", "tick 3"]);
        assert!(errors(&events).is_empty(), "{:?}", events.borrow());
    });
}

/// How many `Log(line)` events were collected.
fn count_logs(events: &Events, line: &str) -> usize {
    logs(events).iter().filter(|l| l.as_str() == line).count()
}

#[test]
fn set_timeout_waits_for_its_delay() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"setTimeout(() => opus.log("t"), 300)"#, Map::new(), &events);
        let _rt = rt.unwrap();
        pump(Duration::from_millis(100));
        assert_eq!(count_logs(&events, "t"), 0, "fired too early: {:?}", events.borrow());
        pump(Duration::from_millis(900));
        assert_eq!(count_logs(&events, "t"), 1, "{:?}", events.borrow());
    });
}

#[test]
fn set_interval_keeps_its_period() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"setInterval(() => opus.log("i"), 200)"#, Map::new(), &events);
        let _rt = rt.unwrap();
        pump(Duration::from_millis(1000));
        let n = count_logs(&events, "i");
        assert!((3..=6).contains(&n), "expected 3 to 6 ticks in 1 s, got {n}");
    });
}

#[test]
fn timer_delay_accepts_floats_and_strings() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"
            setTimeout(() => opus.log("float"), 250.7);
            setTimeout(() => opus.log("string"), "250");
            setTimeout(() => opus.log("nan"), NaN);
            setTimeout(() => opus.log("negative"), -500);
            setTimeout(() => opus.log("text"), "abc");
        "#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        pump(Duration::from_millis(100));
        assert_eq!(count_logs(&events, "float") + count_logs(&events, "string"), 0, "fired too early: {:?}", events.borrow());
        for zero in ["nan", "negative", "text"] {
            assert_eq!(count_logs(&events, zero), 1, "{zero} must mean 0: {:?}", events.borrow());
        }
        pump(Duration::from_millis(500));
        assert_eq!(count_logs(&events, "float"), 1, "{:?}", events.borrow());
        assert_eq!(count_logs(&events, "string"), 1, "{:?}", events.borrow());
    });
}

#[test]
fn timer_delay_reads_json_numbers() {
    assert_eq!(delay_ms(&json!({ "ms": 1000 })), 1000);
    assert_eq!(delay_ms(&json!({ "ms": 1000.0 })), 1000);
    assert_eq!(delay_ms(&json!({ "ms": 50.7 })), 50);
    assert_eq!(delay_ms(&json!({ "ms": -5.0 })), 0);
    assert_eq!(delay_ms(&json!({ "ms": -5 })), 0);
    assert_eq!(delay_ms(&json!({ "ms": null })), 0);
    assert_eq!(delay_ms(&json!({ "ms": "100" })), 0);
    assert_eq!(delay_ms(&json!({})), 0);
    assert_eq!(delay_ms(&json!({ "ms": 1e300 })), MAX_DELAY_MS);
    assert_eq!(delay_ms(&json!({ "ms": u64::MAX })), MAX_DELAY_MS);
}

#[test]
fn notify_timeout_reads_js_floats() {
    on_main_loop(|| {
        let events = Events::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (dir, manifest) = applet(r#"opus.notify("T", { timeout: Math.floor(5000.9) }); opus.notify("T", { timeout: 0 }); opus.notify("T")"#);
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&seen));
        let _rt = rt.unwrap();
        assert!(pump_until(|| seen.lock().unwrap().len() == 3), "{:?}", events.borrow());
        let seen = seen.lock().unwrap();
        let got: Vec<i32> = seen.iter().map(notify_timeout).collect();
        assert_eq!(got, vec![5000, 0, -1]);
    });
}

#[test]
fn http_get_resolves_a_promise_from_a_local_server() {
    on_main_loop(|| {
        let events = Events::default();
        let port = serve_once("hello");
        let code = format!(
            r#"opus.http.get("http://127.0.0.1:{port}/", {{ timeoutMs: 2000 }}).then((r) => opus.log(r.status + " " + r.ok + " " + r.body)); opus.log("sent")"#
        );
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert_eq!(logs(&events), vec!["sent".to_string()]);
        assert!(pump_until(|| has_log(&events, "200 true hello")), "{:?}", events.borrow());
    });
}

#[test]
fn http_post_resolves_a_promise_from_a_local_server() {
    on_main_loop(|| {
        let events = Events::default();
        let port = serve_once("hello");
        let code = format!(
            r#"opus.http.post("http://127.0.0.1:{port}/", {{ headers: {{ "Content-Type": "application/json" }}, body: "{{}}", timeoutMs: 2000 }}).then((r) => opus.log(r.status + " " + r.ok + " " + r.body)); opus.log("sent")"#
        );
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert_eq!(logs(&events), vec!["sent".to_string()]);
        assert!(pump_until(|| has_log(&events, "200 true hello")), "{:?}", events.borrow());
    });
}

#[test]
fn http_post_rejects_on_a_bad_scheme() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.http.post("ftp://example.com/", { body: "x" }).catch((e) => opus.log("rejected " + (e instanceof Error) + " " + e.message))"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| logs(&events).iter().any(|l| l.starts_with("rejected true") && l.contains("scheme"))), "{:?}", events.borrow());
    });
}

#[test]
fn http_get_rejects_on_a_bad_scheme() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.http.get("ftp://example.com/").catch((e) => opus.log("rejected " + (e instanceof Error) + " " + e.message))"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| logs(&events).iter().any(|l| l.starts_with("rejected true") && l.contains("scheme"))), "{:?}", events.borrow());
        assert!(errors(&events).is_empty(), "{:?}", events.borrow());
    });
}

#[test]
fn a_throwing_script_reports_an_error_and_start_returns_err() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"throw new Error("boom")"#, Map::new(), &events);
        let err = rt.err().expect("start must fail");
        assert!(err.contains("boom"), "{err}");
        assert!(has_error(&events, "boom"), "{:?}", events.borrow());
    });
}

#[test]
fn a_missing_entry_script_is_an_error() {
    on_main_loop(|| {
        let events = Events::default();
        let (dir, manifest) = applet("");
        std::fs::remove_file(dir.path().join("main.js")).unwrap();
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&Arc::default()));
        let err = rt.err().expect("start must fail");
        assert!(err.contains("main.js"), "{err}");
    });
}

#[test]
fn a_throwing_timer_callback_reports_an_error_and_other_timers_still_fire() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"setTimeout(() => { throw new Error("bad tick") }, 0); setTimeout(() => opus.log("still here"), 30)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "still here")), "{:?}", events.borrow());
        assert!(has_error(&events, "bad tick"), "{:?}", events.borrow());
    });
}

#[test]
fn an_unhandled_promise_rejection_reports_an_error() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"Promise.reject(new Error("x top"))"#, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert!(has_error(&events, "x top"), "{:?}", events.borrow());
    });
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"setTimeout(async () => { throw new Error("y async") }, 0)"#, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_error(&events, "y async")), "{:?}", events.borrow());
    });
}

#[test]
fn a_handled_rejection_is_not_reported() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"Promise.reject(new Error("z")).catch(() => opus.log("caught")); (async () => { try { await Promise.reject(new Error("w")) } catch (e) { opus.log("awaited") } })()"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert!(has_log(&events, "caught") && has_log(&events, "awaited"), "{:?}", events.borrow());
        assert!(errors(&events).is_empty(), "{:?}", events.borrow());
    });
}

#[test]
fn a_runaway_callback_is_cut_and_reported() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"setTimeout(() => { while (true) {} }, 0); setTimeout(() => opus.log("alive"), 20)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until_within(RUNAWAY_WAIT, || has_log(&events, "alive")), "{:?}", events.borrow());
        assert!(has_error(&events, "loop"), "{:?}", events.borrow());
    });
}

#[test]
fn a_runaway_loop_cannot_be_caught_by_the_script() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"setTimeout(() => { try { while (true) {} } catch (e) { opus.log("caught") } }, 0)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until_within(RUNAWAY_WAIT, || has_error(&events, "loop")), "{:?}", events.borrow());
        assert!(!has_log(&events, "caught"));
    });
}

#[test]
fn a_deep_recursion_is_cut_and_reported() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"function f() { f() } setTimeout(f, 0)"#, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until_within(RUNAWAY_WAIT, || has_error(&events, "recursive")), "{:?}", events.borrow());
    });
}

#[test]
fn deliver_settings_calls_on_settings() {
    on_main_loop(|| {
        let events = Events::default();
        let mut settings = Map::new();
        settings.insert("city".into(), json!("Rome"));
        let code = r#"opus.log("first " + opus.applet.settings.city); opus.applet.onSettings((s) => opus.log("got " + s.city + " " + opus.applet.settings.city))"#;
        let (_dir, rt) = start_code(code, settings, &events);
        let rt = rt.unwrap();
        let mut next = Map::new();
        next.insert("city".into(), json!("Milan"));
        rt.deliver_settings(next);
        assert_eq!(logs(&events), vec!["first Rome".to_string(), "got Milan Milan".to_string()]);
    });
}

#[test]
fn dispatch_calls_a_registered_handler() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"const id = __opusInternal.registerHandler((a) => opus.log("handled " + a.x)); opus.log("id " + id)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        let id: u32 = logs(&events)[0].strip_prefix("id ").unwrap().parse().unwrap();
        rt.dispatch(id, json!({ "x": 7 }));
        rt.dispatch(id, json!({ "x": 8 }));
        assert_eq!(logs(&events)[1..], ["handled 7".to_string(), "handled 8".to_string()]);
        rt.dispatch(id + 100, Value::Null);
        assert!(has_error(&events, "handler"), "{:?}", events.borrow());
    });
}

#[test]
fn redraw_calls_the_js_hook() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code("", Map::new(), &events);
        let rt = rt.unwrap();
        rt.redraw("c1");
        assert!(has_error(&events, "no canvas \"c1\""), "the default hook paints the shown tree's canvas, an unknown one is an error: {:?}", events.borrow());
    });
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"__opusInternal.redraw = (id) => opus.log("redraw " + id)"#, Map::new(), &events);
        let rt = rt.unwrap();
        rt.redraw("c1");
        assert_eq!(logs(&events), vec!["redraw c1".to_string()]);
    });
}

#[test]
fn a_call_while_the_script_runs_is_queued_not_a_panic() {
    on_main_loop(|| {
        let events = Events::default();
        let (dir, manifest) = applet(r#"const id = __opusInternal.registerHandler((a) => opus.log("handled " + a)); setTimeout(() => opus.log("ping " + id), 0)"#);
        let slot: Rc<RefCell<Option<std::rc::Weak<Runtime>>>> = Rc::default();
        let sink: Sink = {
            let events = events.clone();
            let slot = slot.clone();
            Rc::new(move |e: Event| {
                // A sink that calls back into the runtime while the script runs.
                if let Event::Log(text) = &e {
                    if let Some(id) = text.strip_prefix("ping ") {
                        let rt = slot.borrow().as_ref().and_then(std::rc::Weak::upgrade).expect("runtime alive");
                        rt.dispatch(id.parse().unwrap(), json!("late"));
                    }
                }
                events.borrow_mut().push(e);
            })
        };
        let rt = Rc::new(Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), sink, fake_notifier(&Arc::default())).unwrap());
        *slot.borrow_mut() = Some(Rc::downgrade(&rt));
        assert!(pump_until(|| has_log(&events, "handled late")), "{:?}", events.borrow());
    });
}

#[test]
fn read_asset_refuses_a_path_outside_the_applet_folder() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"
            const t = (rel) => { try { opus.applet.readAsset(rel); opus.log("read " + rel) } catch (e) { opus.log("refused " + rel) } };
            t("../../etc/passwd"); t("/etc/passwd"); t("a/../../x"); t("data.txt"); t("sub/../data.txt");
            opus.log(opus.applet.readAsset("data.txt"));
            opus.log(opus.applet.assetPath("data.txt") === opus.applet.dir + "/data.txt" ? "path ok" : "path " + opus.applet.assetPath("data.txt"));
        "#;
        let (dir, manifest) = applet(code);
        std::fs::write(dir.path().join("data.txt"), "asset text").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&Arc::default()));
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(
            logs(&events),
            vec!["refused ../../etc/passwd", "refused /etc/passwd", "refused a/../../x", "read data.txt", "read sub/../data.txt", "asset text", "path ok"]
        );
    });
}

#[test]
fn read_asset_refuses_a_symlink_leaving_the_folder() {
    on_main_loop(|| {
        let events = Events::default();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        let (dir, manifest) = applet(r#"try { opus.applet.readAsset("link/secret"); opus.log("read") } catch (e) { opus.log("refused") }"#);
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&Arc::default()));
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["refused".to_string()]);
    });
}

#[test]
fn fs_reads_files_and_lists_folders() {
    on_main_loop(|| {
        let events = Events::default();
        let (dir, manifest) = applet(
            r#"
            const d = opus.applet.dir;
            opus.log(opus.fs.exists(d + "/data.txt") + " " + opus.fs.exists(d + "/nope"));
            opus.log(opus.fs.readText(d + "/data.txt"));
            opus.log(opus.fs.list(d).map((e) => e.name + (e.isDir ? "/" : "")).join(","));
            try { opus.fs.readText(d + "/nope"); } catch (e) { opus.log("error " + (e instanceof Error)) }
        "#,
        );
        std::fs::write(dir.path().join("data.txt"), "file text").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&Arc::default()));
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["true false", "file text", "data.txt,main.js,sub/", "error true"]);
    });
}

#[test]
fn notify_goes_to_the_injected_notifier() {
    on_main_loop(|| {
        let events = Events::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (dir, manifest) = applet(r#"opus.notify("Title", { body: "Body" }).then((id) => opus.log("id " + id))"#);
        let rt = Runtime::start_with_notifier("test", dir.path(), &manifest, Map::new(), collect(&events), fake_notifier(&seen));
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "id 42")), "{:?}", events.borrow());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["title"], json!("Title"));
        assert_eq!(seen[0]["body"], json!("Body"));
    });
}

#[test]
fn open_refuses_a_bad_scheme() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"for (const u of ["ftp://x", "javascript:alert(1)", "nothing"]) { try { opus.applet.open(u); opus.log("opened") } catch (e) { opus.log("refused " + (e instanceof Error)) } }"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        assert!(rt.is_ok(), "{:?}", rt.err());
        assert_eq!(logs(&events), vec!["refused true"; 3]);
    });
}

#[test]
fn timers_stop_when_the_runtime_is_dropped() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"setInterval(() => opus.log("tick"), 5)"#, Map::new(), &events);
        let rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "tick")));
        drop(rt);
        let count = events.borrow().len();
        pump(Duration::from_millis(100));
        assert_eq!(events.borrow().len(), count, "{:?}", events.borrow());
    });
}

#[test]
fn stop_cancels_pending_timers() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"setTimeout(() => opus.log("late"), 20); setInterval(() => opus.log("tick"), 5); __opusInternal.registerHandler(() => opus.log("handled"))"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        rt.stop();
        pump(Duration::from_millis(100));
        rt.dispatch(1, Value::Null);
        rt.redraw("c");
        rt.deliver_settings(Map::new());
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
    });
}

#[test]
fn an_http_answer_after_stop_is_dropped() {
    on_main_loop(|| {
        let events = Events::default();
        let port = serve_once("late");
        let code = format!(r#"opus.http.get("http://127.0.0.1:{port}/").then(() => opus.log("resolved"))"#);
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let rt = rt.unwrap();
        rt.stop();
        pump(Duration::from_millis(300));
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
    });
}

#[test]
fn every_prelude_call_is_a_known_op() {
    let mut found = 0;
    for marker in ["call(\"", "later(\""] {
        for (at, _) in PRELUDE.match_indices(marker) {
            let rest = &PRELUDE[at + marker.len()..];
            let op = &rest[..rest.find('"').unwrap()];
            assert!(OPS.iter().any(|(name, _)| *name == op), "{op} is not in OPS");
            found += 1;
        }
    }
    assert!(found >= OPS.len(), "only {found} calls found");
}

/// Every `Event::Ui` value sent so far.
fn ui_events(events: &Events) -> Vec<Value> {
    events.borrow().iter().filter_map(|e| if let Event::Ui(v) = e { Some(v.clone()) } else { None }).collect()
}

/// The handler id at `pointer` (a JSON pointer) of `value`.
fn handler_at(value: &Value, pointer: &str) -> u32 {
    let id = value.pointer(pointer).and_then(|h| h.get("handler")).and_then(Value::as_u64);
    u32::try_from(id.unwrap_or_else(|| panic!("no handler at {pointer} in {value}"))).unwrap()
}

#[test]
fn ui_show_sends_the_tree_with_handler_ids() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.ui.show({ type: "box", id: "root", children: [
            { type: "button", id: "b", label: "Go", onClick: () => opus.log("go") },
            { type: "list", items: [{ text: "a", onClick: () => opus.log("a") }] }
        ] })"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        let ui = ui_events(&events);
        assert_eq!(ui.len(), 1, "{:?}", events.borrow());
        assert_eq!(ui[0]["op"], "show");
        assert_eq!(ui[0]["tree"]["children"][0]["label"], "Go");
        crate::ui_tree::parse(&ui[0]["tree"]).unwrap();
        rt.dispatch(handler_at(&ui[0], "/tree/children/0/onClick"), Value::Null);
        rt.dispatch(handler_at(&ui[0], "/tree/children/1/items/0/onClick"), Value::Null);
        assert_eq!(logs(&events), ["go", "a"]);
    });
}

#[test]
fn ui_show_unregisters_the_handlers_of_the_previous_tree() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.ui.show({ type: "button", label: "1", onClick: () => opus.log("first") });
            const again = __opusInternal.registerHandler(() => opus.ui.show({ type: "button", label: "2", onClick: () => opus.log("second") }));
            opus.log("again " + again)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        let again: u32 = logs(&events)[0].strip_prefix("again ").unwrap().parse().unwrap();
        rt.dispatch(again, Value::Null);
        let ui = ui_events(&events);
        assert_eq!(ui.len(), 2, "{:?}", events.borrow());
        let (first, second) = (handler_at(&ui[0], "/tree/onClick"), handler_at(&ui[1], "/tree/onClick"));
        assert_ne!(first, second);
        rt.dispatch(second, Value::Null);
        assert!(has_log(&events, "second"));
        rt.dispatch(first, Value::Null);
        assert!(has_error(&events, &format!("no handler {first}")), "{:?}", events.borrow());
        assert!(!has_log(&events, "first"));
    });
}

#[test]
fn ui_update_sends_props_and_replaces_the_handlers_of_that_prop() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"opus.ui.show({ type: "box", children: [
                { type: "button", id: "b", label: "B", onClick: () => opus.log("old") },
                { type: "list", id: "l", items: [{ text: "a", onClick: () => opus.log("a") }] }
            ] });
            opus.ui.update("b", { label: "N", onClick: () => opus.log("new") });
            opus.ui.update("b", { label: "M" });
            opus.ui.update("l", { items: [{ text: "z", onClick: () => opus.log("z") }] })"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        let ui = ui_events(&events);
        assert_eq!(ui.len(), 4, "{:?}", events.borrow());
        assert_eq!(ui[1]["op"], "update");
        assert_eq!(ui[1]["id"], "b");
        assert_eq!(ui[1]["props"]["label"], "N");
        assert_eq!(ui[2]["props"], json!({ "label": "M" }));
        let old = handler_at(&ui[0], "/tree/children/0/onClick");
        let new = handler_at(&ui[1], "/props/onClick");
        let old_item = handler_at(&ui[0], "/tree/children/1/items/0/onClick");
        let new_item = handler_at(&ui[3], "/props/items/0/onClick");
        rt.dispatch(new, Value::Null);
        rt.dispatch(new_item, Value::Null);
        assert_eq!(logs(&events), ["new", "z"], "a label-only update must keep onClick");
        rt.dispatch(old, Value::Null);
        rt.dispatch(old_item, Value::Null);
        assert!(has_error(&events, &format!("no handler {old}")) && has_error(&events, &format!("no handler {old_item}")), "{:?}", events.borrow());
    });
}

#[test]
fn ui_calls_check_their_arguments() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"for (const f of [() => opus.ui.show(5), () => opus.ui.show(null), () => opus.ui.update(5, {}), () => opus.ui.update("x", null)]) {
                try { f(); } catch (e) { opus.log(e.name); }
            }
            const cyclic = { type: "box", children: [] }; cyclic.children.push(cyclic);
            try { opus.ui.show(cyclic); } catch (e) { opus.log(e.name); }"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        rt.unwrap();
        assert_eq!(logs(&events), ["TypeError", "TypeError", "TypeError", "TypeError", "RangeError"]);
        assert!(ui_events(&events).is_empty());
    });
}

#[test]
fn ui_redraw_calls_the_internal_hook() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"__opusInternal.redraw = (id) => opus.log("redraw " + id); opus.ui.redraw("c1")"#, Map::new(), &events);
        rt.unwrap();
        assert!(has_log(&events, "redraw c1"), "{:?}", events.borrow());
    });
}

#[test]
fn the_dispatcher_calls_a_handler_and_does_nothing_once_the_runtime_is_gone() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"const id = __opusInternal.registerHandler((a) => opus.log("got " + a)); opus.log("id " + id)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        let id: u32 = logs(&events)[0].strip_prefix("id ").unwrap().parse().unwrap();
        let dispatcher = rt.dispatcher();
        dispatcher.dispatch(id, json!(5));
        assert!(has_log(&events, "got 5"), "{:?}", events.borrow());
        drop(rt);
        let count = events.borrow().len();
        dispatcher.dispatch(id, json!(6));
        pump(Duration::from_millis(20));
        assert_eq!(events.borrow().len(), count, "{:?}", events.borrow());
    });
}

/// Every `Event::Redraw` sent so far, as (canvas id, commands).
fn redraws(events: &Events) -> Vec<(String, Value)> {
    events.borrow().iter().filter_map(|e| if let Event::Redraw { id, commands } = e { Some((id.clone(), commands.clone())) } else { None }).collect()
}

/// A tree with the canvas `clock` (40x30) that paints a red square.
const CANVAS_TREE: &str = r##"
    opus.ui.show({ type: "box", children: [
        { type: "canvas", id: "clock", width: 40, height: 30, draw: (ctx) => { ctx.fillStyle = "#f00"; ctx.fillRect(0, 0, 10, 10); } }
    ] });
"##;

#[test]
fn a_canvas_paints_on_show_and_on_redraw() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(CANVAS_TREE, Map::new(), &events);
        let rt = rt.unwrap();
        assert!(errors(&events).is_empty(), "{:?}", errors(&events));
        let sent = redraws(&events);
        assert_eq!(sent.len(), 1, "show paints the canvas once: {sent:?}");
        assert_eq!(sent[0].0, "clock");
        let commands = crate::canvas::parse(&sent[0].1).unwrap();
        assert_eq!(commands, [
            crate::canvas::Command::FillStyle { style: crate::canvas::Style::Color("#f00".to_string()) },
            crate::canvas::Command::FillRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 },
        ]);
        rt.redraw("clock");
        let sent = redraws(&events);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[1], sent[0]);
        assert!(errors(&events).is_empty(), "{:?}", errors(&events));
    });
}

#[test]
fn opus_ui_redraw_from_the_script_paints_the_canvas() {
    on_main_loop(|| {
        let events = Events::default();
        let code = format!("{CANVAS_TREE}\nsetTimeout(() => opus.ui.redraw(\"clock\"), 1);");
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| redraws(&events).len() == 2), "{:?}", events.borrow());
        let sent = redraws(&events);
        assert!(crate::canvas::parse(&sent[1].1).is_ok());
        assert!(errors(&events).is_empty(), "{:?}", errors(&events));
    });
}

#[test]
fn the_draw_function_sees_the_canvas_size_and_the_style_getters() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r##"
            opus.ui.show({ type: "canvas", id: "c", width: 40, height: 30, draw: (ctx) => {
                opus.log([ctx.width, ctx.height, ctx.canvas.width, ctx.canvas.height].join(" "));
                opus.log(ctx.fillStyle + " " + ctx.lineWidth + " " + ctx.globalAlpha + " " + ctx.font + " " + ctx.textAlign + " " + ctx.textBaseline);
                ctx.save(); ctx.fillStyle = "red"; ctx.lineWidth = 3; ctx.restore();
                opus.log(ctx.fillStyle + " " + ctx.lineWidth);
            } });
        "##;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        rt.unwrap();
        assert_eq!(logs(&events), ["40 30 40 30", "#000000 1 1 10px sans-serif start alphabetic", "#000000 1"], "{:?}", events.borrow());
    });
}

#[test]
fn every_ctx_method_records_a_command_that_parses() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r##"
            const img = opus.applet.assetPath("pic.png");
            opus.ui.show({ type: "canvas", id: "c", width: 20, height: 20, draw: (ctx) => {
                ctx.save(); ctx.translate(1, 2); ctx.rotate(0.1); ctx.scale(2, 2); ctx.restore();
                ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(5, 5); ctx.arc(5, 5, 3, 0, Math.PI, true); ctx.rect(0, 0, 4, 4); ctx.closePath();
                ctx.fill(); ctx.stroke(); ctx.fillRect(0, 0, 1, 1); ctx.strokeRect(0, 0, 1, 1); ctx.clearRect(0, 0, 1, 1);
                ctx.font = "bold 12px serif"; ctx.textAlign = "center"; ctx.textBaseline = "middle";
                ctx.fillText("a", 1, 2); ctx.strokeText("b", 1, 2, 30); ctx.drawImage(img, 0, 0); ctx.drawImage(img, 0, 0, 5, 6);
                const g = ctx.createLinearGradient(0, 0, 10, 0); g.addColorStop(0, "red"); g.addColorStop(1, "blue");
                ctx.fillStyle = g;
                const r = ctx.createRadialGradient(5, 5, 0, 5, 5, 5); r.addColorStop(0, "#fff");
                ctx.strokeStyle = r; ctx.lineWidth = 2; ctx.lineCap = "round"; ctx.globalAlpha = 0.5;
                ctx.fillRect(NaN, 0, 1, 1);
            } });
        "##;
        let (dir, rt) = start_code(code, Map::new(), &events);
        rt.unwrap();
        assert!(errors(&events).is_empty(), "{:?}", errors(&events));
        let sent = redraws(&events);
        assert_eq!(sent.len(), 1, "{:?}", events.borrow());
        let commands = crate::canvas::parse(&sent[0].1).unwrap_or_else(|e| panic!("{e}: {}", sent[0].1));
        assert_eq!(commands.len(), 29, "{}", sent[0].1);
        let img = dir.path().canonicalize().unwrap().join("pic.png").display().to_string();
        assert!(commands.contains(&crate::canvas::Command::DrawImage { path: img.clone(), x: 0.0, y: 0.0, w: Some(5.0), h: Some(6.0) }), "{}", sent[0].1);
        assert!(commands.contains(&crate::canvas::Command::DrawImage { path: img, x: 0.0, y: 0.0, w: None, h: None }), "{}", sent[0].1);
        let Some(crate::canvas::Command::FillRect { x, .. }) = commands.last() else { panic!("{commands:?}") };
        assert!(x.is_nan(), "NaN travels as null and parses as NaN");
    });
}

#[test]
fn a_canvas_without_an_id_is_still_painted_on_show() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r##"opus.ui.show({ type: "box", children: [{ type: "canvas", width: 10, height: 10, draw: (ctx) => ctx.fillRect(0, 0, 1, 1) }] });"##;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        rt.unwrap();
        let sent = redraws(&events);
        assert_eq!(sent.len(), 1, "{:?}", events.borrow());
        let tree = &ui_events(&events)[0]["tree"];
        let id = tree.pointer("/children/0/id").and_then(Value::as_str).expect("the canvas got a generated id");
        assert_eq!(sent[0].0, id);
        crate::ui_tree::parse(tree).unwrap();
    });
}

#[test]
fn redraw_of_an_unknown_canvas_is_a_reported_error() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(CANVAS_TREE, Map::new(), &events);
        let rt = rt.unwrap();
        rt.redraw("nope");
        assert!(has_error(&events, "nope"), "{:?}", events.borrow());
        assert_eq!(redraws(&events).len(), 1);
        let events = Events::default();
        let (_dir, rt) = start_code(r##"try { opus.ui.redraw("x") } catch (e) { opus.log(e.name + ": " + e.message) }"##, Map::new(), &events);
        rt.unwrap();
        assert!(logs(&events).iter().any(|l| l.starts_with("Error:") && l.contains("\"x\"")), "{:?}", events.borrow());
    });
}

#[test]
fn a_throwing_draw_is_reported_and_sends_no_picture() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r##"
            let fail = false;
            opus.ui.show({ type: "box", children: [
                { type: "canvas", id: "a", width: 5, height: 5, draw: (ctx) => { if (fail) throw new Error("boom"); ctx.fill(); } },
                { type: "canvas", id: "b", width: 5, height: 5, draw: (ctx) => { throw new Error("always"); } }
            ] });
            setTimeout(() => { fail = true; }, 1);
        "##;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let rt = rt.unwrap();
        assert!(has_error(&events, "always"), "a draw failing on show is reported: {:?}", events.borrow());
        assert_eq!(redraws(&events).iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["a"], "the other canvas still painted");
        pump(Duration::from_millis(20));
        rt.redraw("a");
        assert!(has_error(&events, "boom"), "{:?}", events.borrow());
        assert_eq!(redraws(&events).len(), 1, "a failed draw sends no picture: the previous one stays");
    });
}

#[test]
fn a_draw_recording_too_many_commands_is_refused() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r##"opus.ui.show({ type: "canvas", id: "c", width: 5, height: 5, draw: (ctx) => { for (let i = 0; i < 100001; i++) ctx.beginPath(); } });"##;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        rt.unwrap();
        assert!(has_error(&events, "100000"), "{:?}", errors(&events));
        assert!(redraws(&events).is_empty());
    });
}

#[test]
fn an_update_of_draw_is_used_by_the_next_redraw() {
    on_main_loop(|| {
        let events = Events::default();
        let code = format!("{CANVAS_TREE}\nopus.ui.update(\"clock\", {{ draw: (ctx) => ctx.clearRect(0, 0, ctx.width, ctx.height) }});");
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let rt = rt.unwrap();
        rt.redraw("clock");
        let sent = redraws(&events);
        assert_eq!(sent.len(), 2, "{:?}", events.borrow());
        assert_eq!(crate::canvas::parse(&sent[1].1).unwrap(), [crate::canvas::Command::ClearRect { x: 0.0, y: 0.0, w: 40.0, h: 30.0 }]);
    });
}

#[test]
fn a_new_tree_forgets_the_canvases_of_the_old_one() {
    on_main_loop(|| {
        let events = Events::default();
        let code = format!("{CANVAS_TREE}\nopus.ui.show({{ type: \"label\", text: \"x\" }});");
        let (_dir, rt) = start_code(&code, Map::new(), &events);
        let rt = rt.unwrap();
        rt.redraw("clock");
        assert!(has_error(&events, "clock"), "{:?}", events.borrow());
    });
}

#[test]
fn open_window_validates_in_the_script_and_limits_the_count() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"
const attempt = (name, f) => { try { f(); opus.log(name + " ok"); } catch (e) { opus.log(name + " threw " + e.message); } };
const tree = { type: "label", text: "x" };
attempt("bad-type", () => opus.ui.openWindow({ tree: { type: "gauge" } }));
attempt("canvas", () => opus.ui.openWindow({ tree: { type: "canvas", width: 5, height: 5 } }));
attempt("no-tree", () => opus.ui.openWindow({ title: "t" }));
attempt("bad-onclose", () => opus.ui.openWindow({ tree, onClose: 5 }));
const handles = [];
for (let i = 0; i < 8; i++) handles.push(opus.ui.openWindow({ tree }));
attempt("ninth", () => opus.ui.openWindow({ tree }));
handles[0].close();
attempt("after-close", () => opus.ui.openWindow({ tree }));
handles[1].close();
handles[1].close();
opus.log("ids " + handles[0].id + " " + handles[1].id);
"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        let lines = logs(&events);
        let has = |needle: &str| lines.iter().any(|l| l.contains(needle));
        assert!(has("bad-type threw") && has("gauge"), "{lines:?}");
        assert!(has("canvas threw") && has("canvas is not supported"), "{lines:?}");
        assert!(has("no-tree threw"), "{lines:?}");
        assert!(has("bad-onclose threw"), "{lines:?}");
        assert!(has("ninth threw") && has("at most 8"), "{lines:?}");
        assert!(has("after-close ok"), "{lines:?}");
        assert!(has("ids w"), "ids are never reused: {lines:?}");
        let ui: Vec<String> = events.borrow().iter().filter_map(|e| match e { Event::Ui(v) => v["op"].as_str().map(str::to_string), _ => None }).collect();
        assert_eq!(ui.iter().filter(|op| *op == "openWindow").count(), 9, "{ui:?}");
        assert_eq!(ui.iter().filter(|op| *op == "closeWindow").count(), 2, "closing twice sends one close: {ui:?}");
    });
}
