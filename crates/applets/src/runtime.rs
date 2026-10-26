//! The Boa runtime of one applet: one JavaScript `Context`, the `opus`
//! object of `prelude.js`, timers and async calls on the glib main loop.
//!
//! The prelude is built on one native function, `host(op, payload)`, that
//! looks `op` up in [`OPS`] (the payload and the answer travel as JSON, a
//! failure is thrown as an `Error`). Timers, pending async calls and
//! handlers are kept by the prelude in JavaScript; Rust only knows their
//! numeric ids and calls back through the hooks object the prelude returns.
//!
//! Ownership: the caller owns the [`Runtime`] for as long as the applet is
//! enabled. `Runtime` holds the only `Rc<Inner>`; every timer task, async
//! completion and queued call holds a `Weak<Inner>` and logs with `err_log!`
//! when it can no longer upgrade it. [`Runtime::stop`] (also run on drop)
//! aborts every live timer task and refuses any later call into JavaScript.
//!
//! Re-entrancy: JavaScript only runs from [`run_call`], which takes the
//! `Context` with `try_borrow_mut`. A call that finds the `Context` busy
//! (a sink that calls the runtime back while a script runs) is queued on the
//! main loop and runs once the script returns. After every call the promise
//! job queue runs, then unhandled rejections are reported.

use crate::manifest::Manifest;
use boa_engine::builtins::promise::{OperationType, Promise, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::object::builtins::JsPromise;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Script, Source};
use gtk4::{gio, glib};
use opusdm_ipc::{debug_log, err_log};
use serde_json::{json, Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

/// The JavaScript that defines `opus`, the timers and `console`.
const PRELUDE: &str = include_str!("prelude.js");

/// Tag used in log lines.
const APP_TAG: &str = "applets";

/// Loop iterations one function call may run (Boa counts every loop of a
/// call frame together) before an uncatchable error stops it.
pub const LOOP_ITERATION_LIMIT: u64 = 10_000_000;

/// Nested JavaScript calls before an uncatchable error stops the script.
pub const RECURSION_LIMIT: usize = 512;

/// Shortest `setInterval` period, in milliseconds, as in browsers: a 0 ms
/// interval would keep the main loop busy.
const MIN_INTERVAL_MS: u64 = 4;

/// Largest file `opus.fs.readText` and `opus.applet.readAsset` return.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Name of the worker thread of one `opus.http.get`.
const HTTP_THREAD: &str = "opusdm-applet-http";

/// Name of the worker thread of one `opus.notify`.
const NOTIFY_THREAD: &str = "opusdm-applet-notify";

/// URL schemes `opus.applet.open` accepts.
const OPEN_SCHEMES: [&str; 3] = ["http", "https", "file"];

/// What a runtime tells its host.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A log line (`opus.log`, `console.log`, `console.warn` with a `warn: ` prefix).
    Log(String),
    /// An error: an exception, a cut runaway script, `console.error`.
    Error(String),
    /// A widget tree change (produced from Task 6).
    Ui(Value),
    /// The drawing commands of canvas `id` (produced from Task 7).
    Redraw { id: String, commands: Value },
}

/// Where a runtime sends its events (the GTK side implements it).
pub type Sink = Rc<dyn Fn(Event)>;

/// Shows a desktop notification: takes the `opus.notify` payload
/// (`appName`, `title`, `body`, `icon`, `timeout`, `urgency`) and answers the
/// notification id. Runs on a worker thread.
pub type Notifier = Arc<dyn Fn(&Value) -> Result<Value, String> + Send + Sync>;

/// One running applet. Dropping it stops it (see [`Runtime::stop`]).
pub struct Runtime {
    inner: Rc<Inner>,
}

/// Everything a runtime shares with its timer tasks, async completions and
/// host calls (these hold a `Weak<Inner>`).
struct Inner {
    id: String,
    name: String,
    /// The applet folder, canonical.
    dir: PathBuf,
    sink: Sink,
    notifier: Notifier,
    /// The main context the timers and completions run on.
    main: glib::MainContext,
    /// The live timer tasks, by timer id.
    timers: RefCell<HashMap<u32, glib::JoinHandle<()>>>,
    stopped: Cell<bool>,
    /// The engine; `None` only while `start` builds it.
    engine: RefCell<Option<Engine>>,
}

