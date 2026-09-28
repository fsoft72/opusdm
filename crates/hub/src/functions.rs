//! Executes a Function's steps on their own thread once the hub has
//! collected the SOURCE and (if any) DEST selection: internal steps reuse
//! `opusdm_fsops::run_job` or a direct `std::fs::rename`, external steps
//! spawn a subprocess and stream its output. See
//! `docs/superpowers/specs/2026-09-28-function-editor-design.md`.

use opusdm_fsops::{run_job, CancelToken, JobSpec, Progress};
use opusdm_ipc::function::{expand_argv_element, expand_text, ExpandError, ExpandedArg, FnStep, InternalArgs, InternalCmd, SelectionCtx};
use opusdm_ipc::{Op, Resolution};
use std::io::{BufRead, BufReader};
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
    for (step_idx, step) in steps.iter().enumerate() {
        let result = match step {
            FnStep::Internal { cmd, args } => run_internal(run_id, step_idx, *cmd, args, source, dest, events_tx),
            FnStep::External { argv, cwd_source } => {
                run_external(run_id, step_idx, argv, *cwd_source, source, dest, events_tx)
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
        InternalCmd::Copy | InternalCmd::Move | InternalCmd::Delete => {
            let op = match cmd {
                InternalCmd::Copy => Op::Copy,
                InternalCmd::Move => Op::Move,
                InternalCmd::Delete => Op::Delete,
                _ => unreachable!("Rename/MakeDir/NewFile handled above"),
            };
            let dest_dir = match dest {
                Some(d) => d.cwd.clone(),
                None if cmd == InternalCmd::Delete => source.cwd.clone(),
                None => return Err(format!("{}: no DEST Lister open", cmd.label())),
            };
            let spec = JobSpec { op, srcs: source.paths.clone(), dest_dir };
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
}
