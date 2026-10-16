use super::*;
use crate::scripting::{QueryReply, ScriptQuery};
use std::sync::mpsc::Receiver;

fn ctx(paths: &[&Path], cwd: &Path) -> SelectionCtx {
    SelectionCtx { paths: paths.iter().map(|p| p.to_path_buf()).collect(), cwd: cwd.to_path_buf() }
}

/// What a test script run produced.
struct Run {
    result: Result<(), String>,
    /// The `Output` lines, without the leading `script <name>`.
    log: Vec<String>,
    /// Every query the script asked.
    queries: Vec<ScriptQuery>,
    /// The SOURCE after the run.
    source: SelectionCtx,
}

/// Answers the hub would give in these tests.
struct Hub {
    listers: Value,
    functions: Value,
    confirm: bool,
}

impl Default for Hub {
    fn default() -> Self {
        Hub { listers: json!([]), functions: json!([]), confirm: true }
    }
}

fn answer(hub: &Hub, query: &ScriptQuery) -> QueryReply {
    match query {
        ScriptQuery::Listers => Ok(hub.listers.clone()),
        ScriptQuery::Functions => Ok(hub.functions.clone()),
        ScriptQuery::GoTo { path, .. } => Ok(json!({ "path": path.display().to_string() })),
        ScriptQuery::Refresh { .. } => Ok(Value::Null),
    }
}

/// Run `code` as script `name` on `source` / `dest`, playing the hub.
fn run_with(code: &str, source: SelectionCtx, dest: Option<SelectionCtx>, hub: Hub, cancel: CancelToken) -> Run {
    let (tx, rx): (Sender<(u64, FnEvent)>, Receiver<(u64, FnEvent)>) = channel();
    let mut args = InternalArgs::default();
    args.0.insert("script".into(), "test".into());
    args.0.insert("code".into(), code.into());
    args.0.insert("args".into(), "{o}".into());
    let worker = std::thread::spawn(move || {
        let mut source = source;
        let result = run_script(7, 0, &args, &mut source, dest.as_ref(), &tx, &cancel);
        (result, source)
    });
    let mut log = Vec::new();
    let mut queries = Vec::new();
    for (_, event) in rx {
        match event {
            FnEvent::Output { line, .. } => log.push(line),
            FnEvent::ScriptQuery { query, reply } => {
                let _ = reply.send(answer(&hub, &query));
                queries.push(query);
            }
            FnEvent::Confirm { reply, .. } => {
                let _ = reply.send(hub.confirm);
            }
            FnEvent::Alert { reply, .. } => {
                let _ = reply.send(true);
            }
            FnEvent::Job { spec, done, .. } => {
                let _ = done.send(crate::functions::run_job_inline(spec, &CancelToken::new(), |_, _, _, _| {}));
            }
            _ => {}
        }
    }
    let (result, source) = worker.join().expect("the script thread does not panic");
    if log.first().is_some_and(|l| l == "script test") {
        log.remove(0);
    }
    Run { result, log, queries, source }
}

fn run(code: &str, source: SelectionCtx) -> Run {
    run_with(code, source, None, Hub::default(), CancelToken::new())
}

#[test]
fn log_and_console_print_lines() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("opus.log('a', 1, {x: true}); console.log('b\\nc'); opus.print([1,2]);", ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["a 1 {\"x\":true}", "b", "c", "[1,2]"]);
}

#[test]
fn the_script_sees_its_source_selection_and_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("one.txt");
    std::fs::write(&file, "1").unwrap();
    let code = "const s = opus.source; opus.log(s.path); opus.log(s.selectedNames.join(',')); opus.log(opus.args); opus.log(String(opus.dest)); opus.log(opus.scriptName);";
    let r = run(code, ctx(&[&file], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, [dir.path().display().to_string(), "one.txt".into(), "one.txt".into(), "null".into(), "test".into()]);
}

#[test]
fn a_thrown_error_fails_the_step_with_its_message() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("throw new TypeError('boom')", ctx(&[], dir.path()));
    let error = r.result.unwrap_err();
    assert!(error.contains("TypeError") && error.contains("boom"), "{error}");
    let r = run("throw 'plain'", ctx(&[], dir.path()));
    assert!(r.result.unwrap_err().contains("plain"));
}

#[test]
fn a_syntax_error_fails_the_step() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("let x = ;", ctx(&[], dir.path()));
    let error = r.result.unwrap_err();
    assert!(error.starts_with("script \"test\":"), "{error}");
    assert!(error.to_lowercase().contains("syntax") || error.contains("line"), "{error}");
}

#[test]
fn command_runs_an_internal_command_in_the_source_folder() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("opus.command('MakeDir', { name: 'made' }); opus.command('New File', { name: 'new.txt' });", ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert!(dir.path().join("made").is_dir());
    assert!(dir.path().join("new.txt").is_file());
}

