//! A window showing every running file operation: current file, its own
//! progress bar, and a total (n/tot files) progress bar, one block per job.
//! Shows itself on the first running job and hides once none remain.

use crate::server::HubCmd;
use gtk4 as gtk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Sender;

/// Height in pixels of each progress bar - taller than GTK's thin default so
/// the fraction and overlaid text stay readable.
const BAR_HEIGHT: i32 = 40;

/// Widgets for one running job's block.
struct Row {
    /// Separator placed above this block, if it isn't the first one.
    separator: Option<gtk::Separator>,
    header: gtk::Box,
    file_label: gtk::Label,
    file_bar: gtk::ProgressBar,
    total_bar: gtk::ProgressBar,
}

/// Owns the progress window and its per-job rows.
pub struct ProgressWindow {
    window: gtk::Window,
    list: gtk::Box,
    cmd_tx: Sender<HubCmd>,
    rows: RefCell<HashMap<u64, Row>>,
}

impl ProgressWindow {
    /// Build the (initially hidden) window, attached to `app`.
    pub fn new(app: &gtk::Application, cmd_tx: Sender<HubCmd>) -> Self {
        install_bar_css();

        let window = gtk::Window::builder()
            .application(app)
            .title("File Operations")
            .default_width(420)
            .build();
        // Closing the window just hides it; the operations keep running.
        window.connect_close_request(|w| {
            w.set_visible(false);
            glib::Propagation::Stop
        });

        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.set_margin_start(12);
        list.set_margin_end(12);
        list.set_margin_top(12);
        list.set_margin_bottom(12);
        window.set_child(Some(&list));

        Self { window, list, cmd_tx, rows: RefCell::new(HashMap::new()) }
    }

    /// Update (creating on first call) the block for `job_id`.
    pub fn update(
        &self,
        job_id: u64,
        current_file: &Path,
        files_done: u64,
        files_total: u64,
        file_bytes_done: u64,
        file_bytes_total: u64,
    ) {
        let mut rows = self.rows.borrow_mut();
        if !rows.contains_key(&job_id) {
            let separator = if rows.is_empty() {
                None
            } else {
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                self.list.append(&sep);
                Some(sep)
            };
            let file_label = gtk::Label::new(None);
            file_label.set_xalign(0.0);
            file_label.set_hexpand(true);
            file_label.set_ellipsize(pango::EllipsizeMode::Middle);

            let abort_btn = gtk::Button::with_label("Abort");
            let cmd_tx = self.cmd_tx.clone();
            abort_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(HubCmd::CancelJob(job_id));
            });

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            header.append(&file_label);
            header.append(&abort_btn);

            let file_bar = gtk::ProgressBar::new();
            file_bar.set_show_text(true);
            file_bar.set_height_request(BAR_HEIGHT);
            let total_bar = gtk::ProgressBar::new();
            total_bar.set_show_text(true);
            total_bar.set_height_request(BAR_HEIGHT);
            self.list.append(&header);
            self.list.append(&file_bar);
            self.list.append(&total_bar);
            rows.insert(job_id, Row { separator, header, file_label, file_bar, total_bar });
            self.window.set_visible(true);
        }

        let row = rows.get(&job_id).expect("row inserted above if missing");
        row.file_label.set_text(&current_file.display().to_string());

        let file_frac = if file_bytes_total > 0 {
            file_bytes_done as f64 / file_bytes_total as f64
        } else {
            0.0
        };
        row.file_bar.set_fraction(file_frac);
        row.file_bar.set_text(Some(&format!("{}%", (file_frac * 100.0).round() as u32)));

        let total_frac = if files_total > 0 { files_done as f64 / files_total as f64 } else { 0.0 };
        row.total_bar.set_fraction(total_frac);
        row.total_bar.set_text(Some(&format!("{files_done} / {files_total}")));
    }

    /// Remove the block for a finished job; hide the window once none remain.
    pub fn finish(&self, job_id: u64) {
        let mut rows = self.rows.borrow_mut();
        if let Some(row) = rows.remove(&job_id) {
            if let Some(sep) = &row.separator {
                self.list.remove(sep);
            }
            self.list.remove(&row.header);
            self.list.remove(&row.file_bar);
            self.list.remove(&row.total_bar);
        }
        if rows.is_empty() {
            self.window.set_visible(false);
        }
    }
}

/// GTK4's themed progressbar trough ignores `height_request` and stays at
/// its CSS min-height; grow it directly so the bars actually look taller.
fn install_bar_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        "progressbar > trough {{ min-height: {BAR_HEIGHT}px; }}
         progressbar > trough > progress {{ min-height: {BAR_HEIGHT}px; }}"
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
