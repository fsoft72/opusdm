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
    /// geometry re-applied, not just a window.present(). Updated live when
    /// the monitor's geometry changes (resolution switch, Xephyr resize).
    monitor_size: Cell<(i32, i32)>,
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
    /// Shows progress for every running file operation.
    progress_window: crate::progress_window::ProgressWindow,
}

fn build_ui(app: &gtk::Application, cmd_tx: Sender<HubCmd>, ui_rx: async_channel::Receiver<HubEvent>) {
    let (mon_w, mon_h) = gtk::gdk::Display::default()
        .as_ref()
        .map(crate::desktop::monitor_geometry)
        .unwrap_or((1280, 800));

    let desktop_settings = opusdm_hub::desktop_settings::load();
    let initial_settings = settings::load_settings();
    install_desktop_css(&initial_settings.theme.background);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM")
        .decorated(false)
        // Resizable on the GTK side so the window follows the X geometry we
        // pin when the screen size changes; a non-resizable window would ask
        // the WM for its old natural size back and never shrink. The WM is
        // kept from resizing it by the min = max size hints in `desktop.rs`.
        .resizable(true)
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

    let missing_nerd_font = initial_settings.theme.icon_set == opusdm_ipc::IconSetId::None
        && !opusdm_iconview::font_probe::nerd_font_available();

    let desktop_dir = opusdm_hub::desktop_files::effective_desktop_dir(
        desktop_settings.desktop_dir.as_deref(),
    );
    let (desktop_area, desktop_field) =
        build_desktop_area(desktop_dir.as_deref(), initial_settings.theme.icon_set.clone());
    if let Some(f) = &desktop_field {
        f.set_icon_size(desktop_settings.icon_size);
        f.set_grid_spacing(desktop_settings.grid_spacing);
        f.set_grid_gap(desktop_settings.grid_gap);
    }

    window.set_child(Some(&desktop_area));

    let widgets = Rc::new(Widgets {
        window: window.clone(),
        cmd_tx: cmd_tx.clone(),
        monitor_size: Cell::new((mon_w, mon_h)),
        quitting: Cell::new(false),
        desktop_watch: RefCell::new(None),
        desktop_field: RefCell::new(desktop_field.clone()),
        desktop_dir: RefCell::new(desktop_dir.clone()),
        progress_window: crate::progress_window::ProgressWindow::new(app, cmd_tx.clone()),
    });

    // Follow the screen size live: a resolution change (real mode switch, or
    // a Xephyr host-window resize) must re-pin the desktop window to the new
    // size instead of leaving it at its startup geometry.
    {
        let widgets = widgets.clone();
        opusdm_ui::watch_screen_size(move |w, h| {
            widgets.monitor_size.set((w, h));
            crate::desktop::make_desktop_window(&widgets.window, w, h);
            crate::desktop::pin_geometry(&widgets.window, w, h);
            // The window manager reacts to the new screen size on its own -
            // it shifts the desktop down by the panel's strut - so re-assert
            // the geometry once it has settled, as the startup path does.
            for delay in [150_u64, 600] {
                let window = widgets.window.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                    crate::desktop::pin_geometry(&window, w, h);
                });
            }
        });
    }

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
            let cmd_tx = cmd_tx.clone();
            field.connect_background_activate(move || {
                let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
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

    if missing_nerd_font {
        show_missing_nerd_font_dialog(&window);
    }

    // The panel dock lives in its own window on the same application.
    opusdm_panel::attach(app);

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

thread_local! {
    /// The one desktop stylesheet provider, registered on the display once
    /// and reloaded in place on every theme change so providers never stack
    /// up. GTK runs single-threaded, so a thread-local suffices and avoids
    /// requiring `CssProvider: Sync`.
    static DESKTOP_CSS_PROVIDER: gtk::CssProvider = {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        provider
    };
}

/// Install (or replace) the desktop stylesheet for `bg`. Safe to call
/// repeatedly - it reloads the single shared provider's content rather than
/// registering a new one each time.
fn install_desktop_css(bg: &opusdm_ipc::Background) {
    DESKTOP_CSS_PROVIDER.with(|p| p.load_from_data(&desktop_rule_css(bg)));
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
/// Resolve the `opusdm-theme` binary: preferably a sibling of the running
/// hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
fn theme_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("opusdm-theme");
        }
    }
    PathBuf::from("opusdm-theme")
}

