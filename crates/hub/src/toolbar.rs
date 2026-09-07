//! The always-on desktop: a full-screen background window with the toolbar
//! (New Lister, SRC/DST indicators, job progress, Quit) docked along its top
//! edge and a placeholder desktop area below it.

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

/// Natural height of the top toolbar strip, in pixels. Layout only - the
/// desktop window itself fills the whole monitor.
const TOOLBAR_HEIGHT: i32 = 42;

/// Background colour of the desktop area. Kept as a constant here so it can
/// move into the settings model later without touching the layout code.
const DESKTOP_BG: &str = "#2e3436";

/// Foreground colour of the centred desktop placeholder label.
const DESKTOP_FG: &str = "#d3d7cf";

/// Build and run the desktop application. Returns on quit.
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
    /// Primary monitor size, kept for re-asserting the desktop geometry on
    /// `RaiseToolbar` - a stale hub instance (single-instance handoff, e.g.
    /// after the WM/session restarted) needs its desktop window type and
    /// geometry re-applied, not just a window.present().
    monitor_size: (i32, i32),
}

struct JobRow {
    root: gtk::Box,
    bar: gtk::ProgressBar,
}

fn build_ui(app: &gtk::Application, cmd_tx: Sender<HubCmd>, ui_rx: async_channel::Receiver<HubEvent>) {
    let (mon_w, mon_h) = gtk::gdk::Display::default()
        .as_ref()
        .map(crate::desktop::monitor_geometry)
        .unwrap_or((1280, 800));

    install_desktop_css();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM")
        .decorated(false)
        .resizable(false)
        .default_width(mon_w)
        .default_height(mon_h)
        .build();

    // Turn the window into the WM desktop once its surface exists, then
    // re-assert the geometry after the WM has run its placement policy.
    window.connect_realize(move |w| crate::desktop::make_desktop_window(w, mon_w, mon_h));
    window.connect_map(move |w| {
        crate::desktop::make_desktop_window(w, mon_w, mon_h);
        let w = w.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
            crate::desktop::pin_geometry(&w, mon_w, mon_h);
        });
    });

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.set_margin_top(2);
    bar.set_margin_bottom(2);
    bar.set_margin_start(6);
    bar.set_margin_end(6);
    bar.set_size_request(-1, TOOLBAR_HEIGHT);

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

    let about_btn = gtk::Button::with_label("About");
    {
        let window = window.clone();
        about_btn.connect_clicked(move |_| show_about_dialog(&window));
    }
    bar.append(&about_btn);

    let quit_btn = gtk::Button::with_label("Quit");
    bar.append(&quit_btn);

    root.append(&bar);

    let job_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    job_box.set_margin_start(6);
    job_box.set_margin_end(6);
    root.append(&job_box);

    root.append(&build_desktop_area());

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
        monitor_size: (mon_w, mon_h),
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

/// Register the CSS that paints the desktop background and placeholder. Runs
/// once per process; a second call just replaces the same provider.
fn install_desktop_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        ".opusdm-desktop {{ background-color: {DESKTOP_BG}; }}\n\
         .opusdm-desktop-label {{ color: {DESKTOP_FG}; font-size: 24px; font-weight: bold; }}"
    ));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Build the desktop area that fills the window below the toolbar: a flat
/// background with a centred placeholder label.
fn build_desktop_area() -> gtk::Box {
    let area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    area.add_css_class("opusdm-desktop");
    area.set_hexpand(true);
    area.set_vexpand(true);

    let label = gtk::Label::new(Some("OpusDM"));
    label.add_css_class("opusdm-desktop-label");
    label.set_hexpand(true);
    label.set_vexpand(true);
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::Center);
    area.append(&label);

    area
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

