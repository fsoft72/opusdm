use super::*;
use opusdm_ipc::frame::{recv, send};
use std::sync::Mutex;
use std::time::Duration;

/// Guards tests that mutate the process-wide `XDG_CONFIG_HOME` env var,
/// which libtest would otherwise race across threads.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn bind_temp() -> (tempfile::TempDir, UnixListener, PathBuf) {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("hub.sock");
    let l = UnixListener::bind(&path).unwrap();
    (td, l, path)
}

/// Settings/desktop-settings persistence now happens on its own thread
/// (see `PersistMsg`), so the file may not exist yet the instant the
/// owner loop emits its "changed" event; poll instead of a bare check.
fn wait_for_file(path: &std::path::Path) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        // `fs::write` truncates before it writes, so an empty file means
        // the writer is between the two steps: keep waiting.
        if let Ok(content) = std::fs::read_to_string(path) {
            if !content.is_empty() {
                return content;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("{path:?} was not written in time");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn connect(path: &std::path::Path) -> UnixStream {
    let s = UnixStream::connect(path).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s
}

#[test]
fn register_gets_welcome_and_focus_makes_source() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1000, initial_path: "/home/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert_eq!(welcome.source, None);
    assert!(welcome.dests.is_empty());
    let id_a = welcome.lister_id;

    send(&mut a, &ToHub::FocusGained).unwrap();
    let rc: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(rc, ToLister::RoleChanged { source, .. } if source == Some(id_a)));

    let mut saw_reg = false;
    while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(200)) {
        if matches!(ev, HubEvent::ListerRegistered { .. }) {
            saw_reg = true;
        }
    }
    assert!(saw_reg);
}

#[test]
fn ping_gets_pong_without_registering() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Ping).unwrap();
    assert!(matches!(recv::<_, ToLister>(&mut c).unwrap(), Some(ToLister::Pong)));
    drop(handle);
}

#[test]
fn two_listers_focus_toggles_source_dest() {
    let (_td, listener, path) = bind_temp();
    let _h = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    let mut b = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _wa: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _wb: Welcome = recv(&mut b).unwrap().unwrap();

    send(&mut a, &ToHub::FocusGained).unwrap();
    let _ra: ToLister = recv(&mut a).unwrap().unwrap();
    let _rb: ToLister = recv(&mut b).unwrap().unwrap();
    send(&mut b, &ToHub::FocusGained).unwrap();
    let rb2: ToLister = recv(&mut b).unwrap().unwrap();
    match rb2 {
        ToLister::RoleChanged { source, dests, .. } => {
            assert!(source.is_some());
            assert_eq!(dests.len(), 1);
            assert_ne!(Some(dests[0]), source);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_desktop_settings_emits_a_changed_event() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Keep desktop.json out of the developer's real config dir.
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));

    let mut c = connect(&path);
    let ds = DesktopSettings {
        icon_size: 5,
        ..DesktopSettings::default()
    };
    send(&mut c, &ToHub::SetDesktopSettings(ds)).unwrap();

    let mut got = None;
    while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(500)) {
        if let HubEvent::DesktopSettingsChanged(d) = ev {
            got = Some(d);
            break;
        }
    }
    let got = got.expect("DesktopSettingsChanged not emitted");
    assert_eq!(got.icon_size, opusdm_ipc::ICON_SIZE_RANGE.0);

    // desktop.json landed under the temp config dir.
    wait_for_file(&cfg.path().join("opusdm").join("desktop.json"));
}

#[test]
fn set_background_step_applies_and_persists_the_image() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let img_dir = tempfile::tempdir().unwrap();
    let img = img_dir.path().join("wall.png");
    std::fs::write(&img, b"not really a png").unwrap();

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();

    let mut args = std::collections::BTreeMap::new();
    args.insert("path".to_string(), img.to_string_lossy().into_owned());
    args.insert("fit".to_string(), "tile".to_string());
    let steps = vec![opusdm_ipc::function::FnStep::Internal {
        cmd: opusdm_ipc::function::InternalCmd::SetBackground,
        args: opusdm_ipc::function::InternalArgs(args),
    }];
    send(&mut c, &ToHub::RunFunction { steps }).unwrap();
    let q: ToLister = recv(&mut c).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));
    send(&mut c, &ToHub::SelectionReply { paths: vec![], cwd: img_dir.path().to_path_buf() }).unwrap();

    let mut got = None;
    while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(1000)) {
        if let HubEvent::ThemeChanged(t) = ev {
            got = Some(t);
            break;
        }
    }
    let theme = got.expect("ThemeChanged not emitted");
    assert_eq!(
        theme.background.entries,
        vec![opusdm_ipc::BackgroundEntry::Image {
            path: std::fs::canonicalize(&img).unwrap(),
            fit: opusdm_ipc::ImageFit::Tile,
        }]
    );
    wait_for_file(&cfg.path().join("opusdm").join("settings.json"));
}

/// Read events until `pick` returns `Some`, or `wait` passes without one.
fn next_matching<T>(handle: &ServerHandle, wait: Duration, pick: impl Fn(HubEvent) -> Option<T>) -> Option<T> {
    while let Ok(ev) = handle.evt_rx.recv_timeout(wait) {
        if let Some(v) = pick(ev) {
            return Some(v);
        }
    }
    None
}

#[test]
fn leave_out_from_a_lister_emits_left_out_add_for_existing_paths_only() {
    let td = tempfile::tempdir().unwrap();
    let file = td.path().join("real.txt");
    std::fs::write(&file, b"x").unwrap();
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);

    // A path that does not exist (e.g. inside an archive) is refused.
    send(&mut c, &ToHub::LeaveOut { paths: vec![td.path().join("archive.zip#inner/x")] }).unwrap();
    let none = next_matching(&handle, Duration::from_millis(300), |ev| match ev {
        HubEvent::LeftOutAdd(p) => Some(p),
        _ => None,
    });
    assert!(none.is_none(), "a missing path must not be left out");

    send(&mut c, &ToHub::LeaveOut { paths: vec![file.clone()] }).unwrap();
    let got = next_matching(&handle, Duration::from_millis(1000), |ev| match ev {
        HubEvent::LeftOutAdd(p) => Some(p),
        _ => None,
    });
    assert_eq!(got, Some(vec![file]));
}

#[test]
fn leave_out_and_put_away_steps_emit_hub_events() {
    let td = tempfile::tempdir().unwrap();
    let file = td.path().join("real.txt");
    std::fs::write(&file, b"x").unwrap();
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();

    for cmd in [opusdm_ipc::function::InternalCmd::LeaveOut, opusdm_ipc::function::InternalCmd::PutAway] {
        let steps = vec![opusdm_ipc::function::FnStep::Internal {
            cmd,
            args: opusdm_ipc::function::InternalArgs::default(),
        }];
        send(&mut c, &ToHub::RunFunction { steps }).unwrap();
        let q: ToLister = recv(&mut c).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection));
        send(&mut c, &ToHub::SelectionReply { paths: vec![file.clone()], cwd: td.path().to_path_buf() }).unwrap();
        let expect_add = cmd == opusdm_ipc::function::InternalCmd::LeaveOut;
        let got = next_matching(&handle, Duration::from_millis(1000), |ev| match ev {
            HubEvent::LeftOutAdd(p) if expect_add => Some(p),
            HubEvent::LeftOutRemove(p) if !expect_add => Some(p),
            _ => None,
        });
        assert_eq!(got, Some(vec![file.clone()]), "{cmd:?}");
        // Drain the function's own replies before the next run.
        loop {
            match recv::<_, ToLister>(&mut c) {
                Ok(Some(ToLister::FunctionDone)) => break,
                Ok(Some(_)) => continue,
                _ => panic!("no FunctionDone"),
            }
        }
    }
}

