//! Runs JavaScript scripts (the `Script` internal command) with the Boa
//! engine, on the Function executor's own thread.
//!
//! A script sees one global object, `opus` (defined by `scripting/prelude.js`),
//! on top of a single native function: `__opusHost(op, payload)`. The payload
//! and the answer travel as JSON; a failure is thrown as an `Error`. Commands
//! run through [`crate::functions::run_step`], the same code a Function step
//! uses, so a script can do everything a Function can. What only the hub
//! knows (the open Listers, the saved Functions) is asked with
//! [`FnEvent::ScriptQuery`].
//!
//! Every call is synchronous: the script waits for its commands. A run that
//! is cancelled (its Lister closed) fails at the next `opus` call; a call
//! that blocks (`exec`, `confirm`, `alert`, `sleep`) stops at once.

use crate::functions::{recv_or_cancel, run_step, wait_or_kill, FnEvent, NoAnswer, CANCELLED};
use boa_engine::{js_string, Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Script, Source};
use opusdm_fsops::CancelToken;
use opusdm_ipc::function::{expand_text, keys, schema, FnStep, InternalArgs, InternalCmd, SelectionCtx, BOOL_TRUE};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, UNIX_EPOCH};

/// The JavaScript that defines `opus`.
const PRELUDE: &str = include_str!("scripting/prelude.js");

/// How deep scripts may run scripts (`opus.runScript`, a Function with a
/// `Script` step) before the innermost one fails.
pub const MAX_DEPTH: usize = 8;

/// Largest file `opus.fs.read` returns.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// How long a script waits for the hub to answer a [`ScriptQuery`].
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// VM "clock cycles" (Boa's per-instruction cost) a script runs between two
/// checks for a cancelled run: small enough to stop within milliseconds,
/// large enough that the check costs nothing measurable.
const CANCEL_CHECK_BUDGET: u32 = 1 << 16;

/// What a script asks the hub (see [`FnEvent::ScriptQuery`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptQuery {
    /// Every registered Lister: `[{ id, role, path }]`.
    Listers,
    /// The saved Functions, as stored in `functions.json`.
    Functions,
    /// Show `path` (an existing directory) in Lister `lister`; answers `{ path }`.
    GoTo { lister: u64, path: PathBuf },
    /// Read `path` again in every Lister showing it.
    Refresh { path: PathBuf },
}

/// The hub's answer to a [`ScriptQuery`].
pub type QueryReply = Result<Value, String>;

/// One running script: where it reports, and the SOURCE / DEST it works on.
struct Host {
    run_id: u64,
    step_idx: usize,
    name: String,
    args: String,
    source: SelectionCtx,
    dest: Option<SelectionCtx>,
    events_tx: Sender<(u64, FnEvent)>,
    cancel: CancelToken,
}

thread_local! {
    /// The scripts running on this thread, innermost last. A script that runs
    /// a script pushes a second host; the native calls use the last one.
    static HOSTS: RefCell<Vec<Host>> = const { RefCell::new(Vec::new()) };
}

