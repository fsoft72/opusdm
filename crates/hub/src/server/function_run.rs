//! The Function-run state machine: starting a run, collecting the SOURCE and
//! DEST selections it needs, relaying its events to the Lister that started it
//! (conflict and confirm prompts, `Run` output windows) and cleaning up when a
//! Lister goes away. These are `impl Hub` methods split out of `server.rs`.

use super::*;
use crate::functions::FnEvent;

/// Switch the virtual desktop by `delta` (a `DesktopPrev` / `DesktopNext`
/// step): on Wayland through the compositor session, on X11 through EWMH.
fn switch_desktop(delta: i32) {
    if opusdm_ipc::session::is_wayland() {
        if let Some(session) = opusdm_wayland::shared() {
            session.switch_workspace(delta);
        }
        return;
    }
    if !opusdm_ipc::session::is_x11() {
        return;
    }
    if let Err(e) = opusdm_ui::with_x11(|x11| x11.switch_desktop(delta)) {
        opusdm_ipc::err_log!("hub", "cannot switch the virtual desktop: {e}");
    }
}

/// Why a run from a tool window (Manage Functions' Test) did not start.
pub(super) const NO_SOURCE_ERROR: &str = "There is no SOURCE Lister: select a Lister as SOURCE, then try again.";

/// Whether `steps` is not empty and only runs scripts: such a run may start
/// with no SOURCE Lister.
pub(super) fn only_scripts(steps: &[FnStep]) -> bool {
    !steps.is_empty() && steps.iter().all(|s| matches!(s, FnStep::Internal { cmd: opusdm_ipc::function::InternalCmd::Script, .. }))
}

/// The steps of the function or inline command `id` names: a saved Function
/// from `functions` (it must exist and be valid), or the steps of an inline
/// reference (`bash:`, `exec:`, ...), whose program the run waits for when
/// `wait` is set. The error says why nothing can run.
pub(super) fn resolve_steps(functions: &[opusdm_ipc::function::Function], id: &str, wait: bool) -> Result<Vec<FnStep>, String> {
    let reference = opusdm_ipc::funcref::parse(id);
    if !matches!(reference, opusdm_ipc::funcref::FuncRef::Saved(_)) {
        return opusdm_ipc::funcref::to_steps_with(&reference, wait);
    }
    let function = functions.iter().find(|f| f.id == id).ok_or_else(|| format!("function '{id}' not found (it may have been deleted)"))?;
    if let Some(first) = opusdm_ipc::function::validate(function).first() {
        return Err(format!("function '{}' is invalid: {}", function.name, first.message));
    }
    Ok(function.steps.clone())
}

/// A Function run waiting on `SelectionReply` from SOURCE and/or DEST,
/// tracked between `ToHub::RunFunction` and the point every slot it needs
/// has been filled.
pub(super) struct PendingFunctionRun {
    pub(super) requester_cid: Option<u64>,
    pub(super) steps: Vec<FnStep>,
    pub(super) source_cid: u64,
    pub(super) dest_cid: Option<u64>,
    pub(super) source_sel: Option<SelectionCtx>,
    pub(super) dest_sel: Option<SelectionCtx>,
}

impl Hub {
    /// Start a Function run for `requester`. With `source == Some(cid)` the
    /// hub asks `cid` (SOURCE) and, if one exists, the first DEST Lister other
    /// than `cid` for their selection first. With `None` there is no Lister to
    /// ask: the run starts at once with an empty selection in `$HOME`.
    /// Returns the run id.
    pub(super) fn start_function_run(&mut self, requester: Option<u64>, source: Option<u64>, steps: Vec<FnStep>) -> u64 {
        let run_id = self.runs.next_id();
        let Some(source_cid) = source else {
            let empty = SelectionCtx { paths: Vec::new(), cwd: home_dir() };
            self.spawn_run(run_id, requester, steps, empty);
            return run_id;
        };
        // Exclude the source itself: a Lister that currently holds a DEST role
        // can still be the one requesting the run. Picking it as its own
        // `dest_cid` would make `selection_reply` only ever fill `source_sel`
        // (the two branches are `if`/`else if` on the same `cid`), so
        // `dest_sel` - and the run - could never complete.
        let dest_cid = self.roles.dests().iter().map(|d| d.0).find(|&d| d != source_cid);
        self.runs.add_pending(
            run_id,
            PendingFunctionRun {
                requester_cid: requester,
                steps,
                source_cid,
                dest_cid,
                source_sel: None,
                dest_sel: None,
            },
        );
        self.query_selection(source_cid, run_id);
        if let Some(d) = dest_cid {
            self.query_selection(d, run_id);
        }
        run_id
    }

