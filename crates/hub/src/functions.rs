//! Executes a Function's steps on their own thread once the hub has
//! collected the SOURCE and (if any) DEST selection: internal steps reuse
//! `opusdm_fsops::run_job` or a direct `std::fs::rename`, external steps
//! spawn a subprocess and stream its output. See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use opusdm_fsops::{run_job, CancelToken, JobSpec, Progress};
use opusdm_ipc::function::{expand_argv_element, expand_text, ExpandError, ExpandedArg, FnStep, InternalArgs, InternalCmd, SelectionCtx};
use opusdm_ipc::glob::expand_new_name;
use opusdm_ipc::buttonbank::BankAction;
use opusdm_ipc::{Op, Resolution};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// How often a step that blocks (a running program, an open question)
/// checks whether its run was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// The error text of a step stopped because its run was cancelled.
pub(crate) const CANCELLED: &str = "cancelled";

/// Why [`recv_or_cancel`] returned without an answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoAnswer {
    /// The run was cancelled while waiting.
    Cancelled,
    /// The other side dropped its sender without answering.
    Dropped,
}

/// Wait for an answer on `rx` (a dialog, a job), giving up as soon as
/// `cancel` is set.
pub(crate) fn recv_or_cancel<T>(rx: &Receiver<T>, cancel: &CancelToken) -> Result<T, NoAnswer> {
    loop {
        if cancel.is_cancelled() {
            return Err(NoAnswer::Cancelled);
        }
        match rx.recv_timeout(CANCEL_POLL) {
            Ok(answer) => return Ok(answer),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err(NoAnswer::Dropped),
        }
    }
}

/// Wait for `child` to exit (safe against the SIGCHLD reaper, see
/// `opusdm_ipc::reaped`), killing it when `cancel` is set. A cancelled wait
/// fails with [`CANCELLED`] once the killed child is reaped.
pub(crate) fn wait_or_kill(child: &mut Child, cancel: &CancelToken) -> Result<ExitStatus, String> {
    let mut lost_since = None;
    loop {
        if let Some(status) = opusdm_ipc::reaped::poll_exit(child, &mut lost_since).map_err(|e| format!("wait failed: {e}"))? {
            return Ok(status);
        }
        if cancel.is_cancelled() {
            if let Err(e) = child.kill() {
                opusdm_ipc::err_log!("hub", "cannot kill cancelled pid {}: {e}", child.id());
            }
            if let Err(e) = opusdm_ipc::reaped::wait(child) {
                opusdm_ipc::err_log!("hub", "cannot reap cancelled pid {}: {e}", child.id());
            }
            return Err(CANCELLED.to_string());
        }
        std::thread::sleep(CANCEL_POLL);
    }
}

/// One event a running Function reports, tagged with its `run_id` by the
/// caller when relayed onward (see `server.rs::function_event`).
pub enum FnEvent {
    /// One line of output (internal-step progress, or external-step
    /// stdout/stderr).
    Output { step_idx: usize, line: String },
    /// A step finished successfully.
    StepDone { step_idx: usize, exit_code: Option<i32> },
    /// A step failed; the run stops here.
    Failed { step_idx: usize, error: String },
    /// A `ChangeDir` step resolved `path`: the SOURCE Lister must go there,
    /// or a new Lister must open there (`new_lister`). `select` names an entry
    /// to select, `mode` is `details` or `icons`.
    ChangeDir { path: PathBuf, select: Option<String>, mode: Option<String>, new_lister: bool },
    /// A `NewLister` step: the hub must open a Lister on `path`, or on its
    /// default folder when `None`.
    NewLister { path: Option<PathBuf> },
    /// A `SetBackground` step validated `path` (canonical): the hub must
    /// install it as the desktop background. `fit` is `None` to keep the
    /// current one.
    SetBackground { path: PathBuf, fit: Option<opusdm_ipc::ImageFit> },
    /// A `LeaveOut` step validated `paths`: the hub must leave them out on
    /// the desktop.
    LeaveOut { paths: Vec<PathBuf> },
    /// A `PutAway` step: the hub must remove `paths` from the left-out icons.
    PutAway { paths: Vec<PathBuf> },
    /// A `Hide` step: the hub must hide the desktop's windows.
    Hide,
    /// A `Reveal` step: the hub must bring them back.
    Reveal,
    /// A `ToggleHide` step: the hub must flip the hidden state.
    ToggleHide,
    /// A `DesktopPrev` or `DesktopNext` step: the hub must switch the virtual
    /// desktop by `delta` (`-1` or `1`).
    Desktop { delta: i32 },
    /// A bank step: the hub must apply `action` to the bank called `bank`.
    Bank { action: opusdm_ipc::buttonbank::BankAction, bank: String },
    /// A Copy/Move/Delete/... step wants a file job run. The hub submits it to
    /// its job engine (progress window, conflict prompts, Lister refresh) and
    /// reports the outcome on `done`; a dropped `done` fails the step.
    Job { step_idx: usize, spec: JobSpec, done: Sender<JobResult> },
    /// A `Confirm` step asks the user `message`; `reply` gets their answer. A
    /// dropped `reply` (nobody to ask) fails the step.
    Confirm { step_idx: usize, message: String, reply: Sender<bool> },
    /// A script's `opus.alert`: show `message` and send on `reply` once the
    /// user closed the dialog. A dropped `reply` (nobody to show it to) is
    /// not an error, the script goes on.
    Alert { step_idx: usize, message: String, reply: Sender<bool> },
    /// A `Run` step with a window: the program's output. The window outlives
    /// the function, so these events are routed by `(run_id, step_idx)`.
    Window { step_idx: usize, title: String, msg: WindowMsg },
    /// A script asks the hub something only the hub knows (the open Listers,
    /// the saved Functions) or to act on a Lister; `reply` gets the answer. A
    /// dropped `reply` fails the script's call.
    ScriptQuery { query: crate::scripting::ScriptQuery, reply: Sender<crate::scripting::QueryReply> },
    /// Every step finished successfully.
    AllDone,
}

