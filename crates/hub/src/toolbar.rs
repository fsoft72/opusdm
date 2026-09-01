//! The always-on toolbar: New Lister, SRC/DST indicators, job progress, Quit.

use crate::server::{HubCmd, HubEvent, ServerHandle};
use super::settings;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::ListerId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// Build and run the toolbar application. Returns on quit.
pub fn run(handle: ServerHandle, _sock_path: PathBuf) -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.Hub")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    // Bridge the blocking mpsc receiver onto an async channel the GTK loop drains.
    let (ui_tx, ui_rx) = async_channel::unbounded::<HubEvent>();
    let evt_rx = handle.evt_rx;
    std::thread::spawn(move || {
        for ev in evt_rx.iter() {
            if ui_tx.send_blocking(ev).is_err() {
                break;
            }
        }
    });

    let cmd_tx = handle.cmd_tx;
    app.connect_activate(move |app| {
        build_ui(app, cmd_tx.clone(), ui_rx.clone());
    });
    app.run_with_args::<&str>(&[])
}

/// Live widget handles shared by the event pump.
struct Widgets {
    window: gtk::ApplicationWindow,
    src_label: gtk::Label,
    dst_label: gtk::Label,
    src_id: Cell<Option<ListerId>>,
    dst_id: Cell<Option<ListerId>>,
    job_box: gtk::Box,
    jobs: RefCell<HashMap<u64, JobRow>>,
    cmd_tx: Sender<HubCmd>,
}

struct JobRow {
    root: gtk::Box,
    bar: gtk::ProgressBar,
}

fn build_ui(app: &gtk::Application, cmd_tx: Sender<HubCmd>, ui_rx: async_channel::Receiver<HubEvent>) {
    let panel_width = gtk::gdk::Display::default()
        .as_ref()
        .map(crate::dock::monitor_width)
        .unwrap_or(1280);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM")
        .decorated(false)
        .resizable(false)
        .default_width(panel_width)
        .default_height(crate::dock::PANEL_HEIGHT)
        .build();

    // Turn the window into a top-docked X11 panel once its surface exists, then
    // re-assert the geometry after the WM has run its placement policy.
    window.connect_realize(move |w| crate::dock::make_top_dock(w, panel_width));
    window.connect_map(move |w| {
        crate::dock::make_top_dock(w, panel_width);
        let w = w.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
            crate::dock::pin_geometry(&w, panel_width);
        });
    });

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_margin_top(2);
    root.set_margin_bottom(2);
    root.set_margin_start(6);
    root.set_margin_end(6);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let new_btn = gtk::Button::with_label("New Lister");
    {
        let cmd_tx = cmd_tx.clone();
        new_btn.connect_clicked(move |_| {
            let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
        });
    }
    bar.append(&new_btn);

    let src_label = gtk::Label::new(Some("SRC: -"));
    let dst_label = gtk::Label::new(Some("DST: -"));
    src_label.add_css_class("dim-label");
    dst_label.add_css_class("dim-label");
    bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    bar.append(&src_label);
    bar.append(&dst_label);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let settings_btn = gtk::Button::with_label("Settings");
    bar.append(&settings_btn);

    let quit_btn = gtk::Button::with_label("Quit");
    bar.append(&quit_btn);

    root.append(&bar);

    let job_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.append(&job_box);

    window.set_child(Some(&root));

    let widgets = Rc::new(Widgets {
        window: window.clone(),
        src_label: src_label.clone(),
        dst_label: dst_label.clone(),
        src_id: Cell::new(None),
        dst_id: Cell::new(None),
        job_box: job_box.clone(),
        jobs: RefCell::new(HashMap::new()),
        cmd_tx: cmd_tx.clone(),
    });

    // Click an indicator to raise its Lister.
    attach_present_gesture(&src_label, &widgets, true);
    attach_present_gesture(&dst_label, &widgets, false);

    // Settings button: open settings dialog.
    {
        let widgets = widgets.clone();
        settings_btn.connect_clicked(move |_| show_settings_dialog(&widgets.window, &widgets.cmd_tx));
    }

    // Quit button and window close: confirm if jobs are running.
    {
        let widgets = widgets.clone();
        quit_btn.connect_clicked(move |_| request_quit(&widgets));
    }
    {
        let widgets = widgets.clone();
        window.connect_close_request(move |_| {
            request_quit(&widgets);
            glib::Propagation::Stop
        });
    }

    // Event pump.
    {
        let widgets = widgets.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = ui_rx.recv().await {
                apply_event(&widgets, ev);
            }
        });
    }

    window.present();
}

