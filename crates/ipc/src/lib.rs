//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

/// Debug trace gated on `OPUSDM_DEBUG=1`, quiet otherwise so production runs
/// stay quiet without a recompile. `opusdm-lister`'s `dlog!` and
/// `opusdm-iconview`'s `dbg_log!` are thin per-crate wrappers around this so
/// every debug trace in the workspace shares one gate and one format.
#[macro_export]
macro_rules! debug_log {
    ($tag:expr, $($arg:tt)*) => {
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
    ($tag:expr, $($arg:tt)*) => {
        eprintln!("[{}:{}] {}", $tag, std::process::id(), format_args!($($arg)*))
    };
}

pub mod bindings;
pub mod bookmarks;
pub mod buttonbank;
pub mod cli;
pub mod desktop;
pub mod dock;
pub mod filetypes;
pub mod format;
pub mod frame;
pub mod function;
pub mod glob;
pub mod groups;
pub mod inotify;
pub mod leftout;
pub mod look;
pub mod menu;
pub mod proto;
pub mod reaped;
pub mod settings;
pub mod sockpath;
pub mod theme;
pub mod themes;
pub mod timefmt;

pub use desktop::{
    background_css, is_background_image, path_to_file_url, BACKGROUND_IMAGE_EXTS, Background, BackgroundMode, DesktopSettings, ImageFit,
    DockPosition, PanelPosition, DOCK_ICON_SIZE_RANGE,
    GRADIENT_ANGLE_MAX, GRID_GAP_RANGE, GRID_SPACING_RANGE, ICON_SIZE_RANGE,
};
pub use bindings::Bindings;
pub use dock::DockConfig;
pub use glob::NameFilter;
pub use format::{FormatColumn, FormatEntry, FormatSource, FormatStore, ListerFormat, Separate};
pub use function::{Function, FnStep, FunctionId, InternalArgs, InternalCmd};
pub use menu::{MenuConfig, MenuItem};
pub use leftout::{LeftOut, LeftOutItem};
pub use groups::{Group, Groups};
pub use buttonbank::{BankAction, BankView, Banks, Btn, ButtonBank, MouseButton, Orientation};
pub use proto::{
    Appearance, Behavior, Secret, CacheEntry, ColorSettings, FileMeta, JobOptions, ListerId, Op, Outcome, Resolution, Role,
    Settings, ThemeMode, ToHub, ToLister, Welcome, AUTO_REFRESH_DEBOUNCE_RANGE_MS,
    CACHE_SIZE_RANGE, CURRENT_LOOK_VERSION, DEFAULT_CACHE_SIZE, DEFAULT_STATUS_FORMAT,
};
pub use theme::{IconSetId, Theme};