/// Outcome of a file job started by a step: `(copied, skipped)`, or why it
/// failed or was cancelled.
pub type JobResult = Result<(u64, u64), String>;

/// Run `spec` to completion on the calling thread (plus one worker thread) and
/// summarise it as a [`JobResult`]. `on_conflict` receives every
/// "destination exists" question with the channel to answer on; dropping that
/// channel without an answer ends the job. Used where there is no hub job
/// engine to run the job: shutdown hooks and tests.
pub fn run_job_inline(
    spec: JobSpec,
    cancel: &CancelToken,
    mut on_conflict: impl FnMut(PathBuf, opusdm_ipc::FileMeta, opusdm_ipc::FileMeta, Sender<Resolution>),
) -> JobResult {
    let (tx, rx) = channel::<Progress>();
    // `run_job` blocks until the job finishes, conflict replies included, so
    // it needs its own thread while this one drains `rx`.
    let cancel = cancel.clone();
    let job_thread = std::thread::spawn(move || run_job(spec, tx, cancel));
    let mut result = Err("job ended without a result".to_string());
    for ev in rx {
        match ev {
            Progress::Conflict { path, src, dst, reply } => on_conflict(path, src, dst, reply),
            Progress::Done { copied, skipped, cancelled, errors } => {
                result = if let Some((path, msg)) = errors.first() {
                    Err(format!("{} ({msg})", path.display()))
                } else if cancelled {
                    Err("cancelled".to_string())
                } else {
                    Ok((copied, skipped))
                };
            }
            Progress::File { .. } | Progress::Scanning { .. } => {}
        }
    }
    let _ = job_thread.join();
    result
}

/// What a `Run` output window is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowMsg {
    /// The program started: open the window.
    Open,
    /// One line of its output.
    Line(String),
    /// It ended with this exit code (`None` when killed by a signal).
    Closed(Option<i32>),
}

/// Spawn the thread that runs `steps` in order, reporting each event on
/// `events_tx` tagged with `run_id`.
pub fn spawn_executor(
    run_id: u64,
    steps: Vec<FnStep>,
    source: SelectionCtx,
    dest: Option<SelectionCtx>,
    events_tx: Sender<(u64, FnEvent)>,
    cancel: CancelToken,
) {
    std::thread::Builder::new()
        .name(format!("opusdm-function-{run_id}"))
        .spawn(move || run_steps_cancellable(run_id, &steps, &source, dest.as_ref(), &events_tx, &cancel))
        .expect("spawn function executor thread");
}

/// Run every step in order, stopping (and emitting `Failed`) at the first
/// error; emits `AllDone` only if every step succeeded. Free function (not
/// a method) so it is directly unit-testable without a running hub.
#[cfg(test)]
fn run_steps(
    run_id: u64,
    steps: &[FnStep],
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) {
    run_steps_cancellable(run_id, steps, source, dest, events_tx, &CancelToken::new());
}

