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
    let ctx = MainContext::ref_thread_default();
    let end = Instant::now() + WAIT;
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
        let started = Instant::now();
        let code = r#"setTimeout(() => { while (true) {} }, 0); setTimeout(() => opus.log("alive"), 20)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_log(&events, "alive")), "{:?}", events.borrow());
        assert!(has_error(&events, "loop"), "{:?}", events.borrow());
        assert!(started.elapsed() < WAIT, "took {:?}", started.elapsed());
    });
}

#[test]
fn a_runaway_loop_cannot_be_caught_by_the_script() {
    on_main_loop(|| {
        let events = Events::default();
        let code = r#"setTimeout(() => { try { while (true) {} } catch (e) { opus.log("caught") } }, 0)"#;
        let (_dir, rt) = start_code(code, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_error(&events, "loop")), "{:?}", events.borrow());
        assert!(!has_log(&events, "caught"));
    });
}

#[test]
fn a_deep_recursion_is_cut_and_reported() {
    on_main_loop(|| {
        let events = Events::default();
        let (_dir, rt) = start_code(r#"function f() { f() } setTimeout(f, 0)"#, Map::new(), &events);
        let _rt = rt.unwrap();
        assert!(pump_until(|| has_error(&events, "recursive")), "{:?}", events.borrow());
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
        assert!(events.borrow().is_empty(), "the default hook does nothing: {:?}", events.borrow());
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
