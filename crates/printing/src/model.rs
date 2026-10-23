//! What OpusDM knows about printers and jobs, read from the IPP attributes
//! CUPS sends, and the requests made to CUPS.

use crate::ipp::Group;
use std::fmt;
use std::path::PathBuf;

/// The printer attributes asked to CUPS.
pub const PRINTER_ATTRS: &[&str] = &[
    "printer-name",
    "printer-info",
    "printer-location",
    "printer-make-and-model",
    "device-uri",
    "printer-state",
    "printer-state-reasons",
    "printer-state-message",
    "printer-is-accepting-jobs",
];

/// The job attributes asked to CUPS.
pub const JOB_ATTRS: &[&str] = &[
    "job-id",
    "job-name",
    "job-state",
    "job-state-reasons",
    "job-printer-uri",
    "job-originating-user-name",
    "time-at-creation",
];

/// `printer-state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrinterState {
    Idle,
    Processing,
    Stopped,
}

impl PrinterState {
    /// The state of the IPP enum value `n`; unknown values are `Idle`.
    pub fn from_ipp(n: i32) -> Self {
        match n {
            4 => PrinterState::Processing,
            5 => PrinterState::Stopped,
            _ => PrinterState::Idle,
        }
    }

    /// The words shown for the state.
    pub fn label(&self) -> &'static str {
        match self {
            PrinterState::Idle => "Ready",
            PrinterState::Processing => "Printing",
            PrinterState::Stopped => "Stopped",
        }
    }
}

/// `job-state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Held,
    Processing,
    Stopped,
    Canceled,
    Aborted,
    Completed,
}

impl JobState {
    /// The state of the IPP enum value `n`; unknown values are `Pending`.
    pub fn from_ipp(n: i32) -> Self {
        match n {
            4 => JobState::Held,
            5 => JobState::Processing,
            6 => JobState::Stopped,
            7 => JobState::Canceled,
            8 => JobState::Aborted,
            9 => JobState::Completed,
            _ => JobState::Pending,
        }
    }

    /// The word shown for the state.
    pub fn label(&self) -> &'static str {
        match self {
            JobState::Pending => "Waiting",
            JobState::Held => "Held",
            JobState::Processing => "Printing",
            JobState::Stopped => "Stopped",
            JobState::Canceled => "Canceled",
            JobState::Aborted => "Aborted",
            JobState::Completed => "Completed",
        }
    }

    /// Whether the job is still in the queue.
    pub fn is_active(&self) -> bool {
        matches!(self, JobState::Pending | JobState::Held | JobState::Processing | JobState::Stopped)
    }
}

/// How serious a state reason is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    None,
    Report,
    Warning,
    Error,
}

/// The severity of a `printer-state-reasons` keyword, from its suffix.
pub fn reason_severity(reason: &str) -> Severity {
    if reason.ends_with("-error") { return Severity::Error; }
    if reason.ends_with("-warning") { return Severity::Warning; }
    if reason.ends_with("-report") { return Severity::Report; }
    Severity::None
}

/// A state reason as words: `media-jam-error` is "media jam".
pub fn reason_words(reason: &str) -> String {
    let base = ["-error", "-warning", "-report"].iter().find_map(|s| reason.strip_suffix(s)).unwrap_or(reason);
    base.replace('-', " ")
}

/// One CUPS printer.
#[derive(Clone, Debug, PartialEq)]
pub struct Printer {
    pub name: String,
    pub info: String,
    pub location: String,
    pub make_model: String,
    pub device_uri: String,
    pub state_message: String,
    pub state: PrinterState,
    pub reasons: Vec<String>,
    pub accepting: bool,
    pub is_default: bool,
}