/// The JavaScript side. Field order matters: the JavaScript objects are
/// dropped before the `Context` that made them.
struct Engine {
    /// The object `prelude.js` returned (`fireTimer`, `settle`, ...).
    hooks: JsObject,
    /// Promises rejected with no handler yet, filled by [`RejectionTracker`].
    rejections: Rc<RefCell<Vec<JsObject>>>,
    context: Context,
}

/// A call from Rust into the applet's JavaScript.
enum Call {
    Timer(u32),
    Settle { request: u32, outcome: Result<Value, String> },
    Settings(Map<String, Value>),
    Dispatch { handler: u32, arg: Value },
    Redraw(String),
}

/// The `Context` data that lets [`host_call`] find its runtime.
struct HostData(Weak<Inner>);

/// Boa host hooks that track promises rejected without a handler.
struct RejectionTracker {
    rejections: Rc<RefCell<Vec<JsObject>>>,
}

impl HostHooks for RejectionTracker {
    /// Remember a promise rejected without a handler; forget it when a
    /// handler is added later.
    fn promise_rejection_tracker(&self, promise: &JsObject<Promise>, operation: OperationType, _context: &mut Context) {
        let promise = promise.clone().upcast();
        let mut list = self.rejections.borrow_mut();
        match operation {
            OperationType::Reject => list.push(promise),
            OperationType::Handle => list.retain(|p| !JsObject::equals(p, &promise)),
        }
    }
}

impl Runtime {
    /// Start applet `id` from folder `dir`: build the engine, define `opus`
    /// and run the entry script. Notifications go to the desktop's
    /// notification server. A failure is also sent to `sink` and logged.
    pub fn start(id: &str, dir: &Path, manifest: &Manifest, settings: Map<String, Value>, sink: Sink) -> Result<Runtime, String> {
        Self::start_with_notifier(id, dir, manifest, settings, sink, Arc::new(dbus_notify))
    }

    /// [`Runtime::start`] with `notifier` in place of the D-Bus one (tests
    /// must never reach the desktop's notification server).
    pub fn start_with_notifier(
        id: &str,
        dir: &Path,
        manifest: &Manifest,
        settings: Map<String, Value>,
        sink: Sink,
        notifier: Notifier,
    ) -> Result<Runtime, String> {
        let fail = |message: String| {
            err_log!(APP_TAG, "applet {id}: {message}");
            sink(Event::Error(message.clone()));
            Err(message)
        };
        let dir = match dir.canonicalize() {
            Ok(dir) => dir,
            Err(e) => return fail(format!("cannot open the folder {}: {e}", dir.display())),
        };
        let entry = dir.join(&manifest.entry);
        let code = match std::fs::read_to_string(&entry) {
            Ok(code) => code,
            Err(e) => return fail(format!("cannot read {}: {e}", entry.display())),
        };
        let inner = Rc::new(Inner {
            id: id.to_string(),
            name: manifest.name.clone(),
            dir,
            sink: sink.clone(),
            notifier,
            main: glib::MainContext::ref_thread_default(),
            timers: RefCell::new(HashMap::new()),
            stopped: Cell::new(false),
            engine: RefCell::new(None),
        });
        let engine = match Engine::new(&inner, settings) {
            Ok(engine) => engine,
            Err(e) => return fail(e),
        };
        *inner.engine.borrow_mut() = Some(engine);
        // From here a failure drops `runtime`, which stops its timers.
        let runtime = Runtime { inner };
        runtime.run_entry(&code, &entry)?;
        Ok(runtime)
    }

    /// Run the entry script, then its promise jobs. Every error is reported;
    /// an exception of the script itself is also the `Err`.
    fn run_entry(&self, code: &str, path: &Path) -> Result<(), String> {
        let (result, errors) = {
            let mut slot = self.inner.engine.borrow_mut();
            let engine = slot.as_mut().expect("the engine is set before the entry script runs");
            engine.run_script(code, path)
        };
        for e in errors {
            self.inner.report(e);
        }
        if let Err(e) = &result {
            self.inner.report(e.clone());
        }
        result
    }

    /// Store new settings in `opus.applet.settings` and call the function
    /// registered with `opus.applet.onSettings`.
    pub fn deliver_settings(&self, settings: Map<String, Value>) {
        run_call(&self.inner.clone(), Call::Settings(settings));
    }

