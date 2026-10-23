//! The requests sent to BlueZ, run on their own thread so the window never
//! waits for the bus. A pairing runs on a thread of its own: it can take a
//! minute, and a Cancel must not queue behind it.

use opusdm_bluetooth::{Backend, BtError, Request};
use std::sync::{mpsc, Arc};

/// Tag of the log lines.
const LOG_TAG: &str = "opusdm-bluetooth-manager";

/// What the window asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Run(Request),
    /// Pair the device, then trust and connect it.
    PairAndConnect(String),
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Done {
    /// A request failed.
    Failed(Request, BtError),
    /// A pairing ended: the device and the error, if any.
    Paired(String, Option<BtError>),
}

/// Pair `device`, then trust and connect it. A failed connect after a good
/// pairing is still a pairing: the device stays paired.
pub fn pair_and_connect(backend: &dyn Backend, device: &str) -> Option<BtError> {
    if let Err(e) = backend.run(&Request::Pair(device.to_string())) { return Some(e); }
    for r in [Request::SetTrusted(device.to_string(), true), Request::Connect(device.to_string())] {
        if let Err(e) = backend.run(&r) {
            opusdm_ipc::err_log!(LOG_TAG, "after pairing {device}: {r:?}: {}", e.text());
            return Some(e);
        }
    }
    None
}

/// Start the worker thread. Jobs run in order; failures go to `done`. The
/// thread ends when the job sender or the report receiver is dropped.
pub fn spawn(backend: Arc<dyn Backend>, done: async_channel::Sender<Done>) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new().name("opusdm-bluetooth-jobs".into()).spawn(move || {
        while let Ok(job) = rx.recv() {
            match job {
                Job::Run(request) => {
                    let Err(e) = backend.run(&request) else { continue };
                    opusdm_ipc::err_log!(LOG_TAG, "{request:?}: {}", e.text());
                    if done.send_blocking(Done::Failed(request, e)).is_err() { return; }
                }
                Job::PairAndConnect(device) => spawn_pairing(backend.clone(), device, done.clone()),
            }
        }
    });
    if let Err(e) = spawned { opusdm_ipc::err_log!(LOG_TAG, "cannot start the Bluetooth worker: {e}"); }
    tx
}

/// Run one pairing on its own thread and report how it ended.
fn spawn_pairing(backend: Arc<dyn Backend>, device: String, done: async_channel::Sender<Done>) {
    let spawned = std::thread::Builder::new().name("opusdm-bluetooth-pair".into()).spawn(move || {
        let error = pair_and_connect(&*backend, &device);
        if let Some(e) = &error { opusdm_ipc::err_log!(LOG_TAG, "pairing {device}: {}", e.text()); }
        if done.send_blocking(Done::Paired(device, error)).is_err() { opusdm_ipc::err_log!(LOG_TAG, "a pairing ended after the window was gone"); }
    });
    if let Err(e) = spawned { opusdm_ipc::err_log!(LOG_TAG, "cannot start a pairing thread: {e}"); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_bluetooth::FakeBackend;

    #[test]
    fn pairing_trusts_and_connects() {
        let f = FakeBackend::default();
        assert!(pair_and_connect(&f, "/d").is_none());
        assert_eq!(f.calls(), vec![Request::Pair("/d".into()), Request::SetTrusted("/d".into(), true), Request::Connect("/d".into())]);
    }

    #[test]
    fn a_failed_pairing_stops_there() {
        let f = FakeBackend::default();
        *f.fail.lock().unwrap() = Some(BtError::from_dbus("org.bluez.Error.AuthenticationFailed", ""));
        assert!(pair_and_connect(&f, "/d").is_some());
        assert_eq!(f.calls(), vec![Request::Pair("/d".into())]);
    }

    #[test]
    fn the_worker_reports_failures_and_pairings() {
        let f = Arc::new(FakeBackend::default());
        let (done_tx, done_rx) = async_channel::unbounded();
        let jobs = spawn(f.clone(), done_tx);
        jobs.send(Job::PairAndConnect("/d".into())).unwrap();
        assert!(matches!(done_rx.recv_blocking().unwrap(), Done::Paired(d, None) if d == "/d"));
        *f.fail.lock().unwrap() = Some(BtError::other("no"));
        jobs.send(Job::Run(Request::StopDiscovery)).unwrap();
        assert!(matches!(done_rx.recv_blocking().unwrap(), Done::Failed(Request::StopDiscovery, _)));
    }
}