impl Printer {
    /// The printer described by a printer group; `None` without a name.
    pub fn from_group(g: &Group) -> Option<Printer> {
        let text = |name: &str| g.str(name).unwrap_or_default().to_string();
        Some(Printer {
            name: g.str("printer-name")?.to_string(),
            info: text("printer-info"),
            location: text("printer-location"),
            make_model: text("printer-make-and-model"),
            device_uri: text("device-uri"),
            state_message: text("printer-state-message"),
            state: PrinterState::from_ipp(g.int("printer-state").unwrap_or(3)),
            reasons: g.strs("printer-state-reasons"),
            accepting: g.bool("printer-is-accepting-jobs").unwrap_or(true),
            is_default: false,
        })
    }

    /// Whether the printer is stopped or paused.
    pub fn is_paused(&self) -> bool {
        self.state == PrinterState::Stopped || self.reasons.iter().any(|r| r == "paused")
    }

    /// The worst severity of its state reasons.
    pub fn worst(&self) -> Severity {
        self.reasons.iter().map(|r| reason_severity(r)).max().unwrap_or(Severity::None)
    }
}

/// One job in a queue.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: i32,
    pub printer: String,
    pub name: String,
    pub user: String,
    pub state: JobState,
    pub reasons: Vec<String>,
    pub created: i64,
}

impl Job {
    /// The job described by a job group; `None` without a `job-id`.
    pub fn from_group(g: &Group) -> Option<Job> {
        let uri = g.str("job-printer-uri").unwrap_or_default();
        Some(Job {
            id: g.int("job-id")?,
            printer: uri.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string(),
            name: g.str("job-name").unwrap_or_default().to_string(),
            user: g.str("job-originating-user-name").unwrap_or_default().to_string(),
            state: JobState::from_ipp(g.int("job-state").unwrap_or(3)),
            reasons: g.strs("job-state-reasons"),
            created: i64::from(g.int("time-at-creation").unwrap_or(0)),
        })
    }
}

/// A device CUPS found (`lpinfo -v`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Device {
    pub uri: String,
    pub info: String,
    pub make_model: String,
    pub device_id: String,
    pub class: String,
}

/// A driver CUPS knows (`lpinfo -m`); `name` is what `lpadmin -m` takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Driver {
    pub name: String,
    pub description: String,
}

/// A default job option of a printer (`lpoptions -l`).
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterOption {
    pub key: String,
    pub label: String,
    pub choices: Vec<String>,
    pub default: String,
}

/// The driver of a new printer.
#[derive(Clone, Debug, PartialEq)]
pub enum DriverChoice {
    /// IPP Everywhere (`lpadmin -m everywhere`).
    Driverless,
    /// A driver of the list (`lpadmin -m NAME`).
    Model(String),
    /// A PPD file (`lpadmin -P FILE`).
    PpdFile(PathBuf),
}

/// A printer to add.
#[derive(Clone, Debug, PartialEq)]
pub struct NewPrinter {
    pub name: String,
    pub uri: String,
    pub info: String,
    pub location: String,
    pub driver: DriverChoice,
    pub make_default: bool,
}

/// The changes saved on a printer; `options` holds only the changed ones.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PrinterEdit {
    pub info: String,
    pub location: String,
    pub options: Vec<(String, String)>,
}

/// A failure talking to CUPS, with the text to show.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintError(pub String);

impl PrintError {
    /// An error with `text`.
    pub fn new(text: impl Into<String>) -> Self {
        PrintError(text.into())
    }
}

