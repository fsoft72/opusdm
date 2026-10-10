//! The Function-run state machine: starting a run, collecting the SOURCE and
//! DEST selections it needs, relaying its events to the Lister that started it
//! (conflict and confirm prompts, `Run` output windows) and cleaning up when a
//! Lister goes away. These are `impl Hub` methods split out of `server.rs`.

use super::*;

/// Why a run from a tool window (Manage Functions' Test) did not start.
pub(super) const NO_SOURCE_ERROR: &str = "There is no SOURCE Lister: select a Lister as SOURCE, then try again.";

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
        let run_id = self.next_run_id;
        self.next_run_id += 1;
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
        self.pending_functions.insert(
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
        self.function_requesters.insert(run_id, requester);
        let cancel = CancelToken::new();
        self.function_cancels.insert(run_id, cancel.clone());
        crate::functions::spawn_executor(run_id, steps, selection, None, self.function_events_tx.clone(), cancel);
    }

    /// Queue `run_id` on `cid`'s FIFO of pending selection requests and ask
    /// the Lister for its selection.
    pub(super) fn query_selection(&mut self, cid: u64, run_id: u64) {
        self.selection_waiters.entry(cid).or_default().push_back(run_id);
        if let Some(c) = self.conns.get_mut(&cid) {
            let _ = c.tx.send(ToLister::QuerySelection.into());
        }
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
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        self.spawn_run(run_id, None, steps, SelectionCtx { paths, cwd });
        Some(run_id)
    }

    /// The steps of the function or inline command `id` names. A missing or
    /// invalid one is reported through [`Self::function_failed`].
    fn steps_for(&mut self, requester: Option<u64>, id: &str) -> Option<Vec<FnStep>> {
        let reference = opusdm_ipc::funcref::parse(id);
        if !matches!(reference, opusdm_ipc::funcref::FuncRef::Saved(_)) {
            return match opusdm_ipc::funcref::to_steps(&reference) {
                Ok(steps) => Some(steps),
                Err(e) => {
                    self.function_failed(requester, e);
                    None
                }
            };
        }
        let Some(function) = self.current_functions.iter().find(|f| f.id == id).cloned() else {
            self.function_failed(requester, format!("function '{id}' not found (it may have been deleted)"));
            return None;
        };
        let problems = opusdm_ipc::function::validate(&function);
        if let Some(first) = problems.first() {
            self.function_failed(requester, format!("function '{}' is invalid: {}", function.name, first.message));
            return None;
        }
        Some(function.steps)
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
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(ToLister::FunctionFailed { step_idx: 0, error }.into());
        }
    }

    /// `ToHub::SelectionReply` from `cid`: fill in whichever slot (source or
    /// dest) of its pending run this answers, and start execution once every
    /// slot the run needs is filled.
    pub(super) fn selection_reply(&mut self, cid: u64, paths: Vec<PathBuf>, cwd: PathBuf) {
        let Some(queue) = self.selection_waiters.get_mut(&cid) else { return };
        let Some(run_id) = queue.pop_front() else { return };
        if queue.is_empty() {
            self.selection_waiters.remove(&cid);
        }
        let Some(run) = self.pending_functions.get_mut(&run_id) else { return };
        let sel = SelectionCtx { paths, cwd };
        if cid == run.source_cid {
            run.source_sel = Some(sel);
        } else if Some(cid) == run.dest_cid {
            run.dest_sel = Some(sel);
        }
        let ready = run.source_sel.is_some() && (run.dest_cid.is_none() || run.dest_sel.is_some());
        if !ready {
            return;
        }
        let run = self.pending_functions.remove(&run_id).unwrap();
        self.function_requesters.insert(run_id, run.requester_cid);
        let cancel = CancelToken::new();
        self.function_cancels.insert(run_id, cancel.clone());
        crate::functions::spawn_executor(
            run_id,
            run.steps,
            run.source_sel.unwrap(),
            run.dest_sel,
            self.function_events_tx.clone(),
            cancel,
        );
    }

    /// A `FnEvent` arrived from a running Function's executor thread; turn
    /// it into the matching `ToLister` and send it to whichever connection
    /// asked for this run. A hub-originated run has no such connection: its
    /// failures are logged instead.
    pub(super) fn function_event(&mut self, run_id: u64, ev: crate::functions::FnEvent) {
        if let crate::functions::FnEvent::Job { spec, done, .. } = ev {
            self.function_job(run_id, spec, done);
            return;
        }
        if let crate::functions::FnEvent::Window { step_idx, msg, .. } = &ev {
            self.run_window_event(run_id, *step_idx, msg.clone(), &ev);
            return;
        }
        if let crate::functions::FnEvent::Confirm { message, reply, .. } = ev {
            self.function_confirm(run_id, message, reply);
            return;
        }
        let Some(&requester) = self.function_requesters.get(&run_id) else { return };
        let terminal = matches!(ev, crate::functions::FnEvent::Failed { .. } | crate::functions::FnEvent::AllDone);
        if terminal {
            self.function_requesters.remove(&run_id);
            self.function_cancels.remove(&run_id);
            self.event_guard.finish(run_id);
        }
        // A background change applies for every requester, hub-originated
        // runs (script events) included; the requester still gets the
        // step's Output and StepDone lines from the executor.
        if let crate::functions::FnEvent::SetBackground { path, fit } = &ev {
            self.apply_background_image(path.clone(), *fit);
            return;
        }
        match &ev {
            crate::functions::FnEvent::ChangeDir { path, new_lister: true, .. } => {
                self.spawn_lister(Some(path.clone()));
                return;
            }
            crate::functions::FnEvent::LeaveOut { paths } => {
                self.leave_out(paths.clone());
                return;
            }
            crate::functions::FnEvent::PutAway { paths } => {
                let _ = self.evt_tx.send(HubEvent::LeftOutRemove(paths.clone()));
                return;
            }
            crate::functions::FnEvent::Hide => {
                self.set_hidden(true);
                return;
            }
            crate::functions::FnEvent::Reveal => {
                self.set_hidden(false);
                return;
            }
            crate::functions::FnEvent::ToggleHide => {
                self.set_hidden(!self.hidden);
                return;
            }
            crate::functions::FnEvent::Desktop { delta } => {
                if opusdm_ipc::session::is_wayland() {
                    if let Some(session) = opusdm_wayland::shared() { session.switch_workspace(*delta); }
                    return;
                }
                if !opusdm_ipc::session::is_x11() { return; }
                if let Err(e) = opusdm_ui::with_x11(|x11| x11.switch_desktop(*delta)) {
                    opusdm_ipc::err_log!("hub", "cannot switch the virtual desktop: {e}");
                }
                return;
            }
            crate::functions::FnEvent::Bank { action, bank } => {
                let _ = self.evt_tx.send(HubEvent::Bank { action: *action, bank: bank.clone() });
                return;
            }
            _ => {}
        }
        let Some(requester) = requester else {
            match ev {
                crate::functions::FnEvent::Failed { step_idx, error } => {
                    opusdm_ipc::err_log!("hub", "function run {run_id} failed at step {step_idx}: {error}");
                }
                crate::functions::FnEvent::Output { step_idx, line } => {
                    opusdm_ipc::debug_log!("hub", "function run {run_id} step {step_idx}: {line}");
                }
                _ => {}
            }
            return;
        };
        let msg = match ev {
            crate::functions::FnEvent::ChangeDir { path, select, mode, .. } => ToLister::ScanDir { path, select, mode },
            // Applied above, before this match.
            crate::functions::FnEvent::SetBackground { .. }
            | crate::functions::FnEvent::Job { .. }
            | crate::functions::FnEvent::Confirm { .. }
            | crate::functions::FnEvent::Window { .. } => return,
            crate::functions::FnEvent::LeaveOut { .. }
            | crate::functions::FnEvent::PutAway { .. }
            | crate::functions::FnEvent::Hide
            | crate::functions::FnEvent::Reveal
            | crate::functions::FnEvent::ToggleHide
            | crate::functions::FnEvent::Desktop { .. }
            | crate::functions::FnEvent::Bank { .. } => return,
            crate::functions::FnEvent::Output { step_idx, line } => ToLister::FunctionOutput { step_idx, line },
            crate::functions::FnEvent::StepDone { step_idx, exit_code } => {
                ToLister::FunctionStepDone { step_idx, exit_code }
            }
            crate::functions::FnEvent::Failed { step_idx, error } => ToLister::FunctionFailed { step_idx, error },
            crate::functions::FnEvent::AllDone => ToLister::FunctionDone,
        };
        if let Some(c) = self.conns.get_mut(&requester) {
            let _ = c.tx.send(msg.into());
        }
    }

    /// A file-operation step of run `run_id` wants `spec` run: submit it to
    /// the job engine on behalf of the run's requester, so it gets a progress
    /// window, conflict prompts and the Lister refresh like any other job, and
    /// answer the step on `done` when the job ends. A run nobody owns any more
    /// drops `done`, which fails the step.
    pub(super) fn function_job(&mut self, run_id: u64, spec: opusdm_fsops::JobSpec, done: Sender<crate::functions::JobResult>) {
        let Some(&requester) = self.function_requesters.get(&run_id) else { return };
        self.submit_job(requester, spec, Some(done));
    }

    /// Route one output-window event. `Open` records which Lister owns the
    /// window (the one that started the run); later events go to it even
    /// after the run itself has finished. A run nobody watches has no window.
    pub(super) fn run_window_event(&mut self, run_id: u64, step_idx: usize, msg: crate::functions::WindowMsg, ev: &crate::functions::FnEvent) {
        use crate::functions::WindowMsg;
        let key = (run_id, step_idx);
        let id = run_id * 1000 + step_idx as u64;
        if msg == WindowMsg::Open {
            // A run the hub started on its own (global hotkey, script event)
            // has no requester: its window goes to the SOURCE Lister, if any.
            let owner = self.function_requesters.get(&run_id).copied().flatten().or_else(|| {
                self.roles.source().map(|l| l.0).filter(|cid| self.conns.get(cid).is_some_and(|c| c.registered))
            });
            match owner {
                Some(cid) => {
                    self.output_windows.insert(key, cid);
                }
                None => return,
            }
        }
        let Some(&cid) = self.output_windows.get(&key) else { return };
        let message = match (msg, ev) {
            (WindowMsg::Open, crate::functions::FnEvent::Window { title, .. }) => ToLister::OutputOpen { id, title: title.clone() },
            (WindowMsg::Line(line), _) => ToLister::OutputLine { id, line },
            (WindowMsg::Closed(exit), _) => {
                self.output_windows.remove(&key);
                ToLister::OutputEnd { id, exit }
            }
            _ => return,
        };
        match self.conns.get_mut(&cid) {
            Some(c) => {
                let _ = c.tx.send(message.into());
            }
            None => {
                self.output_windows.remove(&key);
            }
        }
    }

    /// A `Confirm` step of run `run_id` wants an answer from the Lister that
    /// started the run. Without one (script event, headless, Lister gone) the
    /// reply is dropped, which fails the step: a confirmation nobody gave is
    /// not a yes.
    pub(super) fn function_confirm(&mut self, run_id: u64, message: String, reply: Sender<bool>) {
        let owner = self
            .function_requesters
            .get(&run_id)
            .copied()
            .flatten()
            .filter(|cid| self.conns.contains_key(cid));
        let Some(cid) = owner else {
            opusdm_ipc::err_log!("hub", "function run {run_id}: Confirm has nobody to ask, the step fails");
            return;
        };
        let req_id = self.next_req;
        self.next_req += 1;
        self.confirms.insert(req_id, (reply, cid));
        if let Some(c) = self.conns.get_mut(&cid) {
            let _ = c.tx.send(ToLister::ConfirmPrompt { req_id, message }.into());
        }
    }

    /// Clean up any Function-run bookkeeping referencing `cid`, called when
    /// its connection drops. A pending run waiting on `cid` for one of its
    /// two selections can now never complete (its other side, if any, would
    /// otherwise wait forever); a running Function's events for a
    /// since-dropped requester are also dropped instead of accumulating.
    pub(super) fn drop_function_state(&mut self, cid: u64) {
        let doomed: Vec<u64> = self
            .pending_functions
            .iter()
            .filter(|(_, run)| run.source_cid == cid || run.dest_cid == Some(cid))
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in doomed {
            self.pending_functions.remove(&run_id);
            self.event_guard.finish(run_id);
            for queue in self.selection_waiters.values_mut() {
                queue.retain(|&r| r != run_id);
            }
        }
        self.selection_waiters.remove(&cid);
        self.selection_waiters.retain(|_, queue| !queue.is_empty());
        // The runs of a closed Lister stop at their next step, and the jobs
        // they started are cancelled.
        for (run_id, requester) in &self.function_requesters {
            if *requester == Some(cid) {
                if let Some(cancel) = self.function_cancels.remove(run_id) {
                    cancel.cancel();
                }
            }
        }
        self.function_requesters.retain(|_, requester| *requester != Some(cid));
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
        self.output_windows.retain(|_, owner| *owner != cid);
        // Dropping the reply senders releases the jobs waiting on this Lister.
        let orphaned: Vec<u64> =
            self.conflict_owners.iter().filter(|(_, owner)| **owner == cid).map(|(req, _)| *req).collect();
        for req_id in orphaned {
            self.conflict_owners.remove(&req_id);
            self.replies.remove(&req_id);
        }
    }
}