    /// Call the JavaScript handler `handler` (registered by the widget code)
    /// with `arg`.
    pub fn dispatch(&self, handler: u32, arg: Value) {
        run_call(&self.inner.clone(), Call::Dispatch { handler, arg });
    }

    /// Ask the JavaScript side to paint canvas `canvas_id`.
    pub fn redraw(&self, canvas_id: &str) {
        run_call(&self.inner.clone(), Call::Redraw(canvas_id.to_string()));
    }

    /// Stop the applet: abort every live timer and refuse every later call
    /// into JavaScript; an async answer that arrives afterwards is dropped.
    /// Calling it again does nothing.
    pub fn stop(&self) {
        if self.inner.stopped.replace(true) { return; }
        let timers = std::mem::take(&mut *self.inner.timers.borrow_mut());
        for handle in timers.values() {
            handle.abort();
        }
        debug_log!(APP_TAG, "applet {}: stopped, {} timers cancelled", self.inner.id, timers.len());
    }
}

impl Drop for Runtime {
    /// A dropped runtime stops first, so no timer outlives it.
    fn drop(&mut self) {
        self.stop();
    }
}

impl Inner {
    /// Log an error and send it to the sink.
    fn report(&self, message: String) {
        err_log!(APP_TAG, "applet {}: {message}", self.id);
        (self.sink)(Event::Error(message));
    }
}

impl Engine {
    /// A fresh `Context` with the runtime limits set, `opus` defined, and the
    /// prelude's hooks object.
    fn new(inner: &Rc<Inner>, settings: Map<String, Value>) -> Result<Engine, String> {
        let rejections: Rc<RefCell<Vec<JsObject>>> = Rc::default();
        let tracker = Rc::new(RejectionTracker { rejections: rejections.clone() });
        let mut context = Context::builder().host_hooks(tracker).build().map_err(|e| format!("cannot set up the engine: {e}"))?;
        context.runtime_limits_mut().set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
        context.runtime_limits_mut().set_recursion_limit(RECURSION_LIMIT);
        context.insert_data(HostData(Rc::downgrade(inner)));
        let host = FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(host_call)).name(js_string!("host")).length(2).build();
        let init = json!({
            "id": inner.id,
            "name": inner.name,
            "dir": inner.dir.display().to_string(),
            "version": env!("CARGO_PKG_VERSION"),
            "settings": Value::Object(settings),
        });
        let setup = |context: &mut Context| -> JsResult<JsObject> {
            let prelude = context.eval(Source::from_bytes(PRELUDE))?;
            let prelude = prelude.as_callable().ok_or_else(|| JsNativeError::typ().with_message("the prelude is not a function"))?;
            let init = JsValue::from_json(&init, context)?;
            let hooks = prelude.call(&JsValue::undefined(), &[host.into(), init], context)?;
            Ok(hooks.as_object().ok_or_else(|| JsNativeError::typ().with_message("the prelude returned no hooks"))?)
        };
        match setup(&mut context) {
            Ok(hooks) => Ok(Engine { hooks, rejections, context }),
            Err(e) => Err(format!("cannot set up opus: {}", error_text(e, &mut context))),
        }
    }

    /// Run the entry script `code` (named `path` in errors), then the job
    /// queue. Returns the script's own outcome and the other errors (a
    /// failed job, unhandled rejections).
    fn run_script(&mut self, code: &str, path: &Path) -> (Result<(), String>, Vec<String>) {
        let context = &mut self.context;
        let result = Script::parse(Source::from_bytes(code).with_path(path), None, context).and_then(|script| script.evaluate(context));
        let result = result.map(|_| ()).map_err(|e| error_text(e, context));
        let mut errors = Vec::new();
        if let Err(e) = self.context.run_jobs() {
            errors.push(format!("promise job: {}", error_text(e, &mut self.context)));
        }
        errors.extend(self.take_rejections());
        (result, errors)
    }

    /// Run `call` through the prelude's hooks, then the job queue. Returns
    /// every error, as text.
    fn run(&mut self, call: Call) -> Vec<String> {
        let what = call.describe();
        let mut errors = Vec::new();
        if let Err(e) = self.invoke(call) {
            errors.push(format!("{what}: {}", error_text(e, &mut self.context)));
        }
        if let Err(e) = self.context.run_jobs() {
            errors.push(format!("promise job: {}", error_text(e, &mut self.context)));
        }
        errors.extend(self.take_rejections());
        errors
    }

    /// Call the hook that handles `call`.
    fn invoke(&mut self, call: Call) -> JsResult<JsValue> {
        let context = &mut self.context;
        let (name, args) = match call {
            Call::Timer(id) => ("fireTimer", vec![JsValue::from(id)]),
            Call::Settle { request, outcome: Ok(value) } => ("settle", vec![JsValue::from(request), JsValue::from(true), JsValue::from_json(&value, context)?]),
            Call::Settle { request, outcome: Err(message) } => ("settle", vec![JsValue::from(request), JsValue::from(false), JsValue::from(js_string!(message.as_str()))]),
            Call::Settings(settings) => ("settings", vec![JsValue::from_json(&Value::Object(settings), context)?]),
            Call::Dispatch { handler, arg } => ("dispatch", vec![JsValue::from(handler), JsValue::from_json(&arg, context)?]),
            Call::Redraw(id) => ("redraw", vec![JsValue::from(js_string!(id.as_str()))]),
        };
        let hook = self.hooks.get(js_string!(name), context)?;
        let hook = hook.as_callable().ok_or_else(|| JsNativeError::typ().with_message(format!("the prelude hook {name} is not a function")))?;
        hook.call(&JsValue::undefined(), &args, context)
    }

    /// The text of every promise still rejected with no handler, as
    /// "unhandled promise rejection: ..."; the list is emptied.
    fn take_rejections(&mut self) -> Vec<String> {
        let promises = std::mem::take(&mut *self.rejections.borrow_mut());
        let mut errors = Vec::new();
        for object in promises {
            let state = match JsPromise::from_object(object) {
                Ok(promise) => promise.state(),
                Err(e) => {
                    errors.push(format!("unhandled promise rejection (unreadable promise): {e}"));
                    continue;
                }
            };
            let PromiseState::Rejected(reason) = state else { continue };
            errors.push(format!("unhandled promise rejection: {}", error_text(JsError::from_opaque(reason), &mut self.context)));
        }
        errors
    }
}

