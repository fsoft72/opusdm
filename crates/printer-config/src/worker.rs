//! The jobs sent to CUPS, run on their own thread so the window never waits
//! for a command. Every change is followed by a fresh read of the printers
//! and the queue, also when it failed (the job may be gone already).

use opusdm_printing::backend::PrintBackend;
use opusdm_printing::model::{Device, Driver, NewPrinter, PrintError, PrinterEdit, PrinterOption};
use opusdm_printing::watch::Snapshot;
use std::sync::{mpsc, Arc};

/// What the window asks for.
#[derive(Clone, Debug)]
pub enum Job {
    Refresh,
    LoadOptions(String),
    Devices,
    Drivers,
    Add(Box<NewPrinter>),
    Save { printer: String, edit: PrinterEdit },
    Remove(String),
    SetDefault(String),
    SetEnabled(String, bool),
    TestPage(String),
    Cancel(i32),
    CancelAll(String),
    Hold(i32),
    Release(i32),
    Retry(String, Option<i32>),
}

/// What the worker reports back.
#[derive(Clone, Debug)]
pub enum Done {
    Refreshed(Snapshot),
    Options(String, Vec<PrinterOption>),
    Devices(Vec<Device>),
    Drivers(Vec<Driver>),
    Added(String),
    Saved(String),
    /// A change worked; the text for the message line.
    Acted(String),
    /// A job failed: what was being done, why, and whether a refresh follows.
    Failed { what: String, error: PrintError, refresh: bool },
}

/// What a failed [`Job::Save`] reports in [`Done::Failed`].
pub const SAVING: &str = "Saving";
/// What a failed [`Job::Add`] reports in [`Done::Failed`].
pub const ADDING: &str = "Adding the printer";

impl Done {
    /// Whether this is the failure of a [`Job::Save`].
    pub fn is_failed_save(&self) -> bool {
        matches!(self, Done::Failed { what, .. } if what == SAVING)
    }
}

/// Run one job; the reports in order.
pub fn run_job(backend: &dyn PrintBackend, job: Job) -> Vec<Done> {
    let read_failed = |what: &str, error: PrintError| vec![Done::Failed { what: what.to_string(), error, refresh: false }];
    let (what, result) = match job {
        Job::Refresh => return vec![_refresh(backend)],
        Job::LoadOptions(p) => {
            return match backend.options(&p) {
                Ok(o) => vec![Done::Options(p, o)],
                Err(e) => read_failed(&format!("Reading the options of {p}"), e),
            }
        }
        Job::Devices => return backend.devices().map(|d| vec![Done::Devices(d)]).unwrap_or_else(|e| read_failed("Searching for printers", e)),
        Job::Drivers => return backend.drivers().map(|d| vec![Done::Drivers(d)]).unwrap_or_else(|e| read_failed("Reading the drivers", e)),
        Job::Add(new) => (ADDING.to_string(), backend.add(&new).map(|_| Done::Added(new.name.clone()))),
        Job::Save { printer, edit } => (SAVING.to_string(), backend.modify(&printer, &edit).map(|_| Done::Saved(printer.clone()))),
        Job::Remove(p) => (format!("Removing {p}"), backend.remove(&p).map(|_| Done::Acted(format!("{p} removed")))),
        Job::SetDefault(p) => (format!("Making {p} the default"), backend.set_default(&p).map(|_| Done::Acted(format!("{p} is the default printer")))),
        Job::SetEnabled(p, on) => {
            let word = if on { "enabled" } else { "disabled" };
            (format!("Changing {p}"), backend.set_enabled(&p, on).map(|_| Done::Acted(format!("{p} {word}"))))
        }
        Job::TestPage(p) => (format!("Printing the test page on {p}"), backend.test_page(&p).map(|_| Done::Acted(format!("Test page sent to {p}")))),
        Job::Cancel(id) => (format!("Canceling job {id}"), backend.cancel(id).map(|_| Done::Acted(format!("Job {id} canceled")))),
        Job::CancelAll(p) => (format!("Canceling the jobs of {p}"), backend.cancel_all(&p).map(|_| Done::Acted(format!("Jobs of {p} canceled")))),
        Job::Hold(id) => (format!("Holding job {id}"), backend.hold(id).map(|_| Done::Acted(format!("Job {id} held")))),
        Job::Release(id) => (format!("Releasing job {id}"), backend.release(id).map(|_| Done::Acted(format!("Job {id} released")))),
        Job::Retry(p, id) => (format!("Retrying {p}"), backend.retry(&p, id).map(|_| Done::Acted(format!("{p} started again")))),
    };
    let first = result.unwrap_or_else(|error| Done::Failed { what, error, refresh: true });
    vec![first, _refresh(backend)]
}

