//! The always-on desktop: a full-screen background window that shows the
//! wallpaper and the desktop icon field, with a right-click context menu.
//! The chrome (application menu, pager, window list, monitors) lives in the
//! separate `opusdm-panel` dock window, attached here in `build_ui`.

use crate::server::{HubCmd, HubEvent, ServerHandle};
use super::settings;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_panel::PanelHooks;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;

/// Foreground colour of the centred desktop placeholder label.
const DESKTOP_FG: &str = "#d3d7cf";

/// Background and border of the desktop right-click menu.
const DESKTOP_MENU_BG: &str = "#3c4043";
const DESKTOP_MENU_BORDER: &str = "#1b1d1e";

/// Approximate menu width/height, reserved when clamping the menu inside the
/// desktop so a right-click near an edge still shows the whole menu.
const DESKTOP_MENU_W: i32 = 180;
const DESKTOP_MENU_H: i32 = 210;

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
    cmd_tx: Sender<HubCmd>,
    /// Primary monitor size, kept for re-asserting the desktop geometry on
    /// `RaiseToolbar` - a stale hub instance (single-instance handoff, e.g.
    /// after the WM/session restarted) needs its desktop window type and
    /// geometry re-applied, not just a window.present().
    monitor_size: (i32, i32),
    /// Set once the quit confirmation has been accepted, so the `close()` it
    /// triggers does not re-enter `request_quit` and stack another dialog.
    quitting: Cell<bool>,
    /// inotify watch on the desktop directory, kept alive for the window's
    /// lifetime. `None` when there is no desktop directory or the watch failed.
    desktop_watch: RefCell<Option<opusdm_hub::desktop_files::DesktopWatch>>,
    /// The desktop icon field, shared with the context menu ("rearrange icons").
    /// `None` when there is no desktop directory.
    desktop_field: RefCell<Option<opusdm_iconview::IconField>>,
    /// The directory the desktop shows, for re-reading on "rearrange icons"
    /// and for re-targeting when `DesktopSettings::desktop_dir` changes.
    desktop_dir: RefCell<Option<PathBuf>>,
}

fn build_ui(app: &gtk::Application, cmd_tx: Sender<HubCmd>, ui_rx: async_channel::Receiver<HubEvent>) {
    let (mon_w, mon_h) = gtk::gdk::Display::default()
        .as_ref()
        .map(crate::desktop::monitor_geometry)
        .unwrap_or((1280, 800));

    let desktop_settings = opusdm_hub::desktop_settings::load();
    install_desktop_css(&desktop_settings.background);

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

    let desktop_dir = opusdm_hub::desktop_files::effective_desktop_dir(
        desktop_settings.desktop_dir.as_deref(),
    );
    let (desktop_area, desktop_field) = build_desktop_area(desktop_dir.as_deref());
    if let Some(f) = &desktop_field {
        f.set_icon_size(desktop_settings.icon_size);
        f.set_grid_spacing(desktop_settings.grid_spacing);
        f.set_grid_gap(desktop_settings.grid_gap);
    }

    window.set_child(Some(&desktop_area));

    let widgets = Rc::new(Widgets {
        window: window.clone(),
        cmd_tx: cmd_tx.clone(),
        monitor_size: (mon_w, mon_h),
        quitting: Cell::new(false),
        desktop_watch: RefCell::new(None),
        desktop_field: RefCell::new(desktop_field.clone()),
        desktop_dir: RefCell::new(desktop_dir.clone()),
    });

    // Right-click the desktop for the context menu.
    attach_desktop_menu(&desktop_area, &widgets);

    // Desktop icon field: open on activate, copy on drop, live refresh on change.
    if let (Some(field), Some(dir)) = (desktop_field, desktop_dir.clone()) {
        {
            let cmd_tx = cmd_tx.clone();
            let field_for_open = field.clone();
            field.connect_activate(move |i| {
                let Some(entry) = field_for_open.entry(i) else { return };
                if entry.kind == opusdm_iconview::IconKind::Dir {
                    let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(entry.path) });
                } else {
                    let uri = format!("file://{}", entry.path.display());
                    let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
                }
            });
        }
        {
            let widgets = widgets.clone();
            field.connect_drop_uris(move |srcs, _target, shift| {
                if srcs.is_empty() {
                    return;
                }
                let Some(dest_dir) = widgets.desktop_dir.borrow().clone() else { return };
                let op = if shift { opusdm_ipc::Op::Move } else { opusdm_ipc::Op::Copy };
                let _ = widgets.cmd_tx.send(HubCmd::RunJob { op, srcs, dest_dir });
            });
        }
        spawn_desktop_watch(&widgets, &field, &dir);
    }

    // Window close (from the WM): always confirm before shutting down.
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

    // The panel dock lives in its own window on the same application. Its
    // "Settings" item reuses the hub settings dialog.
    let on_settings = {
        let window = window.clone();
        let cmd_tx = cmd_tx.clone();
        Box::new(move || show_settings_dialog(&window, &cmd_tx))
    };
    opusdm_panel::attach(app, PanelHooks { on_settings });

    // The panel reserves a top strut, which shrinks the work area and makes
    // GTK clamp this desktop window a strip short. Re-assert the full-monitor
    // geometry a couple of times once the panel's strut has landed.
    for delay in [600_u64, 1600] {
        let window = window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
            crate::desktop::pin_geometry(&window, mon_w, mon_h);
        });
    }
}