    /// Start the executor of run `run_id` at once, on `selection` and with no
    /// DEST.
    fn spawn_run(&mut self, run_id: u64, requester: Option<u64>, steps: Vec<FnStep>, selection: SelectionCtx) {
        let cancel = self.runs.start(run_id, requester);
        crate::functions::spawn_executor(run_id, steps, selection, None, self.runs.events_tx(), cancel);
    }

    /// Queue `run_id` on `cid`'s FIFO of pending selection requests and ask
    /// the Lister for its selection.
    pub(super) fn query_selection(&mut self, cid: u64, run_id: u64) {
        self.runs.wait_selection(cid, run_id);
        self.send_to(cid, ToLister::QuerySelection);
    }

    /// Run the inline command `id` names (`bash:`, `exec:`), else load the
    /// saved function `id`, validate it and start it for `requester`
    /// (see [`Self::start_function_run`]). A missing or invalid function is
    /// reported through [`Self::function_failed`]. Returns the run id.
    pub(super) fn run_function_by_id(&mut self, requester: Option<u64>, source: Option<u64>, id: &str) -> Option<u64> {
        let steps = self.steps_for(requester, id)?;
        Some(self.start_function_run(requester, source, steps))
    }

    /// Run the function or inline command `id` names on the files `paths`
    /// (the selection it sees, with their folder as working directory), with
    /// no Lister involved. Used by the desktop, whose icons are not in a
    /// Lister. Returns the run id.
    pub(super) fn run_on_paths(&mut self, id: &str, paths: Vec<PathBuf>) -> Option<u64> {
        let steps = self.steps_for(None, id)?;
        let cwd = paths.first().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(home_dir);
        let run_id = self.runs.next_id();
        self.spawn_run(run_id, None, steps, SelectionCtx { paths, cwd });
        Some(run_id)
    }

    /// The steps of the function or inline command `id` names. A missing or
    /// invalid one is reported through [`Self::function_failed`].
    fn steps_for(&mut self, requester: Option<u64>, id: &str) -> Option<Vec<FnStep>> {
        match resolve_steps(&self.current_functions, id, false) {
            Ok(steps) => Some(steps),
            Err(e) => {
                self.function_failed(requester, e);
                None
            }
        }
    }

    /// Run the saved function `id` on the hub's own initiative: SOURCE is the
    /// Lister holding the SOURCE role (if it is registered), else nothing.
    pub(super) fn run_headless(&mut self, id: &str) -> Option<u64> {
        let source = self
            .roles
            .source()
            .map(|l| l.0)
            .filter(|cid| self.conns.get(cid).is_some_and(|c| c.registered));
        self.run_function_by_id(None, source, id)
    }

    /// The SOURCE for a run requested by `cid`: `cid` itself when it is a
    /// registered Lister, else (a tool window such as Manage Functions) the
    /// registered Lister holding the SOURCE role. `None` when there is no
    /// such Lister: the run must not start.
    pub(super) fn run_source_for(&self, cid: u64) -> Option<u64> {
        if self.conns.get(&cid).is_some_and(|c| c.registered) {
            return Some(cid);
        }
        self.roles.source().map(|l| l.0).filter(|id| self.conns.get(id).is_some_and(|c| c.registered))
    }

    /// Tell `requester` a run could not start. A hub-originated run has no
    /// Lister to tell, so the failure goes to the log.
    pub(super) fn function_failed(&mut self, requester: Option<u64>, error: String) {
        let Some(requester) = requester else {
            opusdm_ipc::err_log!("hub", "function run failed: {error}");
            return;
        };
        self.send_to(requester, ToLister::FunctionFailed { step_idx: 0, error });
    }

