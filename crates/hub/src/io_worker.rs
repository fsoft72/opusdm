//! One background thread for the small blocking jobs of the GTK side (saving
//! the desktop icon positions and groups, reloading the compositor), so a
//! slow disk or a slow `pkill` never freezes the desktop. Jobs run one after
//! the other in submission order, so a later save always lands last.

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

/// A unit of blocking work handed to the worker.
type Job = Box<dyn FnOnce() + Send>;

/// Handle to the worker thread; clones share the same thread. Dropping the
/// last one lets the thread finish the jobs already queued and exit.
#[derive(Clone)]
pub struct IoWorker {
    /// `None` when the thread could not be started: jobs then run inline.
    tx: Option<Sender<Job>>,
}

impl IoWorker {
    /// Start the worker thread, named `name`. When the thread cannot be
    /// spawned the error is logged and every job runs on the caller's thread,
    /// so no save is ever lost.
    pub fn new(name: &str) -> IoWorker {
        let (tx, rx) = channel::<Job>();
        let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            for job in rx {
                job();
            }
        });
        match spawned {
            Ok(_) => IoWorker { tx: Some(tx) },
            Err(e) => {
                opusdm_ipc::err_log!("hub", "cannot spawn the {name} thread, its work runs inline: {e}");
                IoWorker { tx: None }
            }
        }
    }

    /// Queue `job`. It runs inline when the worker is not there (never
    /// started, or ended by a panicking job).
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let Some(tx) = &self.tx else { return job() };
        if let Err(back) = tx.send(Box::new(job)) {
            opusdm_ipc::err_log!("hub", "the background worker is gone, running its job inline");
            (back.0)();
        }
    }

    /// Wait until every job queued so far has run, at most `timeout`.
    /// Returns `false` on timeout (or when the worker died with jobs queued).
    pub fn flush(&self, timeout: Duration) -> bool {
        let Some(tx) = &self.tx else { return true };
        let (done_tx, done_rx) = channel::<()>();
        let marker: Job = Box::new(move || {
            let _ = done_tx.send(());
        });
        if tx.send(marker).is_err() { return false; }
        done_rx.recv_timeout(timeout).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn jobs_run_in_order_and_flush_waits_for_them() {
        let worker = IoWorker::new("opusdm-io-test");
        let seen = Arc::new(Mutex::new(Vec::new()));
        for i in 0..20 {
            let seen = seen.clone();
            worker.submit(move || {
                std::thread::sleep(Duration::from_millis(1));
                seen.lock().unwrap().push(i);
            });
        }
        assert!(worker.flush(Duration::from_secs(5)));
        assert_eq!(*seen.lock().unwrap(), (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn submit_does_not_wait_for_the_job() {
        let worker = IoWorker::new("opusdm-io-test");
        let (gate_tx, gate_rx) = channel::<()>();
        let start = std::time::Instant::now();
        worker.submit(move || {
            let _ = gate_rx.recv_timeout(Duration::from_secs(5));
        });
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!worker.flush(Duration::from_millis(50)), "the job is still blocked");
        gate_tx.send(()).unwrap();
        assert!(worker.flush(Duration::from_secs(5)));
    }
}
