//! The gnome-keyring specific part: telling that the Secret Service is
//! gnome-keyring, unlocking it with the login password and restarting it.
//! The password goes to `gnome-keyring-daemon --unlock` on its standard
//! input only: never in an argument, an environment variable or a log.

use crate::probe::{SecretServiceVault, Vault};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;
use zeroize::Zeroizing;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";
/// The daemon binary.
const DAEMON: &str = "gnome-keyring-daemon";
/// The systemd user unit of the daemon.
const UNIT: &str = "gnome-keyring-daemon.service";
/// The systemd user socket unit that owns the control socket of the daemon.
/// It is restarted with the service: restarting only the service hands it the
/// same old socket, and when that socket lost its path on disk the control
/// folder would never come back.
const SOCKET_UNIT: &str = "gnome-keyring-daemon.socket";
/// The well known name of the Secret Service.
const SERVICE_NAME: &str = "org.freedesktop.secrets";
/// Test only: the environment variable that replaces the daemon binary.
#[cfg(test)]
const DAEMON_ENV: &str = "OPUSDM_KEYRING_DAEMON";
/// The environment variable the daemon reads to find the one that runs.
const CONTROL_VAR: &str = "GNOME_KEYRING_CONTROL";
/// The name of the control socket inside the control folder.
const CONTROL_SOCKET: &str = "control";
/// The control folder inside the runtime folder of the user.
const CONTROL_SUBDIR: &str = "keyring";
/// Test only: the environment variable that replaces the control folder.
#[cfg(test)]
const CONTROL_ENV: &str = "OPUSDM_KEYRING_CONTROL";
/// The other wallets that compete for the Secret Service name, stopped
/// before a restart like `opusdm-session` does.
const OTHER_WALLETS: [&str; 3] = ["ksecretd", "kwalletd5", "kwalletd6"];

/// The error text for a password the keyring did not accept. The password
/// window shows it and stays open.
pub const WRONG_PASSWORD: &str = "Wrong password";

/// One step of the restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Stop the processes with this exact name (`pkill -x`).
    Kill(&'static str),
    /// Run `systemctl --user` with these arguments.
    Systemctl(&'static [&'static str]),
    /// Start a daemon that replaces the running one (no systemd unit).
    Replace,
}

/// Whether the process name `comm` (from `/proc/<pid>/comm`, which cuts it at
/// 15 characters) is the gnome-keyring daemon.
pub fn is_gnome_comm(comm: &str) -> bool {
    matches!(comm.trim(), "gnome-keyring-d" | "gnome-keyring-daemon")
}

/// Whether the owner of `org.freedesktop.secrets` is gnome-keyring. A bus
/// that cannot be asked is "no", logged.
pub fn owner_is_gnome() -> bool {
    match _owner_comm() {
        Ok(comm) => is_gnome_comm(&comm),
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot tell who owns {SERVICE_NAME}: {e}");
            false
        }
    }
}

/// The process name of the owner of the Secret Service name.
fn _owner_comm() -> Result<String, String> {
    let conn = crate::probe::session_bus(crate::probe::PROBE_TIMEOUT).map_err(|e| e.to_string())?;
    let bus = DBusProxy::new(&conn).map_err(|e| e.to_string())?;
    let name = BusName::try_from(SERVICE_NAME).map_err(|e| e.to_string())?;
    let pid = bus.get_connection_unix_process_id(name).map_err(|e| e.to_string())?;
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map_err(|e| format!("/proc/{pid}/comm: {e}"))
}

/// The arguments of the unlock call: exactly `--unlock`, the password is
/// read from standard input.
pub fn unlock_args() -> Vec<&'static str> {
    vec!["--unlock"]
}

/// The restart, in order: the other wallets are stopped, then the unit is
/// restarted; [`Step::Replace`] only runs when the unit does not exist.
pub fn restart_steps() -> Vec<Step> {
    let mut steps: Vec<Step> = OTHER_WALLETS.iter().map(|n| Step::Kill(n)).collect();
    steps.push(Step::Systemctl(&["restart", SOCKET_UNIT, UNIT]));
    steps.push(Step::Replace);
    steps
}

/// The daemon binary; tests may replace it through the environment.
fn daemon_bin() -> String {
    #[cfg(test)]
    if let Ok(path) = std::env::var(DAEMON_ENV) { return path; }
    DAEMON.to_string()
}