fn show_about_dialog(parent: &gtk::ApplicationWindow) {
    opusdm_ui::requester_rich(
        parent,
        "About OpusDM",
        "<span size='xx-large' weight='bold'>OpusDM</span>\n\
         <i>A Linux tribute to Amiga Directory Opus 5</i>\n\n\
         Written by Fabio Rotondo\n\
         <a href='https://github.com/fsof72'>https://github.com/fsof72</a>\n\n\
         <small>This software is Open Source</small>",
        &[("OK", ())],
        |_| {},
    );
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
    let parent = widgets.window.clone();
    let widgets = widgets.clone();
    opusdm_ui::requester(
        &parent,
        "Request",
        "Quit OpusDM?",
        Some(&format!("{running} job(s) still running.")),
        &[("Quit", true), ("Cancel", false)],
        move |quit| {
            if !quit {
                return;
            }
            let _ = widgets.cmd_tx.send(HubCmd::Shutdown);
            widgets.window.close();
            if let Some(app) = widgets.window.application() {
                app.quit();
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
                bar.set_valign(gtk::Align::Center);
                let cancel = gtk::Button::with_label("Cancel");
                cancel.set_valign(gtk::Align::Center);
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
        HubEvent::JobFinished { job_id, ok, errors, cancelled } => {
            let mut jobs = widgets.jobs.borrow_mut();
            if let Some(row) = jobs.remove(&job_id) {
                // A cancelled job just disappears; no error to acknowledge.
                if cancelled || (ok && errors == 0) {
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
        HubEvent::RaiseToolbar => {
            // Re-assert the desktop window type and geometry, not present():
            // a redundant hub invocation hands this off to a possibly stale
            // instance whose window may have lost its desktop role or size
            // (WM/session restart). The desktop stays below the Listers, so
            // there is nothing to raise.
            let (w, h) = widgets.monitor_size;
            crate::desktop::make_desktop_window(&widgets.window, w, h);
            let window = widgets.window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
                crate::desktop::pin_geometry(&window, w, h);
            });
        }
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

/// One settings section: a bold heading plus a Background and a Text color
/// row. Returns the two entries so the caller can read the chosen hex on
/// Save.
fn add_color_section(
    vbox: &gtk::Box,
    window: &gtk::Window,
    title: &str,
    bg_hex: &str,
    fg_hex: &str,
) -> (gtk::Entry, gtk::Entry) {
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    vbox.append(&heading);

    let (bg_row, bg_entry) = build_color_row(window, "Background:", bg_hex);
    vbox.append(&bg_row);
    let (fg_row, fg_entry) = build_color_row(window, "Text:", fg_hex);
    vbox.append(&fg_row);

    (bg_entry, fg_entry)
}

/// The ten colour entries of the settings dialog, in the order
/// [`ThemePreset::colors`] fills them.
struct ColorEntries {
    source_bg: gtk::Entry,
    source_fg: gtk::Entry,
    dest_bg: gtk::Entry,
    dest_fg: gtk::Entry,
    dir_bg: gtk::Entry,
    dir_fg: gtk::Entry,
    file_bg: gtk::Entry,
    file_fg: gtk::Entry,
    selected_bg: gtk::Entry,
    selected_fg: gtk::Entry,
}

impl ColorEntries {
    /// Read the entries back into a `ColorSettings`.
    fn read(&self) -> opusdm_ipc::ColorSettings {
        opusdm_ipc::ColorSettings {
            source_bg: self.source_bg.text().to_string(),
            source_fg: self.source_fg.text().to_string(),
            dest_bg: self.dest_bg.text().to_string(),
            dest_fg: self.dest_fg.text().to_string(),
            dir_bg: self.dir_bg.text().to_string(),
            dir_fg: self.dir_fg.text().to_string(),
            file_bg: self.file_bg.text().to_string(),
            file_fg: self.file_fg.text().to_string(),
            selected_bg: self.selected_bg.text().to_string(),
            selected_fg: self.selected_fg.text().to_string(),
        }
    }

    /// Overwrite every entry, which repaints the preview swatches too.
    fn write(&self, c: &opusdm_ipc::ColorSettings) {
        self.source_bg.set_text(&c.source_bg);
        self.source_fg.set_text(&c.source_fg);
        self.dest_bg.set_text(&c.dest_bg);
        self.dest_fg.set_text(&c.dest_fg);
        self.dir_bg.set_text(&c.dir_bg);
        self.dir_fg.set_text(&c.dir_fg);
        self.file_bg.set_text(&c.file_bg);
        self.file_fg.set_text(&c.file_fg);
        self.selected_bg.set_text(&c.selected_bg);
        self.selected_fg.set_text(&c.selected_fg);
    }
}

/// Which preset id the given colours correspond to, or `PRESET_CUSTOM` when
/// they match none. Recomputed on Save so hand-edited colours stop claiming to
/// be a named preset.
fn preset_id_for(colors: &opusdm_ipc::ColorSettings) -> String {
    opusdm_ipc::ThemePreset::ALL
        .into_iter()
        .find(|p| &p.colors() == colors)
        .map(|p| p.id().to_string())
        .unwrap_or_else(|| opusdm_ipc::PRESET_CUSTOM.to_string())
}

/// The non-colour appearance toggles of the settings dialog.
struct AppearanceChecks {
    file_icons: gtk::CheckButton,
    type_indicators: gtk::CheckButton,
    monospace_list: gtk::CheckButton,
    theme_window: gtk::CheckButton,
}

impl AppearanceChecks {
    /// Read the toggles back, tagging the colours with the preset they match.
    fn read(&self, colors: &opusdm_ipc::ColorSettings) -> opusdm_ipc::Appearance {
        opusdm_ipc::Appearance {
            preset: preset_id_for(colors),
            file_icons: self.file_icons.is_active(),
            type_indicators: self.type_indicators.is_active(),
            monospace_list: self.monospace_list.is_active(),
            theme_window: self.theme_window.is_active(),
        }
    }
}

/// The "Theme" section: a preset dropdown that fills every colour entry at
/// once, plus the appearance toggles.
fn add_theme_section(
    vbox: &gtk::Box,
    current_preset: &str,
    appearance: &opusdm_ipc::Appearance,
) -> (gtk::DropDown, AppearanceChecks) {
    let heading = gtk::Label::new(Some("THEME"));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    vbox.append(&heading);

    // Index 0 is "Custom", so a hand-edited scheme has something to show.
    let mut labels = vec![opusdm_ipc::PRESET_CUSTOM.to_string()];
    labels.extend(opusdm_ipc::ThemePreset::ALL.iter().map(|p| p.label().to_string()));
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&label_refs);
    let selected = opusdm_ipc::ThemePreset::ALL
        .iter()
        .position(|p| p.id() == current_preset)
        .map(|i| i as u32 + 1)
        .unwrap_or(0);
    dropdown.set_selected(selected);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let row_label = gtk::Label::new(Some("Preset:"));
    row_label.set_xalign(0.0);
    row_label.set_size_request(100, -1);
    dropdown.set_hexpand(true);
    row.append(&row_label);
    row.append(&dropdown);
    vbox.append(&row);

    let checks = AppearanceChecks {
        file_icons: gtk::CheckButton::with_label("File type icons (requires a Nerd Font)"),
        type_indicators: gtk::CheckButton::with_label("Type indicators after names (/ * @ = |)"),
        monospace_list: gtk::CheckButton::with_label("Monospace file list"),
        theme_window: gtk::CheckButton::with_label("Apply theme colours to the whole window"),
    };
    checks.file_icons.set_active(appearance.file_icons);
    checks.type_indicators.set_active(appearance.type_indicators);
    checks.monospace_list.set_active(appearance.monospace_list);
    checks.theme_window.set_active(appearance.theme_window);
    for c in [
        &checks.file_icons,
        &checks.type_indicators,
        &checks.monospace_list,
        &checks.theme_window,
    ] {
        vbox.append(c);
    }

    (dropdown, checks)
}

/// The "Behaviour" section. Returns the auto-refresh toggle.
fn add_behavior_section(vbox: &gtk::Box, behavior: &opusdm_ipc::Behavior) -> gtk::CheckButton {
    let heading = gtk::Label::new(Some("BEHAVIOUR"));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    vbox.append(&heading);

    let auto_refresh =
        gtk::CheckButton::with_label("Auto-refresh the list when the directory changes (inotify)");
    auto_refresh.set_active(behavior.auto_refresh);
    vbox.append(&auto_refresh);

    auto_refresh
}

fn show_settings_dialog(parent: &gtk::ApplicationWindow, cmd_tx: &Sender<HubCmd>) {
    let settings_data = settings::load_settings();
    let c = &settings_data.colors;

    let shell = opusdm_ui::DialogShell::new(parent, "OpusDM Settings")
        .size(460, 620)
        .scrolled();
    let window = shell.window().clone();
    let vbox = shell.content();

    let (preset_dd, appearance_checks) =
        add_theme_section(vbox, &settings_data.appearance.preset, &settings_data.appearance);
    let auto_refresh = add_behavior_section(vbox, &settings_data.behavior);

    let (src_bg, src_fg) = add_color_section(vbox, &window, "SOURCE", &c.source_bg, &c.source_fg);
    let (dst_bg, dst_fg) = add_color_section(vbox, &window, "DEST", &c.dest_bg, &c.dest_fg);
    let (dir_bg, dir_fg) = add_color_section(vbox, &window, "DIRECTORY", &c.dir_bg, &c.dir_fg);
    let (file_bg, file_fg) = add_color_section(vbox, &window, "FILE", &c.file_bg, &c.file_fg);
    let (sel_bg, sel_fg) =
        add_color_section(vbox, &window, "SELECTION", &c.selected_bg, &c.selected_fg);

    let entries = Rc::new(ColorEntries {
        source_bg: src_bg,
        source_fg: src_fg,
        dest_bg: dst_bg,
        dest_fg: dst_fg,
        dir_bg,
        dir_fg,
        file_bg,
        file_fg,
        selected_bg: sel_bg,
        selected_fg: sel_fg,
    });

    // Selecting a preset fills every entry; index 0 ("Custom") leaves them be.
    {
        let entries = entries.clone();
        preset_dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let Some(preset) = idx.checked_sub(1).and_then(|i| opusdm_ipc::ThemePreset::ALL.get(i))
            else {
                return;
            };
            entries.write(&preset.colors());
        });
    }

    shell.close_button("Cancel");
    let save_btn = shell.button("Save");

    {
        let window = window.clone();
        let cmd_tx = cmd_tx.clone();
        save_btn.connect_clicked(move |_| {
            let colors = entries.read();
            let appearance = appearance_checks.read(&colors);
            let behavior = opusdm_ipc::Behavior {
                auto_refresh: auto_refresh.is_active(),
                // Not exposed in the dialog yet; keep whatever settings.json has.
                auto_refresh_debounce_ms: settings_data.behavior.auto_refresh_debounce_ms,
            };
            // Server owns the in-memory settings and persists to disk.
            let _ = cmd_tx.send(HubCmd::UpdateSettings(Box::new(opusdm_ipc::Settings {
                colors,
                appearance,
                behavior,
            })));
            window.close();
        });
    }

    shell.present();
}

