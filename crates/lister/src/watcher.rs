//! Real-time directory watching via Linux `inotify(7)`.
//!
//! One background thread owns an inotify file descriptor and blocks on it. The
//! directory being watched is swapped whenever the Lister navigates, and bursts
//! of events (a build writing hundreds of files, an unpack, a `git checkout`)
//! are coalesced into a single notice so the UI reloads once instead of
//! hundreds of times.
//!
//! Fd lifecycle, poll and raw event parsing are shared with `hub`'s desktop
//! watcher via `opusdm_ipc::inotify`.

use opusdm_ipc::inotify::{read_raw, wait_readable, InotifyFd, Wait};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Events worth reloading for: an entry appearing, disappearing, being renamed,
/// finishing a write, or changing its metadata.
const WATCH_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF;

/// Read buffer for one `read(2)` on the inotify fd.
const EVENT_BUF: usize = 8 * 1024;

/// How long the idle wait blocks before looking up again whether the receiver
/// is still there, so the thread retires after the window closes.
const IDLE_POLL_MS: i32 = 1_000;

/// Never hold a burst back longer than this, however busy the directory is.
const MAX_COALESCE: Duration = Duration::from_secs(2);

/// Owns the inotify fd. Cloneable state shared with the watcher thread.
struct Inotify {
    fd: InotifyFd,
    /// The watch currently installed, if any.
    current: Mutex<Option<(i32, PathBuf)>>,
}

impl Inotify {
    fn new() -> io::Result<Self> {
        Ok(Self { fd: InotifyFd::new(libc::IN_CLOEXEC)?, current: Mutex::new(None) })
    }

    /// Replace the installed watch with one on `dir`. Passing a path that
    /// cannot be watched (a deleted directory, a virtual archive path) just
    /// clears the watch.
    fn rewatch(&self, dir: Option<&Path>) {
        let mut guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((wd, _)) = guard.take() {
            self.fd.rm_watch(wd);
        }
        let Some(dir) = dir else { return };
        if let Ok(wd) = self.fd.add_watch(dir, WATCH_MASK) {
            *guard = Some((wd, dir.to_path_buf()));
        }
    }

    /// The path a watch descriptor refers to, if it is still the current one.
    /// Events for a directory we have already navigated away from are dropped.
    fn path_for(&self, wd: i32) -> Option<PathBuf> {
        let guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().filter(|(cur, _)| *cur == wd).map(|(_, p)| p.clone())
    }
}

/// Read one batch of events. `None` means the fd failed.
fn read_events(fd: i32, buf: &mut [u8]) -> Option<Vec<i32>> {
    let n = read_raw(fd, buf)?;
    Some(opusdm_ipc::inotify::parse_wds(&buf[..n]))
}

/// A live watch on whatever directory the Lister is showing.
///
/// Dropping it stops the thread at its next idle wakeup.
pub struct DirWatcher {
    inotify: Arc<Inotify>,
}

impl DirWatcher {
    /// Start the watcher thread. Coalesced notices arrive on the returned
    /// receiver, each carrying the directory that changed.
    ///
    /// `debounce` is how long a burst is allowed to keep extending itself
    /// before a notice is sent regardless.
    pub fn start(debounce: Duration) -> io::Result<(DirWatcher, async_channel::Receiver<PathBuf>)> {
        let inotify = Arc::new(Inotify::new()?);
        let (tx, rx) = async_channel::unbounded::<PathBuf>();
        let worker = Arc::clone(&inotify);
        std::thread::Builder::new()
            .name("opusdm-watcher".into())
            .spawn(move || watch_loop(&worker, &tx, debounce))
            .map_err(|e| io::Error::other(format!("cannot spawn watcher thread: {e}")))?;
        Ok((DirWatcher { inotify }, rx))
    }

    /// Watch `dir` instead of whatever was being watched before.
    pub fn watch(&self, dir: &Path) {
        self.inotify.rewatch(Some(dir));
    }

    /// Stop reporting changes until the next [`watch`](Self::watch).
    pub fn unwatch(&self) {
        self.inotify.rewatch(None);
    }
}