/// The full desktop stylesheet: the `.opusdm-desktop` background rule built
/// from `bg`, plus the fixed placeholder-label and context-menu rules.
fn desktop_rule_css(bg: &opusdm_ipc::Background) -> String {
    format!(
        ".opusdm-desktop {{ {bgdecl} }}\n\
         .opusdm-desktop-label {{ color: {DESKTOP_FG}; font-size: 24px; font-weight: bold; }}\n\
         .opusdm-desktop-menu {{ background-color: {DESKTOP_MENU_BG}; color: {DESKTOP_FG}; \
             border: 1px solid {DESKTOP_MENU_BORDER}; border-radius: 4px; padding: 4px; }}\n\
         .opusdm-desktop-menu button {{ padding: 4px 12px; }}",
        bgdecl = opusdm_ipc::background_css(bg),
    )
}

/// Install (or replace) the desktop stylesheet for `bg`. Safe to call
/// repeatedly - each call adds a fresh provider at APPLICATION priority,
/// which overrides earlier ones.
fn install_desktop_css(bg: &opusdm_ipc::Background) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&desktop_rule_css(bg));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Start (or restart) the inotify watch on `dir` and the local future that
/// reloads the desktop icon field when the directory changes. Replaces any
/// previous watch stored in `widgets.desktop_watch`.
fn spawn_desktop_watch(
    widgets: &Rc<Widgets>,
    field: &opusdm_iconview::IconField,
    dir: &Path,
) {
    // Drop the old watch first so its thread exits before we start another.
    *widgets.desktop_watch.borrow_mut() = None;
    match opusdm_hub::desktop_files::DesktopWatch::start(dir.to_path_buf()) {
        Ok((watch, rx)) => {
            let field = field.clone();
            let dir = dir.to_path_buf();
            glib::spawn_future_local(async move {
                while rx.recv().await.is_ok() {
                    match opusdm_iconview::scan::read_entries(&dir, false) {
                        Ok(entries) => field.set_entries(&entries),
                        Err(e) => eprintln!("opusdm-hub: desktop reload failed: {e}"),
                    }
                }
            });
            *widgets.desktop_watch.borrow_mut() = Some(watch);
        }
        Err(e) => eprintln!("opusdm-hub: desktop watch unavailable: {e}"),
    }
}

/// Resolve the `opus-desktop-config` binary: preferably a sibling of the
/// running hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
fn desktop_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("opus-desktop-config");
        }
    }
    PathBuf::from("opus-desktop-config")
}

/// Spawn the desktop configuration app, detached. Errors are logged and
/// otherwise ignored - a missing binary must not take down the hub.
fn launch_desktop_config() {
    let bin = desktop_config_bin();
    if let Err(e) = std::process::Command::new(&bin).spawn() {
        eprintln!("opusdm-hub: cannot launch {}: {e}", bin.display());
    }
}

/// Build the desktop area that fills the window below the toolbar, wrapped in
/// an overlay so the context menu can be laid on top of it. When `desktop_dir`
/// is `Some`, an icon field in `Free` mode fills it and its initial entries are
/// read; otherwise the flat placeholder label is kept.
fn build_desktop_area(
    desktop_dir: Option<&Path>,
) -> (gtk::Overlay, Option<opusdm_iconview::IconField>) {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bg.add_css_class("opusdm-desktop");
    bg.set_hexpand(true);
    bg.set_vexpand(true);

    let field = match desktop_dir {
        Some(dir) => {
            let f = opusdm_iconview::IconField::new();
            f.set_mode(opusdm_iconview::LayoutMode::Free);
            match opusdm_iconview::scan::read_entries(dir, false) {
                Ok(entries) => f.set_entries(&entries),
                Err(e) => eprintln!("opusdm-hub: cannot read desktop {}: {e}", dir.display()),
            }
            f.widget().set_hexpand(true);
            f.widget().set_vexpand(true);
            bg.append(f.widget());
            Some(f)
        }
        None => {
            let label = gtk::Label::new(Some("OpusDM"));
            label.add_css_class("opusdm-desktop-label");
            label.set_hexpand(true);
            label.set_vexpand(true);
            label.set_halign(gtk::Align::Center);
            label.set_valign(gtk::Align::Center);
            bg.append(&label);
            None
        }
    };

    overlay.set_child(Some(&bg));
    (overlay, field)
}