#[test]
fn command_rejects_unknown_commands_and_parameters_catchably() {
    let dir = tempfile::tempdir().unwrap();
    let code = "\
        try { opus.command('Frobnicate'); } catch (e) { opus.log(e.message); }\n\
        try { opus.command('MakeDir', { nome: 'x' }); } catch (e) { opus.log(e.message); }\n\
        try { opus.command('MakeDir'); } catch (e) { opus.log(e.message); }";
    let r = run(code, ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log.len(), 3, "{:?}", r.log);
    assert!(r.log[0].contains("unknown command"), "{}", r.log[0]);
    assert!(r.log[1].contains("unknown parameter \"nome\"") && r.log[1].contains("name"), "{}", r.log[1]);
    assert!(r.log[2].contains("missing required parameter"), "{}", r.log[2]);
}

#[test]
fn copy_takes_files_and_to_in_place_of_the_listers() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), "A").unwrap();
    std::fs::write(src.path().join("b.txt"), "B").unwrap();
    let code = format!("opus.command('Copy', {{ files: ['a.txt'], to: {:?} }});", dst.path().display().to_string());
    let r = run(&code, ctx(&[], src.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(std::fs::read_to_string(dst.path().join("a.txt")).unwrap(), "A");
    assert!(!dst.path().join("b.txt").exists());
    assert!(r.source.paths.is_empty(), "files only stand in for one command");
}

#[test]
fn source_go_moves_the_folder_of_later_commands_and_of_later_steps() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let code = "opus.source.go('sub'); opus.log(opus.source.path); opus.command('MakeDir', { name: 'inner' });";
    let r = run(code, ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    let sub = std::fs::canonicalize(dir.path().join("sub")).unwrap();
    assert!(r.log.contains(&sub.display().to_string()), "{:?}", r.log);
    assert!(sub.join("inner").is_dir());
    assert_eq!(r.source.cwd, sub);
}

#[test]
fn listers_come_from_the_hub_and_dest_go_targets_it_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub {
        listers: json!([{ "id": 3, "role": "source", "path": "/a" }, { "id": 5, "role": "dest", "path": "/b" }]),
        ..Hub::default()
    };
    let dest = SelectionCtx { paths: Vec::new(), cwd: "/b".into() };
    let target = dir.path().display().to_string();
    let code = format!("const l = opus.listers; opus.log(l.length, l[0].role, l[1].id, l[1].path); opus.dest.go({target:?}); ");
    let r = run_with(&code, ctx(&[], dir.path()), Some(dest), hub, CancelToken::new());
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["2 source 5 /b"]);
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert!(r.queries.contains(&ScriptQuery::GoTo { lister: 5, path: canonical }), "{:?}", r.queries);
}

#[test]
fn fs_reads_writes_lists_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let code = "\
        opus.fs.write('notes.txt', 'one');\n\
        opus.fs.append('notes.txt', '+two');\n\
        opus.fs.mkdir('d/e');\n\
        opus.log(opus.fs.read('notes.txt'));\n\
        opus.log(opus.fs.list('.').map(e => e.name + (e.isDir ? '/' : '')).join(' '));\n\
        opus.log(opus.fs.list('.', '*.txt').length);\n\
        opus.log(opus.fs.exists('nope'), opus.fs.isDir('d'), opus.fs.isFile('notes.txt'), opus.fs.stat('notes.txt').size);\n\
        opus.log(opus.path.join('/a', 'b/../c', 'x.tar.gz'), opus.path.extname('x.tar.gz'), opus.path.dirname('/a/b'));";
    let r = run(code, ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["one+two", "d/ notes.txt", "1", "false true true 7", "/a/c/x.tar.gz .gz /a"]);
}

#[test]
fn exec_captures_the_output_of_a_program() {
    let dir = tempfile::tempdir().unwrap();
    let code = "const r = opus.exec('sh', ['-c', 'echo out; echo err >&2; exit 3']); opus.log(r.code, r.stdout.trim(), r.stderr.trim()); opus.log(opus.shell('cat', { input: 'fed' }).stdout);";
    let r = run(code, ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["3 out err", "fed"]);
}

#[test]
fn confirm_returns_the_answer_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub { confirm: false, ..Hub::default() };
    let r = run_with("opus.log(opus.confirm('sure?'))", ctx(&[], dir.path()), None, hub, CancelToken::new());
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["false"]);
}

#[test]
fn alert_waits_for_the_dialog_and_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("opus.alert('careful'); opus.log('after')", ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["after"]);
}

#[test]
fn notify_needs_a_title_and_a_known_urgency() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("opus.notify('')", ctx(&[], dir.path()));
    assert!(r.result.unwrap_err().contains("no title"));
    let r = run("opus.notify('hi', { urgency: 'loud' })", ctx(&[], dir.path()));
    assert!(r.result.unwrap_err().contains("urgency"));
}

