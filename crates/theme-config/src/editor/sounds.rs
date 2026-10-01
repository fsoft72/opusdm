//! Sounds tab: the sound files shipped in the theme. They are stored and
//! carried by Save, Export and Import but not played yet (Sound Events, F-36).

use super::asset_list::AssetList;
use crate::draft::{list_names, queue_remove_sound};
use crate::writer::{AssetOp, AUDIO_EXTS};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::themes::theme_dir;
use std::cell::RefCell;
use std::rc::Rc;

/// The Sounds tab.
pub struct SoundsTab {
    pub root: gtk::Box,
}

impl SoundsTab {
    /// Build the tab for theme `slug`. Added or removed files are queued in `ops`.
    pub fn new(window: &gtk::ApplicationWindow, slug: &str, ops: &Rc<RefCell<Vec<AssetOp>>>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(12);
        root.set_margin_start(12);
        root.set_margin_end(12);
        let note = gtk::Label::builder()
            .label("Sounds are stored in the theme but not played yet (Sound Events arrive with F-36).")
            .xalign(0.0)
            .wrap(true)
            .build();
        root.append(&note);
        let files = AssetList::new(
            window,
            "SOUND FILES",
            list_names(&theme_dir(slug).join("sounds")),
            &AUDIO_EXTS,
            ops.clone(),
            AssetOp::AddSound,
            queue_remove_sound,
        );
        root.append(&files.root);
        Self { root }
    }
}
