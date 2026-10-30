//! The privileged run: one `pkexec` that runs a fixed script, which executes
//! the queued `ufw` commands and then reads the state. Rule arguments reach
//! `ufw` as separate arguments: the script never evaluates them.

use crate::model::{self, Op, Snapshot, MARK_OP_FAILED, MARK_OP_OK, MARK_STATUS};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where `ufw` can be.
const UFW_PATHS: [&str; 2] = ["/usr/sbin/ufw", "/sbin/ufw"];
/// The file with the default policies, readable also while ufw is off.
const DEFAULTS_PATH: &str = "/etc/default/ufw";
/// The program that asks the password.
const PKEXEC: &str = "pkexec";
/// The shell of the script, an absolute path as `pkexec` wants.
const SHELL: &str = "/bin/sh";
/// `pkexec` exit code: the authentication dialog was dismissed.
const PKEXEC_DISMISSED: i32 = 126;
/// `pkexec` exit code: not authorized, or no agent to ask.
const PKEXEC_REFUSED: i32 = 127;

/// The script run as root. Arguments: the `ufw` path, the defaults file, then
/// for each command its argument count followed by its arguments. In the
/// subshell the rotation leaves only the arguments of this command in `$@`.
const SCRIPT: &str = r#"export LC_ALL=C
ufw=$1
defaults=$2
shift 2
while [ "$#" -gt 0 ]; do
    n=$1
    shift
    out=$(
        total=$#
        i=0
        while [ "$i" -lt "$total" ]; do
            arg=$1
            shift
            if [ "$i" -lt "$n" ]; then set -- "$@" "$arg"; fi
            i=$((i + 1))
        done
        "$ufw" "$@" </dev/null 2>&1
    )
    code=$?
    if [ "$code" -ne 0 ]; then
        echo "@@opusdm-op-failed $code"
        printf '%s\n' "$out"
        break
    fi
    echo "@@opusdm-op-ok"
    shift "$n"
done
echo "@@opusdm-status"
"$ufw" status </dev/null 2>&1
echo "@@opusdm-defaults"
cat "$defaults" 2>&1
echo "@@opusdm-rules"
"$ufw" show added </dev/null 2>&1
"#;

/// Why a run did not happen, or its result could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The user dismissed the password dialog.
    Cancelled,
    /// Polkit refused, or nothing could ask the password.
    Refused(String),
    /// The command could not start or its output makes no sense.
    Failed(String),
}

impl RunError {
    /// The text for the user.
    pub fn text(&self) -> String {
        match self {
            RunError::Cancelled => "The authorization was cancelled.".to_string(),
            RunError::Refused(why) => format!("The authorization was refused: {why}"),
            RunError::Failed(why) => why.clone(),
        }
    }
}

/// How a run ended. The state is always read after the commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// How many commands ran.
    pub done: usize,
    /// The output of the command that failed; the run stopped there.
    pub failure: Option<String>,
    pub snapshot: Snapshot,
}

/// Runs the commands with the privileges they need.
pub trait Backend: Send + Sync {
    /// Whether `ufw` is installed.
    fn available(&self) -> bool;

    /// Run `ops` in order, stopping at the first failure, then read the state.
    fn run(&self, ops: &[Op]) -> Result<Report, RunError>;
}

/// The real backend.
pub struct PkexecBackend {
    /// What runs the script: `pkexec`, or nothing in the tests.
    launcher: Option<String>,
    ufw: Option<PathBuf>,
    defaults: PathBuf,
}

impl PkexecBackend {
    /// The backend on this system.
    pub fn new() -> Self {
        let ufw = UFW_PATHS.iter().map(PathBuf::from).find(|p| p.is_file());
        Self { launcher: Some(PKEXEC.to_string()), ufw, defaults: PathBuf::from(DEFAULTS_PATH) }
    }

    /// The command line of a run on `ufw`.
    fn command(&self, ufw: &Path, ops: &[Op]) -> Command {
        let mut cmd = match &self.launcher {
            Some(launcher) => {
                let mut c = Command::new(launcher);
                c.arg(SHELL);
                c
            }
            None => Command::new(SHELL),
        };
        cmd.arg("-c").arg(SCRIPT).arg("sh").arg(ufw).arg(&self.defaults);
        for op in ops {
            cmd.arg(op.args.len().to_string()).args(&op.args);
        }
        cmd.stdin(Stdio::null());
        cmd
    }
}

impl Default for PkexecBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for PkexecBackend {
    fn available(&self) -> bool {
        self.ufw.is_some()
    }

    fn run(&self, ops: &[Op]) -> Result<Report, RunError> {
        let Some(ufw) = &self.ufw else { return Err(RunError::Failed("ufw is not installed.".to_string())) };
        let output = self.command(ufw, ops).output().map_err(|e| RunError::Failed(format!("Cannot start {}: {e}", self.launcher.as_deref().unwrap_or(SHELL))))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        match output.status.code() {
            Some(0) => parse_report(&stdout),
            Some(PKEXEC_DISMISSED) => Err(RunError::Cancelled),
            Some(PKEXEC_REFUSED) => Err(RunError::Refused(stderr.trim().to_string())),
            code => Err(RunError::Failed(format!("The privileged run ended with {code:?}: {}", stderr.trim()))),
        }
    }
}

