//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

pub mod desktop;
pub mod frame;
pub mod inotify;
pub mod proto;
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