#[test]
fn run_function_runs_a_saved_function_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut function = opusdm_ipc::function::Function::new("Make It");
    let mut args = InternalArgs::default();
    args.0.insert("name".into(), "from-function".into());
    function.steps.push(FnStep::Internal { cmd: InternalCmd::MakeDir, args });
    let hub = Hub { functions: serde_json::to_value(vec![function]).unwrap(), ..Hub::default() };
    let code = "opus.log(opus.functions.map(f => f.name).join()); opus.runFunction('make it'); try { opus.runFunction('nope'); } catch (e) { opus.log(e.message); }";
    let r = run_with(code, ctx(&[], dir.path()), None, hub, CancelToken::new());
    assert_eq!(r.result, Ok(()));
    assert!(dir.path().join("from-function").is_dir());
    assert_eq!(r.log[0], "Make It");
    assert!(r.log.last().unwrap().contains("no Function called \"nope\""), "{:?}", r.log);
}

#[test]
fn commands_lists_every_internal_command() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("const c = opus.commands; opus.log(c.length, c.find(x => x.name === 'CopyAs').params[0].key);", ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, [format!("{} new_name", InternalCmd::ALL.len())]);
}

#[test]
fn a_cancelled_run_fails_at_the_next_call() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let r = run_with("opus.log('never')", ctx(&[], dir.path()), None, Hub::default(), cancel);
    assert!(r.result.unwrap_err().contains("cancelled"));
}

/// Run `code`, cancel it after 200 ms and return its result and how long it
/// took. Dialogs are never answered (their replies are kept alive).
fn run_cancelled_mid_call(code: &str) -> (Result<(), String>, std::time::Duration) {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let (tx, rx) = channel::<(u64, FnEvent)>();
    let mut args = InternalArgs::default();
    args.0.insert("script".into(), "test".into());
    args.0.insert("code".into(), code.into());
    let started = std::time::Instant::now();
    let worker = {
        let (cancel, mut source) = (cancel.clone(), ctx(&[], dir.path()));
        std::thread::spawn(move || run_script(7, 0, &args, &mut source, None, &tx, &cancel))
    };
    std::thread::sleep(Duration::from_millis(200));
    cancel.cancel();
    let mut unanswered = Vec::new();
    // Bounded, so a step that ignores the cancel fails the test instead of hanging it.
    while let Ok((_, event)) = rx.recv_timeout(Duration::from_secs(10)) {
        if let FnEvent::Confirm { reply, .. } | FnEvent::Alert { reply, .. } = event {
            unanswered.push(reply);
        }
    }
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "the script was not stopped: {took:?}");
    (worker.join().expect("the script thread does not panic"), took)
}

#[test]
fn cancelling_stops_a_running_exec() {
    let (result, took) = run_cancelled_mid_call("opus.exec('sleep', ['30'])");
    assert!(took < Duration::from_secs(10), "exec was not stopped: {took:?}");
    assert!(result.unwrap_err().contains("cancelled"));
}

#[test]
fn cancelling_stops_a_confirm_or_alert_nobody_answers() {
    for code in ["opus.confirm('sure?')", "opus.alert('careful')"] {
        let (result, took) = run_cancelled_mid_call(code);
        assert!(took < Duration::from_secs(10), "{code} was not stopped: {took:?}");
        assert!(result.unwrap_err().contains("cancelled"), "{code}");
    }
}

#[test]
fn every_call_of_the_prelude_has_an_op() {
    const CALL: &str = "call(\"";
    let called: Vec<&str> = PRELUDE.match_indices(CALL).filter_map(|(at, _)| PRELUDE[at + CALL.len()..].split('"').next()).collect();
    assert!(called.len() >= OPS.len(), "the prelude calls were not found: {called:?}");
    for op in called {
        assert!(OPS.iter().any(|(name, _)| *name == op), "the prelude calls \"{op}\" but OPS has no entry for it");
    }
}

#[test]
fn op_names_are_unique() {
    for (i, (name, _)) in OPS.iter().enumerate() {
        assert!(!OPS[i + 1..].iter().any(|(other, _)| other == name), "\"{name}\" is in OPS twice");
    }
}

#[test]
fn scripts_cannot_nest_forever() {
    std::thread::spawn(|| {
        let (tx, _rx) = channel();
        let source = SelectionCtx { paths: Vec::new(), cwd: "/".into() };
        for _ in 0..MAX_DEPTH {
            HOSTS.with(|h| {
                h.borrow_mut().push(Host {
                    run_id: 1,
                    step_idx: 0,
                    name: "outer".into(),
                    args: String::new(),
                    source: source.clone(),
                    dest: None,
                    events_tx: tx.clone(),
                    cancel: CancelToken::new(),
                })
            });
        }
        let mut args = InternalArgs::default();
        args.0.insert("script".into(), "inner".into());
        args.0.insert("code".into(), "opus.log('too deep')".into());
        let mut source = source.clone();
        let error = run_script(1, 0, &args, &mut source, None, &tx, &CancelToken::new()).unwrap_err();
        assert!(error.contains("nested"), "{error}");
    })
    .join()
    .unwrap();
}

#[test]
fn async_code_settles_before_the_step_ends() {
    let dir = tempfile::tempdir().unwrap();
    let r = run("(async () => { await null; opus.log('later'); })(); opus.log('first');", ctx(&[], dir.path()));
    assert_eq!(r.result, Ok(()));
    assert_eq!(r.log, ["first", "later"]);
}


