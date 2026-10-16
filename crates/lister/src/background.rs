//! Background work for the Lister: blocking work runs off the GTK thread and
//! its result comes back on it, optionally with a [`CancelToken`]; child
//! processes the Lister starts and forgets are reaped on a thread.
//!
//! - [`run_blocking`] / [`run_cancellable`]: one result, on GIO's worker
//!   pool (short jobs: a listing, an extraction, a connection).
//! - [`run_streaming`]: many results, on a dedicated thread (long scans that
//!   must not hold a pool thread: directory sizes, checksums, transfers).
//! - [`reap`]: wait for a child process so it does not stay a zombie.

use gtk4::gio;
use gtk4::glib;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Name of the threads [`run_streaming`] starts.
const STREAM_THREAD_NAME: &str = "opusdm-work";
/// Name of the threads [`reap`] starts.
const REAP_THREAD_NAME: &str = "opusdm-reap";

/// A cancel flag shared by the GTK thread and a worker. Cloning shares it.
#[derive(Clone, Default, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the work to stop; its results are no longer delivered.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether [`CancelToken::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// The flag itself, for APIs that poll an `AtomicBool`.
    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
}

/// Run `work` off the GTK thread and call `on_done` with its result on the
/// GTK thread.
pub fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, on_done: impl FnOnce(T) + 'static) {
    run_cancellable(&CancelToken::new(), move |_| work(), on_done);
}

/// [`run_blocking`] with a cancel token: `work` can poll it, and `on_done`
/// is not called once it is cancelled. A worker that panics is logged.
pub fn run_cancellable<T: Send + 'static>(
    cancel: &CancelToken,
    work: impl FnOnce(&CancelToken) -> T + Send + 'static,
    on_done: impl FnOnce(T) + 'static,
) {
    let worker_cancel = cancel.clone();
    let cancel = cancel.clone();
    glib::spawn_future_local(async move {
        if cancel.is_cancelled() { return; }
        let result = gio::spawn_blocking(move || {
            if worker_cancel.is_cancelled() { return None; }
            Some(work(&worker_cancel))
        })
        .await;
        match result {
            Ok(Some(value)) if !cancel.is_cancelled() => on_done(value),
            Ok(_) => {}
            Err(_) => opusdm_ipc::err_log!("opusdm-lister", "a background worker panicked"),
        }
    });
}

/// The worker side of [`run_streaming`]: hands results to the GTK thread.
pub struct Emitter<T> {
    tx: async_channel::Sender<T>,
    cancel: CancelToken,
}

impl<T> Emitter<T> {
    /// Hand `item` to the GTK thread. `false` once the work was cancelled or
    /// nobody listens any more: the worker should stop.
    pub fn emit(&self, item: T) -> bool {
        !self.cancel.is_cancelled() && self.tx.send_blocking(item).is_ok()
    }

    /// The token of this work, for APIs that poll an `AtomicBool`.
    pub fn cancel(&self) -> &CancelToken {
        &self.cancel
    }
}

/// Run `work` on its own thread; every item it emits reaches `on_item` on the
/// GTK thread, and `on_end` runs once the worker returned. After `cancel` is
/// cancelled neither is called again.
pub fn run_streaming<T: Send + 'static>(
    cancel: &CancelToken,
    work: impl FnOnce(&Emitter<T>) + Send + 'static,
    mut on_item: impl FnMut(T) + 'static,
    on_end: impl FnOnce() + 'static,
) {
    let (tx, rx) = async_channel::unbounded::<T>();
    let emitter = Emitter { tx, cancel: cancel.clone() };
    let spawned = std::thread::Builder::new().name(STREAM_THREAD_NAME.to_string()).spawn(move || work(&emitter));
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-lister", "cannot start a worker thread: {e}");
        return;
    }
    let cancel = cancel.clone();
    glib::spawn_future_local(async move {
        while let Ok(item) = rx.recv().await {
            if cancel.is_cancelled() { return; }
            on_item(item);
        }
        if !cancel.is_cancelled() { on_end(); }
    });
}

