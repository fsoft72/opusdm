//! The hub's job engine. Validates requests, assigns ids, caps concurrency at
//! `MAX_CONCURRENT`, and turns `opusdm_fsops::Progress` into `ToLister`
//! messages. All file I/O runs on `std::thread`s spawned here.

use opusdm_fsops::{CancelToken, JobSpec, Progress};
use opusdm_ipc::{ListerId, Op, Outcome, ToLister};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Most jobs allowed to run at once; the rest queue.
pub const MAX_CONCURRENT: usize = 4;

/// The function that actually performs a job. Injected so tests can fake it.
/// It must emit its `opusdm_fsops::Progress` events on `tx` (untagged - the
/// caller has no job id to tag with) and end with exactly one
/// `Progress::Done`. `JobEngine::start` owns tagging each event with the
/// real job id as it relays it onward, so the runner itself never has to.
pub type Runner = Arc<dyn Fn(JobSpec, Sender<Progress>, CancelToken) + Send + Sync>;

/// Result of [`JobEngine::submit`].
pub struct SubmitOutcomeStarted {
    /// Id assigned to the job.
    pub job_id: u64,
    /// `true` if it started now, `false` if it was queued.
    pub started: bool,
}

/// Reject obviously bad job parameters before scanning. `Ok(())` means proceed.
pub fn validate_job(srcs: &[PathBuf], dest_dir: &Path) -> Result<(), String> {
    if srcs.is_empty() {
        return Err("no source files selected".into());
    }
    for s in srcs {
        if s == dest_dir {
            return Err(format!("source and destination are the same: {}", s.display()));
        }
        if dest_dir.starts_with(s) {
            return Err(format!("destination is inside the source: {}", s.display()));
        }
        if s.parent() == Some(dest_dir) {
            return Err("selected items are already in the destination".into());
        }
    }
    Ok(())
}

/// Whether `path` is an ftp://, ftps:// or sftp:// location.
pub fn is_remote_path(path: &Path) -> bool {
    let text = path.to_string_lossy().to_ascii_lowercase();
    ["ftp://", "ftps://", "sftp://"].iter().any(|p| text.starts_with(p))
}

/// Validate the parameters of `op`. Copy and Move refuse in-place transfers;
/// the "As" variants and Duplicate legitimately land in the source directory,
/// so they only need a non-empty selection, as do Compress, Extract, Encrypt and Decrypt (their
/// output names differ from the source names). Other ops need no check. Remote
/// locations are refused for every op.
pub fn validate_op(op: &Op, srcs: &[PathBuf], dest_dir: &Path) -> Result<(), String> {
    // The job engine only works on local files. Remote (ftp://, ftps://,
    // sftp://) jobs are handed back to the Lister before they get here.
    if std::iter::once(dest_dir).chain(srcs.iter().map(PathBuf::as_path)).any(is_remote_path) {
        return Err("remote locations are read-only: copy from them to a local DEST".into());
    }
    match op {
        Op::Copy | Op::Move => validate_job(srcs, dest_dir),
        Op::CopyAs { .. }
        | Op::MoveAs { .. }
        | Op::Duplicate
        | Op::Link { .. }
        | Op::Join { .. }
        | Op::Split { .. }
        | Op::Compress { .. }
        | Op::Extract
        | Op::Encrypt { .. }
        | Op::Decrypt { .. }
            if srcs.is_empty() =>
        {
            Err("no source files selected".into())
        }
        _ => Ok(()),
    }
}

struct RunningJob {
    requester: Option<ListerId>,
    cancel: CancelToken,
}

struct QueuedJob {
    job_id: u64,
    requester: Option<ListerId>,
    spec: JobSpec,
}

/// Owns job bookkeeping. Not `Sync`; lives on the hub's owner thread.
pub struct JobEngine {
    runner: Runner,
    progress_tx: Sender<(u64, Progress)>,
    running: HashMap<u64, RunningJob>,
    queue: VecDeque<QueuedJob>,
    next_id: u64,
}

impl JobEngine {
    /// Create an engine that runs jobs via `runner`, forwarding progress on `progress_tx`.
    pub fn new(runner: Runner, progress_tx: Sender<(u64, Progress)>) -> Self {
        Self {
            runner,
            progress_tx,
            running: HashMap::new(),
            queue: VecDeque::new(),
            next_id: 1,
        }
    }

    /// Number of jobs currently executing.
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Which Lister asked for `job_id`; `None` when the hub itself did, or the
    /// engine no longer knows the job.
    pub fn requester_of(&self, job_id: u64) -> Option<ListerId> {
        self.running
            .get(&job_id)
            .map(|r| r.requester)
            .or_else(|| self.queue.iter().find(|q| q.job_id == job_id).map(|q| q.requester))
            .flatten()
    }

    /// Accept a job. Starts it if under the cap, else queues it.
    pub fn submit(&mut self, requester: Option<ListerId>, spec: JobSpec) -> SubmitOutcomeStarted {
        let job_id = self.next_id;
        self.next_id += 1;
        if self.running.len() < MAX_CONCURRENT {
            self.start(job_id, requester, spec);
            SubmitOutcomeStarted { job_id, started: true }
        } else {
            self.queue.push_back(QueuedJob { job_id, requester, spec });
            SubmitOutcomeStarted { job_id, started: false }
        }
    }

