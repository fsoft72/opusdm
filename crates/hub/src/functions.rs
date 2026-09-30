//! Executes a Function's steps on their own thread once the hub has
//! collected the SOURCE and (if any) DEST selection: internal steps reuse
//! `opusdm_fsops::run_job` or a direct `std::fs::rename`, external steps
//! spawn a subprocess and stream its output. See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use opusdm_fsops::{run_job, CancelToken, JobSpec, Progress};
use opusdm_ipc::function::{expand_argv_element, expand_text, ExpandError, ExpandedArg, FnStep, InternalArgs, InternalCmd, SelectionCtx};
use opusdm_ipc::glob::expand_new_name;
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
    /// A `ChangeDir` step resolved `path`: the SOURCE Lister must go there.
    ChangeDir { path: PathBuf },
    /// A `SetBackground` step validated `path` (canonical): the hub must
    /// install it as the desktop background. `fit` is `None` to keep the
    /// current one.
    SetBackground { path: PathBuf, fit: Option<opusdm_ipc::ImageFit> },
    /// A `LeaveOut` step validated `paths`: the hub must leave them out on
    /// the desktop.
    LeaveOut { paths: Vec<PathBuf> },
    /// A `PutAway` step: the hub must remove `paths` from the left-out icons.
    PutAway { paths: Vec<PathBuf> },
    /// Every step finished successfully.
    AllDone,
}

/// Spawn the thread that runs `steps` in order, reporting each event on
/// `events_tx` tagged with `run_id`.
pub fn spawn_executor(
    run_id: u64,
    steps: Vec<FnStep>,
    source: SelectionCtx,
    dest: Option<SelectionCtx>,
    events_tx: Sender<(u64, FnEvent)>,
) {
    std::thread::Builder::new()
        .name(format!("opusdm-function-{run_id}"))
        .spawn(move || run_steps(run_id, &steps, &source, dest.as_ref(), &events_tx))
        .expect("spawn function executor thread");
}

/// Run every step in order, stopping (and emitting `Failed`) at the first
/// error; emits `AllDone` only if every step succeeded. Free function (not
/// a method) so it is directly unit-testable without a running hub.
fn run_steps(
    run_id: u64,
    steps: &[FnStep],
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) {
    // A `ChangeDir` step moves the working directory of every later step.
    let mut source = source.clone();
    for (step_idx, step) in steps.iter().enumerate() {
        let result = match step {
            FnStep::Internal { cmd: InternalCmd::ChangeDir, args } => {
                run_change_dir(run_id, step_idx, args, &mut source, dest, events_tx)
            }
            FnStep::Internal { cmd: InternalCmd::SetBackground, args } => {
                run_set_background(run_id, step_idx, args, &source, dest, events_tx)
            }
            FnStep::Internal { cmd: cmd @ (InternalCmd::LeaveOut | InternalCmd::PutAway), .. } => {
                run_leave_out(run_id, step_idx, *cmd == InternalCmd::PutAway, &source, events_tx)
            }
            FnStep::Internal { cmd, args } => run_internal(run_id, step_idx, *cmd, args, &source, dest, events_tx),
            FnStep::External { argv, cwd_source } => {
                run_external(run_id, step_idx, argv, *cwd_source, &source, dest, events_tx)
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
    if !Path::new(&target).is_dir() {
        return Err(format!("Change Directory: {} is not a directory", target.display()));
    }
    source.cwd = target.clone();
    let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("cd {}", target.display()) }));
    let _ = events_tx.send((run_id, FnEvent::ChangeDir { path: target }));
    let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
    Ok(())
}