impl Call {
    /// A short name of the call for error messages.
    fn describe(&self) -> String {
        match self {
            Call::Timer(id) => format!("timer {id}"),
            Call::Settle { request, .. } => format!("async call {request}"),
            Call::Settings(_) => "onSettings".to_string(),
            Call::Dispatch { handler, .. } => format!("handler {handler}"),
            Call::Redraw(id) => format!("redraw {id}"),
        }
    }
}

/// Run `call` in the applet's JavaScript and report its errors. A stopped
/// runtime drops it (debug log); a busy `Context` (JavaScript already
/// running further up the stack) gets it queued on the main loop.
fn run_call(inner: &Rc<Inner>, call: Call) {
    if inner.stopped.get() {
        debug_log!(APP_TAG, "applet {}: {} after stop, dropped", inner.id, call.describe());
        return;
    }
    let errors = {
        let Ok(mut slot) = inner.engine.try_borrow_mut() else {
            queue_call(inner, call);
            return;
        };
        let Some(engine) = slot.as_mut() else {
            err_log!(APP_TAG, "applet {}: {} before the engine was ready, dropped", inner.id, call.describe());
            return;
        };
        engine.run(call)
    };
    // The borrow is released: the sink may call the runtime again.
    for e in errors {
        inner.report(e);
    }
}

/// Run `call` from the main loop, once the JavaScript running now returns.
fn queue_call(inner: &Rc<Inner>, call: Call) {
    debug_log!(APP_TAG, "applet {}: {} while the script runs, queued", inner.id, call.describe());
    let weak = Rc::downgrade(inner);
    let applet = inner.id.clone();
    inner.main.spawn_local(async move {
        let Some(inner) = weak.upgrade() else {
            err_log!(APP_TAG, "applet {applet}: queued {} found the runtime gone, dropped", call.describe());
            return;
        };
        run_call(&inner, call);
    });
}

/// The task of one timer: wait, fire, and for an interval repeat until the
/// timer is cleared. It ends (with a log) when the runtime is gone.
async fn timer_task(weak: Weak<Inner>, applet: String, timer: u32, every: Duration, repeat: bool) {
    loop {
        glib::timeout_future(every).await;
        let Some(inner) = weak.upgrade() else {
            err_log!(APP_TAG, "applet {applet}: timer {timer} fired after the runtime was dropped, ending it");
            return;
        };
        if inner.stopped.get() {
            err_log!(APP_TAG, "applet {applet}: timer {timer} fired after stop, ending it");
            return;
        }
        if !repeat {
            inner.timers.borrow_mut().remove(&timer);
        }
        run_call(&inner, Call::Timer(timer));
        if !repeat || !inner.timers.borrow().contains_key(&timer) { return; }
    }
}

