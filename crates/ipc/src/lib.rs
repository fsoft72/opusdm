//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

/// Writes one `[tag:pid] message` line to stderr, the format `err_log!` and
/// `debug_log!` always had. Unlike `eprintln!` a failed write (for example a
/// closed pipe, EPIPE) is ignored instead of panicking.
pub fn write_log_line(tag: &str, args: std::fmt::Arguments) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "[{}:{}] {}", tag, std::process::id(), args);
}

/// Debug trace gated on `OPUSDM_DEBUG=1`, quiet otherwise so production runs
/// stay quiet without a recompile. `opusdm-lister`'s `dlog!` and
/// `opusdm-iconview`'s `dbg_log!` are thin per-crate wrappers around this so
/// every debug trace in the workspace shares one gate and one format.
#[macro_export]
macro_rules! debug_log {
    ($tag:expr, $($arg:tt)*) => {
        if std::env::var_os("OPUSDM_DEBUG").is_some() {
            $crate::write_log_line($tag, format_args!($($arg)*));
        }
    };
}

/// Always-on warning/error trace, same tag format as `debug_log!` but never
/// gated: failures should stay visible without a recompile or an env var.
/// Every non-debug `eprintln!` in the workspace goes through this instead,
/// so there is one place that controls the format.
#[macro_export]
macro_rules! err_log {
    ($tag:expr, $($arg:tt)*) => {
        $crate::write_log_line($tag, format_args!($($arg)*))
    };
}

pub mod bindings;
pub mod bookmarks;
pub mod buttonbank;
pub mod cli;
pub mod desktop;
pub mod desktop_entry;
pub mod dock;
pub mod filetypes;
pub mod format;
pub mod frame;
pub mod function;
pub mod funcref;
pub mod glob;
pub mod groups;
pub mod inotify;
pub mod instance;
pub mod leftout;
pub mod look;
pub mod menu;
pub mod notify;
pub mod proto;
pub mod quickapps;
pub mod reaped;
pub mod scripts;
pub mod settings;
pub mod session;
pub mod sidecar;
pub mod size;
pub mod sockpath;
pub mod stamp_cache;
pub mod theme;
pub mod themes;
pub mod timefmt;

pub use desktop::{
    background_css, is_background_image, is_background_media, is_background_video, path_to_file_url, BACKGROUND_IMAGE_EXTS, BACKGROUND_VIDEO_EXTS, Background, BackgroundEntry, BackgroundMode, DesktopSettings, ImageFit, FADE_MS_RANGE, INTERVAL_SECS_RANGE,
    DockImageMode, DockPosition, DockStyle, PanelPosition, DEFAULT_DOCK_ZOOM, DOCK_OPACITY_RANGE, DOCK_ZOOM_RANGE, StartupApp, PANEL_HEIGHT, DOCK_ICON_SIZE_RANGE, STARTUP_WAIT_SECS_RANGE, VIRTUAL_SCREENS_RANGE, WINDOW_OPACITY_RANGE,
    GRADIENT_ANGLE_MAX, GRID_GAP_RANGE, GRID_SPACING_RANGE, ICON_SIZE_RANGE,
};
pub use bindings::Bindings;
pub use dock::DockConfig;
pub use glob::NameFilter;
pub use format::{FormatColumn, FormatEntry, FormatSource, FormatStore, ListerFormat, Separate};
pub use function::{Function, FnStep, FunctionId, InternalArgs, InternalCmd};
pub use menu::{MenuConfig, MenuItem};
pub use quickapps::QuickApps;
pub use leftout::{LeftOut, LeftOutItem};
pub use groups::{Group, Groups};
pub use buttonbank::{BankAction, BankView, Banks, Btn, ButtonBank, MouseButton, Orientation};
pub use proto::{
    Appearance, Behavior, Secret, CacheEntry, ColorSettings, FileMeta, JobOptions, ListerId, Op, Outcome, Resolution, Role,
    Settings, ThemeMode, ToHub, ToLister, Welcome, AUTO_REFRESH_DEBOUNCE_RANGE_MS,
    CACHE_SIZE_RANGE, CURRENT_LOOK_VERSION, DEFAULT_CACHE_SIZE, DEFAULT_STATUS_FORMAT,
    FUNCTION_WAIT_PROTOCOL, PROTOCOL_VERSION, protocol_mismatch,
};
pub use theme::{IconSetId, Theme};

#[cfg(test)]
mod log_tests {
    use std::io::Read;
    use std::process::{Command, Stdio};

    const CHILD_ENV: &str = "OPUSDM_IPC_LOG_TEST_CHILD";

    /// Spawns this test binary as a child that runs `test_name` in child mode `mode`.
    fn spawn_child(test_name: &str, mode: &str) -> std::process::Child {
        let exe = std::env::current_exe().expect("current_exe");
        Command::new(exe)
            .args([test_name, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, mode)
            .env_remove("OPUSDM_DEBUG")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn child")
    }

    /// Logging to a stderr pipe whose read end is gone must not panic.
    #[test]
    fn logging_to_a_closed_stderr_does_not_panic() {
        if std::env::var(CHILD_ENV).as_deref() == Ok("closed") {
            for i in 0..100 {
                crate::err_log!("test", "line {i}");
                crate::debug_log!("test", "debug {i}");
            }
            return;
        }
        let mut child = spawn_child("log_tests::logging_to_a_closed_stderr_does_not_panic", "closed");
        // Drop the read end before the child logs: the child blocks on nothing,
        // it only starts logging once the harness has booted, so close at once.
        drop(child.stderr.take());
        let status = child.wait().expect("wait");
        assert!(status.success(), "child failed: {status:?}");
    }

    /// The line format stays `[tag:pid] message\n`.
    #[test]
    fn log_line_format_is_unchanged() {
        if std::env::var(CHILD_ENV).as_deref() == Ok("format") {
            crate::err_log!("tagx", "hello {}", 42);
            return;
        }
        let mut child = spawn_child("log_tests::log_line_format_is_unchanged", "format");
        let mut err = String::new();
        child.stderr.take().expect("stderr").read_to_string(&mut err).expect("read");
        let pid = child.id();
        child.wait().expect("wait");
        let expected = format!("[tagx:{pid}] hello 42\n");
        assert!(err.contains(&expected), "stderr was {err:?}");
    }
}