#[test]
fn set_settings_persists_and_emits_a_changed_event() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Keep settings.json out of the developer's real config dir.
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));

    let mut c = connect(&path);
    let mut settings = opusdm_ipc::Settings::default();
    settings.theme.icon_set = opusdm_ipc::IconSetId::Custom("breeze-dark".to_string());
    settings.theme.background.entries = vec![opusdm_ipc::BackgroundEntry::Gradient {
        start: "#000000".into(),
        end: "#ffffff".into(),
        angle: 900, // above GRADIENT_ANGLE_MAX, hub must clamp
    }];
    settings.appearance.monospace_list = true;
    send(&mut c, &ToHub::SetSettings(Box::new(settings.clone()))).unwrap();

    let mut got = None;
    while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(500)) {
        if let HubEvent::ThemeChanged(t) = ev {
            got = Some(t);
            break;
        }
    }
    let got = got.expect("ThemeChanged not emitted");
    assert_eq!(got.icon_set, opusdm_ipc::IconSetId::Custom("breeze-dark".to_string()));
    assert!(matches!(
        got.background.entries[0],
        opusdm_ipc::BackgroundEntry::Gradient { angle: opusdm_ipc::GRADIENT_ANGLE_MAX, .. }
    ));

    // settings.json landed under the temp config dir and carries appearance too.
    let written = wait_for_file(&cfg.path().join("opusdm").join("settings.json"));
    let back: opusdm_ipc::Settings = serde_json::from_str(&written).unwrap();
    assert!(back.appearance.monospace_list);
}

#[test]
fn goodbye_drops_lister_and_notifies_gtk() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 9, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _wa: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::Goodbye).unwrap();
    drop(a);
    let mut saw_gone = false;
    let mut saw_all_gone = false;
    while let Ok(ev) = handle.evt_rx.recv_timeout(Duration::from_millis(300)) {
        match ev {
            HubEvent::ListerGone(_) => saw_gone = true,
            HubEvent::AllListersGone => saw_all_gone = true,
            _ => {}
        }
    }
    assert!(saw_gone && saw_all_gone);
}

#[test]
fn run_function_queries_source_and_dest_then_executes() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));

    // SOURCE lister.
    let mut src = connect(&path);
    send(&mut src, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut src).unwrap().unwrap();

    // DEST lister.
    let mut dst = connect(&path);
    send(&mut dst, &ToHub::Register { pid: 2, initial_path: "/dst".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut dst).unwrap().unwrap();

    // `start_function_run` treats the RunFunction sender as SOURCE
    // directly (it never consults `roles.source()`), so `src` needs no
    // role at all here; it only reads `roles.dests()` for the DEST
    // connection, so `dst` needs an explicit DEST role. `SetRole`
    // (unlike `FocusGained`, which always makes the focused Lister
    // SOURCE and demotes the previous SOURCE to DEST - see
    // `two_listers_focus_toggles_source_dest` above) sets exactly the
    // role named, with no source required first.
    send(&mut dst, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
    // Drain the RoleChanged broadcast this triggers on both connections.
    let _r: ToLister = recv(&mut src).unwrap().unwrap();
    let _r: ToLister = recv(&mut dst).unwrap().unwrap();

    let steps = vec![opusdm_ipc::function::FnStep::External {
        argv: vec!["true".into()],
        cwd_source: true,
    }];
    send(&mut src, &ToHub::RunFunction { steps }).unwrap();

    // Hub must ask both listers for their selection.
    let q1: ToLister = recv(&mut src).unwrap().unwrap();
    assert!(matches!(q1, ToLister::QuerySelection));
    let q2: ToLister = recv(&mut dst).unwrap().unwrap();
    assert!(matches!(q2, ToLister::QuerySelection));

    // `cwd_source: true` on the step makes the executor's subprocess
    // really `current_dir()` into this path, so it must exist on disk
    // (unlike `initial_path` above, which the hub never uses as a
    // process cwd) - a real temp dir stands in for the Lister's
    // actual open directory.
    let real_cwd = std::env::temp_dir();
    send(&mut src, &ToHub::SelectionReply { paths: vec!["/src/a".into()], cwd: real_cwd.clone() }).unwrap();
    send(&mut dst, &ToHub::SelectionReply { paths: vec![], cwd: real_cwd }).unwrap();

    // Execution happens on a worker thread; poll src for the result.
    let mut done = false;
    for _ in 0..50 {
        match recv::<_, ToLister>(&mut src) {
            Ok(Some(ToLister::FunctionDone)) => {
                done = true;
                break;
            }
            Ok(Some(_)) => continue, // FunctionOutput / FunctionStepDone lines
            _ => break,
        }
    }
    assert!(done, "expected FunctionDone on the requesting (SOURCE) connection");
    let _ = handle;
}

/// SOURCE and DEST listers, DEST role set, ready for a function run.
fn source_and_dest(path: &std::path::Path) -> (UnixStream, UnixStream) {
    let mut src = connect(path);
    send(&mut src, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut src).unwrap().unwrap();
    let mut dst = connect(path);
    send(&mut dst, &ToHub::Register { pid: 2, initial_path: "/dst".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut dst).unwrap().unwrap();
    send(&mut dst, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
    let _r: ToLister = recv(&mut src).unwrap().unwrap();
    let _r: ToLister = recv(&mut dst).unwrap().unwrap();
    (src, dst)
}

#[test]
fn remote_jobs_are_handed_back_to_the_requester_with_the_destination_resolved() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut src, mut dst) = source_and_dest(&path);
    send(&mut src, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    recv_until(&mut src, |m| matches!(m, ToLister::RoleChanged { source: Some(_), .. }).then_some(()));
    send(&mut dst, &ToHub::PathChanged("sftp://me@host/in".into())).unwrap();
    // The hub reads each connection in order, so once it answers this the
    // path is recorded (the two Listers' streams race otherwise).
    send(&mut dst, &ToHub::QueryCaches).unwrap();
    recv_until(&mut dst, |m| matches!(m, ToLister::CacheList(_)).then_some(()));

    // Remote destination, local source: bounced with the DEST path.
    send(&mut src, &ToHub::RunJob { op: Op::Copy, srcs: vec!["/tmp/a".into()], dest_dir: None, options: Default::default() }).unwrap();
    let (op, srcs, dest) = recv_until(&mut src, |m| match m {
        ToLister::RemoteTransfer { op, srcs, dest_dir } => Some((op, srcs, dest_dir)),
        _ => None,
    });
    assert_eq!((op, srcs, dest), (Op::Copy, vec!["/tmp/a".into()], "sftp://me@host/in".into()));

    // Remote source with an explicit destination (Delete): bounced too.
    send(&mut src, &ToHub::RunJob { op: Op::Delete, srcs: vec!["sftp://me@host/x".into()], dest_dir: Some("sftp://me@host/".into()), options: Default::default() }).unwrap();
    let op = recv_until(&mut src, |m| match m {
        ToLister::RemoteTransfer { op, .. } => Some(op),
        _ => None,
    });
    assert_eq!(op, Op::Delete);
}

/// Run a Confirm step from one Lister and answer the question with `ok`.
/// Returns whether the function finished (`true`) or failed (`false`).
fn confirm_round_trip(ok: bool) -> bool {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    let mut args = std::collections::BTreeMap::new();
    args.insert("message".to_string(), "Go on?".to_string());
    let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Confirm, args: opusdm_ipc::function::InternalArgs(args) }];
    send(&mut a, &ToHub::RunFunction { steps }).unwrap();
    recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
    let (req_id, message) = recv_until(&mut a, |m| match m {
        ToLister::ConfirmPrompt { req_id, message } => Some((req_id, message)),
        _ => None,
    });
    assert_eq!(message, "Go on?");
    send(&mut a, &ToHub::ConfirmReply { req_id, ok }).unwrap();
    recv_until(&mut a, |m| match m {
        ToLister::FunctionDone => Some(true),
        ToLister::FunctionFailed { .. } => Some(false),
        _ => None,
    })
}

