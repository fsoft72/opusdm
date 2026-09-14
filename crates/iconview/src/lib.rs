//! Reusable icon-grid widget: file and folder icons with selection, keyboard
//! navigation, rubber-band, and drag-and-drop. Used by the Lister (icon view)
//! and the hub desktop window.

/// Debug log gated on OPUSDM_DEBUG=1, mirroring `opusdm-lister`'s `dlog!`.
#[macro_export]
macro_rules! dbg_log {
    ($($arg:tt)*) => { opusdm_ipc::debug_log!("iconview", $($arg)*) };
}

pub mod arrange;
pub mod dnd;
pub mod field;
pub mod font_probe;
pub mod glyphs;
pub mod icon_set;
pub mod item;
pub mod scan;
pub mod style;
pub mod thumbs;
pub mod types;

pub use field::IconField;
pub use item::IconItem;
pub use types::{
    IconEntry, IconKind, LayoutMode, ThumbSize, DEFAULT_ICON_SIZE, ICON_ZOOM_STEPS,
};