    /// Note that `job_id` finished; start the next queued job if any.
    /// Returns the id of the job that was started, if one was. A job that
    /// never ran (a cancelled queued one) frees no slot, so it starts nothing.
    pub fn on_done(&mut self, job_id: u64) -> Option<u64> {
        self.running.remove(&job_id)?;
        let QueuedJob { job_id: next_id, requester, spec } = self.queue.pop_front()?;
        self.start(next_id, requester, spec);
        Some(next_id)
    }

    /// Cancel a job. A running job's token is set; a queued job is discarded
    /// and reported as a cancelled `Done`, so whoever tracks it (the hub's
    /// `JobMeta`, a waiting Function step) sees it end like any other job.
    pub fn cancel(&mut self, job_id: u64) {
        if let Some(r) = self.running.get(&job_id) {
            r.cancel.cancel();
            return;
        }
        let before = self.queue.len();
        self.queue.retain(|q| q.job_id != job_id);
        if self.queue.len() == before { return; }
        let done = Progress::Done { copied: 0, skipped: 0, cancelled: true, errors: Vec::new() };
        if self.progress_tx.send((job_id, done)).is_err() {
            opusdm_ipc::err_log!("hub", "job {job_id}: cancelled while queued, but nobody reads job progress");
        }
    }

    fn start(&mut self, job_id: u64, requester: Option<ListerId>, spec: JobSpec) {
        let cancel = CancelToken::new();
        self.running.insert(job_id, RunningJob { requester, cancel: cancel.clone() });
        let runner = Arc::clone(&self.runner);
        let shared_tx = self.progress_tx.clone();
        std::thread::Builder::new()
            .name(format!("opusdm-job-{job_id}"))
            .spawn(move || {
                // One relay thread re-tags every event with this job's id as
                // the runner produces them (it has no job id to tag with
                // itself). This is the only extra thread: the runner call
                // below is synchronous, so something else has to be reading
                // concurrently for progress to reach the UI live.
                let (raw_tx, raw_rx) = std::sync::mpsc::channel::<Progress>();
                let relay = std::thread::spawn(move || {
                    for ev in raw_rx {
                        if shared_tx.send((job_id, ev)).is_err() {
                            break;
                        }
                    }
                });
                runner(spec, raw_tx, cancel);
                let _ = relay.join();
            })
            .expect("spawn job thread");
    }
}

/// The production runner: plain `opusdm_fsops::run_job`. Tagging with the
/// job id happens in `JobEngine::start`, so this needs no thread of its own.
pub fn default_runner() -> Runner {
    Arc::new(|spec: JobSpec, tx: Sender<Progress>, cancel: CancelToken| {
        opusdm_fsops::run_job(spec, tx, cancel);
    })
}

/// Running byte/file totals learned from the `Scanning` event, reused to fill
/// later `JobProgress` messages.
#[derive(Default)]
pub struct ProgressTotals {
    /// Total bytes to process (from `Scanning`).
    pub bytes_total: u64,
    /// Total files to process (from `Scanning`).
    pub files_total: u64,
}

