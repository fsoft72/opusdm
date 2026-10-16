//! Starting what the user chose: an application or a command line.

use gtk4::gio;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Split `text` into a program and its arguments, with the same rules as the
/// `exec:` hotkeys (quotes group words, no shell). Errors on empty text or an
/// unclosed quote.
pub fn parse(text: &str) -> Result<Vec<String>, String> {
    let words = opusdm_ipc::cli::tokenize(text.trim())?;
    if words.is_empty() {
        return Err("nothing to run".to_string());
    }
    Ok(words)
}

/// The command line for the command `program` followed by the typed `args`.
pub fn command_argv(program: &str, args: &str) -> Result<Vec<String>, String> {
    let mut argv = vec![program.to_string()];
    argv.extend(opusdm_ipc::cli::tokenize(args)?);
    Ok(argv)
}

/// Start `argv` detached: own process group, no stdio, so it outlives this
/// window. Fails when the program cannot be started (not found, not
/// executable).
pub fn spawn_detached(argv: &[String]) -> Result<(), String> {
    let Some((program, args)) = argv.split_first() else { return Err("nothing to run".to_string()) };
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_child| ())
        .map_err(|e| format!("cannot run '{program}': {e}"))
}

/// Start the application with desktop file id `id` through the display's
/// launch context.
pub fn launch_app(id: &str) -> Result<(), String> {
    let info = gio::DesktopAppInfo::new(id).ok_or_else(|| format!("application '{id}' is not installed"))?;
    opusdm_apps::apps::launch(&info);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_words_and_honours_quotes() {
        assert_eq!(parse("  bash -c 'echo hi' ").unwrap(), ["bash", "-c", "echo hi"]);
    }

    #[test]
    fn parse_refuses_empty_text_and_an_unclosed_quote() {
        assert!(parse("   ").is_err());
        assert!(parse("bash -c 'oops").is_err());
    }

    #[test]
    fn command_argv_appends_the_tokenized_arguments() {
        assert_eq!(command_argv("bash", "-c \"ls -l\"").unwrap(), ["bash", "-c", "ls -l"]);
        assert_eq!(command_argv("bash", "").unwrap(), ["bash"]);
    }

    #[test]
    fn spawn_detached_starts_a_program_and_reports_a_missing_one() {
        assert!(spawn_detached(&["true".to_string()]).is_ok());
        let err = spawn_detached(&["opusdm-no-such-program-xyz".to_string()]).unwrap_err();
        assert!(err.contains("opusdm-no-such-program-xyz"), "{err}");
        assert!(spawn_detached(&[]).is_err());
    }
}