/// [`run_steps`] that also stops, with a `Failed` event, before the first step
/// started after `cancel` was set (the requesting Lister went away).
fn run_steps_cancellable(
    run_id: u64,
    steps: &[FnStep],
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) {
    // A `ChangeDir` step moves the working directory of every later step.
    let mut source = source.clone();
    for (step_idx, step) in steps.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = events_tx.send((run_id, FnEvent::Failed { step_idx, error: "cancelled".to_string() }));
            return;
        }
        let result = run_step(run_id, step_idx, step, &mut source, dest, events_tx, cancel);
        if let Err(error) = result {
            let _ = events_tx.send((run_id, FnEvent::Failed { step_idx, error }));
            return;
        }
    }
    let _ = events_tx.send((run_id, FnEvent::AllDone));
}

/// Run one step on `source` (a `ChangeDir` step moves its cwd) and `dest`.
/// Shared by the step loop and by scripts, whose `opus.command` runs one
/// internal command at a time.
pub(crate) fn run_step(
    run_id: u64,
    step_idx: usize,
    step: &FnStep,
    source: &mut SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) -> Result<(), String> {
    match step {
        // One exhaustive match: a new `InternalCmd` does not compile until
        // it is dispatched here.
        FnStep::Internal { cmd, args } => match *cmd {
            InternalCmd::ChangeDir => run_change_dir(run_id, step_idx, args, source, dest, events_tx),
            InternalCmd::NewLister => run_new_lister(run_id, step_idx, args, source, dest, events_tx),
            InternalCmd::Script => crate::scripting::run_script(run_id, step_idx, args, source, dest, events_tx, cancel),
            InternalCmd::Run => run_run(run_id, step_idx, args, source, dest, events_tx, cancel),
            InternalCmd::Confirm => run_confirm(run_id, step_idx, args, source, dest, events_tx, cancel),
            InternalCmd::SetBackground => run_set_background(run_id, step_idx, args, source, dest, events_tx),
            InternalCmd::LeaveOut => run_leave_out(run_id, step_idx, false, source, events_tx),
            InternalCmd::PutAway => run_leave_out(run_id, step_idx, true, source, events_tx),
            InternalCmd::Hide | InternalCmd::Reveal | InternalCmd::ToggleHide => {
                run_hide(run_id, step_idx, *cmd, events_tx)
            }
            InternalCmd::DesktopPrev | InternalCmd::DesktopNext => run_desktop(run_id, step_idx, *cmd, events_tx),
            InternalCmd::OpenInputConfig => run_open_input_config(run_id, step_idx, events_tx),
            InternalCmd::OpenBank => run_bank(run_id, step_idx, BankAction::Open, args, bank_exists, events_tx),
            InternalCmd::CloseBank => run_bank(run_id, step_idx, BankAction::Close, args, bank_exists, events_tx),
            InternalCmd::ToggleBank => run_bank(run_id, step_idx, BankAction::Toggle, args, bank_exists, events_tx),
            InternalCmd::HideBank => run_bank(run_id, step_idx, BankAction::Hide, args, bank_exists, events_tx),
            InternalCmd::Rename => run_rename(run_id, step_idx, args, source, dest, events_tx),
            InternalCmd::MakeDir => run_make_entry(run_id, step_idx, false, args, source, dest, events_tx),
            InternalCmd::NewFile => run_make_entry(run_id, step_idx, true, args, source, dest, events_tx),
            InternalCmd::Copy => run_file_job(run_id, step_idx, FileJob::Copy, args, source, dest, events_tx),
            InternalCmd::Move => run_file_job(run_id, step_idx, FileJob::Move, args, source, dest, events_tx),
            InternalCmd::Delete => run_file_job(run_id, step_idx, FileJob::Delete, args, source, dest, events_tx),
            InternalCmd::CopyAs => run_file_job(run_id, step_idx, FileJob::CopyAs, args, source, dest, events_tx),
            InternalCmd::MoveAs => run_file_job(run_id, step_idx, FileJob::MoveAs, args, source, dest, events_tx),
            InternalCmd::Duplicate => run_file_job(run_id, step_idx, FileJob::Duplicate, args, source, dest, events_tx),
        },
        FnStep::External { argv, cwd_source } => {
            run_external(run_id, step_idx, ExternalCall { argv, script: None, cwd_source: *cwd_source }, source, dest, events_tx, cancel)
        }
    }
}

