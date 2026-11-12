//! The gnome-keyring specific part: telling that the Secret Service is
//! gnome-keyring, unlocking it with the login password and restarting it.
//! The password goes to the running daemon through its control socket only
//! (the protocol `pam_gnome_keyring` uses): never in an argument, an
//! environment variable or a log, and no process is ever started to unlock.
//! `gnome-keyring-daemon --unlock` is not used: it starts a daemon of its own
//! and leaves it running.

use crate::probe::{SecretServiceVault, Vault};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
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
/// The name of the control socket inside the control folder.
const CONTROL_SOCKET: &str = "control";
/// The control folder inside the runtime folder of the user.
const CONTROL_SUBDIR: &str = "keyring";
/// The control operation that unlocks the login keyring (`GKD_CONTROL_OP_UNLOCK`).
const CONTROL_OP_UNLOCK: u32 = 1;
/// The password was accepted (`GKD_CONTROL_RESULT_OK`).
const CONTROL_RESULT_OK: u32 = 0;
/// The password was refused (`GKD_CONTROL_RESULT_DENIED`).
const CONTROL_RESULT_DENIED: u32 = 1;
/// The size of a reply: its own length field and the result code.
const CONTROL_REPLY_LEN: u32 = 8;
/// The size of a request before its arguments: the length field and the operation.
const CONTROL_HEADER_LEN: usize = 8;
/// How long the daemon may take to answer on the control socket.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
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

/// The restart, in order: the other wallets are stopped, then the unit is
/// restarted; [`Step::Replace`] only runs when the unit does not exist.
pub fn restart_steps() -> Vec<Step> {
    let mut steps: Vec<Step> = OTHER_WALLETS.iter().map(|n| Step::Kill(n)).collect();
    steps.push(Step::Systemctl(&["restart", SOCKET_UNIT, UNIT]));
    steps.push(Step::Replace);
    steps
}

/// Unlock the login keyring of the running daemon with `password` and check
/// that it worked. The daemon answers on its control socket whether the
/// password was accepted; the lock state is read afterwards as well.
pub fn unlock_with_password(password: Zeroizing<String>) -> Result<(), String> {
    let socket = control_socket()?;
    unlock_and_verify(password, &socket, &SecretServiceVault::new())
}

/// [`unlock_with_password`] with the control socket and the vault that
/// answers the lock state given. The daemon unlocks the `login` keyring, so
/// that is the one looked at (the default one when there is no `login`); no
/// keyring at all is an error.
fn unlock_and_verify(password: Zeroizing<String>, socket: &Path, vault: &dyn Vault) -> Result<(), String> {
    control_unlock(socket, &password)?;
    let locked = match vault.login_locked()? {
        Some(locked) => locked,
        None => vault.default_locked()?.ok_or("the unlock ended but no keyring was found")?,
    };
    if locked { Err(WRONG_PASSWORD.to_string()) } else { Ok(()) }
}

/// The control socket of the daemon of this session.
fn control_socket() -> Result<PathBuf, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set, cannot find the keyring control socket")?;
    Ok(PathBuf::from(runtime).join(CONTROL_SUBDIR).join(CONTROL_SOCKET))
}

/// The request that unlocks the login keyring: the length of the whole
/// packet, the operation and the password as a length prefixed string, all
/// numbers big endian. Kept in zeroed memory.
fn unlock_packet(password: &str) -> Zeroizing<Vec<u8>> {
    let total = (CONTROL_HEADER_LEN + 4 + password.len()) as u32;
    let mut packet = Zeroizing::new(Vec::with_capacity(total as usize));
    packet.extend_from_slice(&total.to_be_bytes());
    packet.extend_from_slice(&CONTROL_OP_UNLOCK.to_be_bytes());
    packet.extend_from_slice(&(password.len() as u32).to_be_bytes());
    packet.extend_from_slice(password.as_bytes());
    packet
}

