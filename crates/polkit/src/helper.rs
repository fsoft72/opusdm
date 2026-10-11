//! The conversation with `polkit-agent-helper-1`, the setuid program that
//! checks the password with PAM. It is started as `helper USER`, reads the
//! cookie of the request on stdin, then prints one line per PAM message and
//! reads one line per answer. On success the helper itself tells polkitd
//! that the request is authenticated.

use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Where polkit installs its helper.
pub const HELPER_PATH: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";

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

/// A running helper. Dropping it ends the helper.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
}

impl Session {
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
        let (tx, rx) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("polkit-helper".to_string())
            .spawn(move || read_events(BufReader::new(stdout), &tx))?;
        Ok((Session { child, stdin }, rx))
    }

    /// Answer the last prompt with `text`.
    pub fn respond(&mut self, text: &str) -> io::Result<()> {
        self.stdin.write_all(text.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }
}

impl Drop for Session {
    /// End the helper (it may have ended already) and reap it.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

    /// Write an executable script `body` into `dir` and return its path.
    fn fake_helper(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("helper");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        let (mut session, rx) = Session::start(&helper, "alice", "c1").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::PromptHidden("Password: ".to_string()));
        session.respond("secret").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Success);
    }

    #[test]
    fn a_wrong_password_ends_in_an_error_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), CHECKING_HELPER);
        let (mut session, rx) = Session::start(&helper, "alice", "c1").unwrap();
        rx.recv_blocking().unwrap();
        session.respond("nope").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Error("Wrong".to_string()));
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Failure);
    }

    #[test]
    fn a_helper_that_ends_without_a_result_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let helper = fake_helper(dir.path(), "read cookie\nexit 1");
        let (_session, rx) = Session::start(&helper, "alice", "c1").unwrap();
        assert_eq!(rx.recv_blocking().unwrap(), HelperEvent::Failure);
    }

    #[test]
    fn a_missing_helper_is_an_error() {
        assert!(Session::start(Path::new("/nonexistent/opusdm-helper"), "alice", "c1").is_err());
    }
}
