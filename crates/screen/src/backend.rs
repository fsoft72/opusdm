//! The boundary to the system tools. `ScreenBackend` is what the session
//! and the hub use; `XrandrBackend` (X11, `xrandr`) and `WlrRandrBackend`
//! (Wayland, `wlr-randr`) are the real ones, [`for_session`] picks one.

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::commands::{xrandr_args, XRANDR};
use crate::parse::{parse_query, Parsed};
use crate::{Layout, ScreenError};

/// What reads and changes the monitors of the system.
pub trait ScreenBackend {
    /// The connected monitors and how they are set now.
    fn query(&self) -> Result<Parsed, ScreenError>;
    /// Set the monitors as `layout` says, in one step.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError>;
}

impl<T: ScreenBackend + ?Sized> ScreenBackend for Box<T> {
    /// Ask the boxed backend.
    fn query(&self) -> Result<Parsed, ScreenError> {
        (**self).query()
    }

    /// Ask the boxed backend.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError> {
        (**self).apply(layout)
    }
}

/// Why the session cannot be configured, or `None`.
pub fn session_problem_from(window_system: opusdm_ipc::session::WindowSystem) -> Option<String> {
    match window_system {
        opusdm_ipc::session::WindowSystem::None => Some("No display: run opusdm-screen inside the OpusDM session.".to_string()),
        _ => None,
    }
}

/// `session_problem_from` for the current environment.
pub fn session_problem() -> Option<String> {
    session_problem_from(opusdm_ipc::session::current())
}

/// The real backend of this session: `wlr-randr` on Wayland, `xrandr`
/// otherwise.
pub fn for_session() -> Box<dyn ScreenBackend + Send> {
    if opusdm_ipc::session::is_wayland() { Box::new(WlrRandrBackend) } else { Box::new(XrandrBackend) }
}

/// Read the output of `child` and wait for it. The wait goes through
/// `opusdm_ipc::reaped`, because the hub's SIGCHLD reaper can collect the
/// child first (a plain wait would then fail with `ECHILD`).
fn collect(mut child: Child) -> std::io::Result<(ExitStatus, String, String)> {
    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = stderr {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)?;
    }
    let status = opusdm_ipc::reaped::wait(&mut child)?;
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

/// Run `argv` and return its stdout.
fn run(argv: &[String]) -> Result<String, ScreenError> {
    let Some(program) = argv.first() else { return Err(ScreenError::Session("empty command".to_string())) };
    let failed = |reason: String| ScreenError::CommandFailed { command: argv.join(" "), stderr: reason };
    let child = Command::new(program).args(&argv[1..]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            let package = if program == crate::wlr::WLR_RANDR { "wlr-randr" } else { "x11-xserver-utils" };
            ScreenError::MissingTool { tool: program.clone(), package: package.to_string() }
        } else {
            failed(e.to_string())
        }
    })?;
    let (status, stdout, stderr) = collect(child).map_err(|e| failed(e.to_string()))?;
    if !status.success() { return Err(failed(stderr.trim().to_string())); }
    Ok(stdout)
}

/// The real backend: `xrandr`.
#[derive(Clone, Copy, Debug, Default)]
pub struct XrandrBackend;

impl XrandrBackend {
    /// A backend for the current X display.
    pub fn new() -> Self {
        Self
    }
}

impl ScreenBackend for XrandrBackend {
    /// Run `xrandr --query --props` and parse it.
    fn query(&self) -> Result<Parsed, ScreenError> {
        let args = [XRANDR, "--query", "--props"].map(String::from);
        Ok(parse_query(&run(&args)?))
    }

    /// Run the one `xrandr` call that sets the whole layout.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError> {
        let current = self.query()?.layout;
        run(&xrandr_args(layout, &current)).map(|_| ())
    }
}

/// The Wayland backend: `wlr-randr` (wlroots compositors).
#[derive(Clone, Copy, Debug, Default)]
pub struct WlrRandrBackend;

impl ScreenBackend for WlrRandrBackend {
    /// Run `wlr-randr --json` and parse it.
    fn query(&self) -> Result<Parsed, ScreenError> {
        let args = [crate::wlr::WLR_RANDR, "--json"].map(String::from);
        crate::wlr::parse_json(&run(&args)?)
    }

    /// Run the one `wlr-randr` call that sets the whole layout.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError> {
        run(&crate::wlr::wlr_randr_args(layout)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_no_display_at_all_is_a_problem() {
        use opusdm_ipc::session::WindowSystem;
        assert!(session_problem_from(WindowSystem::None).is_some());
        assert_eq!(session_problem_from(WindowSystem::X11), None);
        assert_eq!(session_problem_from(WindowSystem::Wayland), None);
    }

    #[test]
    fn a_missing_program_is_reported_with_its_package() {
        let err = run(&["opusdm-no-such-tool".to_string()]).unwrap_err();
        assert!(matches!(err, ScreenError::MissingTool { .. }), "{err}");
    }

    #[test]
    fn a_failing_program_reports_its_command_and_stderr() {
        let err = run(&["sh".to_string(), "-c".to_string(), "echo boom >&2; exit 3".to_string()]).unwrap_err();
        assert_eq!(err, ScreenError::CommandFailed { command: "sh -c echo boom >&2; exit 3".to_string(), stderr: "boom".to_string() });
    }

    #[test]
    fn a_working_program_gives_its_output() {
        assert_eq!(run(&["echo".to_string(), "hi".to_string()]).unwrap(), "hi\n");
    }
}
