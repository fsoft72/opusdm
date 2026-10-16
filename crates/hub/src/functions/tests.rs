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

fn new_lister_events(args: &[(&str, &str)], cwd: &Path) -> Vec<FnEvent> {
    let map: BTreeMap<String, String> = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let steps = vec![FnStep::Internal { cmd: InternalCmd::NewLister, args: InternalArgs(map) }];
    let (tx, rx) = channel();
    run_steps(1, &steps, &ctx(&[], cwd.to_str().unwrap()), None, &tx);
    drop(tx);
    drain(rx)
}

#[test]
fn new_lister_without_a_path_uses_the_default_folder() {
    let td = tempfile::tempdir().unwrap();
    let events = new_lister_events(&[], td.path());
    assert!(events.iter().any(|e| matches!(e, FnEvent::NewLister { path: None })));
    assert!(matches!(events.last(), Some(FnEvent::AllDone)));
}

#[test]
fn new_lister_with_a_path_checks_it_is_a_directory() {
    let td = tempfile::tempdir().unwrap();
    std::fs::create_dir(td.path().join("d")).unwrap();
    std::fs::write(td.path().join("f.txt"), b"x").unwrap();
    let want = std::fs::canonicalize(td.path().join("d")).unwrap();
    let events = new_lister_events(&[("path", "d")], td.path());
    assert!(events.iter().any(|e| matches!(e, FnEvent::NewLister { path: Some(p) } if *p == want)));
    let events = new_lister_events(&[("path", "f.txt")], td.path());
    assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
    let events = new_lister_events(&[("path", "missing")], td.path());
    assert!(matches!(events.last(), Some(FnEvent::Failed { .. })));
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
fn run_with_window_and_wait_keeps_the_status_the_sigchld_reaper_took() {
    // The program exits at once, but a background child keeps its output
    // pipes open, so the step waits on them while the "reaper" (this test,
    // standing in for the hub's `waitpid(-1)`) collects the exit status.
    let td = tempfile::tempdir().unwrap();
    let src = ctx(&[], td.path().to_str().unwrap());
    let steps = run_args("sh -c '(sleep 0.5) & exit 0'", &["window", "wait"]);
    let (tx, rx) = channel();
    let events = std::thread::scope(|scope| {
        let src = &src;
        let steps = &steps;
        scope.spawn(move || run_steps(1, steps, src, None, &tx));
        let mut events = Vec::new();
        for (_, ev) in rx {
            if let FnEvent::Output { line, .. } = &ev {
                if let Some(pid) = line.strip_suffix(')').and_then(|l| l.rsplit_once("(pid ")).map(|(_, p)| p.parse::<i32>().unwrap()) {
                    let mut raw: libc::c_int = 0;
                    // SAFETY: waits for a child of this process; `raw` is a valid out-pointer.
                    assert_eq!(unsafe { libc::waitpid(pid, &mut raw, 0) }, pid);
                    opusdm_ipc::reaped::record(pid as u32, raw);
                }
            }
            events.push(ev);
        }
        events
    });
    assert!(events.iter().any(|e| matches!(e, FnEvent::Window { msg: WindowMsg::Closed(Some(0)), .. })));
    assert!(matches!(events.last(), Some(FnEvent::AllDone)), "the step succeeded");
}

/// Run `steps`, cancel the run after `after`, and return its events and how
/// long it took. Questions are never answered (their replies are kept alive),
/// like a dialog the user leaves open.
fn run_cancelled_after(steps: &[FnStep], after: std::time::Duration) -> (Vec<FnEvent>, std::time::Duration) {
    let td = tempfile::tempdir().unwrap();
    let src = ctx(&[], td.path().to_str().unwrap());
    let cancel = CancelToken::new();
    let (tx, rx) = channel();
    let started = std::time::Instant::now();
    let worker = {
        let (steps, run_cancel) = (steps.to_vec(), cancel.clone());
        std::thread::spawn(move || run_steps_cancellable(1, &steps, &src, None, &tx, &run_cancel))
    };
    std::thread::sleep(after);
    cancel.cancel();
    let mut events = Vec::new();
    let mut unanswered = Vec::new();
    // Bounded, so a step that ignores the cancel fails the test instead of hanging it.
    while let Ok((_, ev)) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
        match ev {
            FnEvent::Confirm { reply, .. } => unanswered.push(reply),
            other => events.push(other),
        }
    }
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(10), "the step was not stopped: {took:?}");
    worker.join().unwrap();
    (events, took)
}

#[test]
fn cancelling_stops_an_external_step_that_is_running() {
    let steps = vec![FnStep::External { argv: vec!["sleep".into(), "30".into()], cwd_source: true }];
    let (events, took) = run_cancelled_after(&steps, std::time::Duration::from_millis(200));
    assert!(took < std::time::Duration::from_secs(10), "the step was not stopped: {took:?}");
    assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("cancelled")));
}

#[test]
fn cancelling_stops_a_run_step_that_waits_with_a_window() {
    let (events, took) = run_cancelled_after(&run_args("sleep 30", &["window", "wait"]), std::time::Duration::from_millis(200));
    assert!(took < std::time::Duration::from_secs(10), "the step was not stopped: {took:?}");
    assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("cancelled")));
    assert!(events.iter().any(|e| matches!(e, FnEvent::Window { msg: WindowMsg::Closed(_), .. })), "the window is told the program ended");
}

#[test]
fn cancelling_stops_a_confirm_nobody_answers() {
    let (events, took) = run_cancelled_after(&confirm_steps("Sure?"), std::time::Duration::from_millis(200));
    assert!(took < std::time::Duration::from_secs(10), "the step was not stopped: {took:?}");
    assert!(matches!(events.last(), Some(FnEvent::Failed { step_idx: 0, error }) if error.contains("cancelled")));
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

#[test]
fn usable_cwd_keeps_an_existing_folder() {
    assert_eq!(usable_cwd(Path::new("/tmp")), PathBuf::from("/tmp"));
}

#[test]
fn usable_cwd_maps_an_archive_path_to_the_folder_holding_it() {
    assert_eq!(usable_cwd(Path::new("/tmp/nope.zip#sub/dir")), PathBuf::from("/tmp"));
}

#[test]
fn usable_cwd_falls_back_for_a_remote_url() {
    assert!(usable_cwd(Path::new("sftp://host/some/dir")).is_dir());
}

#[test]
fn run_step_starts_the_program_when_the_lister_folder_is_virtual() {
    let (tx, rx) = channel();
    let steps = vec![FnStep::External { argv: vec!["echo".into(), "ok".into()], cwd_source: true }];
    let src = ctx(&[], "/tmp/nope.zip#sub");
    run_steps(1, &steps, &src, None, &tx);
    drop(tx);
    assert!(matches!(drain(rx).last(), Some(FnEvent::AllDone)));
}
