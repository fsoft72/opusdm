//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

/// Debug trace gated on `OPUSDM_DEBUG=1`, quiet otherwise so production runs
/// stay quiet without a recompile. `opusdm-lister`'s `dlog!` and
/// `opusdm-iconview`'s `dbg_log!` are thin per-crate wrappers around this so
/// every debug trace in the workspace shares one gate and one format.
#[macro_export]
macro_rules! debug_log {
    ($tag:literal, $($arg:tt)*) => {
        if std::env::var_os("OPUSDM_DEBUG").is_some() {
            eprintln!("[{}:{}] {}", $tag, std::process::id(), format_args!($($arg)*));
        }
    };
}

/// Always-on warning/error trace, same tag format as `debug_log!` but never
/// gated: failures should stay visible without a recompile or an env var.
/// Every non-debug `eprintln!` in the workspace goes through this instead,
/// so there is one place that controls the format.
#[macro_export]
macro_rules! err_log {
    ($tag:literal, $($arg:tt)*) => {
        eprintln!("[{}:{}] {}", $tag, std::process::id(), format_args!($($arg)*));
    };
}

pub mod desktop;
pub mod dock;
pub mod frame;
pub mod inotify;
pub mod proto;
pub mod settings;
pub mod sockpath;
pub mod theme;
pub mod themes;

pub use desktop::{
    background_css, path_to_file_url, Background, BackgroundMode, DesktopSettings, ImageFit,
    DockPosition, PanelPosition, DOCK_ICON_SIZE_RANGE,
    GRADIENT_ANGLE_MAX, GRID_GAP_RANGE, GRID_SPACING_RANGE, ICON_SIZE_RANGE,
};
pub use dock::DockConfig;
pub use proto::{
    Appearance, Behavior, ColorSettings, FileMeta, ListerId, Op, Outcome, Resolution, Role,
    Settings, ToHub, ToLister, Welcome, AUTO_REFRESH_DEBOUNCE_RANGE_MS,
};
pub use theme::{IconSetId, Theme};
