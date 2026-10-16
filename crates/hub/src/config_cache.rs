//! Config files the hub reads on user actions (a desktop double click, a
//! bank step), kept in memory and loaded again only when they change on disk
//! (see `opusdm_ipc::stamp_cache`). Shared by the GTK thread and the
//! Function executor threads.

use opusdm_ipc::buttonbank::{banks_dir, Banks};
use opusdm_ipc::filetypes::{filetypes_path, load_filetypes_from, FileTypes};
use opusdm_ipc::stamp_cache::StampCache;
use std::sync::{Arc, Mutex};

/// The user file types of `filetypes.json`.
static FILETYPES: Mutex<StampCache<FileTypes>> = Mutex::new(StampCache::new());

/// The button banks of the banks directory.
static BANKS: Mutex<StampCache<Banks>> = Mutex::new(StampCache::new());

/// The current user file types: cached, reloaded after an edit.
pub fn filetypes() -> Arc<FileTypes> {
    let path = filetypes_path();
    let mut cache = FILETYPES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get_or_load(&path, || load_filetypes_from(&path))
}

/// The current button banks: cached, reloaded after a bank file changed.
pub fn banks() -> Arc<Banks> {
    let dir = banks_dir();
    let mut cache = BANKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get_or_load(&dir, || Banks::load_from_dir(&dir))
}