#[test]
fn a_confirm_step_waits_for_the_users_answer() {
    assert!(confirm_round_trip(true), "a yes lets the function finish");
    assert!(!confirm_round_trip(false), "a no stops it");
}

#[test]
fn a_run_from_a_tool_window_uses_the_source_lister_and_reports_to_the_tool() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    // The Manage Functions window talks to the hub without registering.
    let mut tool = connect(&path);
    let mut args = std::collections::BTreeMap::new();
    args.insert("message".to_string(), "Go on?".to_string());
    let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Confirm, args: opusdm_ipc::function::InternalArgs(args) }];
    send(&mut tool, &ToHub::RunFunction { steps }).unwrap();
    recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
    let req_id = recv_until(&mut tool, |m| match m {
        ToLister::ConfirmPrompt { req_id, .. } => Some(req_id),
        _ => None,
    });
    send(&mut tool, &ToHub::ConfirmReply { req_id, ok: true }).unwrap();
    recv_until(&mut tool, |m| matches!(m, ToLister::FunctionDone).then_some(()));
}

#[test]
fn a_run_from_a_tool_window_without_a_source_lister_fails() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut tool = connect(&path);
    let steps = vec![FnStep::External { argv: vec!["true".into()], cwd_source: true }];
    send(&mut tool, &ToHub::RunFunction { steps }).unwrap();
    let error = recv_until(&mut tool, |m| match m {
        ToLister::FunctionFailed { error, .. } => Some(error),
        ToLister::FunctionDone => panic!("the run must not start without a SOURCE"),
        _ => None,
    });
    assert_eq!(error, function_run::NO_SOURCE_ERROR);
}

/// A `Script` step running `code` inline, as the script editor's Run sends it.
fn inline_script(code: &str) -> Vec<FnStep> {
    let mut args = std::collections::BTreeMap::new();
    args.insert("script".to_string(), "inline".to_string());
    args.insert("code".to_string(), code.to_string());
    vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Script, args: opusdm_ipc::function::InternalArgs(args) }]
}

#[test]
fn a_script_from_a_tool_window_runs_without_a_source_lister() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut tool = connect(&path);
    send(&mut tool, &ToHub::RunFunction { steps: inline_script("opus.log('listers', opus.listers.length, opus.source.path === opus.home)") }).unwrap();
    let mut lines = Vec::new();
    recv_until(&mut tool, |m| match m {
        ToLister::FunctionOutput { line, .. } => {
            lines.push(line);
            None
        }
        ToLister::FunctionFailed { error, .. } => panic!("the script failed: {error}"),
        ToLister::FunctionDone => Some(()),
        _ => None,
    });
    assert!(lines.contains(&"listers 0 true".to_string()), "{lines:?}");
}

#[test]
fn a_script_asks_the_hub_for_the_listers_and_their_roles() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    let mut tool = connect(&path);
    send(&mut tool, &ToHub::RunFunction { steps: inline_script("for (const l of opus.listers) opus.log(l.role, l.path)") }).unwrap();
    recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
    let mut lines = Vec::new();
    recv_until(&mut tool, |m| match m {
        ToLister::FunctionOutput { line, .. } => {
            lines.push(line);
            None
        }
        ToLister::FunctionFailed { error, .. } => panic!("the script failed: {error}"),
        ToLister::FunctionDone => Some(()),
        _ => None,
    });
    assert!(lines.contains(&"source /a".to_string()), "{lines:?}");
}

#[test]
fn a_run_window_keeps_receiving_output_after_the_function_finished() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    let mut args = std::collections::BTreeMap::new();
    args.insert("command".to_string(), "sh -c 'sleep 0.3; echo late'".to_string());
    args.insert("window".to_string(), "true".to_string());
    let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Run, args: opusdm_ipc::function::InternalArgs(args) }];
    send(&mut a, &ToHub::RunFunction { steps }).unwrap();
    recv_until(&mut a, |m| matches!(m, ToLister::QuerySelection).then_some(()));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    // Collect everything up to the end of the output, in arrival order.
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..40 {
        let m: ToLister = recv(&mut a).unwrap().unwrap();
        match m {
            ToLister::OutputOpen { .. } => seen.push("open".into()),
            ToLister::OutputLine { line, .. } => seen.push(format!("line:{line}")),
            ToLister::OutputEnd { exit, .. } => {
                seen.push(format!("end:{exit:?}"));
                break;
            }
            ToLister::FunctionDone => seen.push("done".into()),
            _ => {}
        }
    }
    let pos = |s: &str| seen.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} missing in {seen:?}"));
    assert!(pos("open") < pos("done"), "{seen:?}");
    assert!(pos("done") < pos("line:late"), "the output outlives the function: {seen:?}");
    assert!(pos("line:late") < pos("end:Some(0)"), "{seen:?}");
}

#[test]
fn invalidate_reaches_every_lister() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut a, mut b) = source_and_dest(&path);
    send(&mut a, &ToHub::Invalidate("sftp://h/a".into())).unwrap();
    for lister in [&mut a, &mut b] {
        let p = recv_until(lister, |m| match m {
            ToLister::InvalidatePath(p) => Some(p),
            _ => None,
        });
        assert_eq!(p, std::path::PathBuf::from("sftp://h/a"));
    }
}

#[test]
fn function_copy_conflict_is_prompted_and_the_answer_is_applied() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut src, mut dst) = source_and_dest(&path);

    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let src_file = src_dir.path().join("a.txt");
    std::fs::write(&src_file, b"new").unwrap();
    std::fs::write(dst_dir.path().join("a.txt"), b"existing").unwrap();

    let steps = vec![FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Copy, args: Default::default() }];
    send(&mut src, &ToHub::RunFunction { steps }).unwrap();
    let _q: ToLister = recv(&mut src).unwrap().unwrap();
    let _q: ToLister = recv(&mut dst).unwrap().unwrap();
    send(&mut src, &ToHub::SelectionReply { paths: vec![src_file], cwd: src_dir.path().to_path_buf() }).unwrap();
    send(&mut dst, &ToHub::SelectionReply { paths: vec![], cwd: dst_dir.path().to_path_buf() }).unwrap();

    // The prompt goes to the Lister that started the run.
    let (mut req_id, mut done) = (None, false);
    for _ in 0..50 {
        match recv::<_, ToLister>(&mut src) {
            Ok(Some(ToLister::ConflictPrompt { req_id: id, path, .. })) => {
                assert!(path.ends_with("a.txt"));
                req_id = Some(id);
                send(&mut src, &ToHub::ConflictReply { req_id: id, resolution: opusdm_ipc::Resolution::Overwrite }).unwrap();
            }
            Ok(Some(ToLister::FunctionDone)) => {
                done = true;
                break;
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    assert!(req_id.is_some(), "expected a ConflictPrompt on the requesting connection");
    assert!(done, "expected FunctionDone after the answer");
    assert_eq!(std::fs::read(dst_dir.path().join("a.txt")).unwrap(), b"new");
}

#[test]
fn headless_function_copy_as_conflict_is_skipped_without_a_prompt() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"new").unwrap();
    std::fs::write(dir.path().join("b.txt"), b"existing").unwrap();
    let marker = dir.path().join("marker");

    // Copy a.txt over the existing b.txt (it works in place, so no DEST
    // is needed), then a marker step proving the run went on.
    let mut args = std::collections::BTreeMap::new();
    args.insert("new_name".to_string(), "b.txt".to_string());
    let mut f = opusdm_ipc::Function::new("copy-as-then-mark");
    f.steps = vec![
        FnStep::Internal {
            cmd: opusdm_ipc::function::InternalCmd::CopyAs,
            args: opusdm_ipc::function::InternalArgs(args),
        },
        FnStep::External { argv: vec!["sh".into(), "-c".into(), format!("echo x >> {}", marker.display())], cwd_source: true },
    ];
    opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

    handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
    send(&mut a, &ToHub::SelectionReply { paths: vec![dir.path().join("a.txt")], cwd: dir.path().to_path_buf() }).unwrap();

    wait_for_lines(&marker, 1);
    assert_eq!(std::fs::read(dir.path().join("b.txt")).unwrap(), b"existing");
}