/// Unlock the login keyring with `password` and check that it worked.
/// `gnome-keyring-daemon --unlock` exits 0 and prints nothing for a wrong
/// password (checked on this machine), so the answer is the lock state of
/// the default collection afterwards: still locked is [`WRONG_PASSWORD`].
pub fn unlock_with_password(password: Zeroizing<String>) -> Result<(), String> {
    unlock_and_verify(password, &SecretServiceVault::new())
}

/// [`unlock_with_password`] with the vault that answers the lock state given.
/// The daemon unlocks the `login` keyring, so that is the one looked at (the
/// default one when there is no `login`); no keyring at all is an error.
fn unlock_and_verify(password: Zeroizing<String>, vault: &dyn Vault) -> Result<(), String> {
    _run_unlock(password)?;
    let locked = match vault.login_locked()? {
        Some(locked) => locked,
        None => vault.default_locked()?.ok_or("the unlock ended but no keyring was found")?,
    };
    if locked { Err(WRONG_PASSWORD.to_string()) } else { Ok(()) }
}

/// The control folder of the daemon of this session.
fn control_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Ok(dir) = std::env::var(CONTROL_ENV) { return Some(PathBuf::from(dir)); }
    std::env::var_os("XDG_RUNTIME_DIR").map(|runtime| PathBuf::from(runtime).join(CONTROL_SUBDIR))
}

/// The control folder of the running daemon, only when its socket answers.
/// Without it `gnome-keyring-daemon --unlock` does not unlock the running
/// daemon: it starts a second one and leaves it behind, so it is never run
/// in that case.
fn live_control_dir() -> Result<PathBuf, String> {
    let dir = control_dir().ok_or("XDG_RUNTIME_DIR is not set, cannot find the keyring control socket")?;
    let socket = dir.join(CONTROL_SOCKET);
    UnixStream::connect(&socket).map_err(|e| {
        format!("Cannot reach the keyring control socket {}: {e}. Unlocking with a password needs it (use Restart gnome-keyring, or Unlock).", socket.display())
    })?;
    Ok(dir)
}

/// Run the daemon with `--unlock`, write the password and a newline to its
/// standard input, close it and wait. The error is the text the daemon wrote
/// to its standard error.
fn _run_unlock(password: Zeroizing<String>) -> Result<(), String> {
    let control = live_control_dir()?;
    let mut child = Command::new(daemon_bin())
        .args(unlock_args())
        .env(CONTROL_VAR, control)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {DAEMON}: {e}"))?;
    let mut line = Zeroizing::new(Vec::with_capacity(password.len() + 1));
    line.extend_from_slice(password.as_bytes());
    line.push(b'\n');
    let Some(mut stdin) = child.stdin.take() else { return Err(format!("no standard input for {DAEMON}")) };
    let written = stdin.write_all(&line);
    // Closing the pipe tells the daemon the password is complete.
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| format!("waiting for {DAEMON}: {e}"))?;
    if output.status.success() { return Ok(()); }
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !text.is_empty() { return Err(text); }
    match written {
        Err(e) => Err(format!("{DAEMON} stopped before it read the password: {e}")),
        Ok(()) => Err(format!("{DAEMON} failed: {}", output.status)),
    }
}

/// Restart gnome-keyring and unlock it again with `password` (a restarted
/// daemon comes back locked). Apps lose their open Secret Service session.
pub fn restart(password: Zeroizing<String>) -> Result<(), String> {
    let unit_exists = Command::new("systemctl").args(["--user", "cat", UNIT]).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    for step in restart_steps() {
        match step {
            Step::Kill(name) => _kill(name),
            Step::Systemctl(args) if unit_exists => _systemctl(args)?,
            Step::Replace if !unit_exists => _replace()?,
            Step::Systemctl(_) | Step::Replace => {}
        }
    }
    // The new daemon needs a moment to own its name: unlocking before that
    // may start a second daemon.
    wait_for_service(&SecretServiceVault::new(), SERVICE_WAIT_TRIES, std::thread::sleep)?;
    unlock_with_password(password)
}

/// How many times the restarted service is asked before giving up.
const SERVICE_WAIT_TRIES: u32 = 20;
/// The pause between two of those questions.
const SERVICE_WAIT_PAUSE: std::time::Duration = std::time::Duration::from_millis(500);

/// Wait until the Secret Service answers (any lock state), asking up to
/// `tries` times with a pause in between (`sleep` is given the pause).
fn wait_for_service(vault: &dyn Vault, tries: u32, mut sleep: impl FnMut(std::time::Duration)) -> Result<(), String> {
    let mut last = String::from("no answer");
    for _ in 0..tries {
        match vault.default_locked() {
            Ok(_) => return Ok(()),
            Err(e) => last = e,
        }
        sleep(SERVICE_WAIT_PAUSE);
    }
    Err(format!("the keyring did not come back after the restart: {last}"))
}