/// Read the printers and the queue.
fn _refresh(backend: &dyn PrintBackend) -> Done {
    let snapshot = backend.printers().and_then(|printers| Ok(Snapshot { printers, jobs: backend.jobs()? }));
    snapshot.map(Done::Refreshed).unwrap_or_else(|error| Done::Failed { what: "Reading the printers".to_string(), error, refresh: false })
}

/// Start the worker thread. Jobs run in order; reports go to `done`. The
/// thread ends when the job sender or the report receiver is dropped.
pub fn spawn(backend: Arc<dyn PrintBackend>, done: async_channel::Sender<Done>) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new().name("opusdm-printer-worker".into()).spawn(move || {
        while let Ok(job) = rx.recv() {
            for report in run_job(&*backend, job) {
                if done.send_blocking(report).is_err() { return; }
            }
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-printer-manager", "cannot start the CUPS worker: {e}");
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_printing::backend::{test_job, test_printer, FakeBackend};

    /// A fake with printer `p` and job 41 on it.
    fn fake() -> FakeBackend {
        let f = FakeBackend::default();
        f.printers.lock().unwrap().push(test_printer("p"));
        f.jobs.lock().unwrap().push(test_job(41, "p"));
        f
    }

    #[test]
    fn an_action_reports_and_refreshes() {
        let f = fake();
        let done = run_job(&f, Job::Cancel(41));
        assert!(matches!(done.as_slice(), [Done::Acted(t), Done::Refreshed(s)] if t == "Job 41 canceled" && s.jobs.is_empty()), "{done:?}");
        let done = run_job(&f, Job::SetDefault("p".into()));
        assert!(matches!(done.as_slice(), [Done::Acted(t), Done::Refreshed(s)] if t == "p is the default printer" && s.printers[0].is_default), "{done:?}");
    }

    #[test]
    fn a_failed_queue_action_refreshes() {
        let f = fake();
        *f.fail_once.lock().unwrap() = Some(PrintError::new("cancel: client-error-not-found"));
        let done = run_job(&f, Job::Cancel(41));
        assert!(matches!(done.as_slice(), [Done::Failed { what, error, refresh: true }, Done::Refreshed(_)] if what == "Canceling job 41" && error.0.contains("not-found")), "{done:?}");
    }

    #[test]
    fn reads_report_their_data_and_failures_do_not_refresh() {
        let f = fake();
        assert!(matches!(run_job(&f, Job::Refresh).as_slice(), [Done::Refreshed(s)] if s.printers.len() == 1 && s.jobs.len() == 1));
        assert!(matches!(run_job(&f, Job::LoadOptions("p".into())).as_slice(), [Done::Options(p, o)] if p == "p" && o.is_empty()));
        *f.fail.lock().unwrap() = Some(PrintError::new("down"));
        assert!(matches!(run_job(&f, Job::Devices).as_slice(), [Done::Failed { refresh: false, .. }]));
        assert!(matches!(run_job(&f, Job::Refresh).as_slice(), [Done::Failed { refresh: false, .. }]));
    }

    #[test]
    fn add_and_save_report_the_printer() {
        let f = fake();
        let new = NewPrinter { name: "q".into(), uri: "socket://h".into(), info: String::new(), location: String::new(), driver: opusdm_printing::model::DriverChoice::Driverless, make_default: false };
        assert!(matches!(run_job(&f, Job::Add(Box::new(new))).as_slice(), [Done::Added(n), Done::Refreshed(s)] if n == "q" && s.printers.len() == 2));
        let edit = PrinterEdit { info: "Desk".into(), ..Default::default() };
        assert!(matches!(run_job(&f, Job::Save { printer: "p".into(), edit }).as_slice(), [Done::Saved(n), Done::Refreshed(_)] if n == "p"));
    }

    #[test]
    fn only_a_failed_save_is_a_failed_save() {
        let f = fake();
        *f.fail.lock().unwrap() = Some(PrintError::new("lpadmin: Forbidden"));
        let done = run_job(&f, Job::Save { printer: "p".into(), edit: PrinterEdit::default() });
        assert!(done[0].is_failed_save());
        assert!(!run_job(&f, Job::Refresh)[0].is_failed_save());
        assert!(!run_job(&f, Job::Remove("p".into()))[0].is_failed_save());
    }

    #[test]
    fn the_worker_runs_jobs_in_order() {
        let f = Arc::new(fake());
        let (tx, rx) = async_channel::unbounded();
        let jobs = spawn(f.clone(), tx);
        jobs.send(Job::Hold(41)).unwrap();
        jobs.send(Job::Release(41)).unwrap();
        let reports: Vec<Done> = (0..4).map(|_| rx.recv_blocking().unwrap()).collect();
        assert!(matches!(reports[0], Done::Acted(ref t) if t == "Job 41 held"));
        assert!(matches!(reports[2], Done::Acted(ref t) if t == "Job 41 released"));
        assert_eq!(f.calls()[0], "hold 41");
        assert_eq!(f.calls()[3], "release 41");
    }
}