/// Read the output of the script.
fn parse_report(stdout: &str) -> Result<Report, RunError> {
    let snapshot = model::parse_snapshot(stdout).map_err(RunError::Failed)?;
    let (ops_part, _) = stdout.split_once(MARK_STATUS).unwrap_or((stdout, ""));
    let done = ops_part.lines().filter(|l| *l == MARK_OP_OK).count();
    let failure = ops_part.split_once(MARK_OP_FAILED).map(|(_, text)| text.lines().skip(1).collect::<Vec<_>>().join("\n").trim().to_string());
    Ok(Report { done, failure, snapshot })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, MutexGuard};

    /// Writing a script while another test forks can make its start fail
    /// with "text file busy": the tests that do it run one at a time.
    static SCRIPTS: Mutex<()> = Mutex::new(());

    /// Hold the lock of the tests that write scripts.
    fn scripts_lock() -> MutexGuard<'static, ()> {
        SCRIPTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A `ufw` that logs its arguments one per line and fails on `boom`.
    const FAKE_UFW: &str = r#"#!/bin/sh
if [ "$1" = "status" ]; then echo "Status: active"; exit 0; fi
if [ "$1" = "show" ]; then echo "Added user rules (see 'ufw status' for running firewall):"; echo "ufw deny out 25/tcp"; exit 0; fi
for a in "$@"; do printf '[%s]' "$a" >> "$FAKE_LOG"; done
echo >> "$FAKE_LOG"
if [ "$1" = "boom" ]; then echo "ERROR: Bad port"; exit 1; fi
exit 0
"#;

    /// The real script on a fake `ufw`, without `pkexec`.
    struct Fixture {
        backend: PkexecBackend,
        log: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// Build the fake `ufw`, its log and the defaults file.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ufw = dir.path().join("ufw");
        std::fs::write(&ufw, FAKE_UFW).unwrap();
        std::fs::set_permissions(&ufw, std::fs::Permissions::from_mode(0o755)).unwrap();
        let defaults = dir.path().join("defaults");
        std::fs::write(&defaults, "DEFAULT_INPUT_POLICY=\"DROP\"\nDEFAULT_OUTPUT_POLICY=\"ACCEPT\"\n").unwrap();
        let log = dir.path().join("log");
        std::fs::write(&log, "").unwrap();
        // The log path reaches the fake through the launcher-less shell.
        std::env::set_var("FAKE_LOG", &log);
        Fixture { backend: PkexecBackend { launcher: None, ufw: Some(ufw), defaults }, log, _dir: dir }
    }

    /// An operation from these words.
    fn op(words: &[&str]) -> Op {
        Op { args: words.iter().map(|w| w.to_string()).collect() }
    }

    #[test]
    fn the_script_keeps_arguments_whole_stops_at_a_failure_and_reads_the_state() {
        let _lock = scripts_lock();
        let f = fixture();

        // A read runs no command and still gives the state.
        let read = f.backend.run(&[]).unwrap();
        assert_eq!((read.done, read.failure.clone()), (0, None));
        assert_eq!((read.snapshot.active, read.snapshot.default_in.as_str(), read.snapshot.rules.len()), (true, "deny", 1));
        assert_eq!(std::fs::read_to_string(&f.log).unwrap(), "");

        // Hostile words stay one argument each and run nothing.
        let marker = f._dir.path().join("pwned");
        let hostile = format!("x; touch {}", marker.display());
        let quote = format!("$(touch {})'\"", marker.display());
        let ops = [op(&["delete", "allow", "comment", &hostile]), op(&[&quote, "two words"]), op(&["boom"]), op(&["never"])];
        let report = f.backend.run(&ops).unwrap();
        assert_eq!(report.done, 2);
        assert_eq!(report.failure.as_deref(), Some("ERROR: Bad port"));
        assert_eq!(report.snapshot.rules[0].words, ["deny", "out", "25/tcp"]);
        let expected = format!("[delete][allow][comment][{hostile}]\n[{quote}][two words]\n[boom]\n");
        assert_eq!(std::fs::read_to_string(&f.log).unwrap(), expected);
        assert!(!marker.exists(), "an argument was evaluated by the shell");
    }

    #[test]
    fn exit_codes_of_pkexec_are_told_apart() {
        let _lock = scripts_lock();
        let script = |code: i32| -> Result<Report, RunError> {
            let dir = tempfile::tempdir().unwrap();
            let launcher = dir.path().join("launcher");
            std::fs::write(&launcher, format!("#!/bin/sh\necho refused >&2\nexit {code}\n")).unwrap();
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
            let backend = PkexecBackend { launcher: Some(launcher.display().to_string()), ufw: Some(PathBuf::from("/bin/true")), defaults: PathBuf::from("/dev/null") };
            backend.run(&[])
        };
        assert_eq!(script(126), Err(RunError::Cancelled));
        assert_eq!(script(127), Err(RunError::Refused("refused".to_string())));
        assert!(matches!(script(1), Err(RunError::Failed(_))));
    }

    #[test]
    fn a_missing_ufw_is_said_and_runs_nothing() {
        let backend = PkexecBackend { launcher: Some("/nonexistent".to_string()), ufw: None, defaults: PathBuf::from("/dev/null") };
        assert!(!backend.available());
        assert_eq!(backend.run(&[]).unwrap_err().text(), "ufw is not installed.");
    }
}
