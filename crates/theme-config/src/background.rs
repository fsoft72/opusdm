//! Run slow file work (copying a theme folder, zipping or unzipping an
//! archive) on a thread of the GIO pool, so the window keeps drawing, and
//! hand the result back on the GTK thread.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};

/// What a background task reports when it panicked instead of returning.
const PANIC_MESSAGE: &str = "the background task failed unexpectedly";

/// Run `work` on a worker thread. `set_busy(true)` runs now and
/// `set_busy(false)` once `work` ended, before `done` gets its result on the
/// GTK thread. A panic in `work` reaches `done` as an error message.
pub fn run_blocking<T: Send + 'static>(
    set_busy: impl Fn(bool) + 'static,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    set_busy(true);
    let handle = gio::spawn_blocking(work);
    glib::spawn_future_local(async move {
        let result = handle.await.map_err(|_| PANIC_MESSAGE.to_string());
        if let Err(e) = &result {
            opusdm_ipc::err_log!("theme-config", "{e}");
        }
        set_busy(false);
        done(result);
    });
}

/// A `set_busy` for [`run_blocking`] that makes `widgets` insensitive while
/// busy and gives each its earlier sensitivity back afterwards.
pub fn busy_widgets(widgets: Vec<gtk::Widget>) -> impl Fn(bool) + 'static {
    let before: Vec<bool> = widgets.iter().map(|w| w.is_sensitive()).collect();
    move |busy| {
        for (widget, sensitive) in widgets.iter().zip(&before) {
            widget.set_sensitive(!busy && *sensitive);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Run `start` inside a fresh main context and iterate it until the
    /// returned log has `entries` lines.
    fn drive(entries: usize, start: impl FnOnce(Rc<RefCell<Vec<String>>>)) -> Vec<String> {
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let log = Rc::new(RefCell::new(Vec::new()));
            start(log.clone());
            while log.borrow().len() < entries {
                ctx.iteration(true);
            }
            let lines = log.borrow().clone();
            lines
        })
        .unwrap()
    }

    #[test]
    fn the_work_runs_off_the_caller_and_busy_brackets_it() {
        let log = drive(3, |log| {
            let busy_log = log.clone();
            let caller = std::thread::current().id();
            run_blocking(
                move |b| busy_log.borrow_mut().push(format!("busy {b}")),
                move || std::thread::current().id() != caller,
                move |r| log.borrow_mut().push(format!("done {r:?}")),
            );
        });
        assert_eq!(log, vec!["busy true", "busy false", "done Ok(true)"]);
    }

    #[test]
    fn a_panic_reaches_done_as_an_error() {
        let log = drive(3, |log| {
            let busy_log = log.clone();
            run_blocking(
                move |b| busy_log.borrow_mut().push(format!("busy {b}")),
                || -> u32 { panic!("boom") },
                move |r| log.borrow_mut().push(format!("done {}", r.is_err())),
            );
        });
        assert_eq!(log, vec!["busy true", "busy false", "done true"]);
    }
}