#[test]
fn run_function_from_the_dest_lister_itself_does_not_wait_on_itself_twice() {
    // A Lister holding the DEST role can still be the one requesting a
    // run (e.g. it just pressed Test in its own Function Editor).
    // `start_function_run` must not pick it as its own `dest_cid` -
    // that would make `selection_reply`'s `if cid == source_cid` /
    // `else if Some(cid) == dest_cid` only ever take the first branch,
    // so `dest_sel` (and the whole run) could never complete.
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Dest)).unwrap();
    let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

    let steps = vec![opusdm_ipc::function::FnStep::External { argv: vec!["true".into()], cwd_source: true }];
    send(&mut a, &ToHub::RunFunction { steps }).unwrap();

    // Exactly one QuerySelection - not one that will never be answered
    // because it was sent to `a` for a `dest_cid` that is also `a`.
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));

    let real_cwd = std::env::temp_dir();
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: real_cwd }).unwrap();

    let mut done = false;
    for _ in 0..50 {
        match recv::<_, ToLister>(&mut a) {
            Ok(Some(ToLister::FunctionDone)) => {
                done = true;
                break;
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    assert!(done, "expected FunctionDone - the run must not wait forever on its own DEST selection");
    let _ = handle;
}

#[test]
fn set_bindings_sanitizes_persists_and_broadcasts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert_eq!(welcome.bindings, opusdm_ipc::Bindings::default());

    let dirty = opusdm_ipc::Bindings {
        hotkeys: vec![
            opusdm_ipc::bindings::Hotkey { key: "ctrl+U".into(), function: "f1".into(), global: false },
            opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f2".into(), global: false },
        ],
        ..opusdm_ipc::Bindings::default()
    };
    send(&mut a, &ToHub::SetBindings(dirty)).unwrap();

    let got: ToLister = recv(&mut a).unwrap().unwrap();
    let ToLister::BindingsUpdate(b) = got else { panic!("expected BindingsUpdate, got {got:?}") };
    assert_eq!(b.hotkeys.len(), 1, "the duplicate must be dropped, first wins");
    assert_eq!(b.hotkeys[0].key, "Ctrl+u");
    assert_eq!(b.hotkeys[0].function, "f1");

    let written = wait_for_file(&cfg.path().join("opusdm").join("hotkeys.json"));
    let back: Vec<opusdm_ipc::bindings::Hotkey> = serde_json::from_str(&written).unwrap();
    assert_eq!(back.len(), 1);
}

#[test]
fn set_bindings_from_an_unregistered_tool_connection_is_persisted() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut tool = connect(&path);
    let hotkeys = vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+Alt+u".into(), function: "bash:ls".into(), global: false }];
    send(&mut tool, &ToHub::SetBindings(opusdm_ipc::Bindings { hotkeys, ..opusdm_ipc::Bindings::default() })).unwrap();

    let written = wait_for_file(&cfg.path().join("opusdm").join("hotkeys.json"));
    let back: Vec<opusdm_ipc::bindings::Hotkey> = serde_json::from_str(&written).unwrap();
    assert_eq!(back.len(), 1);
}

#[test]
fn query_config_answers_an_unregistered_tool_with_the_bindings_and_functions_in_force() {
    let (_td, listener, path) = bind_temp();
    let hotkeys = vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+u".into(), function: "f".into(), global: false }];
    let functions = vec![opusdm_ipc::function::Function::new("in memory")];
    let opts = ServerOptions {
        initial_bindings: Some(opusdm_ipc::Bindings { hotkeys: hotkeys.clone(), ..opusdm_ipc::Bindings::default() }),
        initial_functions: Some(functions.clone()),
        ..ServerOptions::default()
    };
    let _handle = spawn_server_with(listener, Box::new(|_p| {}), opts);
    let mut tool = connect(&path);
    send(&mut tool, &ToHub::QueryConfig).unwrap();
    match recv::<_, ToLister>(&mut tool).unwrap() {
        Some(ToLister::BindingsUpdate(b)) => assert_eq!(b.hotkeys, hotkeys),
        other => panic!("expected BindingsUpdate, got {other:?}"),
    }
    match recv::<_, ToLister>(&mut tool).unwrap() {
        Some(ToLister::FunctionsUpdate(f)) => assert_eq!(f, functions),
        other => panic!("expected FunctionsUpdate, got {other:?}"),
    }
}

#[test]
fn set_functions_persists_and_broadcasts_to_every_lister() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert!(welcome.functions.is_empty());
    let mut b = connect(&path);
    send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _: Welcome = recv(&mut b).unwrap().unwrap();

    let f = opusdm_ipc::function::Function::new("Backup");
    send(&mut a, &ToHub::SetFunctions(vec![f.clone()])).unwrap();

    for conn in [&mut a, &mut b] {
        loop {
            let got: ToLister = recv(conn).unwrap().unwrap();
            if let ToLister::FunctionsUpdate(list) = got {
                assert_eq!(list, vec![f.clone()]);
                break;
            }
        }
    }
    let written = wait_for_file(&cfg.path().join("opusdm").join("functions.json"));
    let back: Vec<opusdm_ipc::function::Function> = serde_json::from_str(&written).unwrap();
    assert_eq!(back, vec![f]);
}

#[test]
fn set_formats_sanitizes_persists_and_broadcasts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert_eq!(welcome.formats, opusdm_ipc::FormatStore::default());

    let mut dirty = opusdm_ipc::FormatStore::default();
    dirty.upsert(
        "/tmp".into(),
        false,
        opusdm_ipc::ListerFormat {
            columns: vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Size],
            ..opusdm_ipc::ListerFormat::builtin()
        },
    );
    send(&mut a, &ToHub::SetFormats(dirty)).unwrap();

    let got: ToLister = recv(&mut a).unwrap().unwrap();
    let ToLister::FormatsUpdate(store) = got else { panic!("expected FormatsUpdate, got {got:?}") };
    assert_eq!(
        store.entries[0].format.columns,
        vec![opusdm_ipc::FormatColumn::Size, opusdm_ipc::FormatColumn::Name],
        "duplicates dropped and Name restored"
    );

    let written = wait_for_file(&cfg.path().join("opusdm").join("formats.json"));
    let back: opusdm_ipc::FormatStore = serde_json::from_str(&written).unwrap();
    assert_eq!(back, store);
}

