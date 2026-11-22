//! "Open with...": a window listing the applications that handle the type of
//! a file. Shared with the Desktop, see `opusdm_fileactions::open_with`.

use crate::window::Ui;
use std::path::PathBuf;
use std::rc::Rc;

/// Open the "Open with..." window for `paths`; the handlers shown are those
/// of the first path.
pub fn open_with(ui: &Rc<Ui>, paths: Vec<PathBuf>) {
    opusdm_fileactions::open_with::open_with(&crate::file_host::ListerHost::rc(ui), paths);
}
