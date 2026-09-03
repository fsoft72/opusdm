//! Real-time directory watching via Linux `inotify(7)`.
//!
//! One background thread owns an inotify file descriptor and blocks on it. The
//! directory being watched is swapped whenever the Lister navigates, and bursts
//! of events (a build writing hundreds of files, an unpack, a `git checkout`)
//! are coalesced into a single notice so the UI reloads once instead of
//! hundreds of times.
//!
//! `libc` is already a dependency, so this needs no new crate.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
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

/// The fixed part of `struct inotify_event`, before the variable-length name.
const EVENT_HEADER: usize = std::mem::size_of::<libc::inotify_event>();

/// Pull the watch descriptors out of a raw inotify read buffer.
///
/// Events are variable length: a 16-byte header followed by `len` bytes of
/// name. A short or malformed tail is ignored rather than panicking - the
/// kernel never produces one, but the parse must not be able to run off the
/// end of the buffer.
fn parse_wds(buf: &[u8]) -> Vec<i32> {
    let mut wds = Vec::new();
    let mut off = 0usize;
    while off + EVENT_HEADER <= buf.len() {
        // SAFETY: the slice has at least EVENT_HEADER bytes left, and the read
        // is unaligned so the buffer needs no particular alignment.
        let ev: libc::inotify_event =
            unsafe { std::ptr::read_unaligned(buf[off..].as_ptr() as *const libc::inotify_event) };
        let Some(total) = EVENT_HEADER.checked_add(ev.len as usize) else { break };
        if off + total > buf.len() {
            break;
        }
        wds.push(ev.wd);
        off += total;
    }
    wds
}

/// Owns the inotify fd. Cloneable state shared with the watcher thread.
struct Inotify {
    fd: i32,
    /// The watch currently installed, if any.
    current: Mutex<Option<(i32, PathBuf)>>,
}

impl Inotify {
    fn new() -> io::Result<Self> {
        // SAFETY: no arguments to get wrong; the fd is checked below.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, current: Mutex::new(None) })
    }

    /// Replace the installed watch with one on `dir`. Passing a path that
    /// cannot be watched (a deleted directory, a virtual archive path) just
    /// clears the watch.
    fn rewatch(&self, dir: Option<&Path>) {
        let mut guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((wd, _)) = guard.take() {
            // SAFETY: wd came from inotify_add_watch on this fd.
            unsafe { libc::inotify_rm_watch(self.fd, wd) };
        }
        let Some(dir) = dir else { return };
        let Ok(cpath) = CString::new(dir.as_os_str().as_bytes()) else { return };
        // SAFETY: cpath is a valid NUL-terminated string for the call's duration.
        let wd = unsafe { libc::inotify_add_watch(self.fd, cpath.as_ptr(), WATCH_MASK) };
        if wd >= 0 {
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

impl Drop for Inotify {
    fn drop(&mut self) {
        // SAFETY: fd is ours and is not used again.
        unsafe { libc::close(self.fd) };
    }
}

/// What the idle wait observed.
enum Wait {
    /// At least one event is readable.
    Ready,
    /// Nothing happened before the timeout.
    Timeout,
    /// The fd went bad; the thread should stop.
    Failed,
}

/// Block until `fd` is readable or `timeout_ms` elapses.
fn wait_readable(fd: i32, timeout_ms: i32) -> Wait {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one initialised pollfd is passed with a matching count.
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::Interrupted { Wait::Timeout } else { Wait::Failed };
    }
    if ret == 0 {
        return Wait::Timeout;
    }
    if pfd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
        return Wait::Failed;
    }
    Wait::Ready
}

/// Read one batch of events. `None` means the fd failed.
fn read_events(fd: i32, buf: &mut [u8]) -> Option<Vec<i32>> {
    // SAFETY: buf is a valid writable slice of the length passed.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n <= 0 {
        let err = io::Error::last_os_error();
        // EAGAIN/EINTR just mean "nothing this time".
        return match err.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Some(Vec::new()),
            _ if n == 0 => Some(Vec::new()),
            _ => None,
        };
    }
    Some(parse_wds(&buf[..n as usize]))
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
        match wait_readable(inotify.fd, IDLE_POLL_MS) {
            Wait::Failed => return,
            Wait::Timeout => {
                if tx.is_closed() {
                    return;
                }
                continue;
            }
            Wait::Ready => {}
        }

        let Some(wds) = read_events(inotify.fd, &mut buf) else { return };
        let mut changed = wds.iter().find_map(|wd| inotify.path_for(*wd));

        // Keep swallowing events while they keep arriving, so one `cargo build`
        // produces one reload rather than one per file.
        let deadline = Instant::now() + MAX_COALESCE;
        loop {
            let left = debounce.as_millis().min(i32::MAX as u128) as i32;
            if Instant::now() >= deadline {
                break;
            }
            match wait_readable(inotify.fd, left) {
                Wait::Ready => {}
                Wait::Failed => return,
                Wait::Timeout => break,
            }
            let Some(more) = read_events(inotify.fd, &mut buf) else { return };
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

    /// Build a raw inotify buffer the way the kernel would.
    fn encode(events: &[(i32, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (wd, name) in events {
            // A name is NUL-terminated and padded to a multiple of 16.
            let len = if name.is_empty() { 0 } else { name.len().div_ceil(16) * 16 + 16 };
            let ev = libc::inotify_event { wd: *wd, mask: 0, cookie: 0, len: len as u32 };
            let bytes: [u8; EVENT_HEADER] = unsafe { std::mem::transmute(ev) };
            buf.extend_from_slice(&bytes);
            buf.extend(std::iter::repeat_n(0u8, len));
        }
        buf
    }

    #[test]
    fn parses_a_batch_of_variable_length_events() {
        let buf = encode(&[(1, ""), (2, "a.txt"), (7, "a-much-longer-file-name.bin")]);
        assert_eq!(parse_wds(&buf), vec![1, 2, 7]);
    }

    #[test]
    fn a_truncated_tail_is_ignored_not_panicked_on() {
        let mut buf = encode(&[(3, "x"), (4, "y")]);
        buf.truncate(buf.len() - 3);
        // The first event is complete, the second is not.
        assert_eq!(parse_wds(&buf), vec![3]);
        assert_eq!(parse_wds(&[]), Vec::<i32>::new());
        assert_eq!(parse_wds(&[0u8; 4]), Vec::<i32>::new());
    }

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