    /// `ToHub::SelectionReply` from `cid`: fill in whichever slot (source or
    /// dest) of its pending run this answers, and start execution once every
    /// slot the run needs is filled.
    pub(super) fn selection_reply(&mut self, cid: u64, paths: Vec<PathBuf>, cwd: PathBuf) {
        let Some((run_id, run)) = self.runs.take_selection(cid, paths, cwd) else { return };
        let Some(source_sel) = run.source_sel else { return };
        let cancel = self.runs.start(run_id, run.requester_cid);
        crate::functions::spawn_executor(run_id, run.steps, source_sel, run.dest_sel, self.runs.events_tx(), cancel);
    }

    /// A `FnEvent` arrived from a running Function's executor thread. It is
    /// a request a step waits on, an action for the hub to apply, or
    /// progress for the Lister that asked for the run; each kind has its own
    /// handler. Actions and progress of a run the hub no longer knows (its
    /// Lister closed) are dropped.
    pub(super) fn function_event(&mut self, run_id: u64, ev: FnEvent) {
        let Some(ev) = self.function_request(run_id, ev) else { return };
        let Some(requester) = self.runs.requester(run_id) else { return };
        let Some(ev) = self.function_action(ev) else { return };
        self.function_progress(run_id, requester, ev);
    }

    /// Handle `ev` when it is a request a step of run `run_id` waits on (a
    /// file job, an output window, a question, a script query); give it back
    /// otherwise.
    fn function_request(&mut self, run_id: u64, ev: FnEvent) -> Option<FnEvent> {
        match ev {
            FnEvent::Job { spec, done, .. } => self.function_job(run_id, spec, done),
            FnEvent::Window { step_idx, msg } => self.run_window_event(run_id, step_idx, msg),
            FnEvent::Confirm { message, reply, .. } => self.function_confirm(run_id, message, reply),
            FnEvent::Alert { message, reply, .. } => self.function_alert(run_id, message, reply),
            FnEvent::ScriptQuery { query, reply } => {
                let _ = reply.send(self.script_query(query));
            }
            other => return Some(other),
        }
        None
    }

    /// Apply `ev` when it is an action for the hub (background, new Listers,
    /// left-out icons, hiding, virtual desktops, banks); give it back
    /// otherwise. Actions apply for every run, hub-originated ones (script
    /// events) included; the requester still gets the step's Output and
    /// StepDone lines from the executor.
    fn function_action(&mut self, ev: FnEvent) -> Option<FnEvent> {
        match ev {
            FnEvent::SetBackground { path, fit } => self.apply_background_image(path, fit),
            FnEvent::ChangeDir { path, new_lister: true, .. } => self.spawn_lister(Some(path)),
            FnEvent::NewLister { path } => self.spawn_lister(path),
            FnEvent::LeaveOut { paths } => self.leave_out(paths),
            FnEvent::PutAway { paths } => {
                send_or_log(&self.evt_tx, HubEvent::LeftOutRemove(paths), "GTK event");
            }
            FnEvent::Hide => self.set_hidden(true),
            FnEvent::Reveal => self.set_hidden(false),
            FnEvent::ToggleHide => self.set_hidden(!self.hidden),
            FnEvent::Desktop { delta } => switch_desktop(delta),
            FnEvent::Bank { action, bank } => {
                send_or_log(&self.evt_tx, HubEvent::Bank { action, bank }, "GTK event");
            }
            other => return Some(other),
        }
        None
    }

