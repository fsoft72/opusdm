//! Bookkeeping of the Function runs the hub knows about: the runs still
//! waiting on SOURCE / DEST selections, which Lister started each running
//! Function and how to cancel it. Pure state, no connections or threads, so
//! it can be tested without a whole `Hub`.

use super::function_run::PendingFunctionRun;
use crate::functions::FnEvent;
use opusdm_fsops::CancelToken;
use opusdm_ipc::function::SelectionCtx;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Every Function-run map of the hub, grouped.
pub(super) struct FunctionRuns {
    /// Function runs waiting on `SelectionReply` from SOURCE and/or DEST.
    pending: HashMap<u64, PendingFunctionRun>,
    /// Which pending runs a Lister connection's next `SelectionReply`s are
    /// for, oldest first: a Lister answers `QuerySelection`s in order, so two
    /// runs waiting on it at once are matched by a FIFO, not a single slot.
    waiters: HashMap<u64, VecDeque<u64>>,
    /// Which connection to route a running Function's events back to, kept
    /// alive from the moment execution starts until `FunctionDone`/`Failed`.
    requesters: HashMap<u64, Option<u64>>,
    /// Set when a run's requester goes away: the run stops at its next step.
    cancels: HashMap<u64, CancelToken>,
    next_id: u64,
    /// Where the executor threads send their events.
    events_tx: Sender<(u64, FnEvent)>,
}

impl FunctionRuns {
    /// No runs yet; executors will report on `events_tx`.
    pub(super) fn new(events_tx: Sender<(u64, FnEvent)>) -> Self {
        Self { pending: HashMap::new(), waiters: HashMap::new(), requesters: HashMap::new(), cancels: HashMap::new(), next_id: 1, events_tx }
    }

    /// A fresh run id (ids only ever grow).
    pub(super) fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The sender executors report their events on.
    pub(super) fn events_tx(&self) -> Sender<(u64, FnEvent)> {
        self.events_tx.clone()
    }

    /// Park `run` until its selections arrive.
    pub(super) fn add_pending(&mut self, run_id: u64, run: PendingFunctionRun) {
        self.pending.insert(run_id, run);
    }

    /// Queue `run_id` on `cid`'s FIFO of selection requests.
    pub(super) fn wait_selection(&mut self, cid: u64, run_id: u64) {
        self.waiters.entry(cid).or_default().push_back(run_id);
    }

    /// Record that run `run_id` is executing for `requester` and return the
    /// token that cancels it.
    pub(super) fn start(&mut self, run_id: u64, requester: Option<u64>) -> CancelToken {
        self.requesters.insert(run_id, requester);
        let cancel = CancelToken::new();
        self.cancels.insert(run_id, cancel.clone());
        cancel
    }

    /// The connection a running Function reports to: `None` when the run is
    /// unknown, `Some(None)` for a run the hub started on its own.
    pub(super) fn requester(&self, run_id: u64) -> Option<Option<u64>> {
        self.requesters.get(&run_id).copied()
    }

    /// Run `run_id` ended: forget its requester and cancel token.
    pub(super) fn finish(&mut self, run_id: u64) {
        self.requesters.remove(&run_id);
        self.cancels.remove(&run_id);
    }

    /// `cid` answered a selection query. Fill the slot of the run it answers
    /// and, once that run has every selection it needs, remove and return it.
    pub(super) fn take_selection(&mut self, cid: u64, paths: Vec<PathBuf>, cwd: PathBuf) -> Option<(u64, PendingFunctionRun)> {
        let queue = self.waiters.get_mut(&cid)?;
        let run_id = queue.pop_front()?;
        if queue.is_empty() {
            self.waiters.remove(&cid);
        }
        let run = self.pending.get_mut(&run_id)?;
        let sel = SelectionCtx { paths, cwd };
        if cid == run.source_cid {
            run.source_sel = Some(sel);
        } else if Some(cid) == run.dest_cid {
            run.dest_sel = Some(sel);
        }
        let ready = run.source_sel.is_some() && (run.dest_cid.is_none() || run.dest_sel.is_some());
        if !ready { return None; }
        self.pending.remove(&run_id).map(|run| (run_id, run))
    }

