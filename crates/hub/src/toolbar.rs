//! The always-on desktop: a full-screen background window that shows the
//! wallpaper and the desktop icon field, with a right-click context menu.
//! The chrome (application menu, pager, window list, monitors) lives in the
//! separate `opusdm-panel` dock window, attached here in `build_ui`.
//!
//! This module owns the application, the shared `Widgets` state, the event
//! pump and the quit flow; the desktop windows live in `desktop_windows.rs`,
//! the right-click menus and About dialog in `desktop_menu.rs`, and groups
//! and button banks in `groups_ui.rs`.

use crate::server::{HubCmd, HubEvent, ServerHandle};
use super::settings;
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use super::{desktop_windows::*, groups_ui::*};

/// Delays for re-asserting every desktop window's geometry a couple of times
/// at startup, once the panel's struts have landed (they shrink the work
/// area, which otherwise clamps the desktop windows a strip short).
const PANEL_STRUT_SETTLE_DELAYS_MS: [u64; 2] = [600, 1600];

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

/// One desktop background window and the monitor rect it was placed on -
/// kept for re-asserting its geometry on `RaiseToolbar` and monitor changes.
pub(crate) struct DesktopWindow {
    pub(crate) window: gtk::ApplicationWindow,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
}

/// Desktop-icon state: what's shown on the primary monitor's icon field and
/// how it's kept in sync with disk and with `DesktopSettings`. Grouped into
/// one struct (instead of six loose `Widgets` fields) so these invariants
/// live next to the data they govern:
/// - `watch`, `field` and `dir` are always torn down together (`None`) by a
///   full rebuild, and set together by `full_rebuild_desktop_windows`, so
///   the field, its watch and the directory it's watching never diverge.
/// - `desired_dir` is independent of monitor layout: it is what the user
///   configured (via `DesktopSettings::desktop_dir`), kept so a full rebuild
///   triggered by monitor hotplug shows the same folder instead of
///   re-resolving it, whereas `dir` is only set once a desktop window
///   actually exists to show it.
/// - `icon_set` and `settings` are cached so a desktop window built later
///   (monitor plugged in after startup) starts with the current icons and
///   grid metrics instead of stale ones from construction time.
#[derive(Default)]
pub(crate) struct DesktopState {
    /// inotify watch on the desktop directory, kept alive for the window's
    /// lifetime. `None` when there is no desktop directory or the watch failed.
    pub(crate) watch: RefCell<Option<opusdm_hub::desktop_files::DesktopWatch>>,
    /// The desktop icon field, shared with the context menu ("rearrange icons").
    /// `None` when there is no desktop directory.
    pub(crate) field: RefCell<Option<opusdm_iconview::IconField>>,
    /// The directory the desktop shows, for re-reading on "rearrange icons"
    /// and for re-targeting when `DesktopSettings::desktop_dir` changes.
    pub(crate) dir: RefCell<Option<PathBuf>>,
    /// Current icon set, kept so a desktop window built later (monitor
    /// plugged in after startup) starts with the right icons.
    pub(crate) icon_set: RefCell<opusdm_ipc::IconSetId>,
    /// Current icon grid metrics, kept for the same reason as `icon_set`.
    pub(crate) settings: RefCell<opusdm_ipc::DesktopSettings>,
    /// The desktop directory the user has configured (independent of monitor
    /// layout), kept so a full rebuild on monitor hotplug shows the same
    /// folder rather than re-reading it from disk.
    pub(crate) desired_dir: RefCell<Option<PathBuf>>,
    /// The left-out icons (`leftout.json`), owned by the GTK side: every
    /// change is saved here and the icon field refreshed.
    pub(crate) leftout: RefCell<opusdm_ipc::LeftOut>,
    /// Latest-wins gate shared by every reload of the desktop field, so a slow
    /// off-thread read never overwrites the result of a newer one.
    pub(crate) reload_gate: opusdm_hub::leftout::ReloadGate,
    /// The program groups (`groups/<id>.json`), owned by the GTK side like
    /// `leftout`: every change is saved and the field and windows refreshed.
    pub(crate) groups: RefCell<opusdm_ipc::Groups>,
    /// The group windows currently open.
    pub(crate) group_windows: crate::group_window::GroupWindows,
    /// Whether the desktop is currently hidden (mirrors `HubEvent::HiddenChanged`).
    pub(crate) hidden: Cell<bool>,
    /// The "Hide All" / "Reveal" buttons of the desktop menus, relabelled when
    /// the state changes. Buttons of destroyed windows are pruned on the way.
    pub(crate) hide_buttons: RefCell<Vec<gtk::Button>>,
}

