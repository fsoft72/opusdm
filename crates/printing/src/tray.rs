//! What the panel printer icon shows, and the notifications to send, from
//! the snapshots of the watcher. Pure: the time is passed in.

use crate::model::{reason_severity, reason_words, JobState, Printer, PrinterState, Severity};
use crate::watch::Snapshot;
use std::collections::{HashMap, HashSet};

/// Seconds a printer may not answer, with jobs waiting, before a note.
pub const OFFLINE_NOTIFY_SECS: u64 = 120;
/// The state message `cupsdisable` writes when no reason is given.
pub const PAUSED_MESSAGE: &str = "Paused";
/// Words of a state message that mean the printer does not answer.
const OFFLINE_WORDS: &[&str] = &["unavailable", "not exist", "unable to", "not responding", "offline", "waiting for printer", "network host"];
/// State reasons that mean the printer does not answer.
const OFFLINE_REASONS: &[&str] = &["offline-report", "connecting-to-device", "timed-out-report", "offline-warning", "offline-error"];

/// The look of the icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Hidden,
    Normal,
    /// A printer the owner disabled holds jobs.
    Paused,
    Error,
}

/// What the icon shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    pub look: Look,
    pub jobs: usize,
    pub tooltip: String,
}

/// A notification to send.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub summary: String,
    pub body: String,
}

/// Whether `p` was disabled by the owner (not stopped by CUPS for a fault).
pub fn is_owner_paused(p: &Printer) -> bool {
    p.state == PrinterState::Stopped && (p.state_message.is_empty() || p.state_message == PAUSED_MESSAGE)
}

/// Whether `p` does not answer (CUPS keeps retrying it).
pub fn is_not_answering(p: &Printer) -> bool {
    if p.state != PrinterState::Processing { return false; }
    let message = p.state_message.to_lowercase();
    OFFLINE_WORDS.iter().any(|w| message.contains(w)) || p.reasons.iter().any(|r| OFFLINE_REASONS.contains(&r.as_str()))
}

/// The errors of `s`: a stable key and the text to show, in order.
pub fn errors(s: &Snapshot) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for p in &s.printers {
        for r in p.reasons.iter().filter(|r| reason_severity(r) == Severity::Error) {
            out.push((format!("{}:{r}", p.name), p.name.clone(), reason_words(r)));
        }
        if p.state == PrinterState::Stopped && !is_owner_paused(p) && s.jobs_of(&p.name).next().is_some() {
            out.push((format!("{}:stopped", p.name), p.name.clone(), p.state_message.clone()));
        }
    }
    for j in s.jobs.iter().filter(|j| j.state == JobState::Stopped) {
        let why = s.printers.iter().find(|p| p.name == j.printer).map(|p| p.state_message.clone()).filter(|m| !m.is_empty()).or_else(|| j.reasons.first().cloned()).unwrap_or_default();
        out.push((format!("job:{}", j.id), j.printer.clone(), format!("Job \"{}\" stopped: {why}", j.name)));
    }
    out
}

/// What the icon shows for `s`.
pub fn view(s: &Snapshot) -> TrayView {
    let jobs = s.jobs.iter().filter(|j| j.state.is_active()).count();
    let errors = errors(s);
    let paused = s.printers.iter().any(|p| is_owner_paused(p) && s.jobs_of(&p.name).next().is_some());
    let look = match (errors.is_empty(), jobs) {
        (false, _) => Look::Error,
        (true, 0) => Look::Hidden,
        (true, _) if paused => Look::Paused,
        (true, _) => Look::Normal,
    };
    let mut tooltip = match jobs {
        0 => String::new(),
        1 => "1 print job".to_string(),
        n => format!("{n} print jobs"),
    };
    if let Some((_, printer, text)) = errors.first() {
        if !tooltip.is_empty() { tooltip.push_str(" - "); }
        tooltip.push_str(&format!("{printer}: {text}"));
    }
    TrayView { look, jobs, tooltip }
}

