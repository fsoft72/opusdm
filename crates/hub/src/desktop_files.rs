//! Resolve and watch the directory shown on the hub desktop window.
//!
//! Fd lifecycle, poll and raw event draining are shared with `lister`'s
//! directory watcher via `opusdm_ipc::inotify`.

use opusdm_ipc::inotify::{run_debounce_loop, DebounceConfig, InotifyFd, DEFAULT_WATCH_MASK};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// inotify events worth a reload: an entry appearing, disappearing, being
/// renamed, finishing a write, or changing its metadata.
const WATCH_MASK: u32 = DEFAULT_WATCH_MASK;

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

/// The directory to show as the desktop. A readable directory passed as
/// `override_dir` wins; otherwise fall back to [`desktop_dir`]'s
/// `$XDG_DESKTOP_DIR` then `~/Desktop` resolution.
pub fn effective_desktop_dir(override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_dir {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
    }
    desktop_dir()
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
/// stops when this value is dropped: the channel is closed at once (so a
/// notice still queued is never acted on) and the thread exits by itself.
pub struct DesktopWatch {
    stop: Arc<AtomicBool>,
    /// Kept to close the channel on drop.
    tx: async_channel::Sender<()>,
}

impl DesktopWatch {
    /// Start watching `dir`. The receiver yields one `()` per coalesced burst.
    pub fn start(dir: PathBuf) -> io::Result<(DesktopWatch, async_channel::Receiver<()>)> {
        let inotify = InotifyFd::new(libc::IN_CLOEXEC | libc::IN_NONBLOCK)?;
        inotify.add_watch(&dir, WATCH_MASK)?;

        let (tx, rx) = async_channel::unbounded::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let tx_thread = tx.clone();
        // Detached: the thread owns the inotify fd and the sender, and exits
        // by itself (within one idle poll) once `stop` is set.
        std::thread::Builder::new()
            .name("opusdm-desktop-watch".into())
            .spawn(move || watch_loop(&inotify, &stop_thread, &tx_thread))?;

        Ok((DesktopWatch { stop, tx }, rx))
    }
}

impl Drop for DesktopWatch {
    /// Ask the thread to stop without waiting for it: the drop runs on the
    /// GTK thread, and joining would block it for up to one idle poll.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.tx.close();
    }
}

/// Block on the inotify fd, draining and coalescing events into `tx`, until the
/// stop flag is set or the channel is closed.
fn watch_loop(inotify: &InotifyFd, stop: &AtomicBool, tx: &async_channel::Sender<()>) {
    let config = DebounceConfig {
        idle_poll_ms: 500,
        debounce: DEBOUNCE,
        max_coalesce: Duration::from_secs(2),
    };
    run_debounce_loop(
        inotify.raw(),
        config,
        || stop.load(Ordering::SeqCst) || tx.is_closed(),
        |_wds| {},
        || tx.send_blocking(()).is_ok(),
    );
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

    #[test]
    fn dropping_a_watch_does_not_wait_for_its_thread() {
        // The GTK thread drops the watch on every desktop rebuild: it must
        // not sit out the watcher's idle poll.
        let td = tempfile::tempdir().unwrap();
        let (w, rx) = DesktopWatch::start(td.path().to_path_buf()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let start = std::time::Instant::now();
        drop(w);
        assert!(start.elapsed() < Duration::from_millis(100), "drop took {:?}", start.elapsed());
        // The channel closes with the drop, so a notice the detached thread
        // still sends is never acted on.
        assert!(rx.is_closed());
    }

    #[test]
    fn effective_desktop_dir_prefers_a_valid_override() {
        let td = tempfile::tempdir().unwrap();
        let over = td.path().join("MyDesk");
        fs::create_dir(&over).unwrap();
        assert_eq!(effective_desktop_dir(Some(&over)), Some(over));
    }

    #[test]
    fn effective_desktop_dir_ignores_a_missing_override() {
        let missing = Path::new("/no/such/dir/opusdm-test");
        assert_ne!(effective_desktop_dir(Some(missing)), Some(missing.to_path_buf()));
    }
}