fn attach_present_gesture(label: &gtk::Label, widgets: &Rc<Widgets>, is_src: bool) {
    let gesture = gtk::GestureClick::new();
    let widgets = widgets.clone();
    gesture.connect_released(move |_, _, _, _| {
        let id = if is_src { widgets.src_id.get() } else { widgets.dst_id.get() };
        if let Some(id) = id {
            let _ = widgets.cmd_tx.send(HubCmd::PresentLister(id));
        }
    });
    label.add_controller(gesture);
}

fn request_quit(widgets: &Rc<Widgets>) {
    let running = widgets.jobs.borrow().len();
    if running == 0 {
        let _ = widgets.cmd_tx.send(HubCmd::Shutdown);
        widgets.window.close();
        if let Some(app) = widgets.window.application() {
            app.quit();
        }
        return;
    }
    let dialog = gtk::AlertDialog::builder()
        .message("Quit OpusDM?")
        .detail(format!("{running} job(s) still running."))
        .buttons(["Cancel", "Quit"])
        .cancel_button(0)
        .default_button(0)
        .build();
    let widgets = widgets.clone();
    let parent = widgets.window.clone();
    dialog.choose(
        Some(&parent),
        gio::Cancellable::NONE,
        move |result| {
            if result == Ok(1) {
                let _ = widgets.cmd_tx.send(HubCmd::Shutdown);
                widgets.window.close();
                if let Some(app) = widgets.window.application() {
                    app.quit();
                }
            }
        },
    );
}