/// Run a `Script` step: load the script named by `script` (or, from the
/// editor's Run, the unsaved `code`), expand `args`, and run it. A
/// `ChangeDir` the script does moves `source` for the next steps.
pub fn run_script(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &mut SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) -> Result<(), String> {
    let name = args.get(keys::SCRIPT).map(str::trim).unwrap_or("").to_string();
    if name.is_empty() {
        return Err("Run Script: no script given".to_string());
    }
    let code = match args.get(keys::CODE) {
        Some(code) => code.to_string(),
        None => opusdm_ipc::scripts::load(&name).map_err(|e| format!("Run Script: cannot read script \"{name}\": {e}"))?,
    };
    let script_args = expand_text(args.get(keys::ARGS).unwrap_or(""), source, dest)
        .map_err(|opusdm_ipc::function::ExpandError::MissingDest(token)| format!("{token}: no DEST Lister open"))?;
    if HOSTS.with(|h| h.borrow().len()) >= MAX_DEPTH {
        return Err(format!("Run Script: scripts nested more than {MAX_DEPTH} deep"));
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("script {name}") }));
    HOSTS.with(|h| {
        h.borrow_mut().push(Host {
            run_id,
            step_idx,
            name: name.clone(),
            args: script_args,
            source: source.clone(),
            dest: dest.cloned(),
            events_tx: events_tx.clone(),
            cancel: cancel.clone(),
        })
    });
    let result = execute(&code, &name, cancel);
    let host = HOSTS.with(|h| h.borrow_mut().pop()).expect("the script's host is still there");
    *source = host.source;
    result.map_err(|e| format!("script \"{name}\": {e}"))?;
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run `code` in a fresh engine with `opus` defined. The error is the text
/// to show: the exception, the syntax error with its position, or
/// [`CANCELLED`] when `cancel` stopped the script.
fn execute(code: &str, name: &str, cancel: &CancelToken) -> Result<(), String> {
    let mut context = Context::default();
    context
        .register_global_callable(js_string!("__opusHost"), 2, NativeFunction::from_fn_ptr(host_call))
        .map_err(|e| format!("cannot set up the engine: {e}"))?;
    context.eval(Source::from_bytes(PRELUDE)).map_err(|e| format!("cannot set up opus: {}", error_text(e, &mut context)))?;
    let file = PathBuf::from(format!("{name}.js"));
    let script = Script::parse(Source::from_bytes(code).with_path(&file), None, &mut context)
        .map_err(|e| error_text(e, &mut context))?;
    let outcome = evaluate_cancellable(&script, &mut context, cancel)?;
    // Promises (`async` functions) settle here; a rejection is an error too.
    let outcome = outcome.and_then(|_| context.run_jobs());
    outcome.map(|_| ()).map_err(|e| error_text(e, &mut context))
}

/// Evaluate `script`, checking `cancel` every [`CANCEL_CHECK_BUDGET`] VM
/// cycles: the cancel check of the `opus.*` calls never runs in a loop of
/// pure JavaScript (`while (true) {}`). Boa's budgeted evaluation yields that
/// often; it is polled here on the calling thread, and dropped when the run
/// is cancelled (the caller then drops the engine). Code run from a native
/// callback or a promise job is not budgeted by Boa and only stops at its
/// next `opus.*` call.
fn evaluate_cancellable(script: &Script, context: &mut Context, cancel: &CancelToken) -> Result<JsResult<JsValue>, String> {
    use std::future::Future;
    use std::task::{Poll, Waker};
    let mut evaluation = std::pin::pin!(script.evaluate_async_with_budget(context, CANCEL_CHECK_BUDGET));
    let mut poll_cx = std::task::Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(outcome) = evaluation.as_mut().poll(&mut poll_cx) {
            return Ok(outcome);
        }
        if cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
    }
}

/// The text of a JavaScript error: `Name: message` for an `Error`, the value
/// itself for anything else thrown.
fn error_text(error: JsError, context: &mut Context) -> String {
    if let Ok(native) = error.try_native(context) {
        return native.to_string();
    }
    let fallback = error.to_string();
    let Ok(value) = error.into_opaque(context) else { return fallback };
    if let Some(object) = value.as_object() {
        let field = |key: &str, context: &mut Context| {
            object
                .get(js_string!(key), context)
                .ok()
                .filter(|v| !v.is_null_or_undefined())
                .and_then(|v| v.to_string(context).ok())
                .map(|s| s.to_std_string_escaped())
        };
        if let Some(message) = field("message", context) {
            let name = field("name", context).unwrap_or_else(|| "Error".to_string());
            return format!("{name}: {message}");
        }
    }
    value.to_string(context).map(|s| s.to_std_string_escaped()).unwrap_or(fallback)
}

