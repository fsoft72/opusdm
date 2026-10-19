//! The conversation with `polkit-agent-helper-1`, the program that checks
//! the password with PAM. polkit 126+ runs it as root through the systemd
//! socket `/run/polkit/agent-helper.socket` (the helper is no longer setuid
//! there): the agent connects and writes the user, then the cookie. Older
//! systems start the setuid helper as `helper USER` and write the cookie on
//! its stdin. Either way the helper then prints one line per PAM message and
//! reads one line per answer. On success the helper itself tells polkitd
//! that the request is authenticated.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Where polkit installs its helper.
pub const HELPER_PATH: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";

/// The socket systemd listens on to start the helper as root (polkit 126+).
pub const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";

/// One line of the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    /// PAM asks for a secret (the password): the typing stays hidden.
    PromptHidden(String),
    /// PAM asks for visible text.
    PromptVisible(String),
    /// A PAM error to show.
    Error(String),
    /// A PAM notice to show.
    Info(String),
    /// The user is authenticated.
    Success,
    /// The authentication failed (or the helper ended without saying).
    Failure,
}

/// Builds the event of a line from its text.
type MakeEvent = fn(String) -> HelperEvent;

/// The line prefixes of the helper and the event each one starts.
const PREFIXES: [(&str, MakeEvent); 4] = [
    ("PAM_PROMPT_ECHO_OFF ", HelperEvent::PromptHidden),
    ("PAM_PROMPT_ECHO_ON ", HelperEvent::PromptVisible),
    ("PAM_ERROR_MSG ", HelperEvent::Error),
    ("PAM_TEXT_INFO ", HelperEvent::Info),
];

/// The event of one helper `line` (newline already removed); `None` for a
/// line the protocol does not have.
pub fn parse_line(line: &str) -> Option<HelperEvent> {
    match line {
        "SUCCESS" => return Some(HelperEvent::Success),
        "FAILURE" => return Some(HelperEvent::Failure),
        _ => {}
    }
    PREFIXES
        .iter()
        .find_map(|(prefix, event)| line.strip_prefix(prefix).map(|text| event(unescape(text))))
}

/// Undo the helper's `g_strescape`, like `g_strcompress`: `\n`, `\t` and the
/// other C escapes, and the octal `\NNN` it writes for every non-ASCII byte.
pub fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        if next.is_ascii_digit() && next < b'8' {
            let digits = bytes[i + 1..].iter().take(3).take_while(|b| (b'0'..b'8').contains(b)).count();
            let value = bytes[i + 1..i + 1 + digits].iter().fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
            out.push(value as u8);
            i += 1 + digits;
            continue;
        }
        out.push(match next {
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            other => other,
        });
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How the agent talks to the helper.
enum Link {
    /// A setuid helper started by the agent.
    Child { child: Child, stdin: ChildStdin },
    /// A helper started by systemd on a connection to the socket.
    Socket(UnixStream),
}

/// A running helper. Dropping it ends the helper.
pub struct Session {
    link: Link,
}

