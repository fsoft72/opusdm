//! References to something a hotkey, menu entry or script event can run.
//!
//! The places that used to store a saved Function's id keep storing one
//! string. A reserved prefix turns it into an inline command: `bash:<cmd>`
//! (run by `bash -c`), `bash+out:<cmd>` (the same, output in a window) or
//! `exec:<program and arguments>` (started directly, no shell) or
//! `internal:<command and arguments>` (an internal command, written like a
//! command line entry: `internal:CopyAs new_name=*.bak`) or `script:<name>`
//! (a JavaScript script of `~/.config/opusdm/scripts`). A string
//! without a prefix is a saved Function id, as before. An inline reference
//! runs as one synthetic `Run` step, so the hub needs no new executor. See
//! `docs/superpowers/specs/2026-10-05-inline-commands-design.md`.

use crate::cli::{self, tokenize, CliCommand};
use crate::function::{FnStep, FunctionId, InternalArgs, InternalCmd, BOOL_TRUE};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Prefix of a bash command whose output is not shown.
pub const BASH_PREFIX: &str = "bash:";
/// Prefix of a bash command whose output goes to a window.
pub const BASH_OUT_PREFIX: &str = "bash+out:";
/// Prefix of a program started directly.
pub const EXEC_PREFIX: &str = "exec:";
/// Prefix of an internal command with its arguments.
pub const INTERNAL_PREFIX: &str = "internal:";
/// Prefix of a JavaScript script, by name.
pub const SCRIPT_PREFIX: &str = "script:";

/// What a reference string stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuncRef {
    /// A saved Function, by id (no prefix).
    Saved(FunctionId),
    /// `bash -c cmd`; the output goes to a window when `show_output`.
    Bash { cmd: String, show_output: bool },
    /// A program and its arguments, split like a `Run` step splits them.
    Exec { line: String },
    /// An internal command name followed by its arguments, as on the
    /// command line (`MakeDir backup`).
    Internal { line: String },
    /// A JavaScript script, by name (`script:backup`).
    Script { name: String },
}

/// Read a reference string. Total: anything that is not an inline command is
/// a saved Function id.
pub fn parse(reference: &str) -> FuncRef {
    if let Some(cmd) = reference.strip_prefix(BASH_OUT_PREFIX) {
        return FuncRef::Bash { cmd: cmd.to_string(), show_output: true };
    }
    if let Some(cmd) = reference.strip_prefix(BASH_PREFIX) {
        return FuncRef::Bash { cmd: cmd.to_string(), show_output: false };
    }
    if let Some(line) = reference.strip_prefix(EXEC_PREFIX) {
        return FuncRef::Exec { line: line.to_string() };
    }
    if let Some(line) = reference.strip_prefix(INTERNAL_PREFIX) {
        return FuncRef::Internal { line: line.to_string() };
    }
    if let Some(name) = reference.strip_prefix(SCRIPT_PREFIX) {
        return FuncRef::Script { name: name.to_string() };
    }
    FuncRef::Saved(reference.to_string())
}

/// The reference string for `r`; the inverse of [`parse`].
pub fn format(r: &FuncRef) -> String {
    match r {
        FuncRef::Saved(id) => id.clone(),
        FuncRef::Bash { cmd, show_output: false } => format!("{BASH_PREFIX}{cmd}"),
        FuncRef::Bash { cmd, show_output: true } => format!("{BASH_OUT_PREFIX}{cmd}"),
        FuncRef::Exec { line } => format!("{EXEC_PREFIX}{line}"),
        FuncRef::Internal { line } => format!("{INTERNAL_PREFIX}{line}"),
        FuncRef::Script { name } => format!("{SCRIPT_PREFIX}{name}"),
    }
}

/// `text` as one word that [`tokenize`] reads back unchanged: single quotes
/// around it, with `\` and `'` escaped by a backslash.
pub fn quote(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        if c == '\\' || c == '\'' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Whether `path` is a regular file with an execute bit.
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Whether `program` can be started: a path (it contains `/`) to an
/// executable file, or a name found in a `PATH` directory.
pub fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        return is_executable_file(Path::new(program));
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| is_executable_file(&dir.join(program)))
}