    /// Report progress `ev` of run `run_id` to `requester`, the Lister that
    /// asked for the run; a hub-originated run (`None`) has nobody to tell,
    /// so its failures are logged instead. A run that ended is forgotten.
    fn function_progress(&mut self, run_id: u64, requester: Option<u64>, ev: FnEvent) {
        let msg = match ev {
            FnEvent::ChangeDir { path, select, mode, .. } => ToLister::ScanDir { path, select, mode },
            FnEvent::Output { step_idx, line } => ToLister::FunctionOutput { step_idx, line },
            FnEvent::StepDone { step_idx, exit_code } => ToLister::FunctionStepDone { step_idx, exit_code },
            FnEvent::Failed { step_idx, error } => ToLister::FunctionFailed { step_idx, error },
            FnEvent::AllDone => ToLister::FunctionDone,
            FnEvent::Waiting { program, .. } => ToLister::FunctionWaiting { run_id, program },
            FnEvent::WaitEnded { .. } => ToLister::FunctionWaitEnded { run_id },
            // Requests and actions: handled before this point.
            FnEvent::Job { .. }
            | FnEvent::Window { .. }
            | FnEvent::Confirm { .. }
            | FnEvent::Alert { .. }
            | FnEvent::ScriptQuery { .. }
            | FnEvent::SetBackground { .. }
            | FnEvent::NewLister { .. }
            | FnEvent::LeaveOut { .. }
            | FnEvent::PutAway { .. }
            | FnEvent::Hide
            | FnEvent::Reveal
            | FnEvent::ToggleHide
            | FnEvent::Desktop { .. }
            | FnEvent::Bank { .. } => return,
        };
        if matches!(msg, ToLister::FunctionFailed { .. } | ToLister::FunctionDone) {
            self.runs.finish(run_id);
            self.event_guard.finish(run_id);
        }
        let Some(requester) = requester else {
            match msg {
                ToLister::FunctionFailed { step_idx, error } => {
                    opusdm_ipc::err_log!("hub", "function run {run_id} failed at step {step_idx}: {error}");
                }
                ToLister::FunctionOutput { step_idx, line } => {
                    opusdm_ipc::debug_log!("hub", "function run {run_id} step {step_idx}: {line}");
                }
                _ => {}
            }
            return;
        };
        // An older Lister closes its window on a message it cannot decode.
        let wait_msg = matches!(msg, ToLister::FunctionWaiting { .. } | ToLister::FunctionWaitEnded { .. });
        if wait_msg && !self.speaks(requester, opusdm_ipc::FUNCTION_WAIT_PROTOCOL) {
            return;
        }
        self.send_to(requester, msg);
    }

    /// A file-operation step of run `run_id` wants `spec` run: submit it to
    /// the job engine on behalf of the run's requester, so it gets a progress
    /// window, conflict prompts and the Lister refresh like any other job, and
    /// answer the step on `done` when the job ends. A run nobody owns any more
    /// drops `done`, which fails the step.
    pub(super) fn function_job(&mut self, run_id: u64, spec: opusdm_fsops::JobSpec, done: Sender<crate::functions::JobResult>) {
        let Some(requester) = self.runs.requester(run_id) else { return };
        self.submit_job(requester, spec, Some(done), 0);
    }

    /// Route one output-window event. `Open` records which Lister owns the
    /// window (the one that started the run); later events go to it even
    /// after the run itself has finished. A run nobody watches has no window.
    pub(super) fn run_window_event(&mut self, run_id: u64, step_idx: usize, msg: crate::functions::WindowMsg) {
        use crate::functions::WindowMsg;
        let key = (run_id, step_idx);
        if matches!(msg, WindowMsg::Open { .. }) {
            // A run the hub started on its own (global hotkey, script event)
            // has no requester: its window goes to the SOURCE Lister, if any.
            let owner = self.runs.requester(run_id).flatten().or_else(|| {
                self.roles.source().map(|l| l.0).filter(|cid| self.conns.get(cid).is_some_and(|c| c.registered))
            });
            let Some(cid) = owner else { return };
            self.output_windows.open(key, cid);
        }
        let Some(window) = self.output_windows.get(key) else { return };
        let (cid, id) = (window.owner, window.id);
        let message = match msg {
            WindowMsg::Open { title } => ToLister::OutputOpen { id, title },
            WindowMsg::Lines(lines) => ToLister::OutputLines { id, lines },
            WindowMsg::Closed(exit) => {
                self.output_windows.close(key);
                ToLister::OutputEnd { id, exit }
            }
        };
        match self.conns.get(&cid) {
            Some(c) => {
                c.queue(cid, message);
            }
            None => self.output_windows.close(key),
        }
    }

    /// A `Confirm` step of run `run_id` wants an answer from the Lister that
    /// started the run. Without one (script event, headless, Lister gone) the
    /// reply is dropped, which fails the step: a confirmation nobody gave is
    /// not a yes.
    pub(super) fn function_confirm(&mut self, run_id: u64, message: String, reply: Sender<bool>) {
        let owner = self
            .runs
            .requester(run_id)
            .flatten()
            .filter(|cid| self.conns.contains_key(cid));
        let Some(cid) = owner else {
            opusdm_ipc::err_log!("hub", "function run {run_id}: Confirm has nobody to ask, the step fails");
            return;
        };
        let req_id = self.next_req;
        self.next_req += 1;
        self.confirms.insert(req_id, (reply, cid));
        self.send_to(cid, ToLister::ConfirmPrompt { req_id, message });
    }