/// Ask the running daemon, on its control socket, to unlock the login
/// keyring: a credentials byte, then the request of [`unlock_packet`]; the
/// reply is its length (8) and a result code. A refused password is
/// [`WRONG_PASSWORD`]. Nothing is started if the socket does not answer.
fn control_unlock(socket: &Path, password: &str) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        format!("Cannot reach the keyring control socket {}: {e}. Unlocking with a password needs it (use Restart gnome-keyring, or Unlock).", socket.display())
    })?;
    stream.set_read_timeout(Some(CONTROL_TIMEOUT)).map_err(|e| format!("keyring control socket: {e}"))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT)).map_err(|e| format!("keyring control socket: {e}"))?;
    // The daemon reads the credentials of the peer after this first byte.
    stream.write_all(&[0u8]).map_err(|e| format!("keyring control socket: {e}"))?;
    stream.write_all(&unlock_packet(password)).map_err(|e| format!("keyring control socket: {e}"))?;
    let mut reply = [0u8; 8];
    stream.read_exact(&mut reply).map_err(|e| format!("keyring control socket: no answer: {e}"))?;
    let length = u32::from_be_bytes([reply[0], reply[1], reply[2], reply[3]]);
    let result = u32::from_be_bytes([reply[4], reply[5], reply[6], reply[7]]);
    if length != CONTROL_REPLY_LEN { return Err(format!("keyring control socket: unexpected answer of {length} bytes")); }
    match result {
        CONTROL_RESULT_OK => Ok(()),
        CONTROL_RESULT_DENIED => Err(WRONG_PASSWORD.to_string()),
        other => Err(format!("the keyring refused the request (code {other})")),
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
    let status = Command::new(DAEMON)
        .args(["--replace", "--daemonize", "--components=secrets"])
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {DAEMON}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{DAEMON} --replace ended with {status}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread::JoinHandle;

    #[test]
    fn comm_names_are_recognised() {
        assert!(is_gnome_comm("gnome-keyring-d"));
        assert!(is_gnome_comm("gnome-keyring-daemon\n"));
        assert!(!is_gnome_comm("ksecretd"));
        assert!(!is_gnome_comm("kwalletd6"));
    }

    #[test]
    fn restart_stops_other_wallets_first() {
        let steps = restart_steps();
        assert_eq!(&steps[..3], &[Step::Kill("ksecretd"), Step::Kill("kwalletd5"), Step::Kill("kwalletd6")]);
        assert_eq!(steps[3], Step::Systemctl(&["restart", "gnome-keyring-daemon.socket", "gnome-keyring-daemon.service"]), "the socket unit is restarted too, it owns the control socket");
    }

    #[test]
    fn the_unlock_packet_is_the_daemon_protocol() {
        // length 18 (4 + 4 + 4 + 6), operation 1 (unlock), string length 6, the password.
        let mut expected = Vec::new();
        expected.extend_from_slice(&18u32.to_be_bytes());
        expected.extend_from_slice(&1u32.to_be_bytes());
        expected.extend_from_slice(&6u32.to_be_bytes());
        expected.extend_from_slice(b"s3cret");
        assert_eq!(*unlock_packet("s3cret"), expected);
    }

    /// A reply of the daemon: its length (8) and a result code.
    fn reply(result: u32) -> Vec<u8> {
        let mut bytes = CONTROL_REPLY_LEN.to_be_bytes().to_vec();
        bytes.extend_from_slice(&result.to_be_bytes());
        bytes
    }

    /// A fake control server in `dir`: takes one connection, records the
    /// credentials byte and the request, answers with `answer`. Returns what
    /// it received.
    fn fake_server(dir: &Path, answer: Vec<u8>) -> JoinHandle<Vec<u8>> {
        let listener = UnixListener::bind(dir.join(CONTROL_SOCKET)).unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut received = vec![0u8; 1];
            conn.read_exact(&mut received).unwrap();
            let mut length = [0u8; 4];
            conn.read_exact(&mut length).unwrap();
            let mut rest = vec![0u8; u32::from_be_bytes(length) as usize - 4];
            conn.read_exact(&mut rest).unwrap();
            received.extend_from_slice(&length);
            received.extend_from_slice(&rest);
            conn.write_all(&answer).unwrap();
            received
        })
    }

    #[test]
    fn the_daemon_gets_the_credentials_byte_then_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_server(dir.path(), reply(CONTROL_RESULT_OK));
        assert_eq!(control_unlock(&dir.path().join(CONTROL_SOCKET), "s3cret"), Ok(()));
        let mut expected = vec![0u8];
        expected.extend_from_slice(&unlock_packet("s3cret"));
        assert_eq!(server.join().unwrap(), expected);
    }

    #[test]
    fn a_denied_reply_is_a_wrong_password() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_server(dir.path(), reply(CONTROL_RESULT_DENIED));
        assert_eq!(control_unlock(&dir.path().join(CONTROL_SOCKET), "x"), Err(WRONG_PASSWORD.to_string()));
        server.join().unwrap();
    }

    #[test]
    fn another_failure_is_not_reported_as_a_wrong_password() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_server(dir.path(), reply(2));
        let err = control_unlock(&dir.path().join(CONTROL_SOCKET), "x").unwrap_err();
        assert!(err.contains("refused the request") && !err.contains("Wrong"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn a_reply_of_the_wrong_size_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = 12u32.to_be_bytes().to_vec();
        bad.extend_from_slice(&0u32.to_be_bytes());
        let server = fake_server(dir.path(), bad);
        let err = control_unlock(&dir.path().join(CONTROL_SOCKET), "x").unwrap_err();
        assert!(err.contains("unexpected answer"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn no_control_socket_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = control_unlock(&dir.path().join(CONTROL_SOCKET), "x").unwrap_err();
        assert!(err.contains("control socket"), "{err}");
    }

    #[test]
    fn a_stale_control_socket_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // The file is there, nobody listens.
        drop(UnixListener::bind(dir.path().join(CONTROL_SOCKET)).unwrap());
        let err = control_unlock(&dir.path().join(CONTROL_SOCKET), "x").unwrap_err();
        assert!(err.contains("control socket"), "{err}");
    }

    /// A vault that only knows the lock state of the collections.
    struct LockVault {
        default: Result<Option<bool>, String>,
        login: Result<Option<bool>, String>,
    }

    impl Vault for LockVault {
        fn bus_ok(&self) -> bool { true }
        fn service_present(&self) -> Result<bool, String> { Ok(true) }
        fn default_locked(&self) -> Result<Option<bool>, String> { self.default.clone() }
        fn login_locked_if_distinct(&self) -> Result<Option<bool>, String> { Ok(None) }
        fn login_locked(&self) -> Result<Option<bool>, String> { self.login.clone() }
    }

    /// Unlock through a fake daemon that accepts the password, then look at `vault`.
    fn unlock_accepted(vault: &LockVault) -> Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_server(dir.path(), reply(CONTROL_RESULT_OK));
        let result = unlock_and_verify(Zeroizing::new("pw".to_string()), &dir.path().join(CONTROL_SOCKET), vault);
        server.join().unwrap();
        result
    }

    fn lock(default: Option<bool>, login: Option<bool>) -> LockVault {
        LockVault { default: Ok(default), login: Ok(login) }
    }

    #[test]
    fn accepted_and_unlocked_is_success() {
        assert_eq!(unlock_accepted(&lock(Some(false), Some(false))), Ok(()));
    }

    #[test]
    fn accepted_but_still_locked_is_not_success() {
        assert_eq!(unlock_accepted(&lock(Some(true), Some(true))), Err(WRONG_PASSWORD.to_string()));
    }

    #[test]
    fn the_login_collection_decides_not_the_default_one() {
        assert_eq!(unlock_accepted(&lock(Some(true), Some(false))), Ok(()));
        assert_eq!(unlock_accepted(&lock(Some(false), Some(true))), Err(WRONG_PASSWORD.to_string()));
    }

    #[test]
    fn without_a_login_collection_the_default_decides() {
        assert_eq!(unlock_accepted(&lock(Some(true), None)), Err(WRONG_PASSWORD.to_string()));
        assert_eq!(unlock_accepted(&lock(Some(false), None)), Ok(()));
    }

    #[test]
    fn no_keyring_at_all_is_an_error_not_a_success() {
        let err = unlock_accepted(&lock(None, None)).unwrap_err();
        assert!(err.contains("no keyring"), "{err}");
    }

    #[test]
    fn vault_error_after_unlock_is_reported() {
        let vault = LockVault { default: Err("bus gone".into()), login: Err("bus gone".into()) };
        assert_eq!(unlock_accepted(&vault), Err("bus gone".to_string()));
    }

    #[test]
    fn a_denied_password_never_reads_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake_server(dir.path(), reply(CONTROL_RESULT_DENIED));
        let vault = LockVault { default: Err("must not be asked".into()), login: Err("must not be asked".into()) };
        let result = unlock_and_verify(Zeroizing::new("pw".to_string()), &dir.path().join(CONTROL_SOCKET), &vault);
        assert_eq!(result, Err(WRONG_PASSWORD.to_string()));
        server.join().unwrap();
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
}