/// The internal command `line` asks for, with its arguments checked against
/// the command's parameters. The error is the text to show the user.
fn parse_internal(line: &str) -> Result<(InternalCmd, InternalArgs), String> {
    let head = tokenize(line).map_err(|e| format!("internal: {e}"))?.into_iter().next();
    let Some(head) = head else { return Err("internal: no command given".to_string()) };
    if cli::find_command(&head).is_none() {
        return Err(format!("internal: unknown command '{head}'"));
    }
    match cli::parse(line, &[]) {
        Ok(CliCommand::Internal { cmd, args }) => Ok((cmd, args)),
        Ok(_) => Err("internal: not an internal command".to_string()),
        Err(e) => Err(format!("internal: {e}")),
    }
}

/// Why `r` cannot run, or `None` when it can. A saved id is never a problem
/// here: whether it exists is checked against the function list.
pub fn problem(r: &FuncRef) -> Option<String> {
    match r {
        FuncRef::Saved(_) => None,
        FuncRef::Bash { cmd, .. } => cmd.trim().is_empty().then(|| "the bash command is empty".to_string()),
        FuncRef::Exec { line } => {
            let words = match tokenize(line) {
                Ok(words) => words,
                Err(e) => return Some(format!("exec: {e}")),
            };
            let Some(program) = words.first() else { return Some("exec: no program given".to_string()) };
            (!program_exists(program)).then(|| format!("exec: '{program}' was not found"))
        }
        FuncRef::Internal { line } => parse_internal(line).err(),
        FuncRef::Script { name } => {
            if let Err(e) = crate::scripts::validate_name(name) {
                return Some(format!("script: {e}"));
            }
            let path = crate::scripts::scripts_dir().join(format!("{name}.{}", crate::scripts::EXTENSION));
            (!path.is_file()).then(|| format!("script: '{name}' was not found"))
        }
    }
}

/// A short text for an inline reference (`bash: ls`, `exec: kitty`), or
/// `None` for a saved Function.
pub fn describe(r: &FuncRef) -> Option<String> {
    match r {
        FuncRef::Saved(_) => None,
        FuncRef::Bash { cmd, .. } => Some(format!("bash: {cmd}")),
        FuncRef::Exec { line } => Some(format!("exec: {line}")),
        FuncRef::Internal { line } => Some(format!("internal: {}", line.trim())),
        FuncRef::Script { name } => Some(format!("script: {name}")),
    }
}

/// The text a list shows for `reference`: a saved Function's name (or
/// `(missing function)`), or the [`describe`] text of an inline command.
pub fn title(reference: &str, functions: &[crate::function::Function]) -> String {
    let parsed = parse(reference);
    if let Some(text) = describe(&parsed) {
        return text;
    }
    functions.iter().find(|f| f.id == reference).map(|f| f.name.clone()).unwrap_or_else(|| "(missing function)".to_string())
}

/// The steps that run an inline reference: one `Run` step. A bash command
/// gets the SOURCE selection as `"$@"` and no placeholder expansion inside
/// the script. Fails with the
/// [`problem`] text, and for a saved Function (it has its own steps).
pub fn to_steps(r: &FuncRef) -> Result<Vec<FnStep>, String> {
    to_steps_with(r, false)
}