#[test]
fn welcome_carries_the_formats_loaded_from_disk() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let mut on_disk = opusdm_ipc::FormatStore::default();
    on_disk.upsert(
        "/tmp".into(),
        true,
        opusdm_ipc::ListerFormat { hide_glob: "*.o".into(), ..opusdm_ipc::ListerFormat::builtin() },
    );
    opusdm_ipc::format::save_formats(&on_disk).unwrap();

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert_eq!(welcome.formats, on_disk);
}

/// Read messages from `s` until `pick` returns a value, panicking after 30
/// messages (role broadcasts may interleave with the reply we want).
fn recv_until<T>(s: &mut UnixStream, mut pick: impl FnMut(ToLister) -> Option<T>) -> T {
    for _ in 0..30 {
        let msg: ToLister = recv(s).unwrap().unwrap();
        if let Some(v) = pick(msg) {
            return v;
        }
    }
    panic!("the expected message never arrived");
}

/// Register a Lister and return its stream and id.
fn register(path: &std::path::Path, pid: u32, start: &str) -> (UnixStream, ListerId) {
    let mut s = connect(path);
    send(&mut s, &ToHub::Register { pid, initial_path: start.into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut s).unwrap().unwrap();
    (s, welcome.lister_id)
}

#[test]
fn query_caches_lists_every_listers_reported_paths() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut a, id_a) = register(&path, 1, "/a");
    let (mut b, id_b) = register(&path, 2, "/b");

    send(&mut a, &ToHub::CacheReport(vec!["/x".into()])).unwrap();
    send(&mut b, &ToHub::CacheReport(vec!["/y".into(), "/z".into()])).unwrap();
    // Connections are read by separate threads: b's own query is only
    // answered after its report was handled, so wait for that reply
    // before a asks.
    send(&mut b, &ToHub::QueryCaches).unwrap();
    recv_until(&mut b, |m| matches!(m, ToLister::CacheList(_)).then_some(()));
    send(&mut a, &ToHub::QueryCaches).unwrap();

    let entries = recv_until(&mut a, |m| match m {
        ToLister::CacheList(e) => Some(e),
        _ => None,
    });
    let got: Vec<(ListerId, String)> =
        entries.iter().map(|e| (e.lister, e.path.to_string_lossy().into_owned())).collect();
    assert_eq!(got, vec![(id_a, "/x".into()), (id_b, "/y".into()), (id_b, "/z".into())]);
}

#[test]
fn a_disconnected_listers_cached_paths_disappear() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut a, _id_a) = register(&path, 1, "/a");
    let (mut b, _id_b) = register(&path, 2, "/b");
    send(&mut b, &ToHub::CacheReport(vec!["/y".into()])).unwrap();
    // Make sure the hub processed b's report before b goes away: b's own
    // query is answered only after its report was handled.
    send(&mut b, &ToHub::QueryCaches).unwrap();
    recv_until(&mut b, |m| matches!(m, ToLister::CacheList(_)).then_some(()));
    send(&mut a, &ToHub::QueryCaches).unwrap();
    let first = recv_until(&mut a, |m| match m {
        ToLister::CacheList(e) => Some(e),
        _ => None,
    });
    assert_eq!(first.len(), 1);
    drop(b);

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        send(&mut a, &ToHub::QueryCaches).unwrap();
        let now = recv_until(&mut a, |m| match m {
            ToLister::CacheList(e) => Some(e),
            _ => None,
        });
        if now.is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "b's paths never disappeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn free_all_caches_reaches_every_lister() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut a, _) = register(&path, 1, "/a");
    let (mut b, _) = register(&path, 2, "/b");
    send(&mut a, &ToHub::FreeAllCaches).unwrap();
    recv_until(&mut a, |m| matches!(m, ToLister::FreeCaches).then_some(()));
    recv_until(&mut b, |m| matches!(m, ToLister::FreeCaches).then_some(()));
}

#[test]
fn jump_to_reaches_only_the_target_and_ignores_unknown_ids() {
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let (mut a, _id_a) = register(&path, 1, "/a");
    let (mut b, id_b) = register(&path, 2, "/b");

    send(&mut a, &ToHub::JumpTo { lister: ListerId(9999), path: "/nowhere".into() }).unwrap();
    send(&mut a, &ToHub::JumpTo { lister: id_b, path: "/target".into() }).unwrap();
    let goto = recv_until(&mut b, |m| match m {
        ToLister::GoTo { path } => Some(path),
        _ => None,
    });
    assert_eq!(goto, std::path::PathBuf::from("/target"));

    // a must not have been sent a GoTo: the next reply it reads is the
    // CacheList, with no GoTo before it.
    send(&mut a, &ToHub::QueryCaches).unwrap();
    recv_until(&mut a, |m| match m {
        ToLister::GoTo { .. } => panic!("a GoTo reached the wrong Lister"),
        ToLister::CacheList(_) => Some(()),
        _ => None,
    });
}

#[test]
fn welcome_carries_the_bindings_loaded_from_disk() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let on_disk = opusdm_ipc::Bindings {
        hotkeys: vec![opusdm_ipc::bindings::Hotkey { key: "Ctrl+j".into(), function: "f".into(), global: false }],
        ..opusdm_ipc::Bindings::default()
    };
    opusdm_ipc::bindings::save_bindings(&on_disk).unwrap();

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server_with(listener, Box::new(|_p| {}), ServerOptions::production());

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let welcome: Welcome = recv(&mut a).unwrap().unwrap();
    assert_eq!(welcome.bindings, on_disk);
}

/// A saved function whose only step appends a line to `marker`.
fn marker_function(name: &str, marker: &std::path::Path) -> opusdm_ipc::Function {
    let mut f = opusdm_ipc::Function::new(name);
    f.steps = vec![FnStep::External {
        argv: vec!["sh".into(), "-c".into(), format!("echo x >> {}", marker.display())],
        cwd_source: true,
    }];
    f
}

/// Poll until `marker` has at least `lines` lines, or panic.
fn wait_for_lines(marker: &std::path::Path, lines: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let count = std::fs::read_to_string(marker).map(|s| s.lines().count()).unwrap_or(0);
        if count >= lines {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{marker:?} has {count} lines, wanted {lines}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn run_function_by_id_with_an_unknown_id_reports_failure() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::RunFunctionById { id: "nope".into() }).unwrap();

    let got: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(got, ToLister::FunctionFailed { step_idx: 0, .. }), "got {got:?}");
}

#[test]
fn run_function_by_id_runs_a_saved_function() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("touch", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    send(&mut a, &ToHub::RunFunctionById { id: f.id.clone() }).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn run_function_by_id_runs_an_inline_bash_command() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    // `{a,b}` is bash brace expansion, not a placeholder: it must reach bash untouched.
    let id = format!("bash:echo {{a,b}} \"$((1+1))\" >> {}", marker.display());
    send(&mut a, &ToHub::RunFunctionById { id }).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    wait_for_lines(&marker, 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "a b 2\n");
}