impl fmt::Display for PrintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PrintError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipp::tests::{printers_fixture, Fixture};
    use crate::ipp::{decode, Group, TAG_JOB, TAG_KEYWORD, TAG_NAME, TAG_PRINTER};

    /// The printer groups of the shared fixture.
    fn printer_groups() -> Vec<Group> {
        decode(&printers_fixture()).unwrap().groups_of(TAG_PRINTER).cloned().collect()
    }

    /// A printer with `state`, `message` and `reasons`, for the tests.
    fn printer(state: PrinterState, message: &str, reasons: &[&str]) -> Printer {
        Printer {
            name: "p".into(),
            info: String::new(),
            location: String::new(),
            make_model: String::new(),
            device_uri: String::new(),
            state_message: message.into(),
            state,
            reasons: reasons.iter().map(|r| r.to_string()).collect(),
            accepting: true,
            is_default: false,
        }
    }

    #[test]
    fn printer_from_group() {
        let groups = printer_groups();
        let p = Printer::from_group(&groups[0]).unwrap();
        assert_eq!(p.name, "C364Series");
        assert_eq!(p.info, "KONICA MINOLTA bizhub C224e(98:1F:7C)");
        assert_eq!(p.state, PrinterState::Idle);
        assert!(p.accepting);
        assert!(!p.is_default);
        assert_eq!(p.reasons, vec!["media-empty-error".to_string(), "toner-low-warning".to_string()]);
        assert_eq!(p.worst(), Severity::Error);
        let bare = Group { tag: TAG_PRINTER, attrs: Vec::new() };
        assert!(Printer::from_group(&bare).is_none());
    }

    #[test]
    fn job_from_group() {
        let bytes = Fixture::new(0)
            .group(TAG_JOB)
            .text(0x45, "job-printer-uri", "ipp://localhost:631/printers/opusdm_probe")
            .text(TAG_NAME, "job-originating-user-name", "fabio")
            .text(TAG_NAME, "job-name", "t.txt")
            .attr(0x21, "time-at-creation", &1_792_738_747i32.to_be_bytes())
            .attr(0x21, "job-id", &32i32.to_be_bytes())
            .enumv("job-state", 5)
            .text(TAG_KEYWORD, "job-state-reasons", "job-printing")
            .end();
        let r = decode(&bytes).unwrap();
        let j = Job::from_group(r.groups_of(TAG_JOB).next().unwrap()).unwrap();
        assert_eq!(j.id, 32);
        assert_eq!(j.printer, "opusdm_probe");
        assert_eq!(j.name, "t.txt");
        assert_eq!(j.user, "fabio");
        assert_eq!(j.state, JobState::Processing);
        assert_eq!(j.reasons, vec!["job-printing".to_string()]);
        assert_eq!(j.created, 1_792_738_747);
        assert!(Job::from_group(&Group { tag: TAG_JOB, attrs: Vec::new() }).is_none());
    }

    #[test]
    fn reason_severity_by_suffix() {
        assert_eq!(reason_severity("media-jam-error"), Severity::Error);
        assert_eq!(reason_severity("toner-low-warning"), Severity::Warning);
        assert_eq!(reason_severity("cups-insecure-filter-warning"), Severity::Warning);
        assert_eq!(reason_severity("other-report"), Severity::Report);
        assert_eq!(reason_severity("none"), Severity::None);
        assert_eq!(reason_severity("paused"), Severity::None);
        assert_eq!(reason_words("media-jam-error"), "media jam");
        assert_eq!(reason_words("toner-low-warning"), "toner low");
        assert_eq!(reason_words("paused"), "paused");
    }

    #[test]
    fn paused_printer() {
        assert!(printer(PrinterState::Stopped, "", &[]).is_paused());
        assert!(printer(PrinterState::Idle, "", &["paused"]).is_paused());
        assert!(!printer(PrinterState::Idle, "", &["none"]).is_paused());
        assert_eq!(printer(PrinterState::Idle, "", &["none"]).worst(), Severity::None);
    }

    #[test]
    fn job_state_labels_and_activity() {
        let all: Vec<JobState> = (3..=9).map(JobState::from_ipp).collect();
        assert_eq!(all, vec![JobState::Pending, JobState::Held, JobState::Processing, JobState::Stopped, JobState::Canceled, JobState::Aborted, JobState::Completed]);
        let labels: Vec<&str> = all.iter().map(|s| s.label()).collect();
        assert_eq!(labels, vec!["Waiting", "Held", "Printing", "Stopped", "Canceled", "Aborted", "Completed"]);
        let active: Vec<bool> = all.iter().map(|s| s.is_active()).collect();
        assert_eq!(active, vec![true, true, true, true, false, false, false]);
        assert_eq!(JobState::from_ipp(42), JobState::Pending);
    }
}
