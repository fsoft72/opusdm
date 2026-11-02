//! Runs the crate's GTK test bodies on one thread. GTK may only be
//! initialized and used on a single thread, but libtest runs every
//! `#[test]` on its own thread: with several GTK tests they all initialized
//! GTK at the same moment (a race the `gtk` crate does not catch) and then
//! used it from several threads at once, so a widget constructor sometimes
//! returned NULL. With `--test-threads=1` the race is gone and every GTK test
//! but the first panicked instead.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::OnceLock;

/// A test body and where to send how it ended.
type Job = (Box<dyn FnOnce() + Send>, mpsc::Sender<std::thread::Result<()>>);

/// The job queue of the GTK thread; `None` when GTK cannot start (no
/// display), in which case the GTK tests are skipped.
static GTK_THREAD: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();

/// Run `body` on the GTK thread and wait for it. A panic in `body` (a failed
/// assertion) is raised again here, so the calling test fails. Returns at
/// once, running nothing, when GTK cannot be initialized.
pub fn run(body: impl FnOnce() + Send + 'static) {
    let Some(jobs) = gtk_thread() else { return };
    let (done_tx, done_rx) = mpsc::channel();
    jobs.send((Box::new(body), done_tx)).expect("the GTK test thread is running");
    if let Err(panic) = done_rx.recv().expect("the GTK test thread answered") {
        panic::resume_unwind(panic);
    }
}

/// Start the GTK thread on first use: it initializes GTK once, then runs
/// the queued bodies one at a time for the rest of the test process.
fn gtk_thread() -> Option<&'static mpsc::Sender<Job>> {
    GTK_THREAD
        .get_or_init(|| {
            let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
            let (ready_tx, ready_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("gtk-tests".into())
                .spawn(move || {
                    let ready = gtk4::init().is_ok();
                    let _ = ready_tx.send(ready);
                    if !ready {
                        return;
                    }
                    for (body, done) in jobs_rx {
                        let _ = done.send(panic::catch_unwind(AssertUnwindSafe(body)));
                    }
                })
                .expect("spawn the GTK test thread");
            ready_rx.recv().unwrap_or(false).then_some(jobs_tx)
        })
        .as_ref()
}