    /// A script's `opus.alert` of run `run_id`: show `message` on the Lister
    /// that started the run. Without one the message goes to the log and the
    /// reply is sent at once, so the script is not stopped by a dialog nobody
    /// could see.
    pub(super) fn function_alert(&mut self, run_id: u64, message: String, reply: Sender<bool>) {
        let owner = self
            .runs
            .requester(run_id)
            .flatten()
            .filter(|cid| self.conns.contains_key(cid));
        let Some(cid) = owner else {
            opusdm_ipc::err_log!("hub", "function run {run_id}: alert with nobody to show it to: {message}");
            let _ = reply.send(true);
            return;
        };
        let req_id = self.next_req;
        self.next_req += 1;
        self.confirms.insert(req_id, (reply, cid));
        self.send_to(cid, ToLister::AlertPrompt { req_id, message });
    }

    /// Answer a script's question (see `crate::scripting::ScriptQuery`).
    pub(super) fn script_query(&mut self, query: crate::scripting::ScriptQuery) -> crate::scripting::QueryReply {
        use crate::scripting::ScriptQuery;
        match query {
            ScriptQuery::Listers => {
                let source = self.roles.source().map(|l| l.0);
                let dests: Vec<u64> = self.roles.dests().iter().map(|l| l.0).collect();
                let mut cids: Vec<u64> = self.conns.iter().filter(|(_, c)| c.registered).map(|(cid, _)| *cid).collect();
                cids.sort_unstable();
                let listers = cids
                    .into_iter()
                    .map(|cid| {
                        let role = if source == Some(cid) {
                            "source"
                        } else if dests.contains(&cid) {
                            "dest"
                        } else {
                            "none"
                        };
                        serde_json::json!({ "id": cid, "role": role, "path": self.conns[&cid].path.display().to_string() })
                    })
                    .collect();
                Ok(serde_json::Value::Array(listers))
            }
            ScriptQuery::Functions => serde_json::to_value(&self.current_functions).map_err(|e| e.to_string()),
            ScriptQuery::GoTo { lister, path } => {
                if !self.conns.get(&lister).is_some_and(|c| c.registered) {
                    return Err(format!("Lister {lister} is not open"));
                }
                self.send_to_lister(lister, ToLister::GoTo { path: path.clone() });
                Ok(serde_json::json!({ "path": path.display().to_string() }))
            }
            ScriptQuery::Refresh { path } => {
                self.broadcast(ToLister::InvalidatePath(path));
                Ok(serde_json::Value::Null)
            }
        }
    }

    /// Clean up any Function-run bookkeeping referencing `cid`, called when
    /// its connection drops. A pending run waiting on `cid` for one of its
    /// two selections can now never complete (its other side, if any, would
    /// otherwise wait forever); a running Function's events for a
    /// since-dropped requester are also dropped instead of accumulating.
    pub(super) fn drop_function_state(&mut self, cid: u64) {
        // Pending runs waiting on `cid` are dropped; the runs of a closed
        // Lister stop at their next step, and the jobs they started are cancelled.
        for run_id in self.runs.forget_connection(cid) {
            self.event_guard.finish(run_id);
        }
        let orphaned_jobs: Vec<u64> = self
            .jobs
            .iter()
            .filter(|(_, meta)| meta.requester == Some(cid) && meta.waiter.is_some())
            .map(|(job_id, _)| *job_id)
            .collect();
        for job_id in orphaned_jobs {
            self.engine.cancel(job_id);
        }
        // A question to a closed Lister is never answered: dropping the sender
        // fails the waiting step instead of hanging it.
        self.confirms.retain(|_, (_, owner)| *owner != cid);
        self.output_windows.forget_owner(cid);
        // Dropping the reply senders releases the jobs waiting on this Lister.
        let orphaned: Vec<u64> =
            self.conflict_owners.iter().filter(|(_, owner)| **owner == cid).map(|(req, _)| *req).collect();
        for req_id in orphaned {
            self.conflict_owners.remove(&req_id);
            self.replies.remove(&req_id);
        }
    }
}
