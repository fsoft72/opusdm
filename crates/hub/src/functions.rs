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
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};

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
    /// A `Run` step with a window: the program's output. The window outlives
    /// the function, so these events are routed by `(run_id, step_idx)`.
    Window { step_idx: usize, title: String, msg: WindowMsg },
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
        let result = match step {
            // One exhaustive match: a new `InternalCmd` does not compile until
            // it is dispatched here.
            FnStep::Internal { cmd, args } => match *cmd {
                InternalCmd::ChangeDir => run_change_dir(run_id, step_idx, args, &mut source, dest, events_tx),
                InternalCmd::Run => run_run(run_id, step_idx, args, &source, dest, events_tx),
                InternalCmd::Confirm => run_confirm(run_id, step_idx, args, &source, dest, events_tx),
                InternalCmd::SetBackground => run_set_background(run_id, step_idx, args, &source, dest, events_tx),
                InternalCmd::LeaveOut => run_leave_out(run_id, step_idx, false, &source, events_tx),
                InternalCmd::PutAway => run_leave_out(run_id, step_idx, true, &source, events_tx),
                InternalCmd::Hide | InternalCmd::Reveal | InternalCmd::ToggleHide => {
                    run_hide(run_id, step_idx, *cmd, events_tx)
                }
                InternalCmd::DesktopPrev | InternalCmd::DesktopNext => run_desktop(run_id, step_idx, *cmd, events_tx),
                InternalCmd::OpenInputConfig => run_open_input_config(run_id, step_idx, events_tx),
                InternalCmd::OpenBank => run_bank(run_id, step_idx, BankAction::Open, args, bank_exists, events_tx),
                InternalCmd::CloseBank => run_bank(run_id, step_idx, BankAction::Close, args, bank_exists, events_tx),
                InternalCmd::ToggleBank => run_bank(run_id, step_idx, BankAction::Toggle, args, bank_exists, events_tx),
                InternalCmd::HideBank => run_bank(run_id, step_idx, BankAction::Hide, args, bank_exists, events_tx),
                InternalCmd::Rename => run_rename(run_id, step_idx, args, &source, dest, events_tx),
                InternalCmd::MakeDir => run_make_entry(run_id, step_idx, false, args, &source, dest, events_tx),
                InternalCmd::NewFile => run_make_entry(run_id, step_idx, true, args, &source, dest, events_tx),
                InternalCmd::Copy => run_file_job(run_id, step_idx, FileJob::Copy, args, &source, dest, events_tx),
                InternalCmd::Move => run_file_job(run_id, step_idx, FileJob::Move, args, &source, dest, events_tx),
                InternalCmd::Delete => run_file_job(run_id, step_idx, FileJob::Delete, args, &source, dest, events_tx),
                InternalCmd::CopyAs => run_file_job(run_id, step_idx, FileJob::CopyAs, args, &source, dest, events_tx),
                InternalCmd::MoveAs => run_file_job(run_id, step_idx, FileJob::MoveAs, args, &source, dest, events_tx),
                InternalCmd::Duplicate => run_file_job(run_id, step_idx, FileJob::Duplicate, args, &source, dest, events_tx),
            },
            FnStep::External { argv, cwd_source } => {
                run_external(run_id, step_idx, argv, None, *cwd_source, &source, dest, events_tx)
            }
        };
        if let Err(error) = result {
            let _ = events_tx.send((run_id, FnEvent::Failed { step_idx, error }));
            return;
        }
    }
    let _ = events_tx.send((run_id, FnEvent::AllDone));
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

/// Whether a button bank called `name` exists on disk.
fn bank_exists(name: &str) -> bool {
    opusdm_ipc::buttonbank::Banks::load().get_by_name(name).is_some()
}