/// `__opusHost(op, payload)`: decode the call, run it, encode the answer.
fn host_call(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let op = match args.first() {
        Some(v) => v.to_string(context)?.to_std_string_escaped(),
        None => String::new(),
    };
    let payload = match args.get(1) {
        Some(v) if !v.is_null_or_undefined() => v.to_json(context)?.unwrap_or(Value::Null),
        _ => Value::Null,
    };
    match dispatch(&op, &payload) {
        Ok(value) => JsValue::from_json(&value, context),
        Err(message) => Err(JsNativeError::error().with_message(message).into()),
    }
}

/// What a native call needs from the running script's host, copied out so
/// no borrow is held while a command (maybe another script) runs.
struct Snapshot {
    run_id: u64,
    step_idx: usize,
    name: String,
    args: String,
    source: SelectionCtx,
    dest: Option<SelectionCtx>,
    events_tx: Sender<(u64, FnEvent)>,
    cancel: CancelToken,
}

fn snapshot() -> Result<Snapshot, String> {
    HOSTS.with(|h| {
        let hosts = h.borrow();
        let host = hosts.last().ok_or_else(|| "no script is running".to_string())?;
        Ok(Snapshot {
            run_id: host.run_id,
            step_idx: host.step_idx,
            name: host.name.clone(),
            args: host.args.clone(),
            source: host.source.clone(),
            dest: host.dest.clone(),
            events_tx: host.events_tx.clone(),
            cancel: host.cancel.clone(),
        })
    })
}

/// Store the SOURCE a command left (a `ChangeDir` moved it).
fn set_source(source: SelectionCtx) {
    HOSTS.with(|h| {
        if let Some(host) = h.borrow_mut().last_mut() {
            host.source = source;
        }
    });
}

/// One `opus` call: what it does with the running script's host and the
/// call's payload.
type OpFn = fn(&Snapshot, &Value) -> Result<Value, String>;

/// Every `opus` call the prelude can make, by op name (see
/// `scripting/prelude.js`; a test checks that each one is here).
const OPS: &[(&str, OpFn)] = &[
    ("version", op_version),
    ("log", op_log),
    ("context", op_context),
    ("commands", op_commands),
    ("command", op_command),
    ("confirm", op_confirm),
    ("alert", op_alert),
    ("notify", op_notify),
    ("exec", exec),
    ("sleep", op_sleep),
    ("env", op_env),
    ("listers", op_listers),
    ("functions", op_functions),
    ("scripts", op_scripts),
    ("lister.go", op_lister_go),
    ("refresh", op_refresh),
    ("runFunction", op_run_function),
    ("fs.stat", op_fs_stat),
    ("fs.read", op_fs_read),
    ("fs.write", op_fs_write),
    ("fs.list", op_fs_list),
    ("fs.mkdir", op_fs_mkdir),
];

/// Run one `opus` call: look `op` up in [`OPS`] and run it for the running
/// script. A cancelled run fails every call.
fn dispatch(op: &str, payload: &Value) -> Result<Value, String> {
    let host = snapshot()?;
    if host.cancel.is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    let Some((_, run)) = OPS.iter().find(|(name, _)| *name == op) else {
        return Err(format!("unknown opus call \"{op}\""));
    };
    run(&host, payload)
}

/// `opus.version`: the OpusDM version.
fn op_version(_host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    Ok(json!(env!("CARGO_PKG_VERSION")))
}

/// `opus.log`: one `Output` line per line of `text`.
fn op_log(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    for line in str_field(payload, "text").unwrap_or("").lines() {
        let _ = host.events_tx.send((host.run_id, FnEvent::Output { step_idx: host.step_idx, line: line.to_string() }));
    }
    Ok(Value::Null)
}

/// The script's context: its name, arguments, home folder, SOURCE and DEST.
fn op_context(host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    Ok(json!({
        "script": host.name,
        "args": host.args,
        "home": std::env::var("HOME").unwrap_or_default(),
        "source": selection_json(&host.source),
        "dest": host.dest.as_ref().map(selection_json),
    }))
}