fn expand_err_msg(e: ExpandError) -> String {
    match e {
        ExpandError::MissingDest(token) => format!("{token}: no DEST Lister open"),
    }
}

/// Turn the user-typed `text` into a path: a leading `~` becomes `$HOME`,
/// anything else is returned as is (the caller joins it to the cwd).
/// `label` names the command in the error message.
fn expand_home(text: &str, label: &str) -> Result<PathBuf, String> {
    match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").ok_or_else(|| format!("{label}: HOME is not set"))?;
            Ok(PathBuf::from(home).join(rest.trim_start_matches('/')))
        }
        _ => Ok(PathBuf::from(text)),
    }
}

/// Run a `ChangeDir` step: expand the path template, resolve `~` and
/// relative paths against the SOURCE cwd, require an existing directory,
/// ask the hub to navigate the SOURCE Lister there, and make it the cwd of
/// every later step.
fn run_change_dir(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &mut SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let text = expand_text(args.get("path").unwrap_or(""), source, dest).map_err(expand_err_msg)?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Change Directory: path is empty".to_string());
    }
    let expanded = expand_home(text, "Change Directory")?;
    let target = source.cwd.join(expanded);
    let target = std::fs::canonicalize(&target)
        .map_err(|e| format!("Change Directory: cannot open {}: {e}", target.display()))?;
    // With `container`, a file means "its folder, with the file selected".
    let (target, select) = if args.flag("container") && !Path::new(&target).is_dir() {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned());
        let parent = target.parent().map(Path::to_path_buf);
        match (parent, name) {
            (Some(parent), Some(name)) => (parent, Some(name)),
            _ => return Err(format!("Change Directory: {} has no folder", target.display())),
        }
    } else {
        (target, None)
    };
    if !Path::new(&target).is_dir() {
        return Err(format!("Change Directory: {} is not a directory", target.display()));
    }
    let mode = match args.get("mode").map(|m| m.trim().to_ascii_lowercase()).filter(|m| !m.is_empty()) {
        None => None,
        Some(m) if m == "details" || m == "icons" => Some(m),
        Some(other) => return Err(format!("Change Directory: the view must be details or icons, not {other:?}")),
    };
    source.cwd = target.clone();
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("cd {}", target.display()) }));
    let _ = events_tx.send((run_id, FnEvent::ChangeDir { path: target, select, mode, new_lister: args.flag("new") }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `NewLister` step: with a `path` (template, `~` and relative paths
/// resolved like `ChangeDir`) it must be an existing directory; without one
/// the hub opens the Lister on its default folder.
fn run_new_lister(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let text = expand_text(args.get("path").unwrap_or(""), source, dest).map_err(expand_err_msg)?;
    let text = text.trim();
    let path = if text.is_empty() {
        None
    } else {
        let target = source.cwd.join(expand_home(text, "New Lister")?);
        let target = std::fs::canonicalize(&target).map_err(|e| format!("New Lister: cannot open {}: {e}", target.display()))?;
        if !target.is_dir() {
            return Err(format!("New Lister: {} is not a directory", target.display()));
        }
        Some(target)
    };
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: InternalCmd::NewLister.label().to_string() }));
    let _ = events_tx.send((run_id, FnEvent::NewLister { path }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `SetBackground` step: expand the path template, resolve `~` and
/// relative paths against the SOURCE cwd, require an existing regular file
/// with a picture or video extension (checked on the name the user picked, so
/// a symlink `wall.png` is fine), parse the optional `fit`, and ask the hub to
/// make it the one desktop background. Nothing is emitted when any check fails.
fn run_set_background(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let text = expand_text(args.get("path").unwrap_or(""), source, dest).map_err(expand_err_msg)?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Set Background: path is empty".to_string());
    }
    let picked = source.cwd.join(expand_home(text, "Set Background")?);
    if !opusdm_ipc::is_background_media(&picked) {
        return Err(format!(
            "Set Background: \"{}\" is not a supported image or video (png, jpg, jpeg, webp, bmp, gif, mp4, mkv, webm, avi, mov, m4v, ogv)",
            picked.display()
        ));
    }
    let fit = match args.get("fit").map(str::trim).filter(|f| !f.is_empty()) {
        Some(text) => Some(opusdm_ipc::ImageFit::parse(text).ok_or_else(|| {
            format!("Set Background: unknown fit \"{text}\" (use fill, fit, center or tile)")
        })?),
        None => None,
    };
    let target = std::fs::canonicalize(&picked)
        .map_err(|e| format!("Set Background: cannot open {}: {e}", picked.display()))?;
    if !target.is_file() {
        return Err(format!("Set Background: {} is not a file", target.display()));
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("background {}", target.display()) }));
    let _ = events_tx.send((run_id, FnEvent::SetBackground { path: target, fit }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `LeaveOut` or (with `put_away`) `PutAway` step on the SOURCE
/// selection. The selection must not be empty; leaving out also requires
/// every path to be absolute and present. Nothing is emitted on failure.
fn run_leave_out(
    run_id: u64,
    step_idx: usize,
    put_away: bool,
    source: &SelectionCtx,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let label = if put_away { "Put Away" } else { "Leave Out" };
    if source.paths.is_empty() {
        return Err(format!("{label}: nothing selected"));
    }
    if !put_away {
        opusdm_ipc::leftout::check_leavable(&source.paths)?;
    }
    let paths = source.paths.clone();
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{label} {} item(s)", paths.len()) }));
    let event = if put_away { FnEvent::PutAway { paths } } else { FnEvent::LeaveOut { paths } };
    let _ = events_tx.send((run_id, event));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `Hide`, `Reveal` or `ToggleHide` step: no parameters and no selection
/// needed, the hub does the work when it receives the event.
fn run_hide(run_id: u64, step_idx: usize, cmd: InternalCmd, events_tx: &Sender<(u64, FnEvent)>) -> Result<(), String> {
    let event = match cmd {
        InternalCmd::Hide => FnEvent::Hide,
        InternalCmd::Reveal => FnEvent::Reveal,
        _ => FnEvent::ToggleHide,
    };
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: cmd.label().to_string() }));
    let _ = events_tx.send((run_id, event));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `DesktopPrev` or `DesktopNext` step: no parameters and no selection
/// needed, the hub switches the desktop when it receives the event.
fn run_desktop(run_id: u64, step_idx: usize, cmd: InternalCmd, events_tx: &Sender<(u64, FnEvent)>) -> Result<(), String> {
    let delta = if cmd == InternalCmd::DesktopPrev { -1 } else { 1 };
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: cmd.label().to_string() }));
    let _ = events_tx.send((run_id, FnEvent::Desktop { delta }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run an `OpenInputConfig` step: start the Input window. It needs no
/// selection and no parameters.
fn run_open_input_config(run_id: u64, step_idx: usize, events_tx: &Sender<(u64, FnEvent)>) -> Result<(), String> {
    crate::children::spawn_tool("opusdm-input-config").map_err(|e| format!("cannot start opusdm-input-config: {e}"))?;
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: InternalCmd::OpenInputConfig.label().to_string() }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Whether a button bank called `name` exists on disk (read through the
/// cache, so a bank step does not parse every bank file each time).
fn bank_exists(name: &str) -> bool {
    crate::config_cache::banks().get_by_name(name).is_some()
}

/// Run a `Confirm` step: expand the question, ask through the hub and wait
/// (until `cancel` is set). Anything but a yes stops the function.
fn run_confirm(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) -> Result<(), String> {
    let template = args.get("message").unwrap_or("");
    let message = expand_text(template, source, dest).map_err(expand_err_msg)?;
    if message.trim().is_empty() {
        return Err("Confirm: no question given".to_string());
    }
    let (reply, answer) = channel::<bool>();
    let _ = events_tx.send((run_id, FnEvent::Confirm { step_idx, message, reply }));
    match recv_or_cancel(&answer, cancel) {
        Ok(true) => {
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        Ok(false) => Err("Confirm: stopped by the user".to_string()),
        Err(NoAnswer::Cancelled) => Err(format!("Confirm: {CANCELLED}")),
        Err(NoAnswer::Dropped) => Err("Confirm: nobody was there to answer".to_string()),
    }
}

/// Run a bank step: check the named bank exists (`exists`), then hand the
/// action to the hub. No selection is needed. An empty or unknown name fails
/// the step and emits nothing.
fn run_bank(
    run_id: u64,
    step_idx: usize,
    action: opusdm_ipc::buttonbank::BankAction,
    args: &InternalArgs,
    exists: impl Fn(&str) -> bool,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let name = args.get("bank").map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err("Bank: no bank name given".to_string());
    }
    if !exists(name) {
        return Err(format!("Bank \"{name}\" not found"));
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{action:?} bank {name}") }));
    let _ = events_tx.send((run_id, FnEvent::Bank { action, bank: name.to_string() }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `Rename` step: rename the first selected path inside its own folder,
/// never replacing an existing entry.
fn run_rename(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let template = args.get("new_name").unwrap_or("");
    let new_name = expand_text(template, source, dest).map_err(expand_err_msg)?;
    let old = source.paths.first().ok_or_else(|| "Rename: nothing selected".to_string())?;
    let new_path = old.with_file_name(new_name.trim());
    opusdm_fsops::rename_noreplace(old, &new_path).map_err(|e| format!("rename failed: {e}"))?;
    let _ = events_tx.send((
        run_id,
        FnEvent::Output { step_idx, line: format!("renamed {} -> {}", old.display(), new_path.display()) },
    ));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `MakeDir` (`is_file == false`) or `NewFile` step: create one entry
/// directly (no scan or progress needed).
fn run_make_entry(
    run_id: u64,
    step_idx: usize,
    is_file: bool,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let label = if is_file { InternalCmd::NewFile.label() } else { InternalCmd::MakeDir.label() };
    // Both commands use the same schema key ("name", see Task 1).
    // Always create in the SOURCE Lister's own directory, matching
    // `palette.rs`'s existing Mkdir/NewFile commands (which create
    // in the invoking Lister's cwd, never the DEST pane) - a DEST
    // being open must not redirect where the new entry lands.
    let template = args.get("name").unwrap_or("");
    let name = expand_text(template, source, dest).map_err(expand_err_msg)?;
    // Reuses `opusdm_fsops::delete::make_entry` (the same call the
    // hub's own MakeDir/NewFile jobs use) rather than a bare
    // `std::fs::File::create`, which truncates a same-named file
    // that already exists instead of failing.
    let outcome = opusdm_fsops::delete::make_entry(&source.cwd, name.trim(), is_file);
    let target = source.cwd.join(name.trim());
    if let Progress::Done { errors, .. } = outcome {
        if let Some((_, msg)) = errors.first() {
            return Err(format!("{label}: {msg}"));
        }
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("created {}", target.display()) }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// The step commands that run as a file job on the hub's engine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileJob {
    Copy,
    Move,
    Delete,
    CopyAs,
    MoveAs,
    Duplicate,
}

impl FileJob {
    /// The step command this job belongs to.
    fn cmd(self) -> InternalCmd {
        match self {
            FileJob::Copy => InternalCmd::Copy,
            FileJob::Move => InternalCmd::Move,
            FileJob::Delete => InternalCmd::Delete,
            FileJob::CopyAs => InternalCmd::CopyAs,
            FileJob::MoveAs => InternalCmd::MoveAs,
            FileJob::Duplicate => InternalCmd::Duplicate,
        }
    }

    /// Whether the job needs a DEST Lister. Delete, Duplicate and the "As"
    /// commands work in place, so a missing DEST falls back to the SOURCE
    /// directory.
    fn needs_dest(self) -> bool {
        matches!(self, FileJob::Copy | FileJob::Move)
    }
}

/// Run a Copy/Move/Delete/Copy As/Move As/Duplicate step: describe the job
/// and wait for the hub's job engine to finish it.
fn run_file_job(
    run_id: u64,
    step_idx: usize,
    job: FileJob,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let label = job.cmd().label();
    let op = match job {
        FileJob::Copy => Op::Copy,
        FileJob::Move => Op::Move,
        FileJob::Delete => Op::Delete,
        FileJob::Duplicate => Op::Duplicate,
        FileJob::CopyAs | FileJob::MoveAs => {
            let pattern = expand_text(args.get("new_name").unwrap_or(""), source, dest).map_err(expand_err_msg)?;
            let new_names = source
                .paths
                .iter()
                .map(|p| {
                    let own = p.file_name().map(|n| n.to_string_lossy().into_owned());
                    expand_new_name(pattern.trim(), &own.unwrap_or_default())
                })
                .collect();
            if job == FileJob::CopyAs {
                Op::CopyAs { new_names }
            } else {
                Op::MoveAs { new_names }
            }
        }
    };
    let dest_dir = match dest {
        Some(d) => d.cwd.clone(),
        None if !job.needs_dest() => source.cwd.clone(),
        None => return Err(format!("{label}: no DEST Lister open")),
    };
    crate::jobs::validate_op(&op, &source.paths, &dest_dir)?;
    let spec = JobSpec { op, srcs: source.paths.clone(), dest_dir, options: args.job_options() };
    // The hub runs the job on its engine, so the step waits here for
    // the outcome (conflict prompts and progress are the hub's job).
    let (done_tx, done_rx) = channel::<JobResult>();
    events_tx
        .send((run_id, FnEvent::Job { step_idx, spec, done: done_tx }))
        .map_err(|_| format!("{label}: the hub is gone"))?;
    let (copied, skipped) = done_rx
        .recv()
        .map_err(|_| format!("{label}: the job was dropped"))?
        .map_err(|e| format!("{label}: {e}"))?;
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{label}: {copied} copied, {skipped} skipped") }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Expand every argv element: each one on its own, and an element that is
/// exactly one `{f}`/`{o}`/... code flattens into one element per path.
/// Fails when nothing is left to run.
fn expand_argv(argv: &[String], source: &SelectionCtx, dest: Option<&SelectionCtx>) -> Result<Vec<String>, String> {
    let mut expanded: Vec<String> = Vec::with_capacity(argv.len());
    for a in argv {
        match expand_argv_element(a, source, dest).map_err(expand_err_msg)? {
            ExpandedArg::Many(values) => expanded.extend(values),
            ExpandedArg::One(value) => expanded.push(value),
        }
    }
    if expanded.is_empty() {
        return Err("command expanded to nothing (empty selection?)".to_string());
    }
    Ok(expanded)
}

/// Run a `Run` step: split the command line, expand it and start the program
/// in the SOURCE folder. Without `wait` the step ends at once and the
/// program is left running (and reaped); with `window` its output streams to
/// a window on the requesting Lister that stays open after the function ends.
/// A program the step waits for is killed when `cancel` is set.
fn run_run(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) -> Result<(), String> {
    let templates = opusdm_ipc::cli::tokenize(args.get("command").unwrap_or("")).map_err(|e| format!("Run: {e}"))?;
    if templates.is_empty() {
        return Err("Run: no program given".to_string());
    }
    let (wait, window) = (args.flag("wait"), args.flag("window"));
    // A `bash -c <script> ...` command (flag `script`, set for inline bash
    // commands): the script, the third word, is passed as typed. Expanding
    // placeholders in it would paste file names into shell code and turn an
    // empty selection into an empty string inside it.
    let script = if args.flag("script") && templates.len() > 2 { Some(templates[2].as_str()) } else { None };
    // Waiting without a window is the plain External step.
    if wait && !window {
        return run_external(
            run_id,
            step_idx,
            ExternalCall { argv: &templates, script, cwd_source: true },
            source,
            dest,
            events_tx,
            cancel,
        );
    }
    let argv = expand_run_argv(&templates, script, source, dest).map_err(|e| format!("Run: {e}"))?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(usable_cwd(&source.cwd)).stdin(Stdio::null());
    if window {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    // A detached program may open a window: announce the launch so the
    // window manager gives that window the focus (see `launch_focus`).
    let launch = if window { None } else { crate::launch_focus::begin(&argv[0]) };
    if let Some(launch) = &launch {
        command.env(crate::launch_focus::STARTUP_ID_ENV, launch.startup_id());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(launch) = launch {
                launch.abort();
            }
            return Err(format!("Run: cannot start '{}': {e}", argv[0]));
        }
    };
    if let Some(launch) = launch {
        launch.watch(child.id());
    }
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("started {} (pid {})", argv[0], child.id()) }));

    if !window {
        // Nobody waits: the hub's SIGCHLD reaper collects it, so it never
        // lingers as a zombie. Dropping `child` does not wait for it.
        drop(child);
        let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
        return Ok(());
    }

    let title = argv.join(" ");
    // Sent before the step ends, so the hub knows who owns the window.
    let _ = events_tx.send((run_id, FnEvent::Window { step_idx, title: title.clone(), msg: WindowMsg::Open }));
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stream = |reader: Box<dyn std::io::Read + Send>| {
        let (tx, title) = (events_tx.clone(), title.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let _ = tx.send((run_id, FnEvent::Window { step_idx, title: title.clone(), msg: WindowMsg::Line(line) }));
            }
        })
    };
    let readers = [stream(Box::new(stdout)), stream(Box::new(stderr))];
    let window = WindowRun { run_id, step_idx, title, events_tx: events_tx.clone() };
    if wait {
        let code = window.finish(child, readers, cancel).map_err(|e| format!("Run: '{}': {e}", argv[0]))?;
        let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: code }));
        return if code == Some(0) { Ok(()) } else { Err(format!("Run: '{}' exited with {code:?}", argv[0])) };
    }
    // Not waited for: the program outlives the step, so the run's cancel
    // does not stop it.
    std::thread::spawn(move || {
        let _ = window.finish(child, readers, &CancelToken::new());
    });
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// A `Run` program whose output streams to a window: where to tell the
/// window that the program ended.
struct WindowRun {
    run_id: u64,
    step_idx: usize,
    title: String,
    events_tx: Sender<(u64, FnEvent)>,
}

impl WindowRun {
    /// Wait for `child` (killing it when `cancel` is set), let its output
    /// `readers` finish and tell the window it ended. Returns the exit code
    /// (`None` when killed by a signal), or why the wait failed. The readers
    /// of a cancelled program are not joined: a process it started may still
    /// hold the pipes open.
    fn finish(self, mut child: Child, readers: [std::thread::JoinHandle<()>; 2], cancel: &CancelToken) -> Result<Option<i32>, String> {
        let waited = wait_or_kill(&mut child, cancel);
        if waited.is_ok() {
            for r in readers {
                let _ = r.join();
            }
        }
        let code = waited.as_ref().ok().and_then(ExitStatus::code);
        let closed = FnEvent::Window { step_idx: self.step_idx, title: self.title, msg: WindowMsg::Closed(code) };
        let _ = self.events_tx.send((self.run_id, closed));
        waited.map(|status| status.code())
    }
}

/// The working directory a child process can really start in. A Lister's
/// folder may be virtual (`/x/a.zip#dir`, an `sftp://` URL) or gone, and
/// `spawn` then fails with ENOENT, which reads as "program not found". Walk
/// up to the nearest existing folder (an archive path lands on the folder
/// holding the archive), else `$HOME`, else `/`.
fn usable_cwd(cwd: &Path) -> PathBuf {
    if let Some(found) = cwd.ancestors().find(|p| !p.as_os_str().is_empty() && p.is_dir()) {
        return found.to_path_buf();
    }
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()).unwrap_or_else(|| PathBuf::from("/"))
}

/// [`expand_argv`], except that `script` (the third word of a
/// `bash -c <script> ...` command) is kept as it is, unexpanded. `bash` and
/// `-c` come first and expand to themselves, so the script keeps its
/// position.
fn expand_run_argv(argv: &[String], script: Option<&str>, source: &SelectionCtx, dest: Option<&SelectionCtx>) -> Result<Vec<String>, String> {
    let Some(script) = script else { return expand_argv(argv, source, dest) };
    let masked: Vec<String> = argv.iter().enumerate().map(|(i, a)| if i == 2 { "script".to_string() } else { a.clone() }).collect();
    let mut expanded = expand_argv(&masked, source, dest)?;
    expanded[2] = script.to_string();
    Ok(expanded)
}

/// What an `External` step runs: the argv templates, the script word that
/// is passed as typed, and whether the working directory is the source's.
struct ExternalCall<'a> {
    argv: &'a [String],
    script: Option<&'a str>,
    cwd_source: bool,
}