/// Run a `Confirm` step: expand the question, ask through the hub and wait.
/// Anything but a yes stops the function.
fn run_confirm(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    let template = args.get("message").unwrap_or("");
    let message = expand_text(template, source, dest).map_err(expand_err_msg)?;
    if message.trim().is_empty() {
        return Err("Confirm: no question given".to_string());
    }
    let (reply, answer) = channel::<bool>();
    let _ = events_tx.send((run_id, FnEvent::Confirm { step_idx, message, reply }));
    match answer.recv() {
        Ok(true) => {
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        Ok(false) => Err("Confirm: stopped by the user".to_string()),
        Err(_) => Err("Confirm: nobody was there to answer".to_string()),
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
fn run_run(
    run_id: u64,
    step_idx: usize,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
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
        return run_external(run_id, step_idx, &templates, script, true, source, dest, events_tx);
    }
    let argv = expand_run_argv(&templates, script, source, dest).map_err(|e| format!("Run: {e}"))?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir(&source.cwd).stdin(Stdio::null());
    if window {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn().map_err(|e| format!("Run: cannot start '{}': {e}", argv[0]))?;
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("started {} (pid {})", argv[0], child.id()) }));

    if !window {
        // Nobody waits: a thread reaps it so it never lingers as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
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
    let finish = {
        let (tx, title) = (events_tx.clone(), title.clone());
        move |mut child: std::process::Child, readers: [std::thread::JoinHandle<()>; 2]| {
            for r in readers {
                let _ = r.join();
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let _ = tx.send((run_id, FnEvent::Window { step_idx, title, msg: WindowMsg::Closed(code) }));
            code
        }
    };
    if wait {
        let code = finish(child, readers);
        let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: code }));
        return if code == Some(0) { Ok(()) } else { Err(format!("Run: '{}' exited with {code:?}", argv[0])) };
    }
    std::thread::spawn(move || {
        finish(child, readers);
    });
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
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

