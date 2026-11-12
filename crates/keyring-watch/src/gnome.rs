//! The gnome-keyring specific part: telling that the Secret Service is
//! gnome-keyring, unlocking it with the login password and restarting it.
//! The password goes to `gnome-keyring-daemon --unlock` on its standard
//! input only: never in an argument, an environment variable or a log.

use crate::probe::{SecretServiceVault, Vault};
use std::io::Write;
use std::process::{Command, Stdio};
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::Connection;
use zbus::names::BusName;
use zeroize::Zeroizing;

/// Tag used in log lines.
const TAG: &str = "opusdm-keyring-watch";
/// The daemon binary.
const DAEMON: &str = "gnome-keyring-daemon";
/// The systemd user unit of the daemon.
const UNIT: &str = "gnome-keyring-daemon.service";
/// The well known name of the Secret Service.
const SERVICE_NAME: &str = "org.freedesktop.secrets";
/// Test only: the environment variable that replaces the daemon binary.
#[cfg(test)]
const DAEMON_ENV: &str = "OPUSDM_KEYRING_DAEMON";
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
    let conn = Connection::session().map_err(|e| e.to_string())?;
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
    steps.push(Step::Systemctl(&["restart", UNIT]));
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
fn unlock_and_verify(password: Zeroizing<String>, vault: &dyn Vault) -> Result<(), String> {
    _run_unlock(password)?;
    match vault.default_locked()? {
        Some(true) => Err(WRONG_PASSWORD.to_string()),
        Some(false) | None => Ok(()),
    }
}

/// Run the daemon with `--unlock`, write the password and a newline to its
/// standard input, close it and wait. The error is the text the daemon wrote
/// to its standard error.
fn _run_unlock(password: Zeroizing<String>) -> Result<(), String> {
    let mut child = Command::new(daemon_bin())
        .args(unlock_args())
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
    unlock_with_password(password)
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
        assert_eq!(steps[3], Step::Systemctl(&["restart", "gnome-keyring-daemon.service"]));
    }

    #[test]
    fn unlock_with_a_fake_daemon() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-daemon");
        let stdin_file = dir.path().join("stdin");
        let args_file = dir.path().join("args");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" > '{}'\ncat > '{}'\n", args_file.display(), stdin_file.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(DAEMON_ENV, &script);
        let result = _run_unlock(Zeroizing::new("s3cret".to_string()));
        std::env::remove_var(DAEMON_ENV);
        assert_eq!(result, Ok(()));
        assert_eq!(std::fs::read_to_string(stdin_file).unwrap(), "s3cret\n");
        assert_eq!(std::fs::read_to_string(args_file).unwrap().trim(), "--unlock");
    }

    /// A vault that only knows whether the default collection is locked.
    struct LockVault(Result<Option<bool>, String>);

    impl Vault for LockVault {
        fn bus_ok(&self) -> bool { true }
        fn service_present(&self) -> Result<bool, String> { Ok(true) }
        fn default_locked(&self) -> Result<Option<bool>, String> { self.0.clone() }
        fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { Ok(None) }
    }

    /// Run the unlock against a daemon script that exits 0 and prints nothing.
    fn unlock_with_quiet_daemon(vault: &LockVault) -> Result<(), String> {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("quiet-daemon");
        std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(DAEMON_ENV, &script);
        let result = unlock_and_verify(Zeroizing::new("pw".to_string()), vault);
        std::env::remove_var(DAEMON_ENV);
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
}
