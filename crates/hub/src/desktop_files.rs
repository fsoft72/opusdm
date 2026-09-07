//! Resolve and watch the directory shown on the hub desktop window.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// inotify events worth a reload: an entry appearing, disappearing, being
/// renamed, finishing a write, or changing its metadata.
const WATCH_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB;

/// Poll timeout so the watch thread notices the stop flag between events.
const POLL_TIMEOUT_MS: i32 = 500;

/// Hold a burst of events back this long, then emit a single notice.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Resolve the desktop directory. Order:
/// 1. `XDG_DESKTOP_DIR` from `user_dirs_file` (freedesktop `user-dirs.dirs`),
///    with a leading `$HOME` or `~` expanded against `home`.
/// 2. `<home>/Desktop` if it exists.
/// 3. `None`.
pub fn resolve_desktop_dir(home: &Path, user_dirs_file: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = user_dirs_file.and_then(|f| parse_xdg_desktop(f, home)) {
        if path.is_dir() {
            return Some(path);
        }
    }
    let fallback = home.join("Desktop");
    if fallback.is_dir() {
        return Some(fallback);
    }
    None
}

/// Pull `XDG_DESKTOP_DIR="..."` out of a `user-dirs.dirs` file.
fn parse_xdg_desktop(file: &Path, home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("XDG_DESKTOP_DIR=") else {
            continue;
        };
        let raw = rest.trim().trim_matches('"');
        let expanded = if let Some(tail) = raw.strip_prefix("$HOME/") {
            home.join(tail)
        } else if let Some(tail) = raw.strip_prefix("~/") {
            home.join(tail)
        } else {
            PathBuf::from(raw)
        };
        return Some(expanded);
    }
    None
}

/// Real-environment resolution: `$XDG_DESKTOP_DIR` then `~/Desktop`.
pub fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let user_dirs = home.join(".config/user-dirs.dirs");
    let user_dirs = user_dirs.exists().then_some(user_dirs);
    resolve_desktop_dir(&home, user_dirs.as_deref())
}

/// A background inotify watch on one directory. Sends `()` on the returned
/// channel (coalesced) whenever the directory's contents change. The watch
/// stops when this value is dropped.
pub struct DesktopWatch {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DesktopWatch {
    /// Start watching `dir`. The receiver yields one `()` per coalesced burst.
    pub fn start(dir: PathBuf) -> io::Result<(DesktopWatch, async_channel::Receiver<()>)> {
        // SAFETY: no arguments to get wrong; the fd is checked below.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let cpath = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path has an interior NUL"))?;
        // SAFETY: cpath is a valid NUL-terminated string for the call.
        let wd = unsafe { libc::inotify_add_watch(fd, cpath.as_ptr(), WATCH_MASK) };
        if wd < 0 {
            let err = io::Error::last_os_error();
            // SAFETY: fd is ours and unused past this point.
            unsafe { libc::close(fd) };
            return Err(err);
        }

        let (tx, rx) = async_channel::unbounded::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            watch_loop(fd, &stop_thread, &tx);
            // SAFETY: the loop has returned, nothing else touches fd.
            unsafe { libc::close(fd) };
        });

        Ok((DesktopWatch { stop, handle: Some(handle) }, rx))
    }
}

impl Drop for DesktopWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Block on the inotify fd, draining and coalescing events into `tx`, until the
/// stop flag is set.
fn watch_loop(fd: i32, stop: &AtomicBool, tx: &async_channel::Sender<()>) {
    let mut buf = [0u8; 8 * 1024];
    while !stop.load(Ordering::SeqCst) {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: a single valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut pfd, 1, POLL_TIMEOUT_MS) };
        if n <= 0 {
            continue;
        }
        // Drain everything currently readable.
        loop {
            // SAFETY: buf is valid for `buf.len()` bytes; a negative return is
            // handled below.
            let r = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r <= 0 {
                break;
            }
        }
        // Coalesce a burst: keep draining until the directory goes quiet.
        let deadline = Instant::now() + DEBOUNCE;
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: as above.
            if unsafe { libc::poll(&mut p, 1, 0) } > 0 {
                // SAFETY: as above.
                let _ = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            }
        }
        if tx.send_blocking(()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prefers_xdg_entry_when_dir_exists() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        fs::create_dir(home.join("Schreibtisch")).unwrap();
        fs::create_dir(home.join(".config")).unwrap();
        let ud = home.join(".config/user-dirs.dirs");
        fs::write(&ud, "# comment\nXDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\n").unwrap();
        assert_eq!(resolve_desktop_dir(home, Some(&ud)), Some(home.join("Schreibtisch")));
    }

    #[test]
    fn falls_back_to_desktop_then_none() {
        let td = tempfile::tempdir().unwrap();
        let home = td.path();
        assert_eq!(resolve_desktop_dir(home, None), None);
        fs::create_dir(home.join("Desktop")).unwrap();
        assert_eq!(resolve_desktop_dir(home, None), Some(home.join("Desktop")));
    }

    #[test]
    fn desktop_watch_constructs_and_drops() {
        let td = tempfile::tempdir().unwrap();
        let (w, _rx) = DesktopWatch::start(td.path().to_path_buf()).unwrap();
        drop(w);
    }
}