fn short_path(p: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if home.as_deref() == Some(p) {
        return "~".to_string();
    }
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn apply_event(widgets: &Rc<Widgets>, ev: HubEvent) {
    match ev {
        HubEvent::Roles { source, dests, paths } => {
            let name_of = |id: ListerId| -> String {
                paths
                    .iter()
                    .find(|(pid, _)| *pid == id)
                    .map(|(_, p)| short_path(p))
                    .unwrap_or_else(|| "?".to_string())
            };
            let src_text = source.map(name_of).unwrap_or_else(|| "-".to_string());
            let dst_text = if dests.is_empty() {
                "-".to_string()
            } else {
                dests.iter().map(|d| name_of(*d)).collect::<Vec<_>>().join(", ")
            };
            widgets.src_label.set_text(&format!("SRC: {src_text}"));
            widgets.dst_label.set_text(&format!("DST: {dst_text}"));
            widgets.src_id.set(source);
            widgets.dst_id.set(dests.first().copied());
        }
        HubEvent::JobUpdate { job_id, label, done, total } => {
            let mut jobs = widgets.jobs.borrow_mut();
            let row = jobs.entry(job_id).or_insert_with(|| {
                let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let text = gtk::Label::new(Some(&label));
                let bar = gtk::ProgressBar::new();
                bar.set_hexpand(true);
                let cancel = gtk::Button::with_label("Cancel");
                let cmd_tx = widgets.cmd_tx.clone();
                cancel.connect_clicked(move |_| {
                    let _ = cmd_tx.send(HubCmd::CancelJob(job_id));
                });
                root.append(&text);
                root.append(&bar);
                root.append(&cancel);
                widgets.job_box.append(&root);
                JobRow { root, bar }
            });
            let fraction = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            row.bar.set_fraction(fraction.clamp(0.0, 1.0));
        }
        HubEvent::JobFinished { job_id, ok, errors } => {
            let mut jobs = widgets.jobs.borrow_mut();
            if let Some(row) = jobs.remove(&job_id) {
                if ok && errors == 0 {
                    widgets.job_box.remove(&row.root);
                } else {
                    // Keep the row, mark it, let a click dismiss it.
                    row.root.add_css_class("error");
                    let gesture = gtk::GestureClick::new();
                    let job_box = widgets.job_box.clone();
                    let root = row.root.clone();
                    gesture.connect_released(move |_, _, _, _| job_box.remove(&root));
                    row.root.add_controller(gesture);
                }
            }
        }
        HubEvent::RaiseToolbar => widgets.window.present(),
        HubEvent::ListerRegistered { .. }
        | HubEvent::ListerGone(_)
        | HubEvent::AllListersGone => {}
    }
}

fn rgba_to_hex(rgba: &gtk::gdk::RGBA) -> String {
    let r = (rgba.red() * 255.0) as u8;
    let g = (rgba.green() * 255.0) as u8;
    let b = (rgba.blue() * 255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn show_color_dialog(parent: &gtk::Window, entry: &gtk::Entry, initial_rgba: gtk::gdk::RGBA) {
    let color_dialog = gtk::ColorDialog::new();
    let entry = entry.clone();
    color_dialog.choose_rgba(
        Some(parent),
        Some(&initial_rgba),
        gio::Cancellable::NONE,
        move |result| {
            if let Ok(color) = result {
                // set_text fires "changed", which redraws the preview below.
                entry.set_text(&rgba_to_hex(&color));
            }
        },
    );
}

/// One "<label> <hex entry> <color preview>" settings row. Click the preview
/// swatch to open a GTK color picker; typing a hex value redraws it too.
/// Returns the row box and the entry, so the caller can read the chosen hex.
fn build_color_row(window: &gtk::Window, label: &str, initial_hex: &str) -> (gtk::Box, gtk::Entry) {
    let row_label = gtk::Label::new(Some(label));
    row_label.set_xalign(0.0);
    row_label.set_size_request(100, -1);

    let entry = gtk::Entry::new();
    entry.set_text(initial_hex);
    entry.set_hexpand(true);

    let preview = gtk::DrawingArea::new();
    preview.set_content_width(50);
    preview.set_content_height(30);
    {
        let entry = entry.clone();
        preview.set_draw_func(move |_da, cr, w, h| {
            if let Ok(color) = gtk::gdk::RGBA::parse(entry.text()) {
                cr.set_source_rgb(color.red() as f64, color.green() as f64, color.blue() as f64);
                cr.rectangle(0.0, 0.0, w as f64, h as f64);
                cr.fill().ok();
            }
        });
    }
    {
        let preview = preview.clone();
        entry.connect_changed(move |_| preview.queue_draw());
    }
    {
        let entry = entry.clone();
        let window = window.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            let rgba = gtk::gdk::RGBA::parse(entry.text())
                .unwrap_or_else(|_| gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
            show_color_dialog(&window, &entry, rgba);
        });
        preview.add_controller(gesture);
    }

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&row_label);
    row.append(&entry);
    row.append(&preview);
    (row, entry)
}

fn show_settings_dialog(parent: &gtk::ApplicationWindow, cmd_tx: &Sender<HubCmd>) {
    let settings_data = settings::load_settings();

    let window = gtk::Window::builder()
        .title("OpusDM Settings")
        .modal(true)
        .transient_for(parent)
        .default_width(400)
        .default_height(250)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let (src_box, src_entry) = build_color_row(&window, "SOURCE color:", &settings_data.source_color);
    vbox.append(&src_box);

    let (dst_box, dst_entry) = build_color_row(&window, "DEST color:", &settings_data.dest_color);
    vbox.append(&dst_box);

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_box.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label("Cancel");
    let save_btn = gtk::Button::with_label("Save");

    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }

    {
        let window = window.clone();
        let src = src_entry.clone();
        let dst = dst_entry.clone();
        let cmd_tx = cmd_tx.clone();
        save_btn.connect_clicked(move |_| {
            let src_hex = src.text().to_string();
            let dst_hex = dst.text().to_string();
            // Server owns the in-memory settings and persists to disk.
            let _ = cmd_tx.send(HubCmd::UpdateColors {
                source_color: src_hex,
                dest_color: dst_hex,
            });
            window.close();
        });
    }

    button_box.append(&cancel_btn);
    button_box.append(&save_btn);
    vbox.append(&button_box);

    window.set_child(Some(&vbox));
    window.present();
}

