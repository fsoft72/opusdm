//! `opusdm-hub --run FUNCTION [--sock PATH]`: ask the running hub to run a
//! saved Function, as a global hotkey does, and exit. On Wayland the global
//! hotkeys are compositor keybinds that start this command (see
//! `global_keys_wl`).

use std::path::PathBuf;

/// A parsed `--run` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    /// The Function id (or inline `exec:` command).
    pub id: String,
    /// The hub socket, when not the default one.
    pub sock: Option<PathBuf>,
}

/// The `--run` request in `args` (without the program name). `Ok(None)` when
/// the command line is not a `--run` one.
pub fn parse_run_args(args: &[String]) -> Result<Option<RunRequest>, String> {
    if args.first().map(String::as_str) != Some("--run") { return Ok(None); }
    let Some(id) = args.get(1).filter(|id| !id.is_empty()) else { return Err("--run needs a function id".to_string()) };
    let sock = match args.get(2..).unwrap_or_default() {
        [] => None,
        [flag, path] if flag == "--sock" => Some(PathBuf::from(path)),
        rest => return Err(format!("unexpected arguments after the function id: {}", rest.join(" "))),
    };
    Ok(Some(RunRequest { id: id.clone(), sock }))
}

/// Handle a `--run` command line: send the request and return the exit
/// code. `None` when the command line is not a `--run` one, so the hub
/// starts as usual.
pub fn run_from_args(args: &[String]) -> Option<i32> {
    let request = match parse_run_args(args) {
        Ok(request) => request?,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-hub", "{e}");
            return Some(2);
        }
    };
    let sock = request.sock.unwrap_or_else(opusdm_ipc::sockpath::hub_sock_path);
    let message = opusdm_ipc::ToHub::RunFunctionHeadless { id: request.id };
    let sent = std::os::unix::net::UnixStream::connect(&sock).and_then(|mut stream| opusdm_ipc::frame::send(&mut stream, &message));
    match sent {
        Ok(()) => Some(0),
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-hub", "cannot reach the hub at {}: {e}", sock.display());
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_run_line_names_the_function_and_maybe_the_socket() {
        assert_eq!(parse_run_args(&args(&["--run", "exec:foot"])), Ok(Some(RunRequest { id: "exec:foot".into(), sock: None })));
        let with_sock = parse_run_args(&args(&["--run", "fn-1", "--sock", "/tmp/h.sock"])).unwrap().unwrap();
        assert_eq!(with_sock.sock, Some(PathBuf::from("/tmp/h.sock")));
    }

    #[test]
    fn other_lines_start_the_hub_and_bad_ones_fail() {
        assert_eq!(parse_run_args(&args(&[])), Ok(None));
        assert_eq!(parse_run_args(&args(&["--debug"])), Ok(None));
        assert!(parse_run_args(&args(&["--run"])).is_err());
        assert!(parse_run_args(&args(&["--run", "x", "--sock"])).is_err());
    }
}
