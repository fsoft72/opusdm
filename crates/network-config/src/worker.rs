//! The jobs sent to NetworkManager, run on their own thread so the window
//! never waits for the bus.

use opusdm_network::import::{self, PendingFile};
use opusdm_network::profile::Profile;
use opusdm_network::settings::Settings;
use opusdm_network::{Loaded, NetError, ProfileBackend, ProfileSummary};
use std::sync::{mpsc, Arc};

/// What the window asks for.
pub enum Job {
    List,
    Load(String),
    /// Store a profile; `files` are written first (an import's certificates).
    Save { profile: Box<Profile>, base: Option<Settings>, files: Vec<PendingFile> },
    Delete(String),
    Activate { uuid: String, password: Option<String> },
    Deactivate(String),
}

// A password must never reach a log through `{:?}`.
impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Job::List => write!(f, "List"),
            Job::Load(u) => write!(f, "Load({u})"),
            Job::Save { profile, base, files } => write!(f, "Save({}, new: {}, files: {})", profile.uuid, base.is_none(), files.len()),
            Job::Delete(u) => write!(f, "Delete({u})"),
            Job::Activate { uuid, password } => write!(f, "Activate({uuid}, password: {})", password.is_some()),
            Job::Deactivate(u) => write!(f, "Deactivate({u})"),
        }
    }
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Done {
    Listed(Vec<ProfileSummary>),
    Loaded(Box<Loaded>),
    Saved(String),
    Deleted(String),
    /// A connect or disconnect of this profile ended well.
    Switched(String),
    /// A job failed: what was being done, and why.
    Failed(String, NetError),
}

/// Run one job.
pub fn run_job(backend: &dyn ProfileBackend, job: Job) -> Done {
    let (what, result) = match job {
        Job::List => ("Reading the connections", backend.list().map(Done::Listed)),
        Job::Load(uuid) => ("Opening the connection", backend.load(&uuid).map(|l| Done::Loaded(Box::new(l)))),
        Job::Save { profile, base, files } => ("Saving", save(backend, &profile, base.as_ref(), &files).map(|_| Done::Saved(profile.uuid.clone()))),
        Job::Delete(uuid) => ("Deleting", backend.delete(&uuid).map(|_| Done::Deleted(uuid))),
        Job::Activate { uuid, password } => ("Connecting", backend.activate(&uuid, password.as_deref()).map(|_| Done::Switched(uuid))),
        Job::Deactivate(uuid) => ("Disconnecting", backend.deactivate(&uuid).map(|_| Done::Switched(uuid))),
    };
    result.unwrap_or_else(|e| Done::Failed(what.to_string(), e))
}

/// Write the files of an import, then store the profile. A new profile that
/// could not be stored does not leave its files behind.
fn save(backend: &dyn ProfileBackend, profile: &Profile, base: Option<&Settings>, files: &[PendingFile]) -> Result<(), NetError> {
    import::write_files(files)?;
    let saved = backend.save(profile, base);
    if saved.is_err() && base.is_none() && !files.is_empty() {
        if let Err(e) = import::remove_files(&profile.uuid) {
            opusdm_ipc::err_log!("opusdm-network-manager", "cleaning up the files of {}: {e}", profile.uuid);
        }
    }
    saved
}

/// Start the worker thread. Jobs run in order; reports go to `done`. The
/// thread ends when the job sender or the report receiver is dropped.
pub fn spawn(backend: Arc<dyn ProfileBackend>, done: async_channel::Sender<Done>) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            if done.send_blocking(run_job(&*backend, job)).is_err() { return; }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_network::profile::ProfileKind;
    use opusdm_network::FakeProfiles;

    #[test]
    fn a_job_reports_its_result() {
        let fake = FakeProfiles::default();
        let p = Profile::new(ProfileKind::Ethernet).unwrap();
        let done = run_job(&fake, Job::Save { profile: Box::new(p.clone()), base: None, files: Vec::new() });
        assert!(matches!(done, Done::Saved(u) if u == p.uuid));
        assert!(matches!(run_job(&fake, Job::List), Done::Listed(l) if l.len() == 1));
        assert!(matches!(run_job(&fake, Job::Activate { uuid: p.uuid.clone(), password: None }), Done::Switched(_)));
        assert!(matches!(run_job(&fake, Job::Load(p.uuid.clone())), Done::Loaded(l) if l.profile == p));
        assert!(matches!(run_job(&fake, Job::Delete(p.uuid.clone())), Done::Deleted(_)));
    }

    #[test]
    fn a_failure_says_what_was_being_done() {
        let fake = FakeProfiles::default();
        *fake.fail.lock().unwrap() = Some(NetError::other("Not authorized"));
        assert!(matches!(run_job(&fake, Job::List), Done::Failed(what, e) if what.contains("Reading") && e.text() == "Not authorized"));
        assert!(matches!(run_job(&fake, Job::Deactivate("u".into())), Done::Failed(_, _)));
    }

    #[test]
    fn debug_never_shows_the_password() {
        let job = Job::Activate { uuid: "u".into(), password: Some("hunter2".into()) };
        assert!(!format!("{job:?}").contains("hunter2"));
    }

    #[test]
    fn the_worker_runs_jobs_in_order() {
        let fake = Arc::new(FakeProfiles::default());
        let (tx, rx) = async_channel::unbounded();
        let jobs = spawn(fake.clone(), tx);
        jobs.send(Job::List).unwrap();
        jobs.send(Job::Delete("x".into())).unwrap();
        assert!(matches!(rx.recv_blocking().unwrap(), Done::Listed(_)));
        assert!(matches!(rx.recv_blocking().unwrap(), Done::Deleted(_)));
        assert_eq!(fake.calls(), vec!["list".to_string(), "delete x".to_string()]);
    }
}
