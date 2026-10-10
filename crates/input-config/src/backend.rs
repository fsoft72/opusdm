//! The boundary to the X tools. `Backend` is what the session and the hub
//! use; `XBackend` is the real one (it starts `xinput`, `setxkbmap`, `xset`).

use std::cell::RefCell;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::change::Change;
use crate::commands::{commands_for, PointerDevice};
use crate::parse::{self, device_kind, keyboard_from_outputs, parse_pointer_list, parse_props, parse_slave_devices, PointerKind};
use crate::{InputError, InputSettings, KeyboardSettings};

/// Whether a group of settings can be used, and why not.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupStatus {
    /// The group can be edited.
    pub available: bool,
    /// Why it cannot (empty when available).
    pub reason: String,
}

impl GroupStatus {
    /// An editable group.
    pub fn ok() -> Self {
        Self { available: true, reason: String::new() }
    }

    /// A group that cannot be used, with the reason to show.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self { available: false, reason: reason.into() }
    }
}

/// The state of the three groups.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// Mouse group.
    pub mouse: GroupStatus,
    /// Touchpad group.
    pub touchpad: GroupStatus,
    /// Keyboard group.
    pub keyboard: GroupStatus,
}

impl Status {
    /// Every group editable.
    pub fn all_ok() -> Self {
        Self { mouse: GroupStatus::ok(), touchpad: GroupStatus::ok(), keyboard: GroupStatus::ok() }
    }
}

/// What reads and changes the system input settings.
pub trait Backend {
    /// Which groups can be used.
    fn status(&self) -> Status;
    /// The settings the system has right now.
    fn read_current(&self) -> InputSettings;
    /// Apply one change.
    fn apply(&self, change: &Change) -> Result<(), InputError>;
    /// A string that changes when a pointer is plugged in or removed.
    fn signature(&self) -> String;
    /// Look at the devices again (after `signature` changed).
    fn refresh(&self);
}

/// Why the session cannot be configured, or `None`. On Wayland the
/// settings belong to the compositor, and OpusDM can write them for labwc
/// and Wayfire only (`known_compositor`).
pub fn session_problem_from(window_system: opusdm_ipc::session::WindowSystem, known_compositor: bool) -> Option<String> {
    use opusdm_ipc::session::WindowSystem;
    match window_system {
        WindowSystem::None => Some("No display: run opusdm-input-config inside the OpusDM session.".to_string()),
        WindowSystem::Wayland if !known_compositor => {
            Some("On Wayland the input settings belong to the compositor: OpusDM can set them under labwc and Wayfire only.".to_string())
        }
        _ => None,
    }
}

/// `session_problem_from` for the current environment.
pub fn session_problem() -> Option<String> {
    let known_compositor = opusdm_wayland::labwc::under_labwc() || opusdm_wayland::wayfire::under_wayfire();
    session_problem_from(opusdm_ipc::session::current(), known_compositor)
}

/// The real backend of this session: Wayfire's or labwc's configuration on
/// Wayland, the X tools otherwise.
pub enum SessionBackend {
    X(XBackend),
    Labwc(crate::labwc::LabwcBackend),
    Wayfire(crate::wayfire::WayfireBackend),
}

/// The backend for this session (see [`session_problem`] first).
pub fn for_session() -> SessionBackend {
    if !opusdm_ipc::session::is_wayland() { return SessionBackend::X(XBackend::new()); }
    if opusdm_wayland::wayfire::under_wayfire() { return SessionBackend::Wayfire(crate::wayfire::WayfireBackend::new()); }
    SessionBackend::Labwc(crate::labwc::LabwcBackend::new())
}

impl Backend for SessionBackend {
    /// Ask the backend of the session.
    fn status(&self) -> Status {
        match self { SessionBackend::X(b) => b.status(), SessionBackend::Labwc(b) => b.status(), SessionBackend::Wayfire(b) => b.status() }
    }

    /// Ask the backend of the session.
    fn read_current(&self) -> InputSettings {
        match self { SessionBackend::X(b) => b.read_current(), SessionBackend::Labwc(b) => b.read_current(), SessionBackend::Wayfire(b) => b.read_current() }
    }

    /// Ask the backend of the session.
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        match self { SessionBackend::X(b) => b.apply(change), SessionBackend::Labwc(b) => b.apply(change), SessionBackend::Wayfire(b) => b.apply(change) }
    }

    /// Ask the backend of the session.
    fn signature(&self) -> String {
        match self { SessionBackend::X(b) => b.signature(), SessionBackend::Labwc(b) => b.signature(), SessionBackend::Wayfire(b) => b.signature() }
    }

    /// Ask the backend of the session.
    fn refresh(&self) {
        match self { SessionBackend::X(b) => b.refresh(), SessionBackend::Labwc(b) => b.refresh(), SessionBackend::Wayfire(b) => b.refresh() }
    }
}