/// `host(op, payload)`: find the runtime, run `op`, encode the answer; a
/// failure is thrown as an `Error`.
fn host_call(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let op = match args.first() {
        Some(v) => v.to_string(context)?.to_std_string_escaped(),
        None => String::new(),
    };
    let payload = match args.get(1) {
        Some(v) if !v.is_null_or_undefined() => v.to_json(context)?.unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let inner = context.get_data::<HostData>().and_then(|data| data.0.upgrade());
    let Some(inner) = inner else {
        err_log!(APP_TAG, "applet call {op} found its runtime gone");
        return Err(JsNativeError::error().with_message("the applet runtime is gone").into());
    };
    let Some((_, run)) = OPS.iter().find(|(name, _)| *name == op) else {
        return Err(JsNativeError::error().with_message(format!("unknown opus call \"{op}\"")).into());
    };
    match run(&inner, &payload) {
        Ok(value) => JsValue::from_json(&value, context),
        Err(message) => Err(JsNativeError::error().with_message(message).into()),
    }
}

/// One host call: what it does with the runtime and the call's payload.
type OpFn = fn(&Rc<Inner>, &Value) -> Result<Value, String>;

/// Every host call `prelude.js` makes, by op name (a test checks that each
/// one is here).
const OPS: &[(&str, OpFn)] = &[
    ("log", op_log),
    ("error", op_error),
    ("timer.set", op_timer_set),
    ("timer.clear", op_timer_clear),
    ("http.get", op_http_get),
    ("notify", op_notify),
    ("fs.exists", op_fs_exists),
    ("fs.readText", op_fs_read_text),
    ("fs.list", op_fs_list),
    ("applet.open", op_applet_open),
    ("applet.assetPath", op_applet_asset_path),
    ("applet.readAsset", op_applet_read_asset),
];

/// `opus.log`, `console.log`, `console.warn`: an [`Event::Log`].
fn op_log(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    (inner.sink)(Event::Log(str_field(payload, "text").unwrap_or("").to_string()));
    Ok(Value::Null)
}

/// `console.error`: an [`Event::Error`], logged.
fn op_error(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    inner.report(str_field(payload, "text").unwrap_or("").to_string());
    Ok(Value::Null)
}

/// `setTimeout` / `setInterval`: start the task of timer `id`.
fn op_timer_set(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let id = u32_field(payload, "id")?;
    let ms = payload.get("ms").and_then(Value::as_u64).unwrap_or(0);
    let repeat = payload.get("repeat").and_then(Value::as_bool).unwrap_or(false);
    if inner.stopped.get() {
        debug_log!(APP_TAG, "applet {}: timer {id} set after stop, ignored", inner.id);
        return Ok(Value::Null);
    }
    let every = Duration::from_millis(if repeat { ms.max(MIN_INTERVAL_MS) } else { ms });
    let task = inner.main.spawn_local(timer_task(Rc::downgrade(inner), inner.id.clone(), id, every, repeat));
    if let Some(old) = inner.timers.borrow_mut().insert(id, task) {
        old.abort();
    }
    Ok(Value::Null)
}

/// `clearTimeout` / `clearInterval`: abort the task of timer `id`. Aborting
/// the timer that is firing now is safe: glib destroys its source, and the
/// task, no longer in the map, ends after the callback.
fn op_timer_clear(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let id = u32_field(payload, "id")?;
    let task = inner.timers.borrow_mut().remove(&id);
    if let Some(task) = task {
        task.abort();
    }
    Ok(Value::Null)
}

/// `opus.http.get`: the blocking GET of `opusdm_jscore::http` on a worker
/// thread; the answer settles the promise from the main loop.
fn op_http_get(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let request = u32_field(payload, "request")?;
    let http = opusdm_jscore::http::request_from_json(payload.get("payload").unwrap_or(&Value::Null))?;
    start_async(inner, request, HTTP_THREAD, move || {
        opusdm_jscore::http::get(&http).map(|response| opusdm_jscore::http::response_to_json(&response))
    })?;
    Ok(Value::Null)
}

/// `opus.notify`: the notifier on a worker thread, the applet's name as the
/// application name.
fn op_notify(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let request = u32_field(payload, "request")?;
    let mut notification = payload.get("payload").cloned().unwrap_or(Value::Null);
    let Some(fields) = notification.as_object_mut() else { return Err("notify: no notification given".to_string()) };
    fields.insert("appName".to_string(), Value::String(inner.name.clone()));
    let notifier = inner.notifier.clone();
    start_async(inner, request, NOTIFY_THREAD, move || notifier(&notification))?;
    Ok(Value::Null)
}

/// Run `work` on a new thread named `thread`; its result settles async call
/// `request` from the main loop. A result that arrives after `stop()` is
/// dropped (debug log), one that finds the runtime gone is logged.
fn start_async(inner: &Rc<Inner>, request: u32, thread: &str, work: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Result<(), String> {
    let (tx, rx) = async_channel::bounded::<Result<Value, String>>(1);
    let applet = inner.id.clone();
    std::thread::Builder::new()
        .name(thread.to_string())
        .spawn(move || {
            if tx.send_blocking(work()).is_err() {
                debug_log!(APP_TAG, "applet {applet}: async call {request} answered after its receiver was gone");
            }
        })
        .map_err(|e| format!("cannot start the {thread} thread: {e}"))?;
    let weak = Rc::downgrade(inner);
    let applet = inner.id.clone();
    inner.main.spawn_local(async move {
        let Ok(outcome) = rx.recv().await else {
            err_log!(APP_TAG, "applet {applet}: async call {request} ended without an answer");
            return;
        };
        let Some(inner) = weak.upgrade() else {
            err_log!(APP_TAG, "applet {applet}: async call {request} answered after the runtime was dropped, answer dropped");
            return;
        };
        if inner.stopped.get() {
            debug_log!(APP_TAG, "applet {applet}: async call {request} answered after stop, answer dropped");
            return;
        }
        run_call(&inner, Call::Settle { request, outcome });
    });
    Ok(())
}

/// `opus.fs.exists`: whether the path exists (a dangling link counts).
fn op_fs_exists(_inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let path = path_field(payload)?;
    Ok(Value::Bool(std::fs::symlink_metadata(path).is_ok()))
}

/// `opus.fs.readText`: the text of a file.
fn op_fs_read_text(_inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    read_text(Path::new(path_field(payload)?))
}

/// `opus.fs.list`: the entries of a folder, sorted by name with case ignored.
fn op_fs_list(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let dir = Path::new(path_field(payload)?);
    let entries = std::fs::read_dir(dir).map_err(|e| format!("list {}: {e}", dir.display()))?;
    let mut list = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                err_log!(APP_TAG, "applet {}: list {}: an entry cannot be read: {e}", inner.id, dir.display());
                continue;
            }
        };
        let path = entry.path();
        // A dangling link is still listed: describe the link itself.
        let meta = std::fs::metadata(&path).or_else(|_| std::fs::symlink_metadata(&path));
        let (is_file, is_dir, size) = match meta {
            Ok(meta) => (meta.is_file(), meta.is_dir(), meta.len()),
            Err(e) => {
                err_log!(APP_TAG, "applet {}: list {}: {e}", inner.id, path.display());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        list.push((name.to_lowercase(), json!({ "name": name, "path": path.display().to_string(), "isFile": is_file, "isDir": is_dir, "size": size })));
    }
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Array(list.into_iter().map(|(_, v)| v).collect()))
}