/// Convert a `Progress` into the `ToLister` message for the requester.
/// `Conflict` returns `None` - the server layer handles it (it owns `req_id`
/// and the reply channel).
pub fn map_progress(
    job_id: u64,
    op: &Op,
    ev: &Progress,
    totals: &mut ProgressTotals,
) -> Option<ToLister> {
    match ev {
        Progress::Scanning { files, bytes } => {
            totals.files_total = *files;
            totals.bytes_total = *bytes;
            Some(ToLister::JobProgress {
                job_id,
                op: op.clone(),
                current_file: PathBuf::new(),
                bytes_done: 0,
                bytes_total: *bytes,
                files_done: 0,
                files_total: *files,
                file_bytes_done: 0,
                file_bytes_total: 0,
            })
        }
        Progress::File { path, done_bytes, files_done, file_bytes_done, file_bytes_total, .. } => {
            Some(ToLister::JobProgress {
                job_id,
                op: op.clone(),
                current_file: path.clone(),
                bytes_done: *done_bytes,
                bytes_total: totals.bytes_total,
                files_done: *files_done,
                files_total: totals.files_total,
                file_bytes_done: *file_bytes_done,
                file_bytes_total: *file_bytes_total,
            })
        }
        Progress::Conflict { .. } => None,
        Progress::Done { copied, skipped, cancelled, errors } => {
            let outcome = if *cancelled {
                Outcome::Cancelled { copied: *copied, skipped: *skipped }
            } else {
                Outcome::Ok { copied: *copied, skipped: *skipped }
            };
            Some(ToLister::JobDone { job_id, outcome, errors: errors.clone() })
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_locations_are_refused_as_source_or_destination() {
        let local = PathBuf::from("/tmp/a");
        let remote = PathBuf::from("sftp://me@host/data/a");
        assert!(validate_op(&Op::Copy, std::slice::from_ref(&local), Path::new("sftp://host/dest")).is_err());
        assert!(validate_op(&Op::Delete, std::slice::from_ref(&remote), Path::new("/tmp")).is_err());
        assert!(validate_op(&Op::Copy, &[PathBuf::from("FTP://h/x")], Path::new("/tmp")).is_err());
        assert!(validate_op(&Op::Copy, &[local], Path::new("/other")).is_ok());
    }

    use super::*;
    use opusdm_ipc::Op;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Barrier};

    fn spec() -> JobSpec {
        JobSpec {
            op: Op::Copy,
            srcs: vec![PathBuf::from("/src/a")],
            dest_dir: PathBuf::from("/dst"),
            options: Default::default(),
        }
    }

    #[test]
    fn validate_rejects_empty_sources() {
        assert!(validate_job(&[], Path::new("/dst")).is_err());
    }

    #[test]
    fn validate_rejects_dest_inside_source() {
        let src = PathBuf::from("/home/x/dir");
        assert!(validate_job(std::slice::from_ref(&src), &src.join("child")).is_err());
    }

    #[test]
    fn validate_rejects_source_equals_dest() {
        let p = PathBuf::from("/home/x/dir");
        assert!(validate_job(std::slice::from_ref(&p), &p).is_err());
    }

    #[test]
    fn validate_rejects_same_parent() {
        assert!(validate_job(&[PathBuf::from("/a/b/f.txt")], Path::new("/a/b")).is_err());
    }

    #[test]
    fn validate_accepts_normal_case() {
        assert!(validate_job(&[PathBuf::from("/a/b/f.txt")], Path::new("/a/c")).is_ok());
    }

    #[test]
    fn concurrency_cap_queues_beyond_four() {
        let gate = Arc::new(Barrier::new(2));
        let g2 = gate.clone();
        let runner: Runner = Arc::new(move |_spec, tx, _cancel| {
            g2.wait();
            g2.wait();
            let _ = tx;
        });
        let (ptx, _prx) = channel::<(u64, Progress)>();
        let mut eng = JobEngine::new(runner, ptx);

        let mut ids = Vec::new();
        for _ in 0..6 {
            ids.push(eng.submit(Some(ListerId(1)), spec()));
        }
        let started = ids.iter().filter(|r| r.started).count();
        assert_eq!(started, MAX_CONCURRENT);
        assert_eq!(eng.running_count(), MAX_CONCURRENT);

        gate.wait();
        gate.wait();
        let first_done = ids[0].job_id;
        let next = eng.on_done(first_done);
        assert!(next.is_some(), "a queued job should start");
        assert_eq!(eng.running_count(), MAX_CONCURRENT);
    }

    #[test]
    fn cancelling_a_queued_job_reports_done_cancelled() {
        let gate = Arc::new(Barrier::new(MAX_CONCURRENT + 1));
        let g2 = gate.clone();
        let runner: Runner = Arc::new(move |_spec, _tx, _cancel| {
            g2.wait();
        });
        let (ptx, prx) = channel::<(u64, Progress)>();
        let mut eng = JobEngine::new(runner, ptx);
        for _ in 0..MAX_CONCURRENT {
            assert!(eng.submit(None, spec()).started);
        }
        let queued = eng.submit(None, spec());
        assert!(!queued.started);

        eng.cancel(queued.job_id);
        let (job_id, ev) = prx.recv_timeout(std::time::Duration::from_secs(5)).expect("a Done for the queued job");
        assert_eq!(job_id, queued.job_id);
        assert!(matches!(ev, Progress::Done { cancelled: true, copied: 0, skipped: 0, .. }));
        assert_eq!(eng.requester_of(queued.job_id), None);

        // The hub answers that Done with `on_done`: a job that never ran must
        // not start another one past the cap.
        assert_eq!(eng.on_done(queued.job_id), None);
        assert_eq!(eng.running_count(), MAX_CONCURRENT);
        gate.wait();
    }

    #[test]
    fn requester_is_tracked() {
        let runner: Runner = Arc::new(|_s, _tx, _c| {});
        let (ptx, _prx) = channel::<(u64, Progress)>();
        let mut eng = JobEngine::new(runner, ptx);
        let r = eng.submit(Some(ListerId(7)), spec());
        assert_eq!(eng.requester_of(r.job_id), Some(ListerId(7)));
    }

    #[test]
    fn map_progress_scanning_and_done() {
        let mut totals = ProgressTotals::default();
        let out = map_progress(
            3,
            &Op::Copy,
            &Progress::Scanning { files: 4, bytes: 40 },
            &mut totals,
        );
        match out {
            Some(ToLister::JobProgress { job_id, files_total, bytes_total, .. }) => {
                assert_eq!(job_id, 3);
                assert_eq!((files_total, bytes_total), (4, 40));
            }
            _ => panic!("expected JobProgress"),
        }
        let done = map_progress(
            3,
            &Op::Copy,
            &Progress::Done { copied: 4, skipped: 0, cancelled: false, errors: vec![] },
            &mut totals,
        );
        assert!(matches!(done, Some(ToLister::JobDone { job_id: 3, .. })));
    }
}