/// [`to_steps`], and with `wait` the step ends only when the program has
/// (a failure fails the function). The hub uses it for hooks it must finish
/// before it quits.
pub fn to_steps_with(r: &FuncRef, wait: bool) -> Result<Vec<FnStep>, String> {
    if let FuncRef::Saved(id) = r {
        return Err(format!("'{id}' is a saved function, not an inline command"));
    }
    if let Some(reason) = problem(r) {
        return Err(reason);
    }
    let mut args = InternalArgs::default();
    match r {
        FuncRef::Saved(_) => {}
        FuncRef::Bash { cmd, show_output } => {
            // The script is the third word and is never expanded (the hub's
            // `Run` step honours the `script` flag): file names pasted into
            // it would be run as shell code. The selection is passed as the
            // positional parameters instead, so the script reads `"$@"`.
            args.0.insert("command".to_string(), format!("bash -c {} opusdm {{f}}", quote(cmd)));
            args.0.insert("script".to_string(), BOOL_TRUE.to_string());
            if *show_output {
                args.0.insert("window".to_string(), BOOL_TRUE.to_string());
            }
        }
        FuncRef::Exec { line } => {
            args.0.insert("command".to_string(), line.trim().to_string());
        }
        FuncRef::Internal { line } => {
            let (cmd, args) = parse_internal(line)?;
            return Ok(vec![FnStep::Internal { cmd, args }]);
        }
        FuncRef::Script { name } => {
            // The script itself waits for what it runs: `wait` changes nothing.
            args.0.insert("script".to_string(), name.clone());
            return Ok(vec![FnStep::Internal { cmd: InternalCmd::Script, args }]);
        }
    }
    if wait {
        args.0.insert("wait".to_string(), BOOL_TRUE.to_string());
    }
    Ok(vec![FnStep::Internal { cmd: InternalCmd::Run, args }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(r: &FuncRef) -> InternalArgs {
        match to_steps(r).unwrap().remove(0) {
            FnStep::Internal { cmd: InternalCmd::Run, args } => args,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_string_without_a_prefix_is_a_saved_id() {
        assert_eq!(parse("fn-1a-2"), FuncRef::Saved("fn-1a-2".into()));
        assert_eq!(parse(""), FuncRef::Saved(String::new()));
    }

    #[test]
    fn parse_and_format_round_trip_every_kind() {
        for text in ["fn-1a-2", "bash:ls -l", "bash+out:ls -l", "exec:kitty --class x", "bash:echo a:b", "exec:", "script:my backup"] {
            assert_eq!(format(&parse(text)), text);
        }
        assert_eq!(parse("bash+out:ls"), FuncRef::Bash { cmd: "ls".into(), show_output: true });
        assert_eq!(parse("bash:ls"), FuncRef::Bash { cmd: "ls".into(), show_output: false });
    }

    #[test]
    fn a_script_runs_as_one_script_step_also_from_the_command_line() {
        // Like `exec:` with a missing program, a missing script is reported
        // before anything runs.
        let error = to_steps(&parse("script:no such script here")).unwrap_err();
        assert!(error.contains("'no such script here' was not found"), "{error}");
        assert!(problem(&parse("script:a/b")).unwrap().contains("'/'"));
        match to_steps(&parse("internal:Script backup args='{f}'")).unwrap().remove(0) {
            FnStep::Internal { cmd: InternalCmd::Script, args } => {
                assert_eq!((args.get("script"), args.get("args")), (Some("backup"), Some("{f}")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn quote_survives_tokenize_for_awkward_text() {
        for text in ["plain", "two words", "it's", "say \"hi\"", "back\\slash", "$HOME {a,b} ${HOME}", "", "line1\nline2", "'"] {
            assert_eq!(tokenize(&quote(text)).unwrap(), vec![text.to_string()], "{text:?}");
        }
    }

    #[test]
    fn a_bash_command_becomes_one_quoted_argument_of_bash_c() {
        let r = parse("bash:echo 'x' | wc -l");
        let command = run_args(&r).get("command").unwrap().to_string();
        // The selection follows as "$@"; the script itself is never expanded.
        assert_eq!(tokenize(&command).unwrap(), vec!["bash", "-c", "echo 'x' | wc -l", "opusdm", "{f}"]);
        assert!(run_args(&r).flag("script"));
        assert!(!run_args(&r).flag("window"));
    }

    #[test]
    fn show_output_sets_the_window_flag() {
        assert!(run_args(&parse("bash+out:ls")).flag("window"));
    }

    #[test]
    fn exec_keeps_the_line_and_never_opens_a_window() {
        let args = run_args(&parse("exec:sh -c 'echo hi'"));
        assert_eq!(args.get("command"), Some("sh -c 'echo hi'"));
        assert!(!args.flag("window"));
    }

    #[test]
    fn exec_splits_a_quoted_program_path_with_a_space() {
        let words = tokenize("\"/opt/my app/run\" --flag").unwrap();
        assert_eq!(words, vec!["/opt/my app/run", "--flag"]);
    }

    #[test]
    fn problem_reports_empty_bash_blank_exec_and_unbalanced_quotes() {
        assert!(problem(&parse("bash:   ")).is_some());
        assert!(problem(&parse("exec:  ")).is_some());
        assert!(problem(&parse("exec:sh 'oops")).unwrap().contains("quote"));
        assert_eq!(problem(&parse("bash:ls")), None);
        assert_eq!(problem(&parse("fn-1")), None);
    }

    #[test]
    fn an_internal_reference_round_trips_and_becomes_that_command() {
        assert_eq!(parse("internal:MakeDir backup"), FuncRef::Internal { line: "MakeDir backup".into() });
        assert_eq!(format(&parse("internal:CopyAs new_name=*.bak")), "internal:CopyAs new_name=*.bak");
        let steps = to_steps(&parse("internal:MakeDir backup")).unwrap();
        let FnStep::Internal { cmd, args } = &steps[0] else { panic!("{steps:?}") };
        assert_eq!((*cmd, args.get("name")), (InternalCmd::MakeDir, Some("backup")));
        assert_eq!(describe(&parse("internal:Hide")).as_deref(), Some("internal: Hide"));
    }

    #[test]
    fn problem_reports_a_bad_internal_command() {
        assert_eq!(problem(&parse("internal:Hide")), None);
        assert_eq!(problem(&parse("internal:Copy newer")), None);
        assert!(problem(&parse("internal:")).unwrap().contains("no command"));
        assert!(problem(&parse("internal:frobnicate")).unwrap().contains("unknown command"));
        assert!(problem(&parse("internal:MakeDir")).unwrap().contains("missing required"));
        assert!(problem(&parse("internal:Hide extra")).is_some());
        assert!(problem(&parse("internal:MakeDir 'oops")).unwrap().contains("quote"));
    }

    #[test]
    fn problem_checks_that_the_program_exists() {
        assert_eq!(problem(&parse("exec:sh -c true")), None);
        assert_eq!(problem(&parse("exec:/bin/sh")), None);
        assert!(problem(&parse("exec:opusdm-no-such-program-xyz")).unwrap().contains("was not found"));
        assert!(problem(&parse("exec:/no/such/dir/prog")).is_some());
    }

    #[test]
    fn program_exists_rejects_a_file_without_the_execute_bit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain");
        std::fs::write(&file, "x").unwrap();
        assert!(!program_exists(file.to_str().unwrap()));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(program_exists(file.to_str().unwrap()));
    }

    #[test]
    fn to_steps_refuses_a_saved_id_and_a_broken_command() {
        assert!(to_steps(&parse("fn-1")).is_err());
        assert!(to_steps(&parse("bash:")).is_err());
    }

    #[test]
    fn describe_names_inline_commands_only() {
        assert_eq!(describe(&parse("bash+out:ls")), Some("bash: ls".to_string()));
        assert_eq!(describe(&parse("exec:kitty")), Some("exec: kitty".to_string()));
        assert_eq!(describe(&parse("fn-1")), None);
    }

    #[test]
    fn title_names_a_saved_function_or_describes_an_inline_command() {
        let mut f = crate::function::Function::new("Copy it");
        f.id = "fn-1".to_string();
        let functions = [f];
        assert_eq!(title("fn-1", &functions), "Copy it");
        assert_eq!(title("fn-gone", &functions), "(missing function)");
        assert_eq!(title("bash:ls -l", &functions), "bash: ls -l");
        assert_eq!(title("exec:kitty", &functions), "exec: kitty");
    }
}
