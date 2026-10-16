//! The boundary to the system tools. `ScreenBackend` is what the session
//! and the hub use; `XrandrBackend` (X11, `xrandr`) and `WlrRandrBackend`
//! (Wayland, `wlr-randr`) are the real ones, [`for_session`] picks one.

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::commands::{xrandr_args, XRANDR};
use crate::parse::{parse_query, Parsed};
use crate::{Layout, ScreenError};

/// Longest wait for `xrandr` or `wlr-randr`: a display server that does not
/// answer must not hang the window or the hub.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running command is checked for its exit.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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

/// A backend that can be cloned and used from any thread (one call at a
/// time), so the window can run `xrandr` on a worker thread.
#[derive(Clone)]
pub struct SharedBackend(Arc<Mutex<Box<dyn ScreenBackend + Send>>>);

impl SharedBackend {
    /// Share `backend`.
    pub fn new(backend: Box<dyn ScreenBackend + Send>) -> Self {
        Self(Arc::new(Mutex::new(backend)))
    }

    /// Run `call` on the backend while holding the lock.
    fn with<T>(&self, call: impl FnOnce(&dyn ScreenBackend) -> Result<T, ScreenError>) -> Result<T, ScreenError> {
        let backend = self.0.lock().map_err(|_| ScreenError::Session("the screen backend failed earlier".to_string()))?;
        call(backend.as_ref())
    }
}

impl ScreenBackend for SharedBackend {
    /// Ask the shared backend.
    fn query(&self) -> Result<Parsed, ScreenError> {
        self.with(|b| b.query())
    }

    /// Ask the shared backend.
    fn apply(&self, layout: &Layout) -> Result<(), ScreenError> {
        self.with(|b| b.apply(layout))
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

/// Read all of `pipe` on a helper thread.
fn read_on_thread(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<std::io::Result<String>> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_string(&mut text)?;
        }
        Ok(text)
    })
}

/// Read the output of `child` and wait for it, at most `timeout`; a child
/// still running then is killed and a `TimedOut` error returned. The wait
/// goes through `opusdm_ipc::reaped`, because the hub's SIGCHLD reaper can
/// collect the child first (a plain wait would then fail with `ECHILD`).
fn collect(mut child: Child, timeout: Duration) -> std::io::Result<(ExitStatus, String, String)> {
    let stdout_reader = read_on_thread(child.stdout.take());
    let stderr_reader = read_on_thread(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let mut lost_since = None;
    let status = loop {
        if let Some(status) = opusdm_ipc::reaped::poll_exit(&mut child, &mut lost_since)? { break status; }
        if Instant::now() >= deadline {
            child.kill()?;
            opusdm_ipc::reaped::wait(&mut child)?;
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("did not answer within {} s", timeout.as_secs_f32())));
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout_reader.join().map_err(|_| std::io::Error::other("the stdout reader failed"))??;
    let stderr = stderr_reader.join().unwrap_or(Ok(String::new())).unwrap_or_default();
    Ok((status, stdout, stderr))
}

/// Run `argv` and return its stdout.
fn run(argv: &[String]) -> Result<String, ScreenError> {
    run_with_timeout(argv, COMMAND_TIMEOUT)
}

/// Run `argv` and return its stdout; give up after `timeout`.
fn run_with_timeout(argv: &[String], timeout: Duration) -> Result<String, ScreenError> {
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
    let (status, stdout, stderr) = collect(child, timeout).map_err(|e| failed(e.to_string()))?;
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
    fn a_program_that_hangs_is_stopped_after_the_timeout() {
        let started = std::time::Instant::now();
        let err = run_with_timeout(&["sleep".to_string(), "5".to_string()], std::time::Duration::from_millis(200)).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "waited {:?}", started.elapsed());
        assert!(matches!(&err, ScreenError::CommandFailed { stderr, .. } if stderr.contains("did not answer")), "{err}");
    }

    #[test]
    fn a_shared_backend_reaches_the_same_backend_from_another_thread() {
        let shared = SharedBackend::new(Box::new(crate::fake::FakeBackend::new(crate::fake::two_monitor_system())));
        let other = shared.clone();
        let layout = shared.query().unwrap().layout;
        std::thread::spawn(move || other.apply(&layout).unwrap()).join().unwrap();
        assert_eq!(shared.query().unwrap().layout, crate::fake::two_monitor_system().layout);
    }

    #[test]
    fn a_working_program_gives_its_output() {
        assert_eq!(run(&["echo".to_string(), "hi".to_string()]).unwrap(), "hi\n");
    }
}
