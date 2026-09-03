//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

pub mod frame;
pub mod proto;
pub mod sockpath;
pub mod theme;

pub use proto::{
    Appearance, ColorSettings, FileMeta, ListerId, Op, Outcome, Resolution, Role, Settings, ToHub,
    ToLister, PRESET_CUSTOM,
};
pub use theme::ThemePreset;
