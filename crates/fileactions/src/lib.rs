//! The file actions a Lister and the Desktop share: the context menu of an
//! entry, its dialogs and its keys. Each window implements
//! [`FileActionHost`]; the code here only talks to that trait, so both
//! offer the same menu and the same shortcuts.

pub mod compress;
pub mod crypt;
pub mod extract;
pub mod host;
pub mod menu;
pub mod open_with;
pub mod permissions;

pub use host::{dest_or_folder, file_type_in, targets_for, ExtraPlace, FileActionHost, MenuSink, MenuTarget};

/// Log tag of the shared file actions.
pub(crate) const TAG: &str = "file-actions";