impl Session {
    /// Talk to the helper through `socket` when it exists, like
    /// libpolkit-agent does, else start the setuid `helper`. A socket that
    /// exists but refuses the connection is logged and the setuid helper is
    /// tried instead.
    pub fn open(socket: &Path, helper: &Path, user: &str, cookie: &str) -> io::Result<(Session, async_channel::Receiver<HelperEvent>)> {
        if !socket.exists() { return Session::start(helper, user, cookie); }
        match Session::connect(socket, user, cookie) {
            Ok(opened) => Ok(opened),
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-polkit", "cannot use {}, starting {} instead: {e}", socket.display(), helper.display());
                Session::start(helper, user, cookie)
            }
        }
    }

    /// Connect to the helper `socket` and hand it `user` and `cookie`. Its
    /// lines arrive on the returned channel, as with `start`.
    pub fn connect(socket: &Path, user: &str, cookie: &str) -> io::Result<(Session, async_channel::Receiver<HelperEvent>)> {
        let mut stream = UnixStream::connect(socket)?;
        writeln!(stream, "{user}")?;
        writeln!(stream, "{cookie}")?;
        let reader = stream.try_clone()?;
        let rx = spawn_reader(reader)?;
        Ok((Session { link: Link::Socket(stream) }, rx))
    }

    /// Start `helper` for `user` and hand it `cookie`. Its lines arrive on
    /// the returned channel, read on a thread of their own; the last one is
    /// always `Success` or `Failure`.
    pub fn start(helper: &Path, user: &str, cookie: &str) -> io::Result<(Session, async_channel::Receiver<HelperEvent>)> {
        let mut child = Command::new(helper)
            .arg(user)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the helper has no pipes"));
        };
        if let Err(e) = writeln!(stdin, "{cookie}") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let rx = match spawn_reader(stdout) {
            Ok(rx) => rx,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok((Session { link: Link::Child { child, stdin } }, rx))
    }

    /// Answer the last prompt with `text`.
    pub fn respond(&mut self, text: &str) -> io::Result<()> {
        let writer: &mut dyn Write = match &mut self.link {
            Link::Child { stdin, .. } => stdin,
            Link::Socket(stream) => stream,
        };
        writer.write_all(text.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}

impl Drop for Session {
    /// End the helper (it may have ended already) and reap it. A socket
    /// helper ends when its connection closes.
    fn drop(&mut self) {
        match &mut self.link {
            Link::Child { child, .. } => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Link::Socket(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// Read the helper lines from `source` on a thread of their own and return
/// the channel they arrive on.
fn spawn_reader(source: impl Read + Send + 'static) -> io::Result<async_channel::Receiver<HelperEvent>> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("polkit-helper".to_string())
        .spawn(move || read_events(BufReader::new(source), &tx))?;
    Ok(rx)
}

/// Send one event per helper line until the result, then stop. A helper
/// that ends without a result counts as a failure.
fn read_events(reader: impl BufRead, tx: &async_channel::Sender<HelperEvent>) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Some(event) = parse_line(&line) else {
            opusdm_ipc::err_log!("opusdm-polkit", "unknown line from the helper: {line}");
            continue;
        };
        let last = matches!(event, HelperEvent::Success | HelperEvent::Failure);
        if tx.send_blocking(event).is_err() || last {
            return;
        }
    }
    let _ = tx.send_blocking(HelperEvent::Failure);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn every_line_of_the_protocol_is_parsed() {
        assert_eq!(parse_line("PAM_PROMPT_ECHO_OFF Password: "), Some(HelperEvent::PromptHidden("Password: ".to_string())));
        assert_eq!(parse_line("PAM_PROMPT_ECHO_ON Login:"), Some(HelperEvent::PromptVisible("Login:".to_string())));
        assert_eq!(parse_line("PAM_ERROR_MSG Wrong"), Some(HelperEvent::Error("Wrong".to_string())));
        assert_eq!(parse_line("PAM_TEXT_INFO Touch the key"), Some(HelperEvent::Info("Touch the key".to_string())));
        assert_eq!(parse_line("SUCCESS"), Some(HelperEvent::Success));
        assert_eq!(parse_line("FAILURE"), Some(HelperEvent::Failure));
        assert_eq!(parse_line("HELLO"), None);
    }

    #[test]
    fn unescape_undoes_c_escapes_and_octal_bytes() {
        assert_eq!(unescape(r#"a\nb\tc\\d\"e"#), "a\nb\tc\\d\"e");
        // "Pàssword" with the two UTF-8 bytes of "à" written in octal.
        assert_eq!(unescape(r"P\303\240ssword"), "Pàssword");
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape(r"end\"), r"end\");
    }

    /// Held while a test writes a fake helper and while one is started: a
    /// fork of a parallel test can inherit the write descriptor of a script
    /// for a moment, and executing that script then fails with ETXTBSY.
    static EXEC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `f` holding `EXEC_LOCK`.
    fn exec_locked<T>(f: impl FnOnce() -> T) -> T {
        let _guard = EXEC_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f()
    }

    /// Write an executable script `body` into `dir` and return its path.
    fn fake_helper(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("helper");
        exec_locked(|| {
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        });
        path
    }

    /// A helper that wants cookie `c1`, user `alice` and password `secret`.
    const CHECKING_HELPER: &str = "read cookie\n\
        echo 'PAM_PROMPT_ECHO_OFF Password: '\n\
        read pw\n\
        if [ \"$cookie\" = c1 ] && [ \"$1\" = alice ] && [ \"$pw\" = secret ]; then echo SUCCESS; else echo 'PAM_ERROR_MSG Wrong'; echo FAILURE; fi";

    #[test]
    fn the_right_password_ends_in_success() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), CHECKING_HELPER);
        let (mut session, rx) = exec_locked(|| Session::start(&helper, "alice", "c1")).unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
    }

    #[test]
    fn a_wrong_password_ends_in_an_error_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), CHECKING_HELPER);
        let (mut session, rx) = exec_locked(|| Session::start(&helper, "alice", "c1")).unwrap();
        rx.recv_blocking().unwrap();
        session.respond("nope").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Error("Wrong".to_string()));
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Failure);
    }

    #[test]
    fn a_helper_that_ends_without_a_result_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), "read cookie\nexit 1");
        let (_session, rx) = exec_locked(|| Session::start(&helper, "alice", "c1")).unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Failure);
    }

    /// Serve one connection on a socket in `dir` like the socket-activated
    /// helper: it wants user `alice`, cookie `c1` and password `secret`.
    fn fake_socket_helper(dir: &Path) -> (std::path::PathBuf, std::thread::JoinHandle<()>) {
        let path = dir.join("agent-helper.socket");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut lines = BufReader::new(stream).lines();
            let mut next = || lines.next().unwrap().unwrap();
            let (user, cookie) = (next(), next());
            writeln!(writer, "PAM_PROMPT_ECHO_OFF Password: ").unwrap();
            let ok = user == "alice" && cookie == "c1" && next() == "secret";
            writeln!(writer, "{}", if ok { "SUCCESS" } else { "FAILURE" }).unwrap();
        });
        (path, server)
    }

    #[test]
    fn the_socket_helper_gets_user_cookie_and_password() {
        let dir = tempfile::tempdir().unwrap();
        let (socket, server) = fake_socket_helper(dir.path());
        let (mut session, rx) = Session::connect(&socket, "alice", "c1").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
        server.join().unwrap();
    }

    #[test]
    fn open_prefers_the_socket_over_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let (socket, server) = fake_socket_helper(dir.path());
        let helper = fake_helper(dir.path(), "read cookie\necho FAILURE");
        let (mut session, rx) = exec_locked(|| Session::open(&socket, &helper, "alice", "c1")).unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
        server.join().unwrap();
    }

    #[test]
    fn open_starts_the_helper_without_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), CHECKING_HELPER);
        let (mut session, rx) = exec_locked(|| Session::open(&dir.path().join("missing.socket"), &helper, "alice", "c1")).unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
    }

    #[test]
    fn open_falls_back_to_the_helper_when_the_socket_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_socket = dir.path().join("agent-helper.socket");
        std::fs::write(&not_a_socket, "").unwrap();
        let helper = fake_helper(dir.path(), CHECKING_HELPER);
        let (mut session, rx) = exec_locked(|| Session::open(&not_a_socket, &helper, "alice", "c1")).unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
    }

    #[test]
    fn a_missing_helper_is_an_error() {
        assert!(exec_locked(|| Session::start(Path::new("/nonexistent/opusdm-helper"), "alice", "c1")).is_err());
    }
}