/// `opus.applet.open`: open an `http`, `https` or `file` URL with the
/// desktop's default handler.
fn op_applet_open(_inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let url = str_field(payload, "url").unwrap_or("").trim();
    check_open_url(url)?;
    gio::AppInfo::launch_default_for_uri(url, gio::AppLaunchContext::NONE).map_err(|e| format!("open {url}: {e}"))?;
    Ok(Value::Null)
}

/// `opus.applet.assetPath`: the absolute path of a file of the applet folder.
fn op_applet_asset_path(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    Ok(Value::String(asset_path(&inner.dir, path_field(payload)?)?.display().to_string()))
}

/// `opus.applet.readAsset`: the text of a file of the applet folder; the
/// real path (links resolved) must still be inside the folder.
fn op_applet_read_asset(inner: &Rc<Inner>, payload: &Value) -> Result<Value, String> {
    let rel = path_field(payload)?;
    let path = asset_path(&inner.dir, rel)?;
    let real = path.canonicalize().map_err(|e| format!("readAsset {rel}: {e}"))?;
    if !real.starts_with(&inner.dir) {
        return Err(format!("readAsset {rel}: the path leaves the applet folder"));
    }
    read_text(&real)
}

/// The text of the file `path`, at most [`MAX_READ_BYTES`]; invalid UTF-8
/// is replaced.
fn read_text(path: &Path) -> Result<Value, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("read {}: {e}", path.display()))?.len();
    if size > MAX_READ_BYTES {
        return Err(format!("read {}: the file is larger than {} MiB", path.display(), MAX_READ_BYTES >> 20));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
}