#[test]
fn run_function_by_id_runs_an_inline_exec_line() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    let id = format!("exec:sh -c 'echo x >> {}'", marker.display());
    send(&mut a, &ToHub::RunFunctionById { id }).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn an_inline_command_that_cannot_run_reports_failure() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    send(&mut a, &ToHub::RunFunctionById { id: "exec:opusdm-no-such-program-xyz".into() }).unwrap();
    let got: ToLister = recv(&mut a).unwrap().unwrap();
    match got {
        ToLister::FunctionFailed { error, .. } => assert!(error.contains("was not found"), "{error}"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn headless_run_of_an_inline_command_works_with_no_lister_open() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, _path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let id = format!("bash:echo x >> {}", marker.display());
    handle.cmd_tx.send(HubCmd::RunFunctionHeadless(id)).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn a_bash_command_receives_the_selection_as_positional_parameters() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    // A file name that would run a command if it were pasted into the script.
    let hostile = out.path().join("a b;touch evil");
    let id = format!("bash:printf '%s\\n' \"$@\" >> {}", marker.display());
    send(&mut a, &ToHub::RunFunctionById { id }).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection));
    send(&mut a, &ToHub::SelectionReply { paths: vec![hostile.clone()], cwd: out.path().to_path_buf() }).unwrap();

    wait_for_lines(&marker, 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), format!("{}\n", hostile.display()));
    assert!(!out.path().join("evil").exists(), "the file name must not be run as a command");
}

#[test]
fn placeholders_are_not_expanded_inside_a_bash_script() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, _path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let id = format!("bash:echo '{{o}} ${{f}}' >> {}", marker.display());
    handle.cmd_tx.send(HubCmd::RunFunctionHeadless(id)).unwrap();

    wait_for_lines(&marker, 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "{o} ${f}\n");
}

#[test]
fn run_function_on_paths_gives_the_files_as_the_selection() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, _path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let id = format!("bash:echo \"$@\" >> {}", marker.display());
    handle.cmd_tx.send(HubCmd::RunFunctionOnPaths { id, paths: vec![out.path().join("a b.jpg")] }).unwrap();

    wait_for_lines(&marker, 1);
    let want = format!("{}\n", out.path().join("a b.jpg").display());
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), want);
}

#[test]
fn a_headless_bash_run_with_output_opens_its_window_on_the_source_lister() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

    handle.cmd_tx.send(HubCmd::RunFunctionHeadless("bash+out:echo hi".into())).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    for _ in 0..10 {
        let got: ToLister = recv(&mut a).unwrap().unwrap();
        if matches!(got, ToLister::OutputOpen { .. }) {
            return;
        }
    }
    panic!("no output window was opened on the SOURCE lister");
}

#[test]
fn headless_run_without_listers_executes_immediately() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("touch", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, _path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn headless_run_queries_the_source_lister() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("touch", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::SetRole(opusdm_ipc::Role::Source)).unwrap();
    let _r: ToLister = recv(&mut a).unwrap().unwrap(); // RoleChanged

    handle.cmd_tx.send(HubCmd::RunFunctionHeadless(f.id)).unwrap();
    let q: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
    send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn two_runs_waiting_on_one_lister_both_complete_in_order() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker_a = out.path().join("a");
    let marker_b = out.path().join("b");
    let fa = marker_function("a", &marker_a);
    let fb = marker_function("b", &marker_b);
    opusdm_ipc::function::save_functions(&[fa.clone(), fb.clone()]).unwrap();

    let (_td, listener, path) = bind_temp();
    let _handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    // Both requests reach the hub before the lister answers either.
    send(&mut a, &ToHub::RunFunctionById { id: fa.id.clone() }).unwrap();
    send(&mut a, &ToHub::RunFunctionById { id: fb.id.clone() }).unwrap();
    for _ in 0..2 {
        let q: ToLister = recv(&mut a).unwrap().unwrap();
        assert!(matches!(q, ToLister::QuerySelection), "got {q:?}");
    }
    for _ in 0..2 {
        send(&mut a, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
    }

    wait_for_lines(&marker_a, 1);
    wait_for_lines(&marker_b, 1);
}

/// Server options for an event test: these bindings, no disk read.
fn with_bindings(bindings: opusdm_ipc::Bindings) -> ServerOptions {
    ServerOptions { initial_bindings: Some(bindings), ..ServerOptions::default() }
}

fn script(event: opusdm_ipc::bindings::ScriptEvent, function: &str) -> opusdm_ipc::bindings::ScriptBinding {
    opusdm_ipc::bindings::ScriptBinding { event, function: function.into() }
}

#[test]
fn startup_event_runs_the_bound_function() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("hook", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, _path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Startup, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
    handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn an_event_runs_only_the_functions_bound_to_it() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("hook", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, _path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
    handle.cmd_tx.send(HubCmd::EmitScriptEvent(opusdm_ipc::bindings::ScriptEvent::Startup)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!marker.exists(), "a Startup event must not run a ListerClosed hook");
}

#[test]
fn lister_opened_runs_once_and_is_throttled_inside_the_cooldown() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("hook", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerOpened, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    wait_for_lines(&marker, 1);

    // A second lister right away is the loop a self-spawning hook would
    // create: it must be throttled, not run again.
    let mut b = connect(&path);
    send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut b).unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let lines = std::fs::read_to_string(&marker).unwrap().lines().count();
    assert_eq!(lines, 1, "the second ListerOpened inside the cooldown must be dropped");
}

#[test]
fn lister_closed_runs_the_bound_function() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("hook", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::ListerClosed, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let _handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    send(&mut a, &ToHub::Goodbye).unwrap();

    wait_for_lines(&marker, 1);
}

#[test]
fn shutdown_hooks_finish_before_the_listers_are_told_to_quit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    let f = marker_function("hook", &marker);
    opusdm_ipc::function::save_functions(std::slice::from_ref(&f)).unwrap();

    let (_td, listener, path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    handle.cmd_tx.send(HubCmd::Shutdown).unwrap();
    let got: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(got, ToLister::Shutdown), "got {got:?}");
    let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
    assert_eq!(lines, 1, "the Shutdown hook must have finished before the Shutdown message");
}

#[test]
fn a_shutdown_hook_can_be_an_inline_command() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");

    let (_td, listener, path) = bind_temp();
    let id = format!("bash:echo x >> {}", marker.display());
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &id)],
        ..opusdm_ipc::Bindings::default()
    };
    let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();

    handle.cmd_tx.send(HubCmd::Shutdown).unwrap();
    let got: ToLister = recv(&mut a).unwrap().unwrap();
    assert!(matches!(got, ToLister::Shutdown), "got {got:?}");
    let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
    assert_eq!(lines, 1, "the inline Shutdown hook must have run before the Shutdown message");
}

#[test]
fn shutdown_and_wait_acknowledges_only_after_the_hooks_finished() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());
    let marker = out.path().join("marker");
    // A hook that takes a moment: the ack must not arrive before it ends.
    let mut f = opusdm_ipc::Function::new("slow hook");
    f.steps = vec![FnStep::External {
        argv: vec!["sh".into(), "-c".into(), format!("sleep 0.3; echo x >> {}", marker.display())],
        cwd_source: true,
    }];
    opusdm_ipc::function::save_functions(&[f.clone()]).unwrap();

    let (_td, listener, _path) = bind_temp();
    let bindings = opusdm_ipc::Bindings {
        scripts: vec![script(opusdm_ipc::bindings::ScriptEvent::Shutdown, &f.id)],
        ..opusdm_ipc::Bindings::default()
    };
    let handle = spawn_server_with(listener, Box::new(|_p| {}), with_bindings(bindings));

    let (ack_tx, ack_rx) = channel();
    handle.cmd_tx.send(HubCmd::ShutdownAndWait(ack_tx)).unwrap();
    ack_rx.recv_timeout(Duration::from_secs(5)).expect("no acknowledgement");
    let lines = std::fs::read_to_string(&marker).map(|s| s.lines().count()).unwrap_or(0);
    assert_eq!(lines, 1, "the ack came before the Shutdown hook finished");
}

