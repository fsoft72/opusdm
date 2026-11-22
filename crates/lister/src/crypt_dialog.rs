//! The Encrypt / Decrypt password requester (F-14), shared with the Desktop
//! (see `opusdm_fileactions::crypt`).

use crate::window::Ui;
use std::rc::Rc;

pub use opusdm_fileactions::targets_for;

/// Ask for a password and encrypt (`encrypt`) or decrypt `srcs` into the
/// DEST Lister, or into this folder when there is no DEST.
pub fn open_crypt(ui: &Rc<Ui>, encrypt: bool, srcs: Vec<std::path::PathBuf>) {
    opusdm_fileactions::crypt::open_crypt(&crate::file_host::ListerHost::rc(ui), encrypt, srcs);
}
