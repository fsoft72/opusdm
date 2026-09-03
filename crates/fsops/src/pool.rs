//! A fixed-size worker pool, used to copy many small files at once.
//!
//! Deliberately minimal: a bounded set of OS threads pulling from one shared
//! channel. No work stealing, no async runtime, no extra dependency. Copying a
//! `node_modules` or `target/` tree is bound by per-file metadata latency, not
//! bandwidth, so a handful of threads is enough to saturate the device.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Largest file still handed to the pool.
///
/// Bigger files are copied inline by the orchestrating thread so one long
/// sequential transfer is not interleaved with dozens of others, which would
/// fragment I/O on rotational media.
pub const SMALL_FILE_MAX: u64 = 4 * 1024 * 1024;

/// Upper bound on pool threads, whatever the core count.
pub const MAX_COPY_WORKERS: usize = 8;

/// Environment override for [`worker_count`]. `1` forces the old fully
/// sequential behaviour, which is handy for benchmarking and bug reports.
pub const WORKERS_ENV: &str = "OPUSDM_COPY_WORKERS";

/// How many copy workers to run: one per core, clamped to `[1, MAX_COPY_WORKERS]`.
/// A valid `OPUSDM_COPY_WORKERS` wins over the detected core count.
pub fn worker_count() -> usize {
    if let Some(n) = std::env::var(WORKERS_ENV).ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.clamp(1, MAX_COPY_WORKERS);
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_COPY_WORKERS)
}

/// A fixed set of threads consuming items of type `T` from a shared queue.
///
/// Dropping the pool detaches the workers; call [`join`](Self::join) to close
/// the queue and wait for every queued item to be processed.
pub struct WorkerPool<T> {
    tx: Option<Sender<T>>,
    handles: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> WorkerPool<T> {
    /// Spawn `threads` workers named `{name}-{i}`, each running `job` on every
    /// item it dequeues. `threads` is forced to at least 1.
    pub fn new<F>(threads: usize, name: &str, job: F) -> Self
    where
        F: Fn(T) + Send + Sync + 'static,
    {
        let threads = threads.max(1);
        let (tx, rx) = channel::<T>();
        let rx = Arc::new(Mutex::new(rx));
        let job = Arc::new(job);
        let mut handles = Vec::with_capacity(threads);

        for i in 0..threads {
            let rx = Arc::clone(&rx);
            let job = Arc::clone(&job);
            let handle = std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || loop {
                    // One worker parks on `recv` holding the lock; the others
                    // queue on the mutex. Handing items over is serialised,
                    // running them is not.
                    let item = match rx.lock() {
                        Ok(guard) => guard.recv(),
                        Err(_) => return,
                    };
                    match item {
                        Ok(item) => job(item),
                        Err(_) => return,
                    }
                })
                .expect("spawn worker thread");
            handles.push(handle);
        }

        Self { tx: Some(tx), handles }
    }

    /// Queue one item. Silently drops it if every worker has already exited.
    pub fn submit(&self, item: T) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(item);
    }

    /// Close the queue and wait for every worker to drain and exit.
    pub fn join(mut self) {
        self.tx = None;
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn worker_count_is_within_bounds() {
        let n = worker_count();
        assert!((1..=MAX_COPY_WORKERS).contains(&n));
    }

    #[test]
    fn every_submitted_item_runs_before_join_returns() {
        let seen = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::clone(&seen);
        let pool = WorkerPool::new(4, "test-pool", move |n: usize| {
            s2.fetch_add(n, Ordering::SeqCst);
        });
        for _ in 0..1000 {
            pool.submit(1);
        }
        pool.join();
        assert_eq!(seen.load(Ordering::SeqCst), 1000);
    }

    #[test]
    fn zero_threads_is_clamped_to_one() {
        let seen = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::clone(&seen);
        let pool = WorkerPool::new(0, "test-pool-zero", move |n: usize| {
            s2.fetch_add(n, Ordering::SeqCst);
        });
        pool.submit(7);
        pool.join();
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }
}