/// Run one `External` step: expand every argv element (keeping `script`,
/// see [`expand_run_argv`]), spawn it with piped stdout/stderr, stream every
/// line as it arrives, and report the exit status. The program is killed
/// when `cancel` is set.
fn run_external(
    run_id: u64,
    step_idx: usize,
    call: ExternalCall,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
    cancel: &CancelToken,
) -> Result<(), String> {
    let ExternalCall { argv, script, cwd_source } = call;
    if argv.is_empty() {
        return Err("External step: command is required".to_string());
    }
    let expanded = expand_run_argv(argv, script, source, dest)?;
    let cwd = if cwd_source { &source.cwd } else { dest.map(|d| &d.cwd).unwrap_or(&source.cwd) };

    let mut child = Command::new(&expanded[0])
        .args(&expanded[1..])
        .current_dir(usable_cwd(cwd))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start '{}': {e}", expanded[0]))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stream = |reader: Box<dyn std::io::Read + Send>| {
        let tx = events_tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let _ = tx.send((run_id, FnEvent::Output { step_idx, line }));
            }
        })
    };
    let readers = [stream(Box::new(stdout)), stream(Box::new(stderr))];
    // A cancelled program's readers are not joined: a process it started
    // may still hold the pipes open.
    let status = wait_or_kill(&mut child, cancel).map_err(|e| format!("'{}': {e}", expanded[0]))?;
    for r in readers {
        let _ = r.join();
    }
    let exit_code = status.code();
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code }));
    if !status.success() {
        return Err(format!("'{}' exited with {:?}", expanded[0], exit_code));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