/// The Debian package that holds `tool`, for the error message.
fn package_of(tool: &str) -> &'static str {
    match tool {
        "xinput" => "xinput",
        "setxkbmap" => "x11-xkb-utils",
        _ => "x11-xserver-utils",
    }
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
fn run(argv: &[String]) -> Result<String, InputError> {
    let Some(program) = argv.first() else { return Err(InputError::Session("empty command".to_string())) };
    let failed = |reason: String| InputError::CommandFailed { command: argv.join(" "), stderr: reason };
    let child = Command::new(program).args(&argv[1..]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            InputError::MissingTool { tool: program.clone(), package: package_of(program).to_string() }
        } else {
            failed(e.to_string())
        }
    })?;
    let (status, stdout, stderr) = collect(child).map_err(|e| failed(e.to_string()))?;
    if !status.success() { return Err(failed(stderr.trim().to_string())); }
    Ok(stdout)
}

/// Whether `change` goes to mice or touchpads (and so needs `xinput`).
fn is_pointer_change(change: &Change) -> bool {
    !matches!(change, Change::Layout { .. } | Change::Repeat { .. })
}

/// The signature after a look at the devices: the new listing, or the
/// previous signature when the listing failed (a failure is not a change).
fn next_signature(previous: &str, listing: Result<String, InputError>) -> String {
    match listing {
        Ok(list) => parse_slave_devices(&list).iter().map(|d| format!("{}:{}", d.id, d.name)).collect::<Vec<_>>().join("\n"),
        Err(_) => previous.to_string(),
    }
}