#[test]
fn set_hidden_tells_listers_and_the_gtk_side_once_and_toggle_flips_it() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();
    let hidden_event = |ev: HubEvent| match ev {
        HubEvent::HiddenChanged(h) => Some(h),
        _ => None,
    };

    handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
    assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
    loop {
        match recv::<_, ToLister>(&mut c).unwrap() {
            Some(ToLister::SetHidden(true)) => break,
            Some(_) => continue,
            None => panic!("connection closed before SetHidden(true)"),
        }
    }
    // Already hidden: no second event.
    handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
    assert_eq!(next_matching(&handle, Duration::from_millis(300), hidden_event), None);
    // Toggle reveals.
    handle.cmd_tx.send(HubCmd::ToggleHidden).unwrap();
    assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(false));
    loop {
        match recv::<_, ToLister>(&mut c).unwrap() {
            Some(ToLister::SetHidden(false)) => break,
            Some(_) => continue,
            None => panic!("connection closed before SetHidden(false)"),
        }
    }
}

#[test]
fn a_requested_lister_registering_while_hidden_reveals_everything() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
    // The user asked for a new Lister (desktop menu, "New Lister" button).
    handle.cmd_tx.send(HubCmd::SpawnRequested { start_path: None }).unwrap();
    let hidden_event = |ev: HubEvent| match ev {
        HubEvent::HiddenChanged(h) => Some(h),
        _ => None,
    };
    assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();
    assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(false));
}

#[test]
fn hide_reveal_and_toggle_steps_drive_the_hidden_state() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();
    let hidden_event = |ev: HubEvent| match ev {
        HubEvent::HiddenChanged(h) => Some(h),
        _ => None,
    };
    for (cmd, expected) in [
        (opusdm_ipc::function::InternalCmd::Hide, true),
        (opusdm_ipc::function::InternalCmd::ToggleHide, false),
        (opusdm_ipc::function::InternalCmd::ToggleHide, true),
        (opusdm_ipc::function::InternalCmd::Reveal, false),
    ] {
        let steps = vec![opusdm_ipc::function::FnStep::Internal {
            cmd,
            args: opusdm_ipc::function::InternalArgs::default(),
        }];
        send(&mut c, &ToHub::RunFunction { steps }).unwrap();
        let q = loop {
            match recv::<_, ToLister>(&mut c).unwrap() {
                Some(ToLister::QuerySelection) => break true,
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(q, "no QuerySelection for {cmd:?}");
        send(&mut c, &ToHub::SelectionReply { paths: vec![], cwd: std::env::temp_dir() }).unwrap();
        assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(expected), "{cmd:?}");
        loop {
            match recv::<_, ToLister>(&mut c).unwrap() {
                Some(ToLister::FunctionDone) => break,
                Some(_) => continue,
                None => panic!("no FunctionDone"),
            }
        }
    }
}

#[test]
fn a_lister_nobody_asked_for_registering_while_hidden_is_told_to_hide_too() {
    // The initial Lister of a Startup script that hides the desktop.
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
    let hidden_event = |ev: HubEvent| match ev {
        HubEvent::HiddenChanged(h) => Some(h),
        _ => None,
    };
    assert_eq!(next_matching(&handle, Duration::from_millis(1000), hidden_event), Some(true));
    let mut c = connect(&path);
    send(&mut c, &ToHub::Register { pid: 1, initial_path: "/src".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut c).unwrap().unwrap();
    loop {
        match recv::<_, ToLister>(&mut c).unwrap() {
            Some(ToLister::SetHidden(true)) => break,
            Some(_) => continue,
            None => panic!("the new Lister was not told to hide"),
        }
    }
    assert_eq!(
        next_matching(&handle, Duration::from_millis(300), hidden_event),
        None,
        "an unrequested Lister must not reveal the desktop"
    );
}

#[test]
fn reveal_presents_only_the_source_lister_after_telling_everyone() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut a = connect(&path);
    send(&mut a, &ToHub::Register { pid: 1, initial_path: "/a".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut a).unwrap().unwrap();
    let mut b = connect(&path);
    send(&mut b, &ToHub::Register { pid: 2, initial_path: "/b".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _w: Welcome = recv(&mut b).unwrap().unwrap();
    send(&mut a, &ToHub::FocusGained).unwrap();
    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::RoleChanged { .. }) => break,
            Some(_) => continue,
            None => panic!("no RoleChanged"),
        }
    }
    handle.cmd_tx.send(HubCmd::SetHidden(true)).unwrap();
    handle.cmd_tx.send(HubCmd::SetHidden(false)).unwrap();
    // SOURCE Lister a: SetHidden(true), SetHidden(false), then Present.
    let mut seen_reveal = false;
    loop {
        match recv::<_, ToLister>(&mut a).unwrap() {
            Some(ToLister::SetHidden(false)) => seen_reveal = true,
            Some(ToLister::Present) => {
                assert!(seen_reveal, "Present must come after SetHidden(false)");
                break;
            }
            Some(_) => continue,
            None => panic!("the SOURCE Lister never got Present"),
        }
    }
    // Lister b (not SOURCE) is told to reveal but never asked to present.
    let mut got_reveal = false;
    b.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    while let Ok(Some(msg)) = recv::<_, ToLister>(&mut b) {
        match msg {
            ToLister::SetHidden(false) => got_reveal = true,
            ToLister::Present => panic!("a non-SOURCE Lister must not be presented"),
            _ => {}
        }
    }
    assert!(got_reveal);
}

#[test]
fn an_unchanged_apply_emits_no_event() {
    let _guard = ENV_LOCK.lock().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", cfg.path());

    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut c = connect(&path);

    let mut settings = opusdm_ipc::Settings::default();
    settings.appearance.monospace_list = true;
    let desktop = DesktopSettings { icon_size: 100, ..DesktopSettings::default() };
    send(&mut c, &ToHub::SetSettings(Box::new(settings.clone()))).unwrap();
    send(&mut c, &ToHub::SetDesktopSettings(desktop.clone())).unwrap();
    let first = next_matching(&handle, Duration::from_millis(500), |ev| match ev {
        HubEvent::DesktopSettingsChanged(_) => Some(()),
        _ => None,
    });
    assert!(first.is_some(), "the first apply must be announced");

    // Drain what the first apply produced, then repeat it unchanged.
    while handle.evt_rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
    send(&mut c, &ToHub::SetSettings(Box::new(settings))).unwrap();
    send(&mut c, &ToHub::SetDesktopSettings(desktop)).unwrap();
    let again = next_matching(&handle, Duration::from_millis(500), |ev| match ev {
        HubEvent::ThemeChanged(_) | HubEvent::AppearanceChanged(_) | HubEvent::DesktopSettingsChanged(_) => Some(()),
        _ => None,
    });
    assert!(again.is_none(), "an identical apply must not reach the GTK side");
}

#[test]
fn a_forwarder_wraps_every_message_for_the_owner_loop() {
    let (tx, rx) = channel::<(u64, Progress)>();
    let (owner_tx, owner_rx) = channel::<OwnerMsg>();
    spawn_forwarder("test-forwarder", rx, owner_tx, |(id, ev)| OwnerMsg::JobProgress(id, ev));
    for id in [3, 4] {
        tx.send((id, Progress::Done { copied: 0, skipped: 0, cancelled: false, errors: Vec::new() })).unwrap();
    }
    drop(tx);
    let ids: Vec<u64> = owner_rx
        .iter()
        .map(|msg| match msg {
            OwnerMsg::JobProgress(id, _) => id,
            _ => panic!("unexpected message"),
        })
        .collect();
    assert_eq!(ids, vec![3, 4], "in order, and the thread ends with its input");
}