/// Wait for `child` on a thread so it is reaped when it exits. `what` names
/// it in the log when the wait fails.
pub fn reap(mut child: Child, what: &str) {
    let what = what.to_string();
    let spawned = std::thread::Builder::new().name(REAP_THREAD_NAME.to_string()).spawn(move || {
        if let Err(e) = child.wait() {
            opusdm_ipc::err_log!("opusdm-lister", "cannot wait for {what}: {e}");
        }
    });
    if let Err(e) = spawned {
        opusdm_ipc::err_log!("opusdm-lister", "cannot start a thread to reap a child process: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// How long a test drives its main context before giving up.
    const TEST_WAIT: Duration = Duration::from_secs(5);

    /// Iterate `ctx` until `done()` or the timeout.
    fn drive(ctx: &glib::MainContext, done: impl Fn() -> bool) {
        let deadline = Instant::now() + TEST_WAIT;
        while !done() && Instant::now() < deadline {
            ctx.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn the_result_comes_back_on_the_calling_thread() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let got = Rc::new(Cell::new(None));
            let caller = std::thread::current().id();
            let seen = got.clone();
            run_blocking(|| 6 * 7, move |v| {
                assert_eq!(std::thread::current().id(), caller);
                seen.set(Some(v));
            });
            drive(&ctx, || got.get().is_some());
            assert_eq!(got.get(), Some(42));
        })
        .unwrap();
    }

    #[test]
    fn a_cancelled_job_does_not_deliver() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let cancel = CancelToken::new();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let delivered = Rc::new(Cell::new(false));
            let seen = delivered.clone();
            run_cancellable(&cancel, move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                1
            }, move |_| seen.set(true));
            drive(&ctx, || started_rx.try_recv().is_ok());
            cancel.cancel();
            release_tx.send(()).unwrap();
            // Give the result every chance to arrive.
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                ctx.iteration(false);
            }
            assert!(!delivered.get());
        })
        .unwrap();
    }

    #[test]
    fn streamed_items_arrive_in_order_then_the_end() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let items = Rc::new(RefCell::new(Vec::new()));
            let ended = Rc::new(Cell::new(false));
            let (seen, end) = (items.clone(), ended.clone());
            run_streaming(&CancelToken::new(), |out| {
                for i in 0..5 {
                    out.emit(i);
                }
            }, move |i| seen.borrow_mut().push(i), move || end.set(true));
            drive(&ctx, || ended.get());
            assert_eq!(*items.borrow(), vec![0, 1, 2, 3, 4]);
        })
        .unwrap();
    }

    #[test]
    fn a_cancelled_stream_stops_the_worker_and_skips_the_end() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let cancel = CancelToken::new();
            let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
            let ended = Rc::new(Cell::new(false));
            let end = ended.clone();
            cancel.cancel();
            run_streaming(&cancel, move |out| {
                let accepted = out.emit(1);
                stopped_tx.send((accepted, out.cancel().is_cancelled())).unwrap();
            }, |_: i32| panic!("no item after a cancel"), move || end.set(true));
            let (accepted, cancelled) = stopped_rx.recv_timeout(TEST_WAIT).unwrap();
            assert!(!accepted && cancelled);
            let deadline = Instant::now() + Duration::from_millis(200);
            while Instant::now() < deadline {
                ctx.iteration(false);
            }
            assert!(!ended.get());
        })
        .unwrap();
    }

    #[test]
    fn a_reaped_child_does_not_stay_a_zombie() {
        let child = std::process::Command::new("true").spawn().unwrap();
        let proc_entry = std::path::PathBuf::from(format!("/proc/{}", child.id()));
        reap(child, "true");
        let deadline = Instant::now() + TEST_WAIT;
        // A zombie keeps its /proc entry until it is reaped.
        while Instant::now() < deadline {
            if !proc_entry.exists() { return; }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the child was not reaped");
    }
}
