//! Reusable icon-grid widget: file and folder icons with selection, keyboard
//! navigation, rubber-band, and drag-and-drop. Used by the Lister (icon view)
//! and the hub desktop window.

pub mod arrange;
pub mod dnd;
pub mod glyphs;
pub mod item;
pub mod scan;
pub mod types;

pub use item::IconItem;
pub use types::{IconEntry, IconKind, LayoutMode, ThumbSize};