/// `opus.commands`: every internal command with its parameters.
fn op_commands(_host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    Ok(commands_json())
}

/// `opus.command`: run one internal command, as a Function step would.
fn op_command(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let step = command_step(payload)?;
    let (mut source, dest) = command_context(host, payload)?;
    let result = run_step(host.run_id, host.step_idx, &step, &mut source, dest.as_ref(), &host.events_tx, &host.cancel);
    // A `files` list stands in for the selection for this command
    // only; the folder a `ChangeDir` moved to stays.
    source.paths = host.source.paths.clone();
    set_source(source);
    result.map(|_| Value::Null)
}

/// `opus.confirm`: ask the user `message` and answer yes (`true`) or no.
fn op_confirm(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let message = str_field(payload, "message").unwrap_or("").to_string();
    let (reply, answer) = channel::<bool>();
    let _ = host.events_tx.send((host.run_id, FnEvent::Confirm { step_idx: host.step_idx, message, reply }));
    match recv_or_cancel(&answer, &host.cancel) {
        Ok(yes) => Ok(Value::Bool(yes)),
        Err(NoAnswer::Cancelled) => Err(CANCELLED.to_string()),
        Err(NoAnswer::Dropped) => Err("confirm: nobody was there to answer".to_string()),
    }
}

/// `opus.alert`: show `message` and wait until the dialog is closed.
fn op_alert(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let message = str_field(payload, "message").unwrap_or("").to_string();
    let (reply, answer) = channel::<bool>();
    let _ = host.events_tx.send((host.run_id, FnEvent::Alert { step_idx: host.step_idx, message, reply }));
    // The dialog is closed when the answer comes; a dropped reply
    // (the Lister went away) just lets the script go on.
    match recv_or_cancel(&answer, &host.cancel) {
        Err(NoAnswer::Cancelled) => Err(CANCELLED.to_string()),
        Ok(_) | Err(NoAnswer::Dropped) => Ok(Value::Null),
    }
}

/// `opus.notify`, see [`notify`].
fn op_notify(_host: &Snapshot, payload: &Value) -> Result<Value, String> {
    notify(payload)
}

/// `opus.sleep`: wait `ms` milliseconds, stopping early when cancelled.
fn op_sleep(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let ms = payload.get("ms").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as u64;
    let mut left = Duration::from_millis(ms);
    while !left.is_zero() {
        if host.cancel.is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let step = left.min(Duration::from_millis(50));
        std::thread::sleep(step);
        left -= step;
    }
    Ok(Value::Null)
}

/// `opus.env`: the environment variable `name`, or `null`.
fn op_env(_host: &Snapshot, payload: &Value) -> Result<Value, String> {
    Ok(std::env::var(str_field(payload, "name").unwrap_or("")).map(Value::String).unwrap_or(Value::Null))
}

/// `opus.listers`: the open Listers, asked from the hub.
fn op_listers(host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    query(host, ScriptQuery::Listers)
}

/// `opus.functions`: the saved Functions, asked from the hub.
fn op_functions(host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    query(host, ScriptQuery::Functions)
}

/// `opus.scripts`: the saved scripts with their descriptions.
fn op_scripts(_host: &Snapshot, _payload: &Value) -> Result<Value, String> {
    Ok(Value::Array(
        opusdm_ipc::scripts::list().into_iter().map(|s| json!({ "name": s.name, "description": s.description })).collect(),
    ))
}

/// `lister.go(path)`: show an existing directory in Lister `id`.
fn op_lister_go(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let lister = payload.get("id").and_then(Value::as_u64).ok_or_else(|| "go: no Lister id".to_string())?;
    let dir = existing_dir(&host.source, str_field(payload, "path").unwrap_or(""))?;
    query(host, ScriptQuery::GoTo { lister, path: dir })
}