/// Run a `SetBackground` step: expand the path template, resolve `~` and
/// relative paths against the SOURCE cwd, require an existing regular file
/// with an image extension (checked on the name the user picked, so a
/// symlink `wall.png` is fine), parse the optional `fit`, and ask the hub to
/// install the image. Nothing is emitted when any check fails.
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
    if !opusdm_ipc::is_background_image(&picked) {
        return Err(format!(
            "Set Background: \"{}\" is not a supported image (png, jpg, jpeg, webp, bmp, gif)",
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

/// Run one `Internal` step. `Copy`/`Move`/`Delete` reuse the same
/// `opusdm_fsops::run_job` the hub's own `JobEngine` calls; `MakeDir`/
/// `NewFile` create directly (no scan/progress needed for one entry);
/// `Rename` is a direct `std::fs::rename`.
fn run_internal(
    run_id: u64,
    step_idx: usize,
    cmd: InternalCmd,
    args: &InternalArgs,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    match cmd {
        InternalCmd::ChangeDir => unreachable!("ChangeDir is handled by run_change_dir"),
        InternalCmd::SetBackground => unreachable!("SetBackground is handled by run_set_background"),
        InternalCmd::LeaveOut | InternalCmd::PutAway => {
            unreachable!("LeaveOut and PutAway are handled by run_leave_out")
        }
        InternalCmd::Rename => {
            let template = args.get("new_name").unwrap_or("");
            let new_name = expand_text(template, source, dest).map_err(expand_err_msg)?;
            let old = source.paths.first().ok_or_else(|| "Rename: nothing selected".to_string())?;
            let new_path = old.with_file_name(new_name.trim());
            std::fs::rename(old, &new_path).map_err(|e| format!("rename failed: {e}"))?;
            let _ = events_tx.send((
                run_id,
                FnEvent::Output { step_idx, line: format!("renamed {} -> {}", old.display(), new_path.display()) },
            ));
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        InternalCmd::MakeDir | InternalCmd::NewFile => {
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
            let outcome = opusdm_fsops::delete::make_entry(&source.cwd, name.trim(), cmd == InternalCmd::NewFile);
            let target = source.cwd.join(name.trim());
            match outcome {
                Progress::Done { errors, .. } if !errors.is_empty() => {
                    return Err(format!("{}: {}", cmd.label(), errors[0].1));
                }
                _ => {}
            }
            let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("created {}", target.display()) }));
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
        InternalCmd::Copy
        | InternalCmd::Move
        | InternalCmd::Delete
        | InternalCmd::CopyAs
        | InternalCmd::MoveAs
        | InternalCmd::Duplicate => {
            let op = match cmd {
                InternalCmd::Copy => Op::Copy,
                InternalCmd::Move => Op::Move,
                InternalCmd::Delete => Op::Delete,
                InternalCmd::Duplicate => Op::Duplicate,
                InternalCmd::CopyAs | InternalCmd::MoveAs => {
                    let pattern = expand_text(args.get("new_name").unwrap_or(""), source, dest)
                        .map_err(expand_err_msg)?;
                    let new_names = source
                        .paths
                        .iter()
                        .map(|p| {
                            let own = p.file_name().map(|n| n.to_string_lossy().into_owned());
                            expand_new_name(pattern.trim(), &own.unwrap_or_default())
                        })
                        .collect();
                    if cmd == InternalCmd::CopyAs {
                        Op::CopyAs { new_names }
                    } else {
                        Op::MoveAs { new_names }
                    }
                }
                _ => unreachable!("Rename/MakeDir/NewFile/ChangeDir handled above"),
            };
            // Delete, Duplicate and the "As" commands work in place, so a
            // missing DEST falls back to the SOURCE directory.
            let needs_dest = matches!(cmd, InternalCmd::Copy | InternalCmd::Move);
            let dest_dir = match dest {
                Some(d) => d.cwd.clone(),
                None if !needs_dest => source.cwd.clone(),
                None => return Err(format!("{}: no DEST Lister open", cmd.label())),
            };
            let spec = JobSpec { op, srcs: source.paths.clone(), dest_dir, options: args.job_options() };
            let (tx, rx) = channel::<Progress>();
            // `run_job` blocks the calling thread until the job finishes,
            // including waiting for a `Progress::Conflict` reply - so it
            // must run on its own thread while this thread drains `rx`
            // concurrently (same job-thread/relay-thread split as
            // `JobEngine::start`, crates/hub/src/jobs.rs:125-150).
            // Calling it inline here and draining `rx` only afterward
            // deadlocks the very first conflict: nothing is listening on
            // `rx` yet to send the `Resolution` the worker thread is
            // blocked waiting for.
            let job_thread = std::thread::spawn(move || {
                run_job(spec, tx, CancelToken::new());
            });
            for ev in rx {
                match ev {
                    Progress::File { path, .. } => {
                        let _ = events_tx.send((run_id, FnEvent::Output { step_idx, line: format!("{}: {}", cmd.label(), path.display()) }));
                    }
                    // No conflict-prompt UI exists for Function steps yet
                    // (see the design doc's non-goals): always skip, so a
                    // pre-existing destination entry cannot hang this
                    // thread forever waiting on a reply nobody will send.
                    Progress::Conflict { reply, path, .. } => {
                        let _ = reply.send(Resolution::Skip);
                        let _ = events_tx.send((run_id, FnEvent::Output {
                            step_idx,
                            line: format!("conflict: {} already exists, skipped", path.display()),
                        }));
                    }
                    Progress::Done { copied, skipped, errors, .. } => {
                        if let Some((path, msg)) = errors.first() {
                            return Err(format!("{}: {} ({msg})", cmd.label(), path.display()));
                        }
                        let _ = events_tx.send((run_id, FnEvent::Output {
                            step_idx,
                            line: format!("{}: {copied} copied, {skipped} skipped", cmd.label()),
                        }));
                    }
                    Progress::Scanning { .. } => {}
                }
            }
            let _ = job_thread.join();
            let _ = events_tx.send((run_id, FnEvent::StepDone { step_idx, exit_code: Some(0) }));
            Ok(())
        }
    }
}

/// Run one `External` step: expand every argv element, spawn it with piped
/// stdout/stderr, stream every line as it arrives, and report the exit
/// status.
fn run_external(
    run_id: u64,
    step_idx: usize,
    argv: &[String],
    cwd_source: bool,
    source: &SelectionCtx,
    dest: Option<&SelectionCtx>,
    events_tx: &Sender<(u64, FnEvent)>,
) -> Result<(), String> {
    if argv.is_empty() {
        return Err("External step: command is required".to_string());
    }
    // Each argv element expands independently; a whole element that is
    // exactly one `{f}`/`{o}`/... placeholder flattens into one argv
    // element per selected path/name (see `expand_argv_element`) instead of
    // being joined into a single string - there is no shell splitting them
    // back apart, since this runs the command directly.
    let mut expanded: Vec<String> = Vec::with_capacity(argv.len());
    for a in argv {
        match expand_argv_element(a, source, dest).map_err(expand_err_msg)? {
            ExpandedArg::Many(values) => expanded.extend(values),
            ExpandedArg::One(value) => expanded.push(value),
        }
    }
    if expanded.is_empty() {
        return Err("External step: command expanded to nothing (empty selection?)".to_string());
    }
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

    let status = child.wait().map_err(|e| format!("wait failed: {e}"))?;
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
        assert!(events.iter().any(|e| matches!(e, FnEvent::ChangeDir { path } if *path == want)));
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

    #[test]
    fn copy_conflict_is_auto_skipped_instead_of_deadlocking() {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let src_file = src_dir.path().join("a.txt");
        std::fs::write(&src_file, b"new").unwrap();
        std::fs::write(dst_dir.path().join("a.txt"), b"existing").unwrap();

        let (tx, rx) = channel();
        let steps = vec![FnStep::Internal { cmd: InternalCmd::Copy, args: InternalArgs::default() }];
        let src = ctx(&[src_file.to_str().unwrap()], src_dir.path().to_str().unwrap());
        let dst = ctx(&[], dst_dir.path().to_str().unwrap());

        // Must return promptly (the test's own completion is the proof this
        // does not deadlock on Progress::Conflict).
        run_steps(1, &steps, &src, Some(&dst), &tx);
        drop(tx);
        let events = drain(rx);
        assert!(matches!(events.last(), Some(FnEvent::AllDone)));
        assert_eq!(std::fs::read(dst_dir.path().join("a.txt")).unwrap(), b"existing", "conflict must be skipped, not overwritten");
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
}