/// `rel` inside `dir`, normalized: refused when empty, absolute, or when a
/// `..` climbs out of `dir`. Links are not resolved here.
fn asset_path(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() { return Err("asset: the path is empty".to_string()) }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() { return Err(format!("asset {rel}: the path must be relative to the applet folder")) }
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in rel_path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() { return Err(format!("asset {rel}: the path leaves the applet folder")) }
            }
            Component::RootDir | Component::Prefix(_) => return Err(format!("asset {rel}: the path must be relative to the applet folder")),
        }
    }
    if parts.is_empty() { return Err(format!("asset {rel}: the path names the applet folder itself")) }
    let mut path = dir.to_path_buf();
    path.extend(parts);
    Ok(path)
}

/// Accept only `http://`, `https://` and `file://` URLs.
fn check_open_url(url: &str) -> Result<(), String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(format!("open {url}: not a URL (http, https or file expected)"));
    };
    if rest.is_empty() { return Err(format!("open {url}: the URL is empty after the scheme")) }
    if !OPEN_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(format!("open {url}: unsupported scheme, only http, https and file are allowed"));
    }
    Ok(())
}

/// The string field `key` of `payload`.
fn str_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

/// The non-empty `path` field of `payload`.
fn path_field(payload: &Value) -> Result<&str, String> {
    match str_field(payload, "path") {
        Some(path) if !path.is_empty() => Ok(path),
        _ => Err("the path is empty".to_string()),
    }
}

/// The `u32` field `key` of `payload` (a JS number may arrive as a float).
fn u32_field(payload: &Value, key: &str) -> Result<u32, String> {
    let value = payload.get(key).and_then(|v| v.as_u64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64)));
    value.and_then(|v| u32::try_from(v).ok()).ok_or_else(|| format!("internal: missing or bad `{key}`"))
}

/// The text of a JavaScript error: `Name: message` for an `Error`, the value
/// itself for anything else thrown (same rules as the Lister scripts).
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

/// The default [`Notifier`]: `org.freedesktop.Notifications.Notify` on the
/// session bus, as the Lister scripts' `opus.notify` does. Answers the
/// notification id.
fn dbus_notify(payload: &Value) -> Result<Value, String> {
    use std::collections::HashMap;
    use zbus::zvariant::Value as Variant;

    let title = str_field(payload, "title").unwrap_or("");
    if title.is_empty() { return Err("notify: no title given".to_string()) }
    let urgency: u8 = match str_field(payload, "urgency").unwrap_or("normal") {
        "low" => 0,
        "normal" => 1,
        "critical" => 2,
        other => return Err(format!("notify: urgency must be low, normal or critical, not \"{other}\"")),
    };
    let timeout = payload.get("timeout").and_then(Value::as_i64).unwrap_or(-1).clamp(-1, i32::MAX as i64) as i32;
    let mut hints: HashMap<&str, Variant> = HashMap::new();
    hints.insert("urgency", Variant::U8(urgency));
    let app = str_field(payload, "appName").unwrap_or("OpusDM Applet");
    let conn = zbus::blocking::Connection::session().map_err(|e| format!("notify: no session bus: {e}"))?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(app, 0u32, str_field(payload, "icon").unwrap_or(""), title, str_field(payload, "body").unwrap_or(""), Vec::<&str>::new(), hints, timeout),
        )
        .map_err(|e| format!("notify: the notification server did not take it: {e}"))?;
    let id: u32 = reply.body().deserialize().map_err(|e| format!("notify: unexpected answer: {e}"))?;
    Ok(json!(id))
}

#[cfg(test)]
mod tests;