/// Run one `External` step: expand every argv element (keeping `script`,
/// see [`expand_run_argv`]), spawn it with piped stdout/stderr, stream every
/// line as it arrives, and report the exit status.
fn run_external(
    run_id: u64,
    step_idx: usize,
    argv: &[String],
    script: Option<&str>,
    cwd_source: bool,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    if argv.is_empty() {
        return Err("External step: command is required".to_string());
    }
    let expanded = expand_run_argv(argv, script, source, dest)?;
    let cwd = if cwd_source { &source.cwd } else { dest.map(|d| &d.cwd).unwrap_or(&source.cwd) };

    let mut child = Command::new(&expanded[0])
        .args(&expanded[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start '{}': {e}", expanded[0]))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_tx = events_tx.clone();
    let out_thread = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = out_tx.send((run_id, FnEvent::Output { step_idx, line }));
        }
    });
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line }));
    }
    let _ = out_thread.join();

    let status = opusdm_ipc::reaped::wait(&mut child).map_err(|e| format!("wait failed: {e}"))?;
    let exit_code = status.code();
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code }));
    if !status.success() {
        return Err(format!("'{}' exited with {:?}", expanded[0], exit_code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ctx(paths: &[&str], cwd: &str) -> SelectionCtx {
        SelectionCtx { paths: paths.iter().map(std::path::PathBuf::from).collect(), cwd: cwd.into() }
    }

    fn drain(rx: std::sync::mpsc::Receiver<(u64, FnEvent)>) -> Vec<FnEvent> {
        rx.into_iter().map(|(_, ev)| ev).collect()
    }

    #[test]
    fn external_step_streams_stdout_and_reports_success() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["echo".into(), "hi {o}".into()], cwd_source: true }];
        let src = ctx(&["/tmp/a.txt"], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if line == "hi a.txt")));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn whole_token_placeholder_gives_one_argv_element_per_file_not_a_single_joined_string() {
        // printf repeats its format once per remaining argument, so two
        // real argv elements print two bracketed groups; a single string
        // joining both names (the old, buggy behavior) would print one.
        let (tx, rx) = channel();
        let steps = vec![FnStep::External {
            argv: vec!["printf".into(), "[%s]".into(), "{o}".into()],
            cwd_source: true,
        }];
        let src = ctx(&["/tmp/a.txt", "/tmp/b c.txt"], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        let output: String = events
            .iter()
            .filter_map(|e| match e {
                FnEvent::Output { line, .. } => Some(line.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(output, "[a.txt][b c.txt]");
    }

    #[test]
    fn external_step_nonzero_exit_fails_the_run() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["false".into()], cwd_source: true }];
        let src = ctx(&[], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })));
    }

    #[test]
    fn unknown_command_fails_with_context_not_a_panic() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["opusdm-does-not-exist-xyz".into()], cwd_source: true }];
        let src = ctx(&[], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("opusdm-does-not-exist-xyz")));
    }

    #[test]
    fn dest_placeholder_without_a_dest_lister_fails_with_context() {
        let (tx, rx) = channel();
        let steps = vec![FnStep::External { argv: vec!["echo".into(), "{do}".into()], cwd_source: true }];
        let src = ctx(&["/tmp/a.txt"], "/tmp");
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("DEST")));
    }

    #[test]
    fn makedir_step_creates_in_source_even_with_a_dest_open() {
        // Matches palette.rs's own Mkdir/NewFile commands, which always
        // create in the invoking Lister's own directory - a DEST existing
        // must not redirect a Function's MakeDir/NewFile step there.
        let src_td = tempfile::tempdir().unwrap();
        let dst_td = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), "new_dir".to_string());
        let steps = vec![FnStep::Internal {
            cmd: InternalCmd::MakeDir,
            args: InternalArgs(args),
        }];
        let src = ctx(&[], src_td.path().to_str().unwrap());
        let dst = ctx(&[], dst_td.path().to_str().unwrap());
        run_steps(1, &steps, &src, Some(&dst), &tx);
        drop(tx);
        assert!(src_td.path().join("new_dir").is_dir());
        assert!(!dst_td.path().join("new_dir").exists());
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn newfile_step_does_not_truncate_an_existing_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("notes.txt"), b"important content").unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("name".to_string(), "notes.txt".to_string());
        let steps = vec![FnStep::Internal {
            cmd: InternalCmd::NewFile,
            args: InternalArgs(args),
        }];
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        assert_eq!(std::fs::read(td.path().join("notes.txt")).unwrap(), b"important content");
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })));
    }

    #[test]
    fn changedir_step_moves_later_steps_and_asks_the_lister_to_navigate() {
        let td = tempfile::tempdir().unwrap();
        let sub = td.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), "sub".to_string());
        let steps = vec![
            FnStep::Internal { cmd: InternalCmd::ChangeDir, args: InternalArgs(args) },
            FnStep::External { argv: vec!["pwd".into()], cwd_source: true },
        ];
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        let want = std::fs::canonicalize(&sub).unwrap();
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { path, .. } if *path == want)));
        assert!(events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if *line == want.to_string_lossy())));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn changedir_step_fails_on_a_missing_directory() {
        let td = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), "nope".to_string());
        let steps = vec![FnStep::Internal { cmd: InternalCmd::ChangeDir, args: InternalArgs(args) }];
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })));
    }

    #[test]
    fn rename_step_renames_the_first_selected_path() {
        let td = tempfile::tempdir().unwrap();
        let old = td.path().join("old.txt");
        std::fs::write(&old, b"hi").unwrap();
        let (tx, rx) = channel();
        let mut args = BTreeMap::new();
        args.insert("new_name".to_string(), "new.txt".to_string());
        let steps = vec![FnStep::Internal {
            cmd: InternalCmd::Rename,
            args: InternalArgs(args),
        }];
        let src = ctx(&[old.to_str().unwrap()], td.path().to_str().unwrap());
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        assert!(!old.exists());
        assert!(td.path().join("new.txt").exists());
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    /// Run a Copy step over an existing `a.txt` and answer every conflict with
    /// `answer`, the way the hub relays the Lister's choice. Returns the
    /// destination file's content and the paths the conflicts named.
    fn copy_with_conflict(answer: Option<Resolution>) -> (Vec<u8>, Vec<PathBuf>, Vec<FnEvent>) {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_file = src_dir.path().join("a.txt");
        std::fs::write(&src_file, b"new").unwrap();
        std::fs::write(dst_dir.path().join("a.txt"), b"existing").unwrap();

        let steps = vec![FnStep::Internal { cmd: InternalCmd::Copy, args: InternalArgs::default() }];
        let src = ctx(&[src_file.to_str().unwrap()], src_dir.path().to_str().unwrap());
        let dst = ctx(&[], dst_dir.path().to_str().unwrap());
        let (tx, rx) = channel();
        let mut asked = Vec::new();
        let mut others = Vec::new();
        std::thread::scope(|scope| {
            let (steps, src, dst) = (&steps, &src, &dst);
            // The thread owns the only sender, so the loop below ends when
            // run_steps returns.
            scope.spawn(move || run_steps(1, steps, src, Some(dst), &tx));
            for (_, ev) in rx {
                match ev {
                    // Stand in for the hub's job engine.
                    FnEvent::Job { spec, done, .. } => {
                        let result = run_job_inline(spec, &CancelToken::new(), |path, _, _, reply| {
                            asked.push(path);
                            if let Some(answer) = answer {
                                let _ = reply.send(answer);
                            }
                        });
                        let _ = done.send(result);
                    }
                    other => others.push(other),
                }
            }
        });
        (std::fs::read(dst_dir.path().join("a.txt")).unwrap(), asked, others)
    }

    #[test]
    fn a_cancelled_run_stops_before_its_next_step() {
        let (tx, rx) = channel();
        let cancel = CancelToken::new();
        cancel.cancel();
        let steps = vec![FnStep::External { argv: vec!["echo".into(), "never".into()], cwd_source: true }];
        run_steps_cancellable(1, &steps, &ctx(&[], "/tmp"), None, &tx, &cancel);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })));
        assert!(!events.iter().any(|e| matches!(e, FnEvent::Output { .. })));
    }

    #[test]
    fn copy_conflict_is_relayed_and_skip_keeps_the_destination() {
        let (content, asked, events) = copy_with_conflict(Some(Resolution::Skip));
        assert_eq!(content, b"existing");
        assert_eq!(asked.len(), 1);
        assert!(asked[0].ends_with("a.txt"));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn copy_conflict_overwrite_replaces_the_destination() {
        let (content, asked, events) = copy_with_conflict(Some(Resolution::Overwrite));
        assert_eq!(content, b"new");
        assert_eq!(asked.len(), 1);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn copy_conflict_cancel_stops_the_job_without_overwriting() {
        let (content, _, _) = copy_with_conflict(Some(Resolution::Cancel));
        assert_eq!(content, b"existing");
    }

    #[test]
    fn a_dropped_conflict_reply_does_not_hang_the_step() {
        // Nobody answers and the reply sender is dropped: the job must end.
        let (content, asked, _) = copy_with_conflict(None);
        assert_eq!(content, b"existing");
        assert_eq!(asked.len(), 1);
    }

    fn set_background_steps(path: &str, fit: Option<&str>) -> Vec<FnStep> {
        let mut args = BTreeMap::new();
        args.insert("path".to_string(), path.to_string());
        if let Some(fit) = fit {
            args.insert("fit".to_string(), fit.to_string());
        }
        vec![FnStep::Internal { cmd: InternalCmd::SetBackground, args: InternalArgs(args) }]
    }

    fn has_set_background(events: &[FnEvent]) -> bool {
        events.iter().any(|e| matches!(e, FnEvent::SetBackground { .. }))
    }

    #[test]
    fn set_background_resolves_a_relative_path_and_parses_the_fit() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.PNG"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.PNG", Some("Tile")), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        let want = std::fs::canonicalize(td.path().join("wall.PNG")).unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            FnEvent::SetBackground { path, fit: Some(opusdm_ipc::ImageFit::Tile) } if *path == want
        )));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn set_background_without_fit_keeps_the_current_one() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.png", None), &src, None, &tx);
        drop(tx);
        assert!(drain(rx).iter().any(|e| matches!(e, FnEvent::SetBackground { fit: None, .. })));
    }

    #[test]
    fn set_background_rejects_a_non_image_and_changes_nothing() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("notes.txt"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("notes.txt", None), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("notes.txt")));
    }

    #[test]
    fn set_background_rejects_a_directory_and_a_missing_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("dir.png")).unwrap();
        for name in ["dir.png", "missing.png"] {
            let (tx, rx) = channel();
            let src = ctx(&[], td.path().to_str().unwrap());
            run_steps(1, &set_background_steps(name, None), &src, None, &tx);
            drop(tx);
            let events = drain(rx);
            assert!(!has_set_background(&events), "{name}");
            assert!(matches!(events.last(), Some(FnEvent::Failed { .. })), "{name}");
        }
    }

    #[test]
    fn set_background_rejects_an_unknown_fit_and_changes_nothing() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("wall.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("wall.png", Some("stretch")), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { error, .. }) if error.contains("stretch")));
    }

    #[test]
    fn set_background_follows_a_symlink_and_fails_on_a_dangling_one() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("real.dat"), b"x").unwrap();
        std::os::unix::fs::symlink(td.path().join("real.dat"), td.path().join("link.png")).unwrap();
        std::os::unix::fs::symlink(td.path().join("gone.dat"), td.path().join("dangling.png")).unwrap();
        let src = ctx(&[], td.path().to_str().unwrap());

        let (tx, rx) = channel();
        run_steps(1, &set_background_steps("link.png", None), &src, None, &tx);
        drop(tx);
        assert!(has_set_background(&drain(rx)), "extension is checked on the name the user picked");

        let (tx, rx) = channel();
        run_steps(1, &set_background_steps("dangling.png", None), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!has_set_background(&events));
        assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
    }

    #[test]
    fn set_background_keeps_unknown_braces_in_the_path_literal() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a{x}.png"), b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[], td.path().to_str().unwrap());
        run_steps(1, &set_background_steps("a{x}.png", None), &src, None, &tx);
        drop(tx);
        assert!(has_set_background(&drain(rx)));
    }

    fn selection_step(cmd: InternalCmd) -> Vec<FnStep> {
        vec![FnStep::Internal { cmd, args: InternalArgs::default() }]
    }

    #[test]
    fn leave_out_emits_the_selection() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a.txt");
        std::fs::write(&a, b"x").unwrap();
        let (tx, rx) = channel();
        let src = ctx(&[a.to_str().unwrap()], td.path().to_str().unwrap());
        run_steps(1, &selection_step(InternalCmd::LeaveOut), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(events.iter().any(|e| matches!(e, FnEvent::LeaveOut { paths } if *paths == vec![a.clone()])));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn leave_out_and_put_away_fail_on_an_empty_selection() {
        for (cmd, text) in [(InternalCmd::LeaveOut, "Leave Out: nothing selected"), (InternalCmd::PutAway, "Put Away: nothing selected")] {
            let (tx, rx) = channel();
            let src = ctx(&[], "/tmp");
            run_steps(1, &selection_step(cmd), &src, None, &tx);
            drop(tx);
            let events = drain(rx);
            assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error == text), "{text}");
            assert!(!events.iter().any(|e| matches!(e, FnEvent::LeaveOut { .. } | FnEvent::PutAway { .. })));
        }
    }

    #[test]
    fn leave_out_refuses_a_missing_path_and_emits_nothing() {
        let td = tempfile::tempdir().unwrap();
        let gone = td.path().join("gone.txt");
        let (tx, rx) = channel();
        let src = ctx(&[gone.to_str().unwrap()], td.path().to_str().unwrap());
        run_steps(1, &selection_step(InternalCmd::LeaveOut), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(!events.iter().any(|e| matches!(e, FnEvent::LeaveOut { .. })));
        assert!(matches!(events.last(), Some(FnEvent::Failed { error, .. }) if error.contains("gone.txt")));
    }

    #[test]
    fn put_away_emits_the_selection_even_when_the_target_is_gone() {
        let (tx, rx) = channel();
        let src = ctx(&["/no/longer/here.txt"], "/tmp");
        run_steps(1, &selection_step(InternalCmd::PutAway), &src, None, &tx);
        drop(tx);
        let events = drain(rx);
        assert!(events.iter().any(|e| matches!(e, FnEvent::PutAway { paths } if paths.len() == 1)));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
    }

    #[test]
    fn desktop_steps_emit_a_signed_delta() {
        for (cmd, expected) in [(InternalCmd::DesktopPrev, -1), (InternalCmd::DesktopNext, 1)] {
            let (tx, rx) = channel();
            let src = ctx(&[], "/tmp");
            run_steps(1, &selection_step(cmd), &src, None, &tx);
            drop(tx);
            let events = drain(rx);
            assert!(events.iter().any(|e| matches!(e, FnEvent::Desktop { delta } if *delta == expected)), "{cmd:?}");
            assert!(matches!(events.last(), Some(FnEvent::AllDone)), "{cmd:?}");
        }
    }

    #[test]
    fn hide_reveal_and_toggle_steps_emit_their_events_without_a_selection() {
        type EventCheck = fn(&FnEvent) -> bool;
        let cases: [(InternalCmd, EventCheck); 3] = [
            (InternalCmd::Hide, |e| matches!(e, FnEvent::Hide)),
            (InternalCmd::Reveal, |e| matches!(e, FnEvent::Reveal)),
            (InternalCmd::ToggleHide, |e| matches!(e, FnEvent::ToggleHide)),
        ];
        for (cmd, is_event) in cases {
            let (tx, rx) = channel();
            let src = ctx(&[], "/tmp");
            run_steps(1, &selection_step(cmd), &src, None, &tx);
            drop(tx);
            let events = drain(rx);
            assert!(events.iter().any(is_event), "{cmd:?} must emit its event");
            assert!(matches!(events.last(), Some(FnEvent::AllDone)), "{cmd:?}");
        }
    }

    fn bank_args(name: &str) -> InternalArgs {
        InternalArgs(std::collections::BTreeMap::from([("bank".to_string(), name.to_string())]))
    }

    #[test]
    fn run_bank_emits_the_action_for_a_known_bank() {
        use opusdm_ipc::buttonbank::BankAction;
        for action in [BankAction::Open, BankAction::Close, BankAction::Toggle, BankAction::Hide] {
            let (tx, rx) = channel();
            let result = run_bank(1, 0, action, &bank_args(" Tools "), |n| n == "Tools", &tx);
            drop(tx);
            assert_eq!(result, Ok(()), "{action:?}");
            let events = drain(rx);
            assert!(events.iter().any(|e| matches!(e, FnEvent::Bank { action: a, bank } if *a == action && bank == "Tools")));
            assert!(matches!(events.last(), Some(FnEvent::StepDone { step_idx: 0, exit_code: Some(0) })));
        }
    }

    #[test]
    fn run_bank_fails_for_an_unknown_or_missing_bank_and_emits_no_event() {
        use opusdm_ipc::buttonbank::BankAction;
        for args in [bank_args("Nope"), InternalArgs::default(), bank_args("  ")] {
            let (tx, rx) = channel();
            let result = run_bank(1, 0, BankAction::Open, &args, |_| false, &tx);
            drop(tx);
            let error = result.unwrap_err();
            assert!(error.contains("Bank"), "{error}");
            assert!(!drain(rx).iter().any(|e| matches!(e, FnEvent::Bank { .. })));
        }
    }

    #[test]
    fn run_steps_reports_an_unknown_bank_as_a_failed_step() {
        let (tx, rx) = channel();
        let src = ctx(&[], "/tmp");
        let steps = vec![FnStep::Internal { cmd: InternalCmd::OpenBank, args: bank_args("definitely-not-a-real-bank-name") }];
        run_steps(1, &steps, &src, None, &tx);
        drop(tx);
        assert!(matches!(drain(rx).last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("not found")));
    }

    fn confirm_steps(message: &str) -> Vec<FnStep> {
        let mut args = BTreeMap::new();
        args.insert("message".to_string(), message.to_string());
        vec![
            FnStep::Internal { cmd: InternalCmd::Confirm, args: InternalArgs(args) },
            FnStep::External { argv: vec!["echo".into(), "after".into()], cwd_source: true },
        ]
    }

    /// Run `steps`, answering every Confirm with `answer` (`None` drops it).
    fn run_answering(steps: &[FnStep], answer: Option<bool>) -> (Vec<String>, Vec<FnEvent>) {
        let (tx, rx) = channel();
        let src = ctx(&["/tmp/a.txt"], "/tmp");
        let mut asked = Vec::new();
        let mut others = Vec::new();
        std::thread::scope(|scope| {
            let src = &src;
            // The thread owns the only sender, so the loop ends with the run.
            scope.spawn(move || run_steps(1, steps, src, None, &tx));
            for (_, ev) in rx {
                match ev {
                    FnEvent::Confirm { message, reply, .. } => {
                        asked.push(message);
                        if let Some(a) = answer {
                            let _ = reply.send(a);
                        }
                    }
                    other => others.push(other),
                }
            }
        });
        (asked, others)
    }

    #[test]
    fn confirm_yes_lets_the_function_go_on_and_expands_the_question() {
        let (asked, events) = run_answering(&confirm_steps("Really touch {o}?"), Some(true));
        assert_eq!(asked, vec!["Really touch a.txt?".to_string()]);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
        assert!(events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if line == "after")));
    }

    #[test]
    fn confirm_no_or_no_answer_aborts_before_the_next_step() {
        for answer in [Some(false), None] {
            let (_, events) = run_answering(&confirm_steps("Sure?"), answer);
            assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, .. })), "{answer:?}");
            assert!(!events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if line == "after")));
        }
    }

    #[test]
    fn confirm_without_a_question_fails_without_asking() {
        let (asked, events) = run_answering(&confirm_steps("  "), Some(true));
        assert!(asked.is_empty());
        assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
    }

    fn change_dir_events(args: &[(&str, &str)], cwd: &Path) -> Vec<FnEvent> {
        let map: BTreeMap<String, String> = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let steps = vec![FnStep::Internal { cmd: InternalCmd::ChangeDir, args: InternalArgs(map) }];
        let (tx, rx) = channel();
        run_steps(1, &steps, &ctx(&[], cwd.to_str().unwrap()), None, &tx);
        drop(tx);
        drain(rx)
    }

    #[test]
    fn changedir_container_opens_the_folder_and_selects_the_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a.txt"), b"x").unwrap();
        let events = change_dir_events(&[("path", "a.txt"), ("container", "true")], td.path());
        let folder = std::fs::canonicalize(td.path()).unwrap();
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { path, select: Some(s), .. } if *path == folder && s == "a.txt")), "{}", events.len());
        // Without the flag a file is not a directory.
        let events = change_dir_events(&[("path", "a.txt")], td.path());
        assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
        // A directory with the flag is just a directory.
        std::fs::create_dir(td.path().join("d")).unwrap();
        let events = change_dir_events(&[("path", "d"), ("container", "true")], td.path());
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { select: None, .. })));
    }

    #[test]
    fn changedir_carries_the_new_lister_flag_and_a_checked_view_mode() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("d")).unwrap();
        let events = change_dir_events(&[("path", "d"), ("new", "true"), ("mode", " Icons ")], td.path());
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { new_lister: true, mode: Some(m), .. } if m == "icons")));
        let events = change_dir_events(&[("path", "d"), ("mode", "tiles")], td.path());
        assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
        let events = change_dir_events(&[("path", "d")], td.path());
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { new_lister: false, mode: None, .. })));
    }

    fn run_args(command: &str, flags: &[&str]) -> Vec<FnStep> {
        let mut map = BTreeMap::new();
        map.insert("command".to_string(), command.to_string());
        for f in flags {
            map.insert(f.to_string(), "true".to_string());
        }
        vec![FnStep::Internal { cmd: InternalCmd::Run, args: InternalArgs(map) }]
    }

    fn run_events(steps: &[FnStep], src: &SelectionCtx) -> Vec<FnEvent> {
        let (tx, rx) = channel();
        run_steps(1, steps, src, None, &tx);
        drop(tx);
        // Reader and waiter threads hold clones: this ends when they are done.
        drain(rx)
    }

    #[test]
    fn run_starts_a_program_without_waiting_and_reaps_it() {
        let td = tempfile::tempdir().unwrap();
        let marker = td.path().join("made");
        let events = run_events(&run_args(&format!("sh -c 'sleep 0.2; touch {}'", marker.display()), &[]), &ctx(&[], td.path().to_str().unwrap()));
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
        assert!(!marker.exists(), "the step must not wait for the program");
        for _ in 0..50 {
            if marker.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the program never ran");
    }

    #[test]
    fn run_with_wait_fails_the_step_on_a_bad_exit_and_succeeds_otherwise() {
        let td = tempfile::tempdir().unwrap();
        let src = ctx(&[], td.path().to_str().unwrap());
        assert!(matches!(run_events(&run_args("true", &["wait"]), &src).last(), Some(FnEvent::AllDone)));
        assert!(matches!(run_events(&run_args("false", &["wait"]), &src).last(), Some(FnEvent::Failed { step_idx: 0, .. })));
    }

    #[test]
    fn run_with_a_window_streams_output_and_reports_the_exit() {
        let td = tempfile::tempdir().unwrap();
        let events = run_events(&run_args("sh -c 'echo one; echo two >&2; exit 3'", &["window"]), &ctx(&[], td.path().to_str().unwrap()));
        let msgs: Vec<&WindowMsg> = events.iter().filter_map(|e| match e { FnEvent::Window { msg, .. } => Some(msg), _ => None }).collect();
        assert_eq!(msgs.first(), Some(&&WindowMsg::Open));
        assert!(msgs.contains(&&WindowMsg::Line("one".into())) && msgs.contains(&&WindowMsg::Line("two".into())));
        assert_eq!(msgs.last(), Some(&&WindowMsg::Closed(Some(3))));
        // Not waiting: the step itself succeeded.
        assert!(events.iter().any(|e| matches!(e, FnEvent::AllDone)));
    }

    #[test]
    fn run_with_window_and_wait_blocks_and_checks_the_exit() {
        let td = tempfile::tempdir().unwrap();
        let src = ctx(&[], td.path().to_str().unwrap());
        let ok = run_events(&run_args("echo hi", &["window", "wait"]), &src);
        assert!(matches!(ok.last(), Some(FnEvent::AllDone)));
        let bad = run_events(&run_args("sh -c 'exit 2'", &["window", "wait"]), &src);
        assert!(matches!(bad.last(), Some(FnEvent::Failed { .. })));
        assert!(bad.iter().any(|e| matches!(e, FnEvent::Window { msg: WindowMsg::Closed(Some(2)), .. })));
    }

    #[test]
    fn run_expands_codes_and_rejects_empty_or_unknown_programs() {
        let td = tempfile::tempdir().unwrap();
        let src = ctx(&["/tmp/some file.txt"], td.path().to_str().unwrap());
        let events = run_events(&run_args("echo {o}", &["wait"]), &src);
        assert!(events.iter().any(|e| matches!(e, FnEvent::Output { line, .. } if line.contains("some file.txt"))));
        assert!(matches!(run_events(&run_args("  ", &[]), &src).last(), Some(FnEvent::Failed { .. })));
        assert!(matches!(run_events(&run_args("definitely-not-a-program-xyz", &[]), &src).last(), Some(FnEvent::Failed { .. })));
        assert!(matches!(run_events(&run_args("echo 'unclosed", &[]), &src).last(), Some(FnEvent::Failed { .. })));
    }
}