/// The watcher thread: wait, drain, coalesce, notify, repeat.
fn watch_loop(inotify: &Inotify, tx: &async_channel::Sender<PathBuf>, debounce: Duration) {
    let mut buf = vec![0u8; EVENT_BUF];
    loop {
        match wait_readable(inotify.fd.raw(), IDLE_POLL_MS) {
            Wait::Failed => return,
            Wait::Timeout => {
                if tx.is_closed() {
                    return;
                }
                continue;
            }
            Wait::Ready => {}
        }

        let Some(wds) = read_events(inotify.fd.raw(), &mut buf) else { return };
        let mut changed = wds.iter().find_map(|wd| inotify.path_for(*wd));

        // Keep swallowing events while they keep arriving, so one `cargo build`
        // produces one reload rather than one per file.
        let deadline = Instant::now() + MAX_COALESCE;
        loop {
            let left = debounce.as_millis().min(i32::MAX as u128) as i32;
            if Instant::now() >= deadline {
                break;
            }
            match wait_readable(inotify.fd.raw(), left) {
                Wait::Ready => {}
                Wait::Failed => return,
                Wait::Timeout => break,
            }
            let Some(more) = read_events(inotify.fd.raw(), &mut buf) else { return };
            if changed.is_none() {
                changed = more.iter().find_map(|wd| inotify.path_for(*wd));
            }
        }

        // Events for an already-abandoned directory leave `changed` empty.
        let Some(dir) = changed else { continue };
        if tx.send_blocking(dir).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `parse_wds`/`EVENT_HEADER` fd-parsing coverage lives with the shared
    // helper in `opusdm_ipc::inotify`; this module tests the watcher's own
    // rewatch/coalesce/dispatch behavior end to end.

    /// Wait up to `secs` for a notice, polling so the test cannot hang.
    fn wait_for(rx: &async_channel::Receiver<PathBuf>, secs: u64) -> Option<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if let Ok(p) = rx.try_recv() {
                return Some(p);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn reports_a_file_created_in_the_watched_directory() {
        let td = tempfile::tempdir().unwrap();
        let (watcher, rx) = DirWatcher::start(Duration::from_millis(50)).unwrap();
        watcher.watch(td.path());

        std::fs::write(td.path().join("new.txt"), b"hi").unwrap();

        let got = wait_for(&rx, 5).expect("a change notice");
        assert_eq!(got, td.path());
    }

    #[test]
    fn a_burst_of_writes_collapses_into_few_notices() {
        let td = tempfile::tempdir().unwrap();
        let (watcher, rx) = DirWatcher::start(Duration::from_millis(80)).unwrap();
        watcher.watch(td.path());

        for i in 0..200 {
            std::fs::write(td.path().join(format!("f{i:03}")), b"x").unwrap();
        }

        assert!(wait_for(&rx, 5).is_some(), "expected at least one notice");
        // Let the coalescing window close, then count what is left queued.
        std::thread::sleep(Duration::from_millis(600));
        let mut extra = 0;
        while rx.try_recv().is_ok() {
            extra += 1;
        }
        assert!(extra < 20, "200 writes produced {} notices", extra + 1);
    }

    #[test]
    fn changes_elsewhere_are_not_reported() {
        let watched = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (watcher, rx) = DirWatcher::start(Duration::from_millis(50)).unwrap();
        watcher.watch(watched.path());

        std::fs::write(other.path().join("elsewhere.txt"), b"hi").unwrap();
        assert!(wait_for(&rx, 1).is_none());

        // The watch is still live for its own directory.
        std::fs::write(watched.path().join("here.txt"), b"hi").unwrap();
        assert_eq!(wait_for(&rx, 5).as_deref(), Some(watched.path()));
    }

    #[test]
    fn unwatch_stops_the_notices() {
        let td = tempfile::tempdir().unwrap();
        let (watcher, rx) = DirWatcher::start(Duration::from_millis(50)).unwrap();
        watcher.watch(td.path());
        std::fs::write(td.path().join("a"), b"1").unwrap();
        assert!(wait_for(&rx, 5).is_some());

        watcher.unwatch();
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}

        std::fs::write(td.path().join("b"), b"2").unwrap();
        assert!(wait_for(&rx, 1).is_none(), "no notice after unwatch");
    }

    #[test]
    fn watching_a_second_directory_replaces_the_first() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (watcher, rx) = DirWatcher::start(Duration::from_millis(50)).unwrap();
        watcher.watch(a.path());
        watcher.watch(b.path());

        std::fs::write(a.path().join("stale"), b"1").unwrap();
        assert!(wait_for(&rx, 1).is_none(), "the old directory must be dropped");

        std::fs::write(b.path().join("fresh"), b"2").unwrap();
        assert_eq!(wait_for(&rx, 5).as_deref(), Some(b.path()));
    }
}
