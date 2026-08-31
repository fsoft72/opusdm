//! Drag-and-drop of file selections between Lister windows via `text/uri-list`.
//!
//! Task 13 calls `attach`; Task 15 fills the body.

use crate::window::Ui;
use gtk4 as gtk;
use std::rc::Rc;

/// Attach a drag source and a drop target to the column view. Populated in Task 15.
pub fn attach(_ui: &Rc<Ui>, _view: &gtk::ColumnView) {}