/// `opus.refresh`: read `path` again in every Lister showing it.
fn op_refresh(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let dir = resolve(&host.source, str_field(payload, "path").unwrap_or(""))?;
    query(host, ScriptQuery::Refresh { path: dir })
}

/// `opus.runFunction`, see [`run_function`].
fn op_run_function(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    run_function(host, str_field(payload, "name").unwrap_or(""))
}

/// `opus.fs.stat`: what [`stat_json`] tells about `path`, or `null`.
fn op_fs_stat(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let path = resolve(&host.source, str_field(payload, "path").unwrap_or(""))?;
    Ok(stat_json(&path).unwrap_or(Value::Null))
}

/// `opus.fs.read`: the text of `path`, up to [`MAX_READ_BYTES`].
fn op_fs_read(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let path = resolve(&host.source, str_field(payload, "path").unwrap_or(""))?;
    let size = std::fs::metadata(&path).map_err(|e| format!("read {}: {e}", path.display()))?.len();
    if size > MAX_READ_BYTES {
        return Err(format!("read {}: the file is larger than {} MiB", path.display(), MAX_READ_BYTES >> 20));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
}

/// `opus.fs.write` / `opus.fs.append`: write `data` to `path`.
fn op_fs_write(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    use std::io::Write;
    let path = resolve(&host.source, str_field(payload, "path").unwrap_or(""))?;
    let data = str_field(payload, "data").unwrap_or("");
    let append = payload.get("append").and_then(Value::as_bool).unwrap_or(false);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    file.write_all(data.as_bytes()).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(Value::Null)
}

/// `opus.fs.list`: the entries of a directory (optionally matching
/// `pattern`), sorted by name with case ignored.
fn op_fs_list(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let dir = resolve(&host.source, str_field(payload, "path").unwrap_or("."))?;
    let pattern = str_field(payload, "pattern").filter(|p| !p.trim().is_empty());
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("list {}: {e}", dir.display()))?;
    let mut list: Vec<(String, Value)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if pattern.is_some_and(|p| !opusdm_ipc::glob::glob_match(p, &name)) {
                return None;
            }
            stat_json(&entry.path()).map(|v| (name.to_lowercase(), v))
        })
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(list.into_iter().map(|(_, v)| v).collect()))
}

/// `opus.fs.mkdir`: create `path` and its missing parents.
fn op_fs_mkdir(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    let path = resolve(&host.source, str_field(payload, "path").unwrap_or(""))?;
    std::fs::create_dir_all(&path).map_err(|e| format!("mkdir {}: {e}", path.display()))?;
    Ok(Value::Null)
}

/// The string field `key` of `payload`.
fn str_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

/// `{ path, selected }` of a Lister's selection.
fn selection_json(ctx: &SelectionCtx) -> Value {
    json!({
        "path": ctx.cwd.display().to_string(),
        "selected": ctx.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    })
}

/// Every internal command with its parameters, for `opus.commands`.
fn commands_json() -> Value {
    Value::Array(
        InternalCmd::ALL
            .iter()
            .map(|cmd| {
                let params: Vec<Value> = schema::params_for(*cmd)
                    .iter()
                    .map(|p| json!({ "key": p.key, "label": p.label, "required": p.required, "kind": format!("{:?}", p.kind) }))
                    .collect();
                json!({ "name": format!("{cmd:?}"), "label": cmd.label(), "params": params })
            })
            .collect(),
    )
}

