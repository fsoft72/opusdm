//! One call to let the user choose a single file, instead of building a
//! `gtk::FileDialog` and its filter list at every call site.

use gtk4 as gtk;
use gtk::gio;
use gtk::prelude::*;
use std::path::{Path, PathBuf};

/// Tag of the log lines of a failed file chooser.
const LOG_TAG: &str = "opusdm-ui";

/// The one file filter a [`pick_file`] dialog offers. A file matches when it
/// matches any of the patterns, suffixes or MIME types.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileFilterSpec<'a> {
    /// Name shown in the dialog, e.g. "Pictures".
    pub name: &'a str,
    /// Glob patterns, e.g. `"*.zip"`.
    pub patterns: &'a [&'a str],
    /// File name suffixes without the dot, e.g. `"png"`.
    pub suffixes: &'a [&'a str],
    /// MIME types, e.g. `"image/*"`.
    pub mime_types: &'a [&'a str],
}

impl FileFilterSpec<'_> {
    /// The GTK filter for this spec.
    fn build(&self) -> gtk::FileFilter {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(self.name));
        for pattern in self.patterns {
            filter.add_pattern(pattern);
        }
        for suffix in self.suffixes {
            filter.add_suffix(suffix);
        }
        for mime in self.mime_types {
            filter.add_mime_type(mime);
        }
        filter
    }
}

/// What a [`pick_file`] dialog shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct FilePick<'a> {
    /// Title of the dialog.
    pub title: &'a str,
    /// The only filter offered; `None` shows every file.
    pub filter: Option<FileFilterSpec<'a>>,
    /// The file selected when the dialog opens.
    pub initial_file: Option<&'a Path>,
    /// The folder shown when the dialog opens.
    pub initial_folder: Option<&'a Path>,
}

impl FilePick<'_> {
    /// The GTK dialog for this pick.
    fn dialog(&self) -> gtk::FileDialog {
        let dialog = gtk::FileDialog::builder().title(self.title).modal(true).build();
        if let Some(spec) = &self.filter {
            let filter = spec.build();
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&filter));
        }
        if let Some(file) = self.initial_file {
            dialog.set_initial_file(Some(&gio::File::for_path(file)));
        }
        if let Some(folder) = self.initial_folder {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        dialog
    }
}

/// Let the user choose one local file over `parent`; `on_result` gets its
/// path, or why no file came (a failed dialog, a file that is not local).
/// Nothing is called when the user cancels.
pub fn pick_file_or_error(parent: Option<&gtk::Window>, pick: &FilePick, on_result: impl FnOnce(Result<PathBuf, String>) + 'static) {
    pick.dialog().open(parent, gio::Cancellable::NONE, move |result| {
        let file = match result {
            Ok(file) => file,
            Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
            Err(e) => return on_result(Err(format!("File chooser: {e}"))),
        };
        match file.path() {
            Some(path) => on_result(Ok(path)),
            None => on_result(Err("Only local files can be chosen".to_string())),
        }
    });
}

/// Let the user choose one local file over `parent`; `on_pick` gets its
/// path. A failure is logged; nothing is called when the user cancels.
pub fn pick_file(parent: Option<&gtk::Window>, pick: &FilePick, on_pick: impl FnOnce(PathBuf) + 'static) {
    pick_file_or_error(parent, pick, move |result| match result {
        Ok(path) => on_pick(path),
        Err(e) => opusdm_ipc::err_log!(LOG_TAG, "{e}"),
    });
}
