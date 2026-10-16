//! Small config files (`filetypes.json`, `bookmarks.json`) parsed once and
//! kept in memory until the file changes on disk, so a click or an Enter in
//! the path bar does not read and parse the file again on the GTK thread.
//!
//! A `gio::FileMonitor` on the file drops the parsed copy whenever the file
//! is written, by this Lister, another one or a standalone editor. When the
//! monitor cannot be set up the file is read on every call, as before.

use gtk4::gio;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A config file parsed by `load`, cached while a monitor watches it.
pub struct FileCache<T> {
    path: PathBuf,
    load: fn(&Path) -> T,
    /// The parsed file; `None` until read or after the file changed.
    value: Rc<RefCell<Option<Rc<T>>>>,
    /// Keeps the monitor alive. `None` until the first read; after that a
    /// `None` means the monitor could not be created and nothing is cached.
    monitor: RefCell<Option<Option<gio::FileMonitor>>>,
}

impl<T: 'static> FileCache<T> {
    /// A cache of the file at `path`, read with `load`. Nothing is read yet.
    pub fn new(path: PathBuf, load: fn(&Path) -> T) -> Self {
        FileCache { path, load, value: Rc::new(RefCell::new(None)), monitor: RefCell::new(None) }
    }

    /// The parsed file: the cached copy, or a fresh read when there is none
    /// (or the file cannot be watched).
    pub fn get(&self) -> Rc<T> {
        self.start_monitor();
        if let Some(value) = self.value.borrow().as_ref() { return value.clone(); }
        let value = Rc::new((self.load)(&self.path));
        if self.is_watched() {
            *self.value.borrow_mut() = Some(value.clone());
        }
        value
    }

    /// Forget the cached copy, e.g. right after this process saved the file,
    /// before the monitor reports the change.
    pub fn invalidate(&self) {
        self.value.borrow_mut().take();
    }

    /// Whether a monitor watches the file, so a cached copy stays valid.
    fn is_watched(&self) -> bool {
        matches!(*self.monitor.borrow(), Some(Some(_)))
    }

    /// Create the file monitor on first use. A failure is logged once and
    /// leaves the cache disabled.
    fn start_monitor(&self) {
        if self.monitor.borrow().is_some() { return; }
        let monitor = match gio::File::for_path(&self.path).monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(monitor) => {
                let value = self.value.clone();
                monitor.connect_changed(move |_, _, _, _| {
                    value.borrow_mut().take();
                });
                Some(monitor)
            }
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-lister", "cannot watch {}: {e}; it is read on every use", self.path.display());
                None
            }
        };
        *self.monitor.borrow_mut() = Some(monitor);
    }
}

thread_local! {
    /// The user file types.
    static FILETYPES: FileCache<opusdm_ipc::filetypes::FileTypes> =
        FileCache::new(opusdm_ipc::filetypes::filetypes_path(), opusdm_ipc::filetypes::load_filetypes_from);
    /// The user bookmarks.
    static BOOKMARKS: FileCache<opusdm_ipc::bookmarks::Bookmarks> =
        FileCache::new(opusdm_ipc::bookmarks::bookmarks_path(), opusdm_ipc::bookmarks::load_bookmarks_from);
}

/// The user file types, from the cache.
pub fn filetypes() -> Rc<opusdm_ipc::filetypes::FileTypes> {
    FILETYPES.with(FileCache::get)
}

/// Drop the cached file types (this process just saved them).
pub fn invalidate_filetypes() {
    FILETYPES.with(FileCache::invalidate);
}

/// The user bookmarks, from the cache.
pub fn bookmarks() -> Rc<opusdm_ipc::bookmarks::Bookmarks> {
    BOOKMARKS.with(FileCache::get)
}

/// Drop the cached bookmarks (this process just saved them).
pub fn invalidate_bookmarks() {
    BOOKMARKS.with(FileCache::invalidate);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::glib;
    use std::time::{Duration, Instant};

    /// How long a test waits for the monitor to report a write.
    const MONITOR_WAIT: Duration = Duration::from_secs(5);

    /// Reads the file as a string.
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn the_file_is_read_once_until_invalidated() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let td = tempfile::tempdir().unwrap();
            let path = td.path().join("c.json");
            std::fs::write(&path, "one").unwrap();
            let cache = FileCache::new(path.clone(), read);
            assert_eq!(*cache.get(), "one");
            // Written behind the cache's back, with no main loop running:
            // the cached copy is still served.
            std::fs::write(&path, "two").unwrap();
            assert_eq!(*cache.get(), "one");
            cache.invalidate();
            assert_eq!(*cache.get(), "two");
        })
        .unwrap();
    }

    #[test]
    fn a_write_seen_by_the_monitor_drops_the_cached_copy() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let td = tempfile::tempdir().unwrap();
            let path = td.path().join("c.json");
            std::fs::write(&path, "one").unwrap();
            let cache = FileCache::new(path.clone(), read);
            assert_eq!(*cache.get(), "one");
            std::fs::write(&path, "two").unwrap();
            let deadline = Instant::now() + MONITOR_WAIT;
            while cache.value.borrow().is_some() && Instant::now() < deadline {
                ctx.iteration(false);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(*cache.get(), "two");
        })
        .unwrap();
    }

    #[test]
    fn a_missing_file_is_cached_and_picked_up_once_created() {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let td = tempfile::tempdir().unwrap();
            let path = td.path().join("later.json");
            let cache = FileCache::new(path.clone(), read);
            assert_eq!(*cache.get(), "");
            std::fs::write(&path, "made").unwrap();
            let deadline = Instant::now() + MONITOR_WAIT;
            while cache.value.borrow().is_some() && Instant::now() < deadline {
                ctx.iteration(false);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(*cache.get(), "made");
        })
        .unwrap();
    }
}
