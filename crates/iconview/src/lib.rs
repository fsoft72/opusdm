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
pub mod icon_set;
pub mod item;
pub mod scan;
pub mod style;
pub mod thumbs;
pub mod types;

/// Serializes the tests that change process-wide environment variables
/// (`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `OPUSDM_NO_THUMBS`): libtest runs
/// tests on parallel threads, and one test's `remove_var` would otherwise
/// pull the folder from under another. A panicking holder does not block
/// the others.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub use field::IconField;
pub use item::IconItem;
pub use types::{
    IconEntry, IconKind, LayoutMode, ThumbSize, DEFAULT_ICON_SIZE, ICON_ZOOM_STEPS,
};