/// Wire the right-click context menu onto the desktop overlay. The desktop
/// window never holds the input focus, so a `Popover` parented to it can never
/// grab; the menu is a plain box laid on the overlay instead, positioned with
/// margins at the click point.
fn attach_desktop_menu(overlay: &gtk::Overlay, widgets: &Rc<Widgets>) {
    let menu = build_desktop_menu(widgets);
    menu.set_halign(gtk::Align::Start);
    menu.set_valign(gtk::Align::Start);
    menu.set_visible(false);
    overlay.add_overlay(&menu);

    // Secondary click: place the menu at the pointer and show it.
    let open = gtk::GestureClick::new();
    open.set_button(gtk::gdk::BUTTON_SECONDARY);
    {
        let menu = menu.clone();
        let overlay = overlay.clone();
        open.connect_pressed(move |gesture, n_press, x, y| {
            if n_press != 1 {
                return;
            }
            let max_x = (overlay.width() - DESKTOP_MENU_W).max(0);
            let max_y = (overlay.height() - DESKTOP_MENU_H).max(0);
            menu.set_margin_start((x as i32).min(max_x));
            menu.set_margin_top((y as i32).min(max_y));
            menu.set_visible(true);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
    }
    overlay.add_controller(open);

    // Primary click outside the open menu: dismiss it. Clicks on the menu
    // buttons are handled by the buttons themselves.
    let dismiss = gtk::GestureClick::new();
    dismiss.set_button(gtk::gdk::BUTTON_PRIMARY);
    {
        let menu = menu.clone();
        let overlay = overlay.clone();
        dismiss.connect_pressed(move |_, _, x, y| {
            if !menu.get_visible() {
                return;
            }
            if let Some(b) = menu.compute_bounds(&overlay) {
                let (x, y) = (x as f32, y as f32);
                let inside = x >= b.x()
                    && x < b.x() + b.width()
                    && y >= b.y()
                    && y < b.y() + b.height();
                if inside {
                    return;
                }
            }
            menu.set_visible(false);
        });
    }
    overlay.add_controller(dismiss);
}

/// Flat, left-aligned menu button, matching the Lister context menu look.
fn menu_button(title: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(title);
    btn.add_css_class("flat");
    btn.set_halign(gtk::Align::Fill);
    if let Some(child) = btn.child().and_downcast::<gtk::Label>() {
        child.set_xalign(0.0);
    }
    btn
}

/// The desktop context menu box: New Lister / --- / Settings / About / ---
/// / Quit. Every item hides the menu before running its action.
fn build_desktop_menu(widgets: &Rc<Widgets>) -> gtk::Box {
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.add_css_class("opusdm-desktop-menu");

    // New Lister
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("New Lister");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
        });
        menu.append(&btn);
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Settings
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Settings");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            show_settings_dialog(&widgets.window, &widgets.cmd_tx);
        });
        menu.append(&btn);
    }

    // About
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("About");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            show_about_dialog(&widgets.window);
        });
        menu.append(&btn);
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Desktop submenu: an expander so the hand-rolled flyout can still nest
    // without a real GtkPopoverMenu (the desktop window cannot grab focus).
    {
        let sub = gtk::Box::new(gtk::Orientation::Vertical, 2);
        sub.set_margin_start(12);

        // Rearrange icons: re-read the desktop, sort dirs-first by name, and
        // hand the sorted list back so the field auto-slots it into the grid
        // (set_entries clears any manual positions).
        {
            let widgets = widgets.clone();
            let menu_ref = menu.clone();
            let btn = menu_button("Rearrange icons");
            btn.connect_clicked(move |_| {
                menu_ref.set_visible(false);
                let field = widgets.desktop_field.borrow().clone();
                let dir = widgets.desktop_dir.borrow().clone();
                let (Some(field), Some(dir)) = (field, dir) else {
                    return;
                };
                match opusdm_iconview::scan::read_entries(&dir, false) {
                    Ok(mut entries) => {
                        opusdm_iconview::scan::sort_dirs_first(&mut entries);
                        field.set_entries(&entries);
                    }
                    Err(e) => eprintln!("opusdm-hub: rearrange failed: {e}"),
                }
            });
            sub.append(&btn);
        }

        // Settings: launch the standalone opus-desktop-config app.
        {
            let menu_ref = menu.clone();
            let btn = menu_button("Settings");
            btn.connect_clicked(move |_| {
                menu_ref.set_visible(false);
                launch_desktop_config();
            });
            sub.append(&btn);
        }

        let expander = gtk::Expander::new(Some("Desktop"));
        expander.set_child(Some(&sub));
        menu.append(&expander);
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Quit always asks for confirmation before closing everything.
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Quit");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            request_quit(&widgets);
        });
        menu.append(&btn);
    }

    menu
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