/// Spawn the theme configuration app, detached.
fn launch_theme_config() {
    let bin = theme_config_bin();
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
    icon_set: opusdm_ipc::IconSetId,
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
            f.set_icon_set(icon_set);
            match opusdm_iconview::scan::read_entries(dir, false) {
                Ok(entries) => f.set_entries(&entries),
                Err(e) => eprintln!("opusdm-hub: cannot read desktop {}: {e}", dir.display()),
            }
            f.widget().set_hexpand(true);
            f.widget().set_vexpand(true);
            // Keep the icon area clear of the panel dock: the background box
            // still fills the whole monitor, but the first row of icons must
            // start below the panel's top strut instead of under it.
            f.widget().set_margin_top(opusdm_panel::PANEL_HEIGHT);
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
                    Ok(entries) => field.set_entries(&entries),
                    Err(e) => eprintln!("opusdm-hub: rearrange failed: {e}"),
                }
            });
            sub.append(&btn);
        }

        // Theme: launch the standalone opusdm-theme app. Icon grid metrics
        // and the desktop folder override live there too now.
        {
            let menu_ref = menu.clone();
            let btn = menu_button("Theme");
            btn.connect_clicked(move |_| {
                menu_ref.set_visible(false);
                launch_theme_config();
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

/// Shown once at startup when text glyphs are the active icon set but no
/// patched Nerd Font is installed - without one, `opusdm_iconview::
/// font_probe` already blanks the glyphs rather than showing hex tofu, but
/// an empty icon slot is still worse than telling the user how to fix it.
fn show_missing_nerd_font_dialog(parent: &gtk::ApplicationWindow) {
    opusdm_ui::requester_rich(
        parent,
        "Nerd Font Not Found",
        "<b>No Nerd Font is installed.</b>\n\n\
         OpusDM's text-glyph icon set needs a patched \"Nerd Font\" to \
         show file-type icons; without one, names appear with no icon \
         instead. You can also pick an image-based theme instead - see \
         \"Desktop \u{2192} Theme\".\n\n\
         <a href='https://www.nerdfonts.com/font-downloads'>https://www.nerdfonts.com/font-downloads</a>",
        &[("OK", ())],
        |_| {},
    );
}

fn show_about_dialog(parent: &gtk::ApplicationWindow) {
    opusdm_ui::requester_rich(
        parent,
        "About OpusDM",
        &format!(
            "<span size='xx-large' weight='bold'>OpusDM</span>\n\
             <i>A Linux tribute to Amiga Directory Opus 5</i>\n\n\
             Version {version}\n\n\
             Written by Fabio Rotondo\n\
             <a href='https://github.com/fsof72'>https://github.com/fsof72</a>\n\n\
             <small>This software is Open Source</small>",
            version = opusdm_ui::VERSION,
        ),
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
            let (w, h) = widgets.monitor_size.get();
            crate::desktop::make_desktop_window(&widgets.window, w, h);
            let window = widgets.window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
                crate::desktop::pin_geometry(&window, w, h);
            });
        }
        HubEvent::ThemeChanged(theme) => {
            install_desktop_css(&theme.background);
            if let Some(field) = widgets.desktop_field.borrow().as_ref() {
                field.set_icon_set(theme.icon_set);
            }
        }
        HubEvent::DesktopSettingsChanged(ds) => {
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
        HubEvent::JobUpdate {
            job_id,
            current_file,
            files_done,
            files_total,
            file_bytes_done,
            file_bytes_total,
        } => {
            widgets.progress_window.update(
                job_id,
                &current_file,
                files_done,
                files_total,
                file_bytes_done,
                file_bytes_total,
            );
        }
        HubEvent::JobFinished { job_id, .. } => {
            widgets.progress_window.finish(job_id);
        }
        // Roles no longer have a home in the hub chrome (they moved out with
        // the old toolbar strip); the server still emits them.
        HubEvent::ListerRegistered { .. }
        | HubEvent::ListerGone(_)
        | HubEvent::AllListersGone
        | HubEvent::Roles { .. } => {}
    }
}


#[cfg(test)]
mod tests {
    use super::*;

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