/// Live widget handles shared by the event pump.
pub(crate) struct Widgets {
    /// One window per distinct monitor rect (mirrored outputs collapse to
    /// one - see `opusdm_ui::list_monitors`). Index 0 is the primary monitor:
    /// the only one carrying the interactive icon field: duplicating one
    /// folder's icons across monitors would make drag-and-drop and selection
    /// ambiguous, so the rest are background-only.
    pub(crate) windows: RefCell<Vec<DesktopWindow>>,
    pub(crate) cmd_tx: Sender<HubCmd>,
    /// Set once the quit confirmation has been accepted, so the `close()` it
    /// triggers does not re-enter `request_quit` and stack another dialog.
    pub(crate) quitting: Cell<bool>,
    pub(crate) desktop: DesktopState,
    /// Shows progress for every running file operation.
    pub(crate) progress_window: crate::progress_window::ProgressWindow,
    /// Re-docks the panel bars to another screen edge. Set once the panel
    /// is attached, which happens after the `Widgets` are built.
    pub(crate) set_panel_position: RefCell<Option<Rc<dyn Fn(opusdm_ipc::PanelPosition)>>>,
    /// Re-lays out the dock (edge, icon size, panel edge). Set once the dock
    /// window exists.
    pub(crate) set_dock_layout: RefCell<Option<Rc<dyn Fn(opusdm_dock::DockLayout)>>>,
    /// The button banks (`buttons/<id>.json`), owned by the GTK side.
    pub(crate) banks: Rc<RefCell<opusdm_ipc::Banks>>,
    /// The open bank windows.
    /// The saved Functions, kept in step with the hub by `HubEvent::FunctionsChanged`.
    pub(crate) functions: Rc<RefCell<Vec<opusdm_ipc::function::Function>>>,
    pub(crate) bank_windows: crate::bank_windows::BankWindows,
}

impl Widgets {
    /// The primary monitor's window - parent for dialogs and the one
    /// carrying the interactive icon field. Always present: `list_monitors`
    /// falls back to a single synthetic rect when it finds none.
    pub(crate) fn primary_window(&self) -> gtk::ApplicationWindow {
        self.windows.borrow()[0].window.clone()
    }
}