    /// Connection `cid` dropped. Pending runs waiting on it can never
    /// complete and are removed; running Functions it started are cancelled.
    /// Returns the ids of the removed pending runs.
    pub(super) fn forget_connection(&mut self, cid: u64) -> Vec<u64> {
        let doomed: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, run)| run.source_cid == cid || run.dest_cid == Some(cid))
            .map(|(run_id, _)| *run_id)
            .collect();
        for run_id in &doomed {
            self.pending.remove(run_id);
            for queue in self.waiters.values_mut() {
                queue.retain(|r| r != run_id);
            }
        }
        self.waiters.remove(&cid);
        self.waiters.retain(|_, queue| !queue.is_empty());
        for (run_id, requester) in &self.requesters {
            if *requester == Some(cid) {
                if let Some(cancel) = self.cancels.remove(run_id) {
                    cancel.cancel();
                }
            }
        }
        self.requesters.retain(|_, requester| *requester != Some(cid));
        doomed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs() -> FunctionRuns {
        FunctionRuns::new(std::sync::mpsc::channel().0)
    }

    fn pending(source_cid: u64, dest_cid: Option<u64>) -> PendingFunctionRun {
        PendingFunctionRun { requester_cid: Some(source_cid), steps: Vec::new(), source_cid, dest_cid, source_sel: None, dest_sel: None }
    }

    #[test]
    fn ids_only_grow() {
        let mut r = runs();
        assert_eq!((r.next_id(), r.next_id()), (1, 2));
    }

    #[test]
    fn a_run_is_ready_once_source_and_dest_answered() {
        let mut r = runs();
        r.add_pending(7, pending(1, Some(2)));
        r.wait_selection(1, 7);
        r.wait_selection(2, 7);
        assert!(r.take_selection(1, vec![PathBuf::from("/a")], PathBuf::from("/")).is_none());
        let (id, run) = r.take_selection(2, Vec::new(), PathBuf::from("/d")).expect("ready");
        assert_eq!(id, 7);
        assert_eq!(run.source_sel.unwrap().paths, vec![PathBuf::from("/a")]);
        assert_eq!(run.dest_sel.unwrap().cwd, PathBuf::from("/d"));
    }

    #[test]
    fn selections_of_one_lister_are_matched_in_order() {
        let mut r = runs();
        r.add_pending(1, pending(5, None));
        r.add_pending(2, pending(5, None));
        r.wait_selection(5, 1);
        r.wait_selection(5, 2);
        assert_eq!(r.take_selection(5, Vec::new(), PathBuf::new()).map(|(id, _)| id), Some(1));
        assert_eq!(r.take_selection(5, Vec::new(), PathBuf::new()).map(|(id, _)| id), Some(2));
        assert!(r.take_selection(5, Vec::new(), PathBuf::new()).is_none());
    }

    #[test]
    fn a_dropped_connection_removes_its_pending_runs_and_cancels_its_running_ones() {
        let mut r = runs();
        r.add_pending(1, pending(3, Some(4)));
        r.wait_selection(3, 1);
        r.wait_selection(4, 1);
        let token = r.start(9, Some(4));
        assert_eq!(r.forget_connection(4), vec![1]);
        assert!(token.is_cancelled());
        assert_eq!(r.requester(9), None);
        // The SOURCE side no longer waits for the dropped run.
        assert!(r.take_selection(3, Vec::new(), PathBuf::new()).is_none());
    }

    #[test]
    fn finish_forgets_the_requester() {
        let mut r = runs();
        r.start(4, None);
        assert_eq!(r.requester(4), Some(None));
        r.finish(4);
        assert_eq!(r.requester(4), None);
    }
}