#[test]
fn accept_errors_are_classified_by_errno() {
    let os = std::io::Error::from_raw_os_error;
    assert_eq!(classify_accept_error(&os(libc::ECONNABORTED)), AcceptErrorAction::Retry);
    assert_eq!(classify_accept_error(&os(libc::EINTR)), AcceptErrorAction::Retry);
    assert_eq!(classify_accept_error(&os(libc::EMFILE)), AcceptErrorAction::Backoff);
    assert_eq!(classify_accept_error(&os(libc::ENFILE)), AcceptErrorAction::Backoff);
    assert_eq!(classify_accept_error(&os(libc::EBADF)), AcceptErrorAction::Stop);
    assert_eq!(classify_accept_error(&os(libc::EINVAL)), AcceptErrorAction::Stop);
}

#[test]
fn accept_loop_keeps_accepting_after_a_temporary_error() {
    let (a, _b) = UnixStream::pair().unwrap();
    let mut script: std::collections::VecDeque<std::io::Result<UnixStream>> = vec![
        Err(std::io::Error::from_raw_os_error(libc::ECONNABORTED)),
        Err(std::io::Error::from_raw_os_error(libc::EMFILE)),
        Ok(a),
        Err(std::io::Error::from_raw_os_error(libc::EBADF)),
    ]
    .into();
    let (tx, rx) = channel::<OwnerMsg>();
    accept_loop(|| script.pop_front().expect("the loop must stop on EBADF"), &tx, Duration::from_millis(1));
    assert!(script.is_empty(), "the loop went on until the fatal error");
    let accepted = rx.try_iter().filter(|m| matches!(m, OwnerMsg::Accepted(_))).count();
    assert_eq!(accepted, 1, "the client after the temporary errors was accepted");
}

#[test]
fn accept_loop_stops_when_the_owner_loop_is_gone() {
    let (tx, rx) = channel::<OwnerMsg>();
    drop(rx);
    let mut calls = 0;
    accept_loop(
        || {
            calls += 1;
            Ok(UnixStream::pair().unwrap().0)
        },
        &tx,
        Duration::from_millis(1),
    );
    assert_eq!(calls, 1);
}

#[test]
fn a_lister_that_stops_reading_is_disconnected_instead_of_queueing_forever() {
    let (_td, listener, path) = bind_temp();
    let handle = spawn_server(listener, Box::new(|_p| {}));
    let mut stalled = connect(&path);
    send(&mut stalled, &ToHub::Register { pid: 1, initial_path: "/s".into(), version: opusdm_ipc::PROTOCOL_VERSION }).unwrap();
    let _welcome: Welcome = recv(&mut stalled).unwrap().unwrap();
    // An unregistered peer gets no broadcasts, so it can flood without reading.
    let mut flooder = connect(&path);
    let long = PathBuf::from("/x".repeat(500));
    for _ in 0..(WRITER_QUEUE * 2) {
        send(&mut flooder, &ToHub::Invalidate(long.clone())).unwrap();
    }
    let gone = next_matching(&handle, Duration::from_secs(5), |ev| matches!(ev, HubEvent::ListerGone(_)).then_some(()));
    assert!(gone.is_some(), "the stalled Lister was not disconnected");
}

#[test]
fn a_full_writer_queue_shuts_the_connection_down() {
    let (hub_side, mut peer) = UnixStream::pair().unwrap();
    let (tx, _rx) = sync_channel::<OutMsg>(1);
    let conn = Conn { tx, shutdown: hub_side, registered: true, pid: 0, path: PathBuf::new(), cache_paths: Vec::new() };
    assert!(conn.queue(1, ToLister::Pong));
    assert!(!conn.queue(1, ToLister::Pong), "the queue is full");
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert!(recv::<_, ToLister>(&mut peer).unwrap().is_none(), "the peer sees the end of the connection");
}

#[test]
fn a_broadcast_is_encoded_once_and_written_as_a_normal_frame() {
    let (hub_side, mut peer) = UnixStream::pair().unwrap();
    let (tx, rx) = sync_channel::<OutMsg>(4);
    let mut conns = HashMap::new();
    conns.insert(1, Conn { tx, shutdown: hub_side.try_clone().unwrap(), registered: true, pid: 0, path: PathBuf::new(), cache_paths: Vec::new() });
    broadcast(&conns, &ToLister::InvalidatePath("/a".into()));
    drop(conns);
    assert!(matches!(rx.try_recv(), Ok(OutMsg::Encoded(_))));
    let (tx, rx) = sync_channel::<OutMsg>(4);
    tx.send(OutMsg::Encoded(serde_json::to_vec(&ToLister::InvalidatePath("/b".into())).unwrap().into())).unwrap();
    drop(tx);
    writer_loop(hub_side, rx);
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert!(matches!(recv::<_, ToLister>(&mut peer).unwrap(), Some(ToLister::InvalidatePath(p)) if p == std::path::Path::new("/b")));
}

#[test]
fn resolve_steps_finds_saved_functions_and_rejects_missing_or_invalid_ones() {
    use opusdm_ipc::function::{Function, InternalArgs, InternalCmd};
    let mut good = Function::new("Good");
    good.steps.push(FnStep::External { argv: vec!["true".into()], cwd_source: true });
    let mut bad = Function::new("Bad");
    bad.steps.push(FnStep::Internal { cmd: InternalCmd::Confirm, args: InternalArgs::default() });
    let functions = vec![good.clone(), bad.clone()];
    assert_eq!(function_run::resolve_steps(&functions, &good.id, false), Ok(good.steps.clone()));
    assert!(function_run::resolve_steps(&functions, &bad.id, false).unwrap_err().contains("is invalid"));
    let missing = opusdm_ipc::function::Function::new("Gone").id;
    assert!(function_run::resolve_steps(&functions, &missing, false).unwrap_err().contains("not found"));
}

#[test]
fn resolve_steps_waits_for_an_inline_command_only_when_asked() {
    let waits = |steps: Vec<FnStep>| matches!(&steps[..], [FnStep::Internal { args, .. }] if args.flag("wait"));
    assert!(!waits(function_run::resolve_steps(&[], "exec:true", false).unwrap()));
    assert!(waits(function_run::resolve_steps(&[], "exec:true", true).unwrap()));
}

#[test]
fn an_unknown_message_is_a_decode_error_and_ends_the_connection() {
    let mut frame = Vec::new();
    opusdm_ipc::frame::write_frame(&mut frame, br#"{"FromTheFuture":{}}"#).unwrap();
    let err = recv::<_, ToHub>(&mut std::io::Cursor::new(frame.clone())).unwrap_err();
    assert!(is_decode_error(&err), "{err:?}");
    assert!(!is_decode_error(&std::io::Error::from(std::io::ErrorKind::ConnectionReset)));

    let (mut peer, hub_side) = UnixStream::pair().unwrap();
    std::io::Write::write_all(&mut peer, &frame).unwrap();
    let (tx, rx) = channel::<OwnerMsg>();
    reader_loop(7, hub_side, tx);
    assert!(matches!(rx.try_recv(), Ok(OwnerMsg::ListerEof(7))));
}