/// Stop the processes called `name` of this user. "None found" is fine.
fn _kill(name: &str) {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    match Command::new("pkill").args(["-u", &uid.to_string(), "-x", name]).status() {
        Ok(s) if s.success() || s.code() == Some(1) => {}
        Ok(s) => opusdm_ipc::err_log!(TAG, "pkill {name} ended with {s}"),
        Err(e) => opusdm_ipc::err_log!(TAG, "cannot run pkill for {name}: {e}"),
    }
}

/// Run `systemctl --user` with `args`.
fn _systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl").arg("--user").args(args).status().map_err(|e| format!("cannot run systemctl: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("systemctl --user {} ended with {status}", args.join(" "))) }
}

/// Start a daemon that replaces the running one.
fn _replace() -> Result<(), String> {
    let status = Command::new(daemon_bin())
        .args(["--replace", "--daemonize", "--components=secrets"])
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {DAEMON}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{DAEMON} --replace ended with {status}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;

    /// The tests that replace the daemon through the environment run one at a time.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn comm_names_are_recognised() {
        assert!(is_gnome_comm("gnome-keyring-d"));
        assert!(is_gnome_comm("gnome-keyring-daemon\n"));
        assert!(!is_gnome_comm("ksecretd"));
        assert!(!is_gnome_comm("kwalletd6"));
    }

    #[test]
    fn unlock_args_never_contain_a_password() {
        assert_eq!(unlock_args(), vec!["--unlock"]);
    }

    #[test]
    fn restart_stops_other_wallets_first() {
        let steps = restart_steps();
        assert_eq!(&steps[..3], &[Step::Kill("ksecretd"), Step::Kill("kwalletd5"), Step::Kill("kwalletd6")]);
        assert_eq!(steps[3], Step::Systemctl(&["restart", "gnome-keyring-daemon.socket", "gnome-keyring-daemon.service"]), "the socket unit is restarted too, it owns the control socket");
    }

    #[test]
    fn unlock_with_a_fake_daemon() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-daemon");
        let stdin_file = dir.path().join("stdin");
        let args_file = dir.path().join("args");
        let env_file = dir.path().join("env");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" > '{}'\necho \"$GNOME_KEYRING_CONTROL\" > '{}'\ncat > '{}'\n", args_file.display(), env_file.display(), stdin_file.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _listener = UnixListener::bind(dir.path().join(CONTROL_SOCKET)).unwrap();
        std::env::set_var(DAEMON_ENV, &script);
        std::env::set_var(CONTROL_ENV, dir.path());
        let result = _run_unlock(Zeroizing::new("s3cret".to_string()));
        std::env::remove_var(DAEMON_ENV);
        std::env::remove_var(CONTROL_ENV);
        assert_eq!(result, Ok(()));
        assert_eq!(std::fs::read_to_string(env_file).unwrap().trim(), dir.path().to_str().unwrap(), "the daemon is told where the running one is");
        assert_eq!(std::fs::read_to_string(stdin_file).unwrap(), "s3cret\n");
        assert_eq!(std::fs::read_to_string(args_file).unwrap().trim(), "--unlock");
    }

    /// A vault that only knows whether the default collection is locked.
    struct LockVault(Result<Option<bool>, String>);

    /// A vault with separate answers for the default and the `login` collection.
    struct TwoVault {
        default: Option<bool>,
        login: Option<bool>,
    }

    impl Vault for TwoVault {
        fn bus_ok(&self) -> bool { true }
        fn service_present(&self) -> Result<bool, String> { Ok(true) }
        fn default_locked(&self) -> Result<Option<bool>, String> { Ok(self.default) }
        fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { Ok(self.login) }
        fn login_locked(&self) -> Result<Option<bool>, String> { Ok(self.login) }
    }

    impl Vault for LockVault {
        fn bus_ok(&self) -> bool { true }
        fn service_present(&self) -> Result<bool, String> { Ok(true) }
        fn default_locked(&self) -> Result<Option<bool>, String> { self.0.clone() }
        fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { Ok(None) }
        fn login_locked(&self) -> Result<Option<bool>, String> { self.0.clone() }
    }

    /// Run the unlock against a daemon script that exits 0 and prints nothing.
    fn unlock_with_quiet_daemon(vault: &dyn Vault) -> Result<(), String> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("quiet-daemon");
        std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _listener = UnixListener::bind(dir.path().join(CONTROL_SOCKET)).unwrap();
        std::env::set_var(DAEMON_ENV, &script);
        std::env::set_var(CONTROL_ENV, dir.path());
        let result = unlock_and_verify(Zeroizing::new("pw".to_string()), vault);
        std::env::remove_var(DAEMON_ENV);
        std::env::remove_var(CONTROL_ENV);
        result
    }

    #[test]
    fn exit_zero_with_a_still_locked_keyring_is_a_wrong_password() {
        // Found by hand: a wrong password makes the daemon exit 0, silently.
        assert_eq!(unlock_with_quiet_daemon(&LockVault(Ok(Some(true)))), Err(WRONG_PASSWORD.to_string()));
    }

    #[test]
    fn unlocked_afterwards_is_success() {
        assert_eq!(unlock_with_quiet_daemon(&LockVault(Ok(Some(false)))), Ok(()));
    }

    #[test]
    fn vault_error_after_unlock_is_reported() {
        assert_eq!(unlock_with_quiet_daemon(&LockVault(Err("bus gone".into()))), Err("bus gone".to_string()));
    }

    #[test]
    fn the_login_collection_decides_not_the_default_one() {
        // default is another, still locked collection; login is unlocked now.
        assert_eq!(unlock_with_quiet_daemon(&TwoVault { default: Some(true), login: Some(false) }), Ok(()));
        assert_eq!(unlock_with_quiet_daemon(&TwoVault { default: Some(false), login: Some(true) }), Err(WRONG_PASSWORD.to_string()));
    }

    #[test]
    fn without_a_login_collection_the_default_decides() {
        assert_eq!(unlock_with_quiet_daemon(&TwoVault { default: Some(true), login: None }), Err(WRONG_PASSWORD.to_string()));
        assert_eq!(unlock_with_quiet_daemon(&TwoVault { default: Some(false), login: None }), Ok(()));
    }

    #[test]
    fn no_keyring_at_all_is_an_error_not_a_success() {
        let err = unlock_with_quiet_daemon(&TwoVault { default: None, login: None }).unwrap_err();
        assert!(err.contains("no keyring"), "{err}");
    }

    #[test]
    fn waiting_for_the_service_retries_then_succeeds() {
        struct Flaky(std::cell::Cell<u32>);
        impl Vault for Flaky {
            fn bus_ok(&self) -> bool { true }
            fn service_present(&self) -> Result<bool, String> { Ok(true) }
            fn default_locked(&self) -> Result<Option<bool>, String> {
                self.0.set(self.0.get() + 1);
                if self.0.get() < 3 { Err("no such object".into()) } else { Ok(Some(true)) }
            }
            fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { Ok(None) }
            fn login_locked(&self) -> Result<Option<bool>, String> { Ok(None) }
        }
        let mut pauses = 0;
        assert_eq!(wait_for_service(&Flaky(Default::default()), 5, |_| pauses += 1), Ok(()));
        assert_eq!(pauses, 2);
        let err = wait_for_service(&Flaky(Default::default()), 2, |_| {}).unwrap_err();
        assert!(err.contains("did not come back") && err.contains("no such object"), "{err}");
    }

    /// Run the unlock with a control folder that does not answer and a daemon
    /// script that leaves a marker file when it runs. Returns the result and
    /// whether the daemon ran.
    fn unlock_without_a_live_socket(make_stale_socket: bool) -> (Result<(), String>, bool) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let script = dir.path().join("daemon");
        std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\ncat > /dev/null\n", marker.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A stale socket: the file is there, nobody listens.
        if make_stale_socket { drop(UnixListener::bind(dir.path().join(CONTROL_SOCKET)).unwrap()); }
        std::env::set_var(DAEMON_ENV, &script);
        std::env::set_var(CONTROL_ENV, dir.path());
        let result = _run_unlock(Zeroizing::new("pw".to_string()));
        std::env::remove_var(DAEMON_ENV);
        std::env::remove_var(CONTROL_ENV);
        (result, marker.exists())
    }

    #[test]
    fn no_control_socket_never_starts_a_daemon() {
        let (result, ran) = unlock_without_a_live_socket(false);
        let err = result.unwrap_err();
        assert!(err.contains("control socket"), "{err}");
        assert!(!ran, "the daemon must not run: it would start a second one");
    }

    #[test]
    fn a_stale_control_socket_never_starts_a_daemon() {
        let (result, ran) = unlock_without_a_live_socket(true);
        assert!(result.unwrap_err().contains("control socket"));
        assert!(!ran, "the daemon must not run: it would start a second one");
    }
}
