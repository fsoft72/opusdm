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

pub mod desktop;
pub mod frame;
pub mod inotify;
pub mod proto;
pub mod settings;
pub mod sockpath;
pub mod theme;
pub mod themes;

pub use desktop::{
    background_css, path_to_file_url, Background, BackgroundMode, DesktopSettings, ImageFit,
    GRADIENT_ANGLE_MAX, GRID_GAP_RANGE, GRID_SPACING_RANGE, ICON_SIZE_RANGE,
};
pub use proto::{
    Appearance, Behavior, ColorSettings, FileMeta, ListerId, Op, Outcome, Resolution, Role,
    Settings, ToHub, ToLister, Welcome, AUTO_REFRESH_DEBOUNCE_RANGE_MS,
};
pub use theme::{IconSetId, Theme};