/// `run` for a literal command line.
fn run_str(argv: &[&str]) -> Result<String, InputError> {
    run(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// What one look at the system found.
#[derive(Default)]
struct Probe {
    devices: Vec<PointerDevice>,
    pointer_error: Option<InputError>,
    keyboard: Option<KeyboardSettings>,
    keyboard_error: Option<String>,
}

/// Read the pointers and the keyboard.
fn probe() -> Probe {
    let mut probe = Probe::default();
    match run_str(&["xinput", "list"]) {
        Ok(list) => {
            for raw in parse_pointer_list(&list) {
                let Ok(text) = run_str(&["xinput", "list-props", &raw.id.to_string()]) else { continue };
                let props = parse_props(&text);
                let Some(kind) = device_kind(&props) else { continue };
                probe.devices.push(PointerDevice { id: raw.id, name: raw.name, kind, props });
            }
        }
        Err(e) => probe.pointer_error = Some(e),
    }
    let query = run_str(&["setxkbmap", "-query"]);
    let xset = run_str(&["xset", "q"]);
    match (query, xset) {
        (Ok(q), Ok(x)) => match keyboard_from_outputs(&q, &x) {
            Some(k) => probe.keyboard = Some(k),
            None => probe.keyboard_error = Some("cannot read the keyboard layout".to_string()),
        },
        (Err(e), _) | (_, Err(e)) => probe.keyboard_error = Some(e.to_string()),
    }
    probe
}

/// The real backend: runs the X command line tools.
pub struct XBackend {
    probe: RefCell<Probe>,
    last_signature: RefCell<String>,
}

impl XBackend {
    /// Look at the system once and keep what was found.
    pub fn new() -> Self {
        Self { probe: RefCell::new(probe()), last_signature: RefCell::new(String::new()) }
    }

    /// The group status for devices of `kind`.
    fn pointer_status(&self, kind: PointerKind, none_found: &str) -> GroupStatus {
        let probe = self.probe.borrow();
        if let Some(e) = &probe.pointer_error { return GroupStatus::unavailable(e.to_string()); }
        if probe.devices.iter().any(|d| d.kind == kind) { return GroupStatus::ok(); }
        GroupStatus::unavailable(none_found)
    }
}

impl Default for XBackend {
    /// Same as `new`.
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for XBackend {
    /// Mouse and touchpad need a libinput device, the keyboard needs both tools.
    fn status(&self) -> Status {
        let keyboard = match &self.probe.borrow().keyboard_error {
            Some(e) => GroupStatus::unavailable(e.clone()),
            None => GroupStatus::ok(),
        };
        Status {
            mouse: self.pointer_status(PointerKind::Mouse, "No mouse with libinput settings was found."),
            touchpad: self.pointer_status(PointerKind::Touchpad, "No touchpad was found."),
            keyboard,
        }
    }

    /// The first mouse, the first touchpad and the keyboard as the system has them.
    fn read_current(&self) -> InputSettings {
        let probe = self.probe.borrow();
        let first = |kind: PointerKind| probe.devices.iter().find(|d| d.kind == kind);
        InputSettings {
            mouse: first(PointerKind::Mouse).map(|d| parse::mouse_from_props(&d.props)).unwrap_or_default(),
            touchpad: first(PointerKind::Touchpad).map(|d| parse::touchpad_from_props(&d.props)).unwrap_or_default(),
            keyboard: probe.keyboard.clone().unwrap_or_default(),
        }
    }

    /// Run the commands of `change`; the first failure is returned. A pointer
    /// change fails when `xinput` could not be used at all (no device is then
    /// known), so the hub can log it; no mouse or touchpad is not an error.
    fn apply(&self, change: &Change) -> Result<(), InputError> {
        if is_pointer_change(change) {
            if let Some(e) = &self.probe.borrow().pointer_error { return Err(e.clone()); }
        }
        let commands = commands_for(change, &self.probe.borrow().devices);
        for argv in commands {
            run(&argv)?;
        }
        Ok(())
    }

    /// The pointer and keyboard ids and names, one per line (the last good value when listing fails).
    fn signature(&self) -> String {
        let next = next_signature(&self.last_signature.borrow(), run_str(&["xinput", "list"]));
        *self.last_signature.borrow_mut() = next.clone();
        next
    }

    /// Probe the system again.
    fn refresh(&self) {
        *self.probe.borrow_mut() = probe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_display_or_an_unknown_wayland_compositor_is_a_problem() {
        use opusdm_ipc::session::WindowSystem;
        assert!(session_problem_from(WindowSystem::None, false).is_some());
        assert!(session_problem_from(WindowSystem::Wayland, false).is_some());
        assert_eq!(session_problem_from(WindowSystem::Wayland, true), None);
        assert_eq!(session_problem_from(WindowSystem::X11, false), None);
    }

    #[test]
    fn a_missing_tool_is_reported_with_its_package() {
        let err = run_str(&["opusdm-no-such-tool"]).unwrap_err();
        assert!(matches!(err, InputError::MissingTool { .. }));
    }

    #[test]
    fn a_failing_command_reports_its_stderr() {
        let err = run_str(&["sh", "-c", "echo boom >&2; exit 3"]).unwrap_err();
        match err {
            InputError::CommandFailed { stderr, .. } => assert_eq!(stderr, "boom"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_child_reaped_by_the_hub_still_gives_its_output_and_status() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::Stdio;
        // Simulate the hub's SIGCHLD reaper taking the child before we wait for it.
        let child = Command::new("sh").args(["-c", "echo hi; exit 3"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut raw = 0;
        // SAFETY: waitpid on our own child with a valid out pointer.
        assert_eq!(unsafe { libc::waitpid(pid as i32, &mut raw, 0) }, pid as i32);
        opusdm_ipc::reaped::record(pid, raw);
        let (status, stdout, _stderr) = collect(child).unwrap();
        assert_eq!(status.code(), Some(3));
        assert_eq!(stdout.trim(), "hi");
        assert_eq!(status.into_raw(), raw);
    }

    #[test]
    fn a_failed_listing_keeps_the_last_good_signature() {
        let listing = "\u{239c}   \u{21b3} mouse\t\tid=9\t[slave  pointer  (2)]\n";
        assert_eq!(next_signature("old", Ok(listing.to_string())), "9:mouse");
        let err = InputError::CommandFailed { command: "xinput list".into(), stderr: "No child processes".into() };
        assert_eq!(next_signature("9:mouse", Err(err)), "9:mouse");
    }

    #[test]
    fn a_missing_xinput_is_an_error_for_pointer_changes_but_no_mouse_is_not() {
        let missing = InputError::MissingTool { tool: "xinput".into(), package: "xinput".into() };
        let broken = XBackend { probe: RefCell::new(Probe { pointer_error: Some(missing.clone()), ..Probe::default() }), last_signature: RefCell::new(String::new()) };
        assert_eq!(broken.apply(&Change::MouseSpeed(0.5)), Err(missing));
        let no_devices = XBackend { probe: RefCell::new(Probe::default()), last_signature: RefCell::new(String::new()) };
        assert_eq!(no_devices.apply(&Change::TouchpadTap(true)), Ok(()));
    }

    #[test]
    fn a_keyboard_plugged_in_changes_the_signature() {
        let pointer = "\u{239c}   \u{21b3} mouse\t\tid=9\t[slave  pointer  (2)]\n";
        let keyboard = "    \u{21b3} USB Keyboard\t\tid=11\t[slave  keyboard (3)]\n";
        let without = next_signature("", Ok(pointer.to_string()));
        let with = next_signature("", Ok(format!("{pointer}{keyboard}")));
        assert_ne!(without, with);
        assert!(with.contains("11:USB Keyboard"));
    }
}