/// The step `opus.command(name, params)` asks for. Parameters must be known
/// to the command; `true` turns a checkbox on, numbers and lists (joined by
/// spaces) become text.
fn command_step(payload: &Value) -> Result<FnStep, String> {
    let name = str_field(payload, "name").unwrap_or("");
    let cmd = opusdm_ipc::cli::find_command(name).ok_or_else(|| format!("unknown command \"{name}\""))?;
    let specs = schema::params_for(cmd);
    let mut args = InternalArgs::default();
    let empty = Map::new();
    let params = match payload.get("params") {
        Some(Value::Object(map)) => map,
        Some(Value::Null) | None => &empty,
        Some(_) => return Err(format!("{}: the parameters must be an object", cmd.label())),
    };
    for (key, value) in params {
        let Some(spec) = specs.iter().find(|s| s.key == key) else {
            let known: Vec<&str> = specs.iter().map(|s| s.key).collect();
            return Err(format!("{}: unknown parameter \"{key}\" (known: {})", cmd.label(), if known.is_empty() { "none".to_string() } else { known.join(", ") }));
        };
        let text = match value {
            Value::Bool(true) => BOOL_TRUE.to_string(),
            Value::Bool(false) | Value::Null => continue,
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Array(items) => items.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(" "),
            Value::Object(_) => return Err(format!("{}: parameter \"{}\" cannot be an object", cmd.label(), spec.key)),
        };
        args.0.insert(key.clone(), text);
    }
    for spec in specs {
        if spec.required && args.get(spec.key).is_none_or(|v| v.trim().is_empty()) {
            return Err(format!("{}: missing required parameter \"{}\" ({})", cmd.label(), spec.key, spec.label));
        }
    }
    Ok(FnStep::Internal { cmd, args })
}

/// The SOURCE and DEST a command runs on: the script's, with `files` in place
/// of the SOURCE selection and `to` in place of the DEST folder when given.
fn command_context(host: &Snapshot, payload: &Value) -> Result<(SelectionCtx, Option<SelectionCtx>), String> {
    let mut source = host.source.clone();
    if let Some(Value::Array(files)) = payload.get("files") {
        source.paths = files
            .iter()
            .map(|f| resolve(&host.source, f.as_str().unwrap_or("")))
            .collect::<Result<Vec<_>, _>>()?;
    }
    let dest = match str_field(payload, "to") {
        Some(to) => Some(SelectionCtx { paths: Vec::new(), cwd: existing_dir(&host.source, to)? }),
        None => host.dest.clone(),
    };
    Ok((source, dest))
}

/// `text` as a path: `~` is the home folder, a relative path is relative to
/// the SOURCE folder.
fn resolve(source: &SelectionCtx, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the path is empty".to_string());
    }
    let path = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => source.cwd.join(text),
    };
    Ok(path)
}

/// [`resolve`], then require an existing directory (returned canonical).
fn existing_dir(source: &SelectionCtx, text: &str) -> Result<PathBuf, String> {
    let path = resolve(source, text)?;
    let path = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

/// What `opus.fs.stat` tells about `path`; `None` when it does not exist.
fn stat_json(path: &Path) -> Option<Value> {
    let link = std::fs::symlink_metadata(path).ok()?;
    // A dangling link is still there: describe the link itself.
    let meta = std::fs::metadata(path).unwrap_or_else(|_| link.clone());
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
    use std::os::unix::fs::PermissionsExt;
    Some(json!({
        "path": path.display().to_string(),
        "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "isFile": meta.is_file(),
        "isDir": meta.is_dir(),
        "isLink": link.file_type().is_symlink(),
        "size": meta.len(),
        "modified": modified,
        "mode": meta.permissions().mode() & 0o7777,
    }))
}

/// Ask the hub `q` and wait for the answer.
fn query(host: &Snapshot, q: ScriptQuery) -> Result<Value, String> {
    let (reply, answer) = channel::<QueryReply>();
    let _ = host.events_tx.send((host.run_id, FnEvent::ScriptQuery { query: q, reply }));
    match answer.recv_timeout(QUERY_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err("the hub did not answer".to_string()),
    }
}

/// `opus.runFunction`: the saved Function called `wanted` (id, then name,
/// then name with case ignored), its steps run one by one on the script's
/// SOURCE and DEST.
fn run_function(host: &Snapshot, wanted: &str) -> Result<Value, String> {
    let functions: Vec<opusdm_ipc::function::Function> =
        serde_json::from_value(query(host, ScriptQuery::Functions)?).map_err(|e| format!("runFunction: {e}"))?;
    let function = functions
        .iter()
        .find(|f| f.id == wanted)
        .or_else(|| functions.iter().find(|f| f.name == wanted))
        .or_else(|| functions.iter().find(|f| f.name.to_lowercase() == wanted.to_lowercase()))
        .ok_or_else(|| format!("runFunction: no Function called \"{wanted}\""))?;
    let mut source = host.source.clone();
    for (index, step) in function.steps.iter().enumerate() {
        if host.cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }
        if let Err(e) = run_step(host.run_id, host.step_idx, step, &mut source, host.dest.as_ref(), &host.events_tx, &host.cancel) {
            set_source(source);
            return Err(format!("{} (step {}): {e}", function.name, index + 1));
        }
    }
    set_source(source);
    Ok(Value::Null)
}