/// Whether the notes depend on time for `s`: a printer does not answer
/// while jobs wait, so [`Notifier::update`] must run again later even if
/// nothing changes.
pub fn needs_timer(s: &Snapshot) -> bool {
    s.printers.iter().any(|p| is_not_answering(p) && s.jobs_of(&p.name).next().is_some())
}

/// Remembers which errors were already notified, and since when each
/// printer does not answer.
#[derive(Default)]
pub struct Notifier {
    notified: HashSet<String>,
    offline_since: HashMap<String, u64>,
    offline_notified: HashSet<String>,
}

impl Notifier {
    /// The notes for `s` seen at `now_secs`.
    pub fn update(&mut self, s: &Snapshot, now_secs: u64) -> Vec<Note> {
        let mut notes = Vec::new();
        let errors = errors(s);
        // Keys gone from the snapshot are forgotten: a later return notifies again.
        self.notified.retain(|k| errors.iter().any(|(key, _, _)| key == k));
        for (key, printer, text) in errors {
            if self.notified.insert(key) { notes.push(Note { summary: format!("Printer {printer}"), body: text }); }
        }
        let offline: Vec<&Printer> = s.printers.iter().filter(|p| is_not_answering(p) && s.jobs_of(&p.name).next().is_some()).collect();
        self.offline_since.retain(|name, _| offline.iter().any(|p| &p.name == name));
        self.offline_notified.retain(|name| offline.iter().any(|p| &p.name == name));
        for p in offline {
            let since = *self.offline_since.entry(p.name.clone()).or_insert(now_secs);
            if now_secs.saturating_sub(since) < OFFLINE_NOTIFY_SECS || self.offline_notified.contains(&p.name) { continue; }
            self.offline_notified.insert(p.name.clone());
            notes.push(Note { summary: format!("Printer {}", p.name), body: format!("Not answering: {}", p.state_message) });
        }
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{test_job, test_printer};

    /// A snapshot with `printer` and `jobs` jobs on it.
    fn snap(printer: Printer, jobs: usize) -> Snapshot {
        let name = printer.name.clone();
        Snapshot { printers: vec![printer], jobs: (0..jobs as i32).map(|i| test_job(40 + i, &name)).collect() }
    }

    /// The test printer with `state`, `message` and `reasons`.
    fn printer(state: PrinterState, message: &str, reasons: &[&str]) -> Printer {
        let mut p = test_printer("C364Series");
        p.state = state;
        p.state_message = message.to_string();
        p.reasons = reasons.iter().map(|r| r.to_string()).collect();
        p
    }

    /// A ready printer.
    fn ready() -> Printer {
        printer(PrinterState::Idle, "", &["none"])
    }

    /// The printer CUPS shows while it retries an unreachable device.
    fn not_answering() -> Printer {
        printer(PrinterState::Processing, "The printer may not exist or is unavailable at this time.", &["none"])
    }

    #[test]
    fn hidden_without_jobs_or_errors() {
        assert_eq!(view(&snap(ready(), 0)).look, Look::Hidden);
        assert_eq!(view(&Snapshot::default()).look, Look::Hidden);
    }

    #[test]
    fn badge_counts_active_jobs() {
        let mut s = snap(ready(), 2);
        s.jobs[1].state = JobState::Held;
        let v = view(&s);
        assert_eq!((v.look, v.jobs), (Look::Normal, 2));
        assert_eq!(v.tooltip, "2 print jobs");
        assert_eq!(view(&snap(ready(), 1)).tooltip, "1 print job");
    }

    #[test]
    fn error_look_for_error_reason_and_stopped_job() {
        let v = view(&snap(printer(PrinterState::Idle, "", &["media-jam-error"]), 0));
        assert_eq!(v.look, Look::Error);
        assert!(v.tooltip.contains("C364Series: media jam"), "{}", v.tooltip);
        let mut s = snap(ready(), 1);
        s.jobs[0].state = JobState::Stopped;
        assert_eq!(view(&s).look, Look::Error);
        let stopped_by_cups = snap(printer(PrinterState::Stopped, "Unable to send data to printer.", &["paused"]), 1);
        let v = view(&stopped_by_cups);
        assert_eq!(v.look, Look::Error);
        assert!(v.tooltip.contains("Unable to send data"), "{}", v.tooltip);
    }

    #[test]
    fn owner_paused_is_grey_not_red() {
        let paused = printer(PrinterState::Stopped, "Paused", &["paused"]);
        assert!(is_owner_paused(&paused));
        assert_eq!(view(&snap(paused.clone(), 1)).look, Look::Paused);
        assert_eq!(view(&snap(paused.clone(), 0)).look, Look::Hidden);
        assert!(Notifier::default().update(&snap(paused, 1), 0).is_empty());
        assert!(is_owner_paused(&printer(PrinterState::Stopped, "", &["paused"])));
        assert!(!is_owner_paused(&printer(PrinterState::Stopped, "Unable to send data to printer.", &["paused"])));
    }

    #[test]
    fn warning_does_not_make_error() {
        let s = snap(printer(PrinterState::Idle, "", &["toner-low-warning"]), 1);
        assert_eq!(view(&s).look, Look::Normal);
        assert!(Notifier::default().update(&s, 0).is_empty());
    }

    #[test]
    fn not_answering_is_normal_with_a_badge() {
        assert!(is_not_answering(&not_answering()));
        assert!(!is_not_answering(&printer(PrinterState::Processing, "Sending data to printer.", &["none"])));
        assert!(is_not_answering(&printer(PrinterState::Processing, "", &["connecting-to-device"])));
        assert_eq!(view(&snap(not_answering(), 1)).look, Look::Normal);
    }

    #[test]
    fn notifies_once_per_error_episode() {
        let jam = snap(printer(PrinterState::Idle, "", &["media-jam-error"]), 1);
        let fine = snap(ready(), 1);
        let mut n = Notifier::default();
        let notes = n.update(&jam, 0);
        assert_eq!(notes, vec![Note { summary: "Printer C364Series".into(), body: "media jam".into() }]);
        assert!(n.update(&jam, 2).is_empty());
        assert!(n.update(&fine, 4).is_empty());
        assert_eq!(n.update(&jam, 6).len(), 1);
    }

    #[test]
    fn a_stopped_job_is_notified_with_its_name() {
        let mut s = snap(printer(PrinterState::Processing, "Unable to locate printer.", &["none"]), 1);
        s.jobs[0].state = JobState::Stopped;
        s.jobs[0].name = "fattura.pdf".into();
        let notes = Notifier::default().update(&s, 0);
        assert_eq!(notes, vec![Note { summary: "Printer C364Series".into(), body: "Job \"fattura.pdf\" stopped: Unable to locate printer.".into() }]);
    }

    #[test]
    fn a_timer_is_needed_only_while_a_printer_does_not_answer() {
        // The watcher sends a snapshot only when something changes: a steady
        // "not answering" must be looked at again by a timer.
        assert!(needs_timer(&snap(not_answering(), 1)));
        assert!(!needs_timer(&snap(not_answering(), 0)));
        assert!(!needs_timer(&snap(ready(), 3)));
    }

    #[test]
    fn offline_notifies_after_two_minutes() {
        let offline = snap(not_answering(), 1);
        let fine = snap(ready(), 1);
        let mut n = Notifier::default();
        assert!(n.update(&offline, 0).is_empty());
        assert!(n.update(&offline, 119).is_empty());
        let notes = n.update(&offline, 120);
        assert_eq!(notes, vec![Note { summary: "Printer C364Series".into(), body: "Not answering: The printer may not exist or is unavailable at this time.".into() }]);
        assert!(n.update(&offline, 200).is_empty());
        assert!(n.update(&fine, 250).is_empty());
        assert!(n.update(&offline, 300).is_empty());
        assert!(n.update(&offline, 419).is_empty());
        assert_eq!(n.update(&offline, 420).len(), 1);
        assert!(n.update(&snap(not_answering(), 0), 1000).is_empty(), "no jobs: nothing to say");
    }
}