/// Always confirm before shutting everything down, whether the request comes
/// from the toolbar button, the desktop menu, or the window manager closing
/// the window. On accept: tell the server to shut down, then close.
fn request_quit(widgets: &Rc<Widgets>) {
    // The accepted quit calls close(), which re-enters here via
    // connect_close_request; bail so we do not stack a second dialog.
    if widgets.quitting.get() {
        return;
    }
    let parent = widgets.window.clone();
    let widgets = widgets.clone();
    opusdm_ui::requester(
        &parent,
        "Request",
        "Quit OpusDM?",
        Some("This will close every Lister and the desktop."),
        &[("Quit", true), ("Cancel", false)],
        move |quit| {
            if !quit {
                return;
            }
            widgets.quitting.set(true);
            let _ = widgets.cmd_tx.send(HubCmd::Shutdown);
            widgets.window.close();
            if let Some(app) = widgets.window.application() {
                app.quit();
            }
        },
    );
}

fn apply_event(widgets: &Rc<Widgets>, ev: HubEvent) {
    match ev {
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
        HubEvent::DesktopSettingsChanged(ds) => {
            install_desktop_css(&ds.background);
            let field = widgets.desktop_field.borrow().clone();
            if let Some(field) = field.as_ref() {
                field.set_icon_size(ds.icon_size);
                field.set_grid_spacing(ds.grid_spacing);
                field.set_grid_gap(ds.grid_gap);
            }

            let wanted =
                opusdm_hub::desktop_files::effective_desktop_dir(ds.desktop_dir.as_deref());
            if *widgets.desktop_dir.borrow() != wanted {
                *widgets.desktop_dir.borrow_mut() = wanted.clone();
                // A desktop that started with no directory has no icon field to
                // populate; that only comes back on the next hub start.
                if let (Some(field), Some(dir)) = (field.as_ref(), wanted.as_ref()) {
                    match opusdm_iconview::scan::read_entries(dir, false) {
                        Ok(entries) => field.set_entries(&entries),
                        Err(e) => {
                            eprintln!("opusdm-hub: cannot read desktop {}: {e}", dir.display())
                        }
                    }
                    spawn_desktop_watch(widgets, field, dir);
                }
            }
        }
        // Roles and job progress no longer have a home in the hub chrome
        // (they moved out with the old toolbar strip); the server still emits
        // them and the requesting Lister still shows its own progress.
        HubEvent::ListerRegistered { .. }
        | HubEvent::ListerGone(_)
        | HubEvent::AllListersGone
        | HubEvent::Roles { .. }
        | HubEvent::JobUpdate { .. }
        | HubEvent::JobFinished { .. } => {}
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_config_bin_is_named_correctly_and_absolute_when_possible() {
        let p = desktop_config_bin();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("opus-desktop-config"));
        if std::env::current_exe().is_ok() {
            assert!(p.is_absolute());
        }
    }

    #[test]
    fn desktop_rule_css_embeds_the_background_declarations() {
        let mut bg = opusdm_ipc::Background::default();
        bg.mode = opusdm_ipc::BackgroundMode::Gradient;
        bg.gradient_angle = 30;
        let css = desktop_rule_css(&bg);
        assert!(css.contains(".opusdm-desktop {"));
        assert!(css.contains("linear-gradient(30deg,"));
        assert!(css.contains(".opusdm-desktop-label {")); // other rules kept
        assert!(css.contains(".opusdm-desktop-menu {"));
    }

    /// Parse the generated desktop stylesheet with the real GTK CSS parser for
    /// every background mode. Self-skips when no display is available.
    #[test]
    fn desktop_rule_css_parses_without_error() {
        if gtk::init().is_err() {
            return;
        }
        let provider = gtk::CssProvider::new();
        let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        {
            let errors = errors.clone();
            provider.connect_parsing_error(move |_, section, err| {
                errors.borrow_mut().push(format!("{section}: {err}"));
            });
        }
        for mode in [
            opusdm_ipc::BackgroundMode::Solid,
            opusdm_ipc::BackgroundMode::Gradient,
            opusdm_ipc::BackgroundMode::Image,
        ] {
            let mut bg = opusdm_ipc::Background::default();
            bg.mode = mode;
            bg.image_path = Some(std::path::PathBuf::from("/tmp/opusdm-test bg.png"));
            provider.load_from_data(&desktop_rule_css(&bg));
            assert!(errors.borrow().is_empty(), "{mode:?}: {:?}", errors.borrow());
        }
    }
}