fn build_ui(app: &gtk::Application, cmd_tx: Sender<HubCmd>, ui_rx: async_channel::Receiver<HubEvent>) {
    let desktop_settings = opusdm_hub::desktop_settings::load();
    let initial_settings = settings::load_settings();
    install_desktop_css(&initial_settings.theme.background);
    opusdm_ui::apply_theme_mode(initial_settings.appearance.theme_mode);

    let desired_desktop_dir = opusdm_hub::desktop_files::effective_desktop_dir(
        desktop_settings.desktop_dir.as_deref(),
    );

    let banks = Rc::new(RefCell::new(opusdm_ipc::Banks::load()));
    let functions = Rc::new(RefCell::new(opusdm_ipc::function::load_functions()));
    let bank_windows = crate::bank_windows::BankWindows::new(banks.clone(), functions.clone(), cmd_tx.clone());

    let widgets = Rc::new(Widgets {
        windows: RefCell::new(Vec::new()),
        cmd_tx: cmd_tx.clone(),
        quitting: Cell::new(false),
        desktop: DesktopState {
            icon_set: RefCell::new(initial_settings.theme.icon_set.clone()),
            settings: RefCell::new(desktop_settings),
            desired_dir: RefCell::new(desired_desktop_dir),
            leftout: RefCell::new(opusdm_ipc::LeftOut::load()),
            groups: RefCell::new(opusdm_ipc::Groups::load()),
            ..Default::default()
        },
        progress_window: crate::progress_window::ProgressWindow::new(app, cmd_tx.clone()),
        banks,
        functions,
        bank_windows,
        set_panel_position: RefCell::new(None),
        set_dock_layout: RefCell::new(None),
    });

    // Build one desktop window per monitor, primary first.
    let display = gtk::gdk::Display::default();
    let rects = display.as_ref().map(opusdm_ui::list_monitors).unwrap_or_default();
    full_rebuild_desktop_windows(app, &widgets, rects);
    widgets.bank_windows.restore(app);

    // Follow the monitor layout live: a resolution change (real mode switch,
    // or a Xephyr host-window resize), a monitor plugged/unplugged, or a
    // mirror layout toggled at runtime must all re-place the desktop windows.
    // Same count -> just re-pin each in place (cheap, keeps the primary's
    // icon field and inotify watch alive); different count -> full rebuild.
    if let Some(d) = display.as_ref() {
        let (app, widgets) = (app.clone(), widgets.clone());
        opusdm_ui::watch_monitors(d, move |rects| reconcile_desktop_windows(&app, &widgets, rects));
    }
    {
        let (app, widgets) = (app.clone(), widgets.clone());
        opusdm_ui::watch_screen_size(move |_w, _h| {
            let rects = gtk::gdk::Display::default()
                .as_ref()
                .map(opusdm_ui::list_monitors)
                .unwrap_or_default();
            reconcile_desktop_windows(&app, &widgets, rects);
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

    // The panel dock lives in its own windows on the same application.
    let panel_position = widgets.desktop.settings.borrow().panel_position;
    *widgets.set_panel_position.borrow_mut() = Some(opusdm_panel::attach(app, panel_position));

    // The launcher dock is one more window on the same application.
    let dock_layout = dock_layout(&widgets.desktop.settings.borrow());
    *widgets.set_dock_layout.borrow_mut() = Some(opusdm_dock::attach(app, dock_layout));

    // The panel reserves a top strut, which shrinks the work area and makes
    // GTK clamp the desktop windows a strip short. Re-assert the full-monitor
    // geometry a couple of times once the panel's struts have landed.
    let widgets = widgets.clone();
    opusdm_ui::retry_after_map(&PANEL_STRUT_SETTLE_DELAYS_MS, move || {
        for dw in widgets.windows.borrow().iter() {
            crate::desktop::pin_geometry(&dw.window, dw.x, dw.y, dw.w, dw.h);
        }
    });
}

/// Always confirm before shutting everything down, whether the request comes
/// from the toolbar button, the desktop menu, or the window manager closing
/// the window. On accept: tell the server to shut down, then close.
pub(crate) fn request_quit(widgets: &Rc<Widgets>) {
    // The accepted quit calls close(), which re-enters here via
    // connect_close_request; bail so we do not stack a second dialog.
    if widgets.quitting.get() {
        return;
    }
    let parent = widgets.primary_window();
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
            widgets.bank_windows.begin_shutdown();
            // Wait for the Shutdown hooks (capped): the process exits right
            // after this returns and would kill a hook still running.
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            if widgets.cmd_tx.send(HubCmd::ShutdownAndWait(done_tx)).is_ok() {
                let cap = opusdm_hub::script_events::SHUTDOWN_HOOK_TIMEOUT + std::time::Duration::from_secs(1);
                let _ = done_rx.recv_timeout(cap);
            }
            let app = widgets.primary_window().application();
            for dw in widgets.windows.borrow().iter() {
                dw.window.close();
            }
            if let Some(app) = app {
                app.quit();
            }
        },
    );
}

fn apply_event(widgets: &Rc<Widgets>, ev: HubEvent) {
    match ev {
        HubEvent::RaiseToolbar => {
            // Re-assert every desktop window's type and geometry, not
            // present(): a redundant hub invocation hands this off to a
            // possibly stale instance whose windows may have lost their
            // desktop role or size (WM/session restart). The desktop stays
            // below the Listers, so there is nothing to raise.
            for dw in widgets.windows.borrow().iter() {
                crate::desktop::make_desktop_window(&dw.window, dw.x, dw.y, dw.w, dw.h);
                let (window, x, y, w, h) = (dw.window.clone(), dw.x, dw.y, dw.w, dw.h);
                glib::timeout_add_local_once(std::time::Duration::from_millis(WM_PLACEMENT_SETTLE_DELAY_MS), move || {
                    crate::desktop::pin_geometry(&window, x, y, w, h);
                });
            }
        }
        HubEvent::ThemeChanged(theme) => {
            install_desktop_css(&theme.background);
            *widgets.desktop.icon_set.borrow_mut() = theme.icon_set.clone();
            if let Some(field) = widgets.desktop.field.borrow().as_ref() {
                field.set_icon_set(theme.icon_set);
            }
        }
        HubEvent::HiddenChanged(hidden) => apply_hidden(widgets, hidden),
        HubEvent::FunctionsChanged(functions) => {
            *widgets.functions.borrow_mut() = functions;
            // Buttons bound to a deleted Function show insensitive.
            let ids: Vec<String> = widgets.banks.borrow().banks.iter().map(|b| b.id.clone()).collect();
            for id in ids {
                widgets.bank_windows.refresh(&id);
            }
        }
        HubEvent::Bank { action, bank } => {
            if let Some(app) = widgets.primary_window().application() {
                widgets.bank_windows.apply(&app, action, &bank);
            }
        }
        HubEvent::OpenBankManager => open_bank_manager(widgets),
        HubEvent::LeftOutAdd(paths) => {
            {
                let mut left = widgets.desktop.leftout.borrow_mut();
                left.add(&paths);
                save_leftout(&left);
            }
            reload_desktop_field(widgets);
        }
        HubEvent::LeftOutRemove(paths) => {
            {
                let mut left = widgets.desktop.leftout.borrow_mut();
                left.remove(&paths);
                save_leftout(&left);
            }
            reload_desktop_field(widgets);
        }
        HubEvent::AppearanceChanged(appearance) => {
            opusdm_ui::apply_theme_mode(appearance.theme_mode);
        }
        HubEvent::DesktopSettingsChanged(ds) => {
            *widgets.desktop.settings.borrow_mut() = ds.clone();
            let field = widgets.desktop.field.borrow().clone();
            if let Some(field) = field.as_ref() {
                field.set_icon_size(ds.icon_size);
                field.set_grid_spacing(ds.grid_spacing);
                field.set_grid_gap(ds.grid_gap);
                keep_clear_of_panel(field.widget(), ds.panel_position);
            }
            if let Some(set_position) = widgets.set_panel_position.borrow().as_ref() {
                set_position(ds.panel_position);
            }
            if let Some(set_layout) = widgets.set_dock_layout.borrow().as_ref() {
                set_layout(dock_layout(&ds));
            }

            let wanted =
                opusdm_hub::desktop_files::effective_desktop_dir(ds.desktop_dir.as_deref());
            *widgets.desktop.desired_dir.borrow_mut() = wanted.clone();
            if *widgets.desktop.dir.borrow() != wanted {
                *widgets.desktop.dir.borrow_mut() = wanted.clone();
                // A desktop that started with no directory has no icon field to
                // populate; that only comes back on the next hub start.
                if let (Some(field), Some(dir)) = (field.as_ref(), wanted.as_ref()) {
                    let field = field.clone();
                    let dir = dir.clone();
                    reload_desktop_field(widgets);
                    spawn_desktop_watch(widgets, &field, &dir);
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
