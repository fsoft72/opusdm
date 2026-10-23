//! A thread that polls CUPS for the printers and the queue and reports what
//! changed. Polling a local socket costs little; it slows down while no job
//! is in the queue.

use crate::backend::PrintBackend;
use crate::model::{Job, Printer};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Wait between two polls while a job is in the queue, in milliseconds.
pub const ACTIVE_POLL_MS: u64 = 2000;
/// Wait between two polls with an empty queue, in milliseconds.
pub const IDLE_POLL_MS: u64 = 5000;

/// The printers and the jobs not completed, at one moment.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub printers: Vec<Printer>,
    pub jobs: Vec<Job>,
}

impl Snapshot {
    /// Whether a job is still in a queue.
    pub fn has_active_jobs(&self) -> bool {
        self.jobs.iter().any(|j| j.state.is_active())
    }

    /// The active jobs of `printer`.
    pub fn jobs_of<'a>(&'a self, printer: &'a str) -> impl Iterator<Item = &'a Job> + 'a {
        self.jobs.iter().filter(move |j| j.printer == printer && j.state.is_active())
    }
}

/// What the watcher reports.
#[derive(Clone, Debug, PartialEq)]
pub enum WatchMsg {
    Snapshot(Snapshot),
    /// CUPS cannot be read, and why.
    Unavailable(String),
}

/// Read CUPS once. Returns the message to send when the result differs from
/// `last` (which is updated), `None` when nothing changed.
pub fn poll_once(backend: &dyn PrintBackend, last: &mut Option<Result<Snapshot, String>>) -> Option<WatchMsg> {
    let now = backend.printers().and_then(|printers| Ok(Snapshot { printers, jobs: backend.jobs()? })).map_err(|e| e.0);
    if last.as_ref() == Some(&now) { return None; }
    *last = Some(now.clone());
    match now {
        Ok(s) => Some(WatchMsg::Snapshot(s)),
        Err(why) => {
            opusdm_ipc::err_log!("opusdm-printing", "cannot read CUPS: {why}");
            Some(WatchMsg::Unavailable(why))
        }
    }
}

/// Start the watcher thread. The receiver gets every change; a `()` on the
/// returned sender polls at once (after an action). The thread ends when the
/// receiver is dropped.
pub fn spawn(backend: Arc<dyn PrintBackend>) -> (async_channel::Receiver<WatchMsg>, mpsc::Sender<()>) {
    let (tx, rx) = async_channel::unbounded();
    let (poke_tx, poke_rx) = mpsc::channel::<()>();
    let spawned = std::thread::Builder::new().name("opusdm-printing-watch".into()).spawn(move || {
        let mut last = None;
        loop {
            if let Some(msg) = poll_once(&*backend, &mut last) {
                if tx.send_blocking(msg).is_err() { return; }
            }
            if tx.is_closed() { return; }
            let active = matches!(&last, Some(Ok(s)) if s.has_active_jobs());
            let wait = Duration::from_millis(if active { ACTIVE_POLL_MS } else { IDLE_POLL_MS });
            // A poke or the timeout both mean "poll now"; a closed poke
            // channel only means nobody pokes any more.
            if let Err(mpsc::RecvTimeoutError::Disconnected) = poke_rx.recv_timeout(wait) {
                std::thread::sleep(wait);
            }
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-printing", "cannot start the CUPS watcher: {e}");
    }
    (rx, poke_tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{test_job, test_printer, FakeBackend};
    use crate::model::PrintError;

    #[test]
    fn poll_sends_only_changes() {
        let fake = FakeBackend::default();
        fake.printers.lock().unwrap().push(test_printer("p"));
        let mut last = None;
        let first = poll_once(&fake, &mut last);
        assert!(matches!(first, Some(WatchMsg::Snapshot(ref s)) if s.printers.len() == 1 && s.jobs.is_empty()));
        assert_eq!(poll_once(&fake, &mut last), None);
        fake.jobs.lock().unwrap().push(test_job(7, "p"));
        assert!(matches!(poll_once(&fake, &mut last), Some(WatchMsg::Snapshot(ref s)) if s.has_active_jobs()));
    }

    #[test]
    fn watcher_logs_unavailable_once() {
        let fake = FakeBackend::default();
        *fake.fail.lock().unwrap() = Some(PrintError::new("CUPS is not available"));
        let mut last = None;
        assert_eq!(poll_once(&fake, &mut last), Some(WatchMsg::Unavailable("CUPS is not available".into())));
        assert_eq!(poll_once(&fake, &mut last), None);
        *fake.fail.lock().unwrap() = None;
        assert!(matches!(poll_once(&fake, &mut last), Some(WatchMsg::Snapshot(_))));
    }

    #[test]
    fn the_thread_reports_and_stops() {
        let fake = Arc::new(FakeBackend::default());
        fake.printers.lock().unwrap().push(test_printer("p"));
        let (rx, poke) = spawn(fake.clone());
        assert!(matches!(rx.recv_blocking(), Ok(WatchMsg::Snapshot(_))));
        fake.jobs.lock().unwrap().push(test_job(1, "p"));
        poke.send(()).unwrap();
        assert!(matches!(rx.recv_blocking(), Ok(WatchMsg::Snapshot(s)) if s.jobs.len() == 1));
        drop(rx);
        poke.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let polls = fake.calls().iter().filter(|c| *c == "jobs").count();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.calls().iter().filter(|c| *c == "jobs").count(), polls, "the thread stopped polling");
    }
}
