//! The Compress requester (shared with the Desktop, see
//! `opusdm_fileactions::compress`).

use crate::window::Ui;
use std::path::PathBuf;
use std::rc::Rc;

/// Ask for the archive name and format, then compress `srcs` into the DEST
/// Lister, or into this folder when there is no DEST.
pub fn open_compress(ui: &Rc<Ui>, srcs: Vec<PathBuf>) {
    opusdm_fileactions::compress::open_compress(&crate::file_host::ListerHost::rc(ui), srcs);
}