/// `opus.notify`: show a desktop notification through
/// `org.freedesktop.Notifications` (the OpusDM notification server, or any
/// other the session runs). Fields: `title`, `body`, `icon`, `timeout` (ms,
/// `-1` the server's default, `0` never) and `urgency` (`low`, `normal`,
/// `critical`). Answers the notification id.
fn notify(payload: &Value) -> Result<Value, String> {
    use std::collections::HashMap;
    use zbus::zvariant::Value as Variant;

    let title = str_field(payload, "title").unwrap_or("");
    if title.is_empty() {
        return Err("notify: no title given".to_string());
    }
    let urgency: u8 = match str_field(payload, "urgency").unwrap_or("normal") {
        "low" => 0,
        "normal" => 1,
        "critical" => 2,
        other => return Err(format!("notify: urgency must be low, normal or critical, not \"{other}\"")),
    };
    let timeout = payload.get("timeout").and_then(Value::as_i64).unwrap_or(-1).clamp(-1, i32::MAX as i64) as i32;
    let mut hints: HashMap<&str, Variant> = HashMap::new();
    hints.insert("urgency", Variant::U8(urgency));
    let conn = zbus::blocking::Connection::session().map_err(|e| format!("notify: no session bus: {e}"))?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &("OpusDM Script", 0u32, str_field(payload, "icon").unwrap_or(""), title, str_field(payload, "body").unwrap_or(""), Vec::<&str>::new(), hints, timeout),
        )
        .map_err(|e| format!("notify: the notification server did not take it: {e}"))?;
    let id: u32 = reply.body().deserialize().map_err(|e| format!("notify: unexpected answer: {e}"))?;
    Ok(json!(id))
}

/// `opus.exec`: run a program, wait, and capture its output. The program is
/// killed when the run is cancelled.
fn exec(host: &Snapshot, payload: &Value) -> Result<Value, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let program = str_field(payload, "program").unwrap_or("").trim();
    if program.is_empty() {
        return Err("exec: no program given".to_string());
    }
    let args: Vec<String> = match payload.get("args") {
        Some(Value::Array(items)) => items.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect(),
        _ => Vec::new(),
    };
    let cwd = match str_field(payload, "cwd") {
        Some(dir) => existing_dir(&host.source, dir)?,
        None if host.source.cwd.is_dir() => host.source.cwd.clone(),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())),
    };
    let input = str_field(payload, "input").map(str::to_string);
    let mut child = Command::new(program)
        .args(&args)
        .current_dir(&cwd)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("exec {program}: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        // Written from a thread: a program that answers before reading all
        // its input would otherwise block on a full pipe.
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let collect = |mut reader: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = collect(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = collect(Box::new(child.stderr.take().expect("piped stderr")));
    // A cancelled program's readers are not joined: a process it started
    // may still hold the pipes open.
    let status = wait_or_kill(&mut child, &host.cancel).map_err(|e| format!("exec {program}: {e}"))?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok(json!({
        "code": status.code(),
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
    }))
}

#[cfg(test)]
mod tests;
