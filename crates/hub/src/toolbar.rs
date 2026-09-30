//! The always-on desktop: a full-screen background window that shows the
//! wallpaper and the desktop icon field, with a right-click context menu.
//! The chrome (application menu, pager, window list, monitors) lives in the
//! separate `opusdm-panel` dock window, attached here in `build_ui`.

use crate::server::{HubCmd, HubEvent, ServerHandle};
use opusdm_ipc::bindings::ScriptEvent;
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
const DESKTOP_MENU_H: i32 = 270;

/// Delay before re-asserting a desktop window's geometry once, after the WM
/// has had a chance to run its placement policy (on first map) or after a
/// redundant `RaiseToolbar` finds a possibly stale window.
const WM_PLACEMENT_SETTLE_DELAY_MS: u64 = 80;

/// Delays for re-asserting a desktop window's geometry a couple of times
/// after a resolution change: the WM reacts to the new screen size on its
/// own (shifting the desktop down by the panel's strut), so this re-asserts
/// once it has settled.
const RESOLUTION_CHANGE_SETTLE_DELAYS_MS: [u64; 2] = [150, 600];

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
struct DesktopWindow {
    window: gtk::ApplicationWindow,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
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
struct DesktopState {
    /// inotify watch on the desktop directory, kept alive for the window's
    /// lifetime. `None` when there is no desktop directory or the watch failed.
    watch: RefCell<Option<opusdm_hub::desktop_files::DesktopWatch>>,
    /// The desktop icon field, shared with the context menu ("rearrange icons").
    /// `None` when there is no desktop directory.
    field: RefCell<Option<opusdm_iconview::IconField>>,
    /// The directory the desktop shows, for re-reading on "rearrange icons"
    /// and for re-targeting when `DesktopSettings::desktop_dir` changes.
    dir: RefCell<Option<PathBuf>>,
    /// Current icon set, kept so a desktop window built later (monitor
    /// plugged in after startup) starts with the right icons.
    icon_set: RefCell<opusdm_ipc::IconSetId>,
    /// Current icon grid metrics, kept for the same reason as `icon_set`.
    settings: RefCell<opusdm_ipc::DesktopSettings>,
    /// The desktop directory the user has configured (independent of monitor
    /// layout), kept so a full rebuild on monitor hotplug shows the same
    /// folder rather than re-reading it from disk.
    desired_dir: RefCell<Option<PathBuf>>,
    /// The left-out icons (`leftout.json`), owned by the GTK side: every
    /// change is saved here and the icon field refreshed.
    leftout: RefCell<opusdm_ipc::LeftOut>,
    /// Latest-wins gate shared by every reload of the desktop field, so a slow
    /// off-thread read never overwrites the result of a newer one.
    reload_gate: opusdm_hub::leftout::ReloadGate,
    /// The program groups (`groups/<id>.json`), owned by the GTK side like
    /// `leftout`: every change is saved and the field and windows refreshed.
    groups: RefCell<opusdm_ipc::Groups>,
    /// The group windows currently open.
    group_windows: crate::group_window::GroupWindows,
    /// Whether the desktop is currently hidden (mirrors `HubEvent::HiddenChanged`).
    hidden: Cell<bool>,
    /// The "Hide All" / "Reveal" buttons of the desktop menus, relabelled when
    /// the state changes. Buttons of destroyed windows are pruned on the way.
    hide_buttons: RefCell<Vec<gtk::Button>>,
}

/// Live widget handles shared by the event pump.
struct Widgets {
    /// One window per distinct monitor rect (mirrored outputs collapse to
    /// one - see `opusdm_ui::list_monitors`). Index 0 is the primary monitor:
    /// the only one carrying the interactive icon field: duplicating one
    /// folder's icons across monitors would make drag-and-drop and selection
    /// ambiguous, so the rest are background-only.
    windows: RefCell<Vec<DesktopWindow>>,
    cmd_tx: Sender<HubCmd>,
    /// Set once the quit confirmation has been accepted, so the `close()` it
    /// triggers does not re-enter `request_quit` and stack another dialog.
    quitting: Cell<bool>,
    desktop: DesktopState,
    /// Shows progress for every running file operation.
    progress_window: crate::progress_window::ProgressWindow,
    /// Re-docks the panel bars to another screen edge. Set once the panel
    /// is attached, which happens after the `Widgets` are built.
    set_panel_position: RefCell<Option<Rc<dyn Fn(opusdm_ipc::PanelPosition)>>>,
    /// Re-lays out the dock (edge, icon size, panel edge). Set once the dock
    /// window exists.
    set_dock_layout: RefCell<Option<Rc<dyn Fn(opusdm_dock::DockLayout)>>>,
}

impl Widgets {
    /// The primary monitor's window - parent for dialogs and the one
    /// carrying the interactive icon field. Always present: `list_monitors`
    /// falls back to a single synthetic rect when it finds none.
    fn primary_window(&self) -> gtk::ApplicationWindow {
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
        set_panel_position: RefCell::new(None),
        set_dock_layout: RefCell::new(None),
    });

    // Build one desktop window per monitor, primary first.
    let display = gtk::gdk::Display::default();
    let rects = display.as_ref().map(opusdm_ui::list_monitors).unwrap_or_default();
    full_rebuild_desktop_windows(app, &widgets, rects);

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
    for delay in PANEL_STRUT_SETTLE_DELAYS_MS {
        let widgets = widgets.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
            for dw in widgets.windows.borrow().iter() {
                crate::desktop::pin_geometry(&dw.window, dw.x, dw.y, dw.w, dw.h);
            }
        });
    }
}

/// Re-place the desktop windows for a new monitor layout `rects`. Reuses the
/// existing windows (and the primary's icon field/watch) when the monitor
/// count hasn't changed - the common case, a plain resolution change - and
/// only tears everything down and rebuilds when monitors were added, removed,
/// or a mirror layout was toggled.
fn reconcile_desktop_windows(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    mut rects: Vec<(i32, i32, i32, i32)>,
) {
    if rects.is_empty() {
        rects.push((0, 0, 1280, 800));
    }
    if widgets.windows.borrow().len() == rects.len() {
        reposition_desktop_windows(widgets, &rects);
    } else {
        full_rebuild_desktop_windows(app, widgets, rects);
    }
}

/// Re-pin every existing desktop window to its (possibly new) rect, matched
/// by index - index 0 stays the primary monitor's window.
fn reposition_desktop_windows(widgets: &Rc<Widgets>, rects: &[(i32, i32, i32, i32)]) {
    let mut windows = widgets.windows.borrow_mut();
    for (dw, &(x, y, w, h)) in windows.iter_mut().zip(rects.iter()) {
        crate::desktop::make_desktop_window(&dw.window, x, y, w, h);
        crate::desktop::pin_geometry(&dw.window, x, y, w, h);
        dw.x = x;
        dw.y = y;
        dw.w = w;
        dw.h = h;
        // The window manager reacts to the new screen size on its own - it
        // shifts the desktop down by the panel's strut - so re-assert the
        // geometry once it has settled, as the startup path does.
        for delay in RESOLUTION_CHANGE_SETTLE_DELAYS_MS {
            let window = dw.window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                crate::desktop::pin_geometry(&window, x, y, w, h);
            });
        }
    }
}

/// Tear down every desktop window (and the primary's icon field/watch) and
/// rebuild the whole set from `rects` - index 0 becomes the new primary.
fn full_rebuild_desktop_windows(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    mut rects: Vec<(i32, i32, i32, i32)>,
) {
    for dw in widgets.windows.borrow_mut().drain(..) {
        dw.window.close();
    }
    *widgets.desktop.watch.borrow_mut() = None;
    *widgets.desktop.field.borrow_mut() = None;
    *widgets.desktop.dir.borrow_mut() = None;

    if rects.is_empty() {
        rects.push((0, 0, 1280, 800));
    }

    let icon_set = widgets.desktop.icon_set.borrow().clone();
    let desired_dir = widgets.desktop.desired_dir.borrow().clone();
    let built = rects
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| {
            let is_primary = i == 0;
            let window = build_one_desktop_window(
                app,
                widgets,
                x,
                y,
                w,
                h,
                is_primary,
                desired_dir.as_deref(),
                icon_set.clone(),
            );
            DesktopWindow { window, x, y, w, h }
        })
        .collect();
    *widgets.windows.borrow_mut() = built;
}

/// Build one desktop window at `(x, y)`, `w` x `h`. Only the primary monitor
/// (`is_primary`) gets the interactive icon field, its drag/drop wiring and
/// the inotify watch; the rest are background-only.
fn build_one_desktop_window(
    app: &gtk::Application,
    widgets: &Rc<Widgets>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    is_primary: bool,
    desktop_dir: Option<&Path>,
    icon_set: opusdm_ipc::IconSetId,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM")
        .decorated(false)
        // Resizable on the GTK side so the window follows the X geometry we
        // pin when the screen size changes; a non-resizable window would ask
        // the WM for its old natural size back and never shrink. The WM is
        // kept from resizing it by the min = max size hints in `desktop.rs`.
        .resizable(true)
        .default_width(w)
        .default_height(h)
        .build();

    // Turn the window into the WM desktop once its surface exists, then
    // re-assert the geometry after the WM has run its placement policy.
    window.connect_realize(move |win| crate::desktop::make_desktop_window(win, x, y, w, h));
    window.connect_map(move |win| {
        crate::desktop::make_desktop_window(win, x, y, w, h);
        let win = win.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(WM_PLACEMENT_SETTLE_DELAY_MS), move || {
            crate::desktop::pin_geometry(&win, x, y, w, h);
        });
    });

    let panel_position = widgets.desktop.settings.borrow().panel_position;
    let (desktop_area, field) = build_desktop_area(
        if is_primary { desktop_dir } else { None },
        icon_set,
        desktop_extras(widgets),
        panel_position,
    );
    window.set_child(Some(&desktop_area));

    // Right-click the desktop for the context menu (background, or - on the
    // primary monitor, where `field` is `Some` - a per-icon menu).
    attach_desktop_menu(&desktop_area, widgets, field.clone());

    // Window close (from the WM): always confirm before shutting down.
    {
        let widgets = widgets.clone();
        window.connect_close_request(move |_| {
            request_quit(&widgets);
            glib::Propagation::Stop
        });
    }

    if is_primary {
        *widgets.desktop.dir.borrow_mut() = desktop_dir.map(Path::to_path_buf);
        *widgets.desktop.field.borrow_mut() = field.clone();
        if let (Some(f), Some(dir)) = (field, desktop_dir) {
            let ds = widgets.desktop.settings.borrow();
            f.set_icon_size(ds.icon_size);
            f.set_grid_spacing(ds.grid_spacing);
            f.set_grid_gap(ds.grid_gap);
            drop(ds);
            wire_primary_icon_field(widgets, &f);
            spawn_desktop_watch(widgets, &f, dir);
        }
    }

    window.present();
    window
}

/// Wire the primary monitor's icon field: open on activate, copy/move on
/// drop, and spawn a new Lister on a background double-click.
fn wire_primary_icon_field(widgets: &Rc<Widgets>, field: &opusdm_iconview::IconField) {
    {
        let cmd_tx = widgets.cmd_tx.clone();
        let widgets = widgets.clone();
        let field_for_open = field.clone();
        field.connect_activate(move |i| {
            let Some(entry) = field_for_open.entry(i) else { return };
            if let Some(id) = opusdm_ipc::groups::group_id(&entry.path) {
                open_group(&widgets, id);
                return;
            }
            if entry.left_out && entry.kind == opusdm_iconview::IconKind::Broken {
                opusdm_ipc::err_log!("hub", "left-out target missing: {}", entry.path.display());
                return;
            }
            if opusdm_hub::leftout::opens_in_lister(&entry) {
                let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(entry.path) });
            } else {
                let uri = format!("file://{}", entry.path.display());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            }
        });
    }
    {
        let cmd_tx = widgets.cmd_tx.clone();
        field.connect_background_activate(move || {
            let _ = cmd_tx.send(HubCmd::SpawnRequested { start_path: None });
            let _ = cmd_tx.send(HubCmd::EmitScriptEvent(ScriptEvent::DesktopDoubleClick));
        });
    }
    {
        let widgets = widgets.clone();
        let field_for_drop = field.clone();
        field.connect_drop_uris(move |srcs, target, shift| {
            if srcs.is_empty() {
                return;
            }
            // Dropped on a group icon: add the items to the group instead of
            // copying them into the desktop folder.
            let group_target = usize::try_from(target)
                .ok()
                .and_then(|i| field_for_drop.entry(i))
                .and_then(|e| opusdm_ipc::groups::group_id(&e.path).map(str::to_string));
            if let Some(id) = group_target {
                add_to_group(&widgets, &id, srcs);
                return;
            }
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            let op = if shift { opusdm_ipc::Op::Move } else { opusdm_ipc::Op::Copy };
            let _ = widgets.cmd_tx.send(HubCmd::RunJob { op, srcs, dest_dir });
        });
    }
    {
        let widgets = widgets.clone();
        field.connect_pinned_moved(move |moved| {
            let mut left_out_moved = false;
            let mut moved_groups = Vec::new();
            for (path, (x, y)) in &moved {
                match opusdm_ipc::groups::group_id(path) {
                    Some(id) => {
                        widgets.desktop.groups.borrow_mut().set_position(id, *x, *y);
                        moved_groups.push(id.to_string());
                    }
                    None => {
                        widgets.desktop.leftout.borrow_mut().set_position(path, *x, *y);
                        left_out_moved = true;
                    }
                }
            }
            if left_out_moved {
                save_leftout(&widgets.desktop.leftout.borrow());
            }
            for id in moved_groups {
                save_group(&widgets, &id);
            }
        });
    }
}

/// The full desktop stylesheet: the `.opusdm-desktop` background rule built
/// from `bg`, plus the fixed context-menu rules.
fn desktop_rule_css(bg: &opusdm_ipc::Background) -> String {
    format!(
        ".opusdm-desktop {{ {bgdecl} }}\n\
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

/// Read `dir` into desktop entries, merged with the left-out icons in `left`,
/// on a background thread and invoke `on_done` with the result back on the
/// GTK main loop, so scanning a large or slow desktop directory (or stat-ing
/// a left-out target on a slow disk) never freezes the desktop and panel.
fn read_entries_async(
    dir: PathBuf,
    extras: opusdm_hub::leftout::DesktopExtras,
    on_done: impl Fn(std::io::Result<Vec<opusdm_iconview::IconEntry>>) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(opusdm_hub::leftout::read_desktop_entries(&dir, &extras));
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            on_done(result);
        }
    });
}

/// Show `entries` in `field`, first handing it the saved positions of the
/// left-out icons so they land where the user dragged them.
fn show_entries(
    field: &opusdm_iconview::IconField,
    extras: &opusdm_hub::leftout::DesktopExtras,
    entries: &[opusdm_iconview::IconEntry],
) {
    field.set_pinned_positions(extras.positions());
    field.set_entries(entries);
}

/// Re-read the desktop directory and refresh the icon field with the current
/// left-out store merged in. Does nothing without a field or directory.
fn reload_desktop_field(widgets: &Rc<Widgets>) {
    let field = widgets.desktop.field.borrow().clone();
    let dir = widgets.desktop.dir.borrow().clone();
    let (Some(field), Some(dir)) = (field, dir) else { return };
    let ticket = widgets.desktop.reload_gate.begin();
    let extras = desktop_extras(widgets);
    let widgets = widgets.clone();
    read_entries_async(dir.clone(), extras, move |result| {
        if !widgets.desktop.reload_gate.is_latest(ticket) {
            return;
        }
        match result {
            // The store as it is now, not as it was when the read started: an
            // icon dragged in the meantime keeps its new spot.
            Ok(entries) => show_entries(&field, &desktop_extras(&widgets), &entries),
            Err(e) => opusdm_ipc::err_log!("hub", "cannot read desktop {}: {e}", dir.display()),
        }
    });
}

/// Text of the desktop menu's hide button for the current state.
fn hide_label(hidden: bool) -> &'static str {
    if hidden {
        "Reveal"
    } else {
        "Hide All"
    }
}

/// Hide (`true`) or show the panel, the dock and the group windows, and
/// relabel the desktop menu button. The desktop itself always stays, so it can
/// reveal the rest; the Lister windows react to their own `SetHidden`.
fn apply_hidden(widgets: &Rc<Widgets>, hidden: bool) {
    widgets.desktop.hidden.set(hidden);
    if let Some(app) = widgets.primary_window().application() {
        for window in app.windows() {
            let title = window.title();
            let is_chrome = matches!(
                title.as_deref(),
                Some(t) if t == opusdm_panel::WINDOW_TITLE || t == opusdm_dock::WINDOW_TITLE
            );
            if is_chrome {
                window.set_visible(!hidden);
            }
        }
    }
    widgets.desktop.group_windows.set_visible(!hidden);
    let mut buttons = widgets.desktop.hide_buttons.borrow_mut();
    buttons.retain(|b| b.root().is_some());
    for button in buttons.iter() {
        button.set_label(hide_label(hidden));
    }
}

/// Everything the desktop shows besides the Desktop folder, as it is now.
fn desktop_extras(widgets: &Widgets) -> opusdm_hub::leftout::DesktopExtras {
    opusdm_hub::leftout::DesktopExtras {
        left_out: widgets.desktop.leftout.borrow().clone(),
        groups: widgets.desktop.groups.borrow().clone(),
    }
}

/// Persist group `id`, logging (not failing) on error.
fn save_group(widgets: &Widgets, id: &str) {
    if let Err(e) = widgets.desktop.groups.borrow().save_group(id) {
        opusdm_ipc::err_log!("hub", "cannot save group {id}: {e}");
    }
}

/// After any change to the groups: bring the open group windows and the
/// desktop icons in line with the store.
fn groups_changed(widgets: &Rc<Widgets>) {
    let snapshot = widgets.desktop.groups.borrow().clone();
    widgets.desktop.group_windows.refresh(&snapshot);
    reload_desktop_field(widgets);
}

/// Tell the user why a group operation was refused.
fn show_group_error(widgets: &Rc<Widgets>, message: &str) {
    opusdm_ui::requester(&widgets.primary_window(), "Request", message, None, &[("OK", true)], |_| {});
}

/// Add `paths` to group `id`, save and refresh. Nothing happens when every path
/// is already a member (or is not absolute).
fn add_to_group(widgets: &Rc<Widgets>, id: &str, paths: Vec<PathBuf>) {
    let added = widgets.desktop.groups.borrow_mut().add_items(id, &paths);
    if added == 0 {
        return;
    }
    save_group(widgets, id);
    groups_changed(widgets);
}

/// Start a group member the way [`opusdm_hub::leftout::launch_action`] says.
/// Failures are logged.
fn launch_entry(widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    use opusdm_hub::leftout::LaunchAction;
    match opusdm_hub::leftout::launch_action(entry) {
        LaunchAction::Lister => {
            let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(entry.path.clone()) });
        }
        LaunchAction::DesktopEntry => match gio::DesktopAppInfo::from_filename(&entry.path) {
            Some(info) => {
                if let Err(e) = info.launch(&[], gio::AppLaunchContext::NONE) {
                    opusdm_ipc::err_log!("hub", "cannot launch {}: {e}", entry.path.display());
                    show_group_error(widgets, &format!("Cannot launch {}: {e}", entry.path.display()));
                }
            }
            None => opusdm_ipc::err_log!("hub", "{} is not a valid .desktop file", entry.path.display()),
        },
        LaunchAction::Program => {
            let mut cmd = std::process::Command::new(&entry.path);
            if let Some(dir) = entry.path.parent() {
                cmd.current_dir(dir);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Err(e) = cmd.spawn() {
                opusdm_ipc::err_log!("hub", "cannot run {}: {e}", entry.path.display());
                show_group_error(widgets, &format!("Cannot run {}: {e}", entry.path.display()));
            }
        }
        LaunchAction::Default => {
            let uri = gio::File::for_path(&entry.path).uri();
            if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
                opusdm_ipc::err_log!("hub", "cannot open {}: {e}", entry.path.display());
                show_group_error(widgets, &format!("Cannot open {}: {e}", entry.path.display()));
            }
        }
    }
}

/// The callbacks a group window uses to talk back to the desktop.
fn group_actions(widgets: &Rc<Widgets>) -> crate::group_window::GroupActions {
    let for_launch = widgets.clone();
    let for_add = widgets.clone();
    let for_remove = widgets.clone();
    crate::group_window::GroupActions {
        launch: Rc::new(move |entry| launch_entry(&for_launch, entry)),
        add: Rc::new(move |id, paths| add_to_group(&for_add, id, paths)),
        remove: Rc::new(move |id, path| {
            for_remove.desktop.groups.borrow_mut().remove_item(id, &path);
            save_group(&for_remove, id);
            groups_changed(&for_remove);
        }),
    }
}

/// Open (or raise) the window of group `id`.
fn open_group(widgets: &Rc<Widgets>, id: &str) {
    let Some(app) = widgets.primary_window().application() else { return };
    let Some(group) = widgets.desktop.groups.borrow().get(id).cloned() else { return };
    let icon_set = widgets.desktop.icon_set.borrow().clone();
    widgets.desktop.group_windows.show(&app, &group, &icon_set, &group_actions(widgets));
}

/// Ask for a name and create a new, empty group.
fn new_group_dialog(widgets: &Rc<Widgets>) {
    let widgets = widgets.clone();
    opusdm_ui::requester_entry(
        &widgets.primary_window(),
        "New group name",
        "",
        &[("OK", true), ("Cancel", false)],
        move |accepted, text| {
            if !accepted {
                return;
            }
            let created = widgets
                .desktop
                .groups
                .borrow_mut()
                .create_in_dir(&opusdm_ipc::groups::groups_dir(), &text);
            match created {
                Ok(id) => {
                    save_group(&widgets, &id);
                    groups_changed(&widgets);
                }
                Err(message) => show_group_error(&widgets, &message),
            }
        },
    );
}

/// The context menu of a group icon: Open, Rename Group..., Delete Group...
fn fill_group_menu(menu: &gtk::Box, widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    let Some(id) = opusdm_ipc::groups::group_id(&entry.path).map(str::to_string) else { return };
    {
        let id = id.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Open");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            open_group(&widgets, &id);
        });
        menu.append(&btn);
    }
    {
        let id = id.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Rename Group...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let id = id.clone();
            let widgets_for_cb = widgets.clone();
            opusdm_ui::requester_entry(
                &widgets.primary_window(),
                "Rename group",
                &name,
                &[("OK", true), ("Cancel", false)],
                move |accepted, text| {
                    if !accepted {
                        return;
                    }
                    let renamed = widgets_for_cb.desktop.groups.borrow_mut().rename(&id, &text);
                    match renamed {
                        Ok(()) => {
                            save_group(&widgets_for_cb, &id);
                            groups_changed(&widgets_for_cb);
                        }
                        Err(message) => show_group_error(&widgets_for_cb, &message),
                    }
                },
            );
        });
        menu.append(&btn);
    }
    {
        let id = id.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Delete Group...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let id = id.clone();
            let widgets_for_cb = widgets.clone();
            opusdm_ui::requester(
                &widgets.primary_window(),
                "Request",
                &format!("Delete the group \"{name}\"? Its members are not touched."),
                None,
                &[("Delete", true), ("Cancel", false)],
                move |confirmed| {
                    if !confirmed {
                        return;
                    }
                    widgets_for_cb.desktop.groups.borrow_mut().delete(&id);
                    if let Err(e) = opusdm_ipc::Groups::remove_group_file(&id) {
                        opusdm_ipc::err_log!("hub", "cannot delete group file {id}: {e}");
                    }
                    groups_changed(&widgets_for_cb);
                },
            );
        });
        menu.append(&btn);
    }
}

/// Persist the left-out store, logging (not failing) on error: the in-memory
/// state stays valid and the next change tries again.
fn save_leftout(left: &opusdm_ipc::LeftOut) {
    if let Err(e) = left.save() {
        opusdm_ipc::err_log!("hub", "cannot save {}: {e}", opusdm_ipc::leftout::leftout_path().display());
    }
}

/// Start (or restart) the inotify watch on `dir` and the local future that
/// reloads the desktop icon field when the directory changes. Replaces any
/// previous watch stored in `widgets.desktop.watch`.
fn spawn_desktop_watch(
    widgets: &Rc<Widgets>,
    field: &opusdm_iconview::IconField,
    dir: &Path,
) {
    // Drop the old watch first so its thread exits before we start another.
    *widgets.desktop.watch.borrow_mut() = None;
    match opusdm_hub::desktop_files::DesktopWatch::start(dir.to_path_buf()) {
        Ok((watch, rx)) => {
            let field = field.clone();
            let dir = dir.to_path_buf();
            let widgets_in = widgets.clone();
            glib::spawn_future_local(async move {
                let widgets = widgets_in;
                // Reloads run off-thread, so several can be in flight at
                // once; the shared reload gate drops any read superseded by a
                // later one (from here or from `reload_desktop_field`).
                while rx.recv().await.is_ok() {
                    let ticket = widgets.desktop.reload_gate.begin();
                    let field = field.clone();
                    let dir = dir.clone();
                    let extras = desktop_extras(&widgets);
                    let widgets = widgets.clone();
                    read_entries_async(dir.clone(), extras, move |result| {
                        if !widgets.desktop.reload_gate.is_latest(ticket) {
                            return;
                        }
                        match result {
                            Ok(entries) => show_entries(&field, &desktop_extras(&widgets), &entries),
                            Err(e) => {
                                opusdm_ipc::err_log!("hub", "desktop reload failed for {}: {e}", dir.display())
                            }
                        }
                    });
                }
            });
            *widgets.desktop.watch.borrow_mut() = Some(watch);
        }
        Err(e) => opusdm_ipc::err_log!("hub", "desktop watch unavailable: {e}"),
    }
}

/// Resolve the `opus-desktop-config` binary: preferably a sibling of the
/// running hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
/// Resolve the `opusdm-settings` binary: preferably a sibling of the running
/// hub executable (so a `cargo run` / installed pair stay together),
/// otherwise the bare name for a `PATH` lookup.
fn theme_config_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("opusdm-settings");
        }
    }
    PathBuf::from("opusdm-settings")
}

/// Spawn the theme configuration app, detached.
fn launch_theme_config() {
    let bin = theme_config_bin();
    if let Err(e) = std::process::Command::new(&bin).spawn() {
        opusdm_ipc::err_log!("hub", "cannot launch {}: {e}", bin.display());
    }
}

/// Build the desktop area that fills the window below the toolbar, wrapped in
/// an overlay so the context menu can be laid on top of it. When `desktop_dir`
/// is `Some`, an icon field in `Free` mode fills it and its initial entries
/// are read. Otherwise the background stays plain.
fn build_desktop_area(
    desktop_dir: Option<&Path>,
    icon_set: opusdm_ipc::IconSetId,
    extras: opusdm_hub::leftout::DesktopExtras,
    panel_position: opusdm_ipc::PanelPosition,
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
            {
                let f = f.clone();
                let dir = dir.to_path_buf();
                let extras_for_show = extras.clone();
                read_entries_async(dir.clone(), extras, move |result| match result {
                    Ok(entries) => show_entries(&f, &extras_for_show, &entries),
                    Err(e) => opusdm_ipc::err_log!("hub", "cannot read desktop {}: {e}", dir.display()),
                });
            }
            f.widget().set_hexpand(true);
            f.widget().set_vexpand(true);
            keep_clear_of_panel(f.widget(), panel_position);
            bg.append(f.widget());
            Some(f)
        }
        None => None,
    };

    overlay.set_child(Some(&bg));
    (overlay, field)
}

/// The dock layout `ds` asks for: its own edge and icon size, plus the
/// panel's edge and height so it can keep clear of the panel.
fn dock_layout(ds: &opusdm_ipc::DesktopSettings) -> opusdm_dock::DockLayout {
    opusdm_dock::DockLayout {
        position: ds.dock_position,
        icon_size: ds.dock_icon_size,
        panel: ds.panel_position,
        panel_height: opusdm_panel::PANEL_HEIGHT,
    }
}

/// Keep the icon area clear of the panel dock: the background box still
/// fills the whole monitor, but the icons must stay inside the strip the
/// panel's strut leaves free instead of sitting under it. The margin goes on
/// the edge the panel is docked to and is cleared on the other.
fn keep_clear_of_panel(field: &impl IsA<gtk::Widget>, position: opusdm_ipc::PanelPosition) {
    let (top, bottom) = match position {
        opusdm_ipc::PanelPosition::Top => (opusdm_panel::PANEL_HEIGHT, 0),
        opusdm_ipc::PanelPosition::Bottom => (0, opusdm_panel::PANEL_HEIGHT),
    };
    field.set_margin_top(top);
    field.set_margin_bottom(bottom);
}

/// Approximate icon menu width/height, reserved when clamping it inside the
/// desktop the same way as [`DESKTOP_MENU_W`]/[`DESKTOP_MENU_H`].
const ICON_MENU_W: i32 = 160;
const ICON_MENU_H: i32 = 140;

/// Place `menu` at `(x, y)` inside `overlay`, clamped so it never runs past
/// the overlay's edge, and show it.
fn popup_at(menu: &gtk::Box, overlay: &gtk::Overlay, x: f64, y: f64, w: i32, h: i32) {
    let max_x = (overlay.width() - w).max(0);
    let max_y = (overlay.height() - h).max(0);
    menu.set_margin_start((x as i32).clamp(0, max_x));
    menu.set_margin_top((y as i32).clamp(0, max_y));
    menu.set_visible(true);
}

/// Emit `DesktopMiddleDoubleClick` and `DesktopRightDoubleClick` for double
/// clicks on the desktop overlay. The gestures run in the capture phase and
/// never claim the sequence, so the icon field and the context menu keep
/// working. The right button already opens the context menu on its first
/// press, so a right double click also shows the menu (accepted).
fn attach_desktop_double_click_events(overlay: &gtk::Overlay, cmd_tx: &std::sync::mpsc::Sender<HubCmd>) {
    for (button, event) in [
        (gtk::gdk::BUTTON_MIDDLE, ScriptEvent::DesktopMiddleDoubleClick),
        (gtk::gdk::BUTTON_SECONDARY, ScriptEvent::DesktopRightDoubleClick),
    ] {
        let gesture = gtk::GestureClick::new();
        gesture.set_button(button);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let cmd_tx = cmd_tx.clone();
        gesture.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                let _ = cmd_tx.send(HubCmd::EmitScriptEvent(event));
            }
        });
        overlay.add_controller(gesture);
    }
}

/// Wire the right-click context menus onto the desktop overlay: the
/// background menu (System actions, New Folder/File, Paste, Terminal), and -
/// when `field` is `Some` (the primary monitor only) - a per-icon menu with
/// the same file operations a Lister offers on a row (Open, Cut, Copy,
/// Rename, Delete). The desktop window never holds the input focus, so a
/// `Popover` parented to it can never grab; both menus are plain boxes laid
/// on the overlay instead, positioned with margins at the click point.
fn attach_desktop_menu(overlay: &gtk::Overlay, widgets: &Rc<Widgets>, field: Option<opusdm_iconview::IconField>) {
    attach_desktop_double_click_events(overlay, &widgets.cmd_tx);
    let bg_menu = build_desktop_menu(widgets);
    bg_menu.set_halign(gtk::Align::Start);
    bg_menu.set_valign(gtk::Align::Start);
    bg_menu.set_visible(false);
    overlay.add_overlay(&bg_menu);

    let icon_menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    icon_menu.add_css_class("opusdm-desktop-menu");
    icon_menu.set_halign(gtk::Align::Start);
    icon_menu.set_valign(gtk::Align::Start);
    icon_menu.set_visible(false);
    overlay.add_overlay(&icon_menu);

    match field {
        // Primary monitor: the field's own secondary-click controller already
        // hit-tests icons vs. background (see `IconField::connect_context_menu`),
        // so route through it instead of a second overlay-level gesture.
        Some(field) => {
            let widgets = widgets.clone();
            let overlay = overlay.clone();
            let bg_menu = bg_menu.clone();
            let icon_menu = icon_menu.clone();
            let field_for_cb = field.clone();
            field.connect_context_menu(move |idx, x, y| {
                let field = &field_for_cb;
                let (x, y) = field.translate_from_canvas(&overlay, x, y).unwrap_or((x, y));
                if idx >= 0 {
                    let Some(entry) = field.entry(idx as usize) else { return };
                    fill_icon_menu(&icon_menu, &widgets, &entry);
                    bg_menu.set_visible(false);
                    popup_at(&icon_menu, &overlay, x, y, ICON_MENU_W, ICON_MENU_H);
                } else {
                    icon_menu.set_visible(false);
                    popup_at(&bg_menu, &overlay, x, y, DESKTOP_MENU_W, DESKTOP_MENU_H);
                }
            });
        }
        // No icon field on this monitor (secondary monitor, or no desktop
        // directory resolved): background menu only.
        None => {
            let open = gtk::GestureClick::new();
            open.set_button(gtk::gdk::BUTTON_SECONDARY);
            {
                let bg_menu = bg_menu.clone();
                let overlay = overlay.clone();
                open.connect_pressed(move |gesture, n_press, x, y| {
                    if n_press != 1 {
                        return;
                    }
                    popup_at(&bg_menu, &overlay, x, y, DESKTOP_MENU_W, DESKTOP_MENU_H);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                });
            }
            overlay.add_controller(open);
        }
    }

    // Primary click outside whichever menu is open: dismiss it. Clicks on the
    // menu buttons are handled by the buttons themselves.
    let dismiss = gtk::GestureClick::new();
    dismiss.set_button(gtk::gdk::BUTTON_PRIMARY);
    {
        let overlay = overlay.clone();
        dismiss.connect_pressed(move |_, _, x, y| {
            for menu in [&bg_menu, &icon_menu] {
                if !menu.get_visible() {
                    continue;
                }
                if let Some(b) = menu.compute_bounds(&overlay) {
                    let (x, y) = (x as f32, y as f32);
                    let inside = x >= b.x()
                        && x < b.x() + b.width()
                        && y >= b.y()
                        && y < b.y() + b.height();
                    if inside {
                        continue;
                    }
                }
                menu.set_visible(false);
            }
        });
    }
    overlay.add_controller(dismiss);
}

/// The per-icon context menu, rebuilt into `menu` on every click since its
/// content (name, file vs. directory) depends on the clicked entry: Open,
/// Cut, Copy, Rename, Delete. Every item hides the menu before running its
/// action.
fn fill_icon_menu(menu: &gtk::Box, widgets: &Rc<Widgets>, entry: &opusdm_iconview::IconEntry) {
    while let Some(child) = menu.first_child() {
        menu.remove(&child);
    }

    if entry.group {
        fill_group_menu(menu, widgets, entry);
        return;
    }

    // Open
    {
        let path = entry.path.clone();
        let opens_in_lister = opusdm_hub::leftout::opens_in_lister(entry);
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Open");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            if opens_in_lister {
                let _ = widgets.cmd_tx.send(HubCmd::SpawnRequested { start_path: Some(path.clone()) });
            } else {
                let uri = format!("file://{}", path.display());
                let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
            }
        });
        menu.append(&btn);
    }

    // Put Away (left-out icons only): forget the icon, never touch the file.
    if entry.left_out {
        let path = entry.path.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Put Away");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            {
                let mut left = widgets.desktop.leftout.borrow_mut();
                left.remove(std::slice::from_ref(&path));
                save_leftout(&left);
            }
            reload_desktop_field(&widgets);
        });
        menu.append(&btn);
    }

    menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Cut, Rename and Delete act on the real file, which for a left-out icon
    // lives somewhere else and is not what the user sees: not offered there.
    // Cut
    if !entry.left_out {
        let path = entry.path.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Cut");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), true);
        });
        menu.append(&btn);
    }

    // Copy
    {
        let path = entry.path.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Copy");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::copy_or_cut(std::slice::from_ref(&path), false);
        });
        menu.append(&btn);
    }

    // Rename...
    if !entry.left_out {
        let path = entry.path.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Rename...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            crate::desktop_ops::rename(&widgets.primary_window(), path.clone(), &name);
        });
        menu.append(&btn);
    }

    // Delete...
    if !entry.left_out {
        menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let path = entry.path.clone();
        let name = entry.name.clone();
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Delete...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::confirm_delete(
                &widgets.primary_window(),
                &widgets.cmd_tx,
                &format!("Delete \"{name}\" permanently?"),
                vec![path.clone()],
                dest_dir,
            );
        });
        menu.append(&btn);
    }
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

/// The desktop context menu box: New Lister / --- / Paste, New Folder, New
/// File, Open Terminal Here / --- / Settings / About / --- / Quit. Every
/// item hides the menu before running its action.
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

    // Paste: whatever a Cut/Copy (here or in a Lister) last put on the
    // clipboard, into the desktop directory. A no-op if there is nothing
    // pasteable, so it is always shown rather than probed asynchronously.
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Paste");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::paste(&widgets.cmd_tx, dest_dir);
        });
        menu.append(&btn);
    }

    // New Folder
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("New Folder...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::prompt_new(
                &widgets.primary_window(),
                "New folder name",
                &widgets.cmd_tx,
                dest_dir,
                |name| opusdm_ipc::Op::MakeDir { name },
            );
        });
        menu.append(&btn);
    }

    // New File
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("New File...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dest_dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::prompt_new(
                &widgets.primary_window(),
                "New file name",
                &widgets.cmd_tx,
                dest_dir,
                |name| opusdm_ipc::Op::NewFile { name },
            );
        });
        menu.append(&btn);
    }

    // Hide All / Reveal
    {
        let cmd_tx = widgets.cmd_tx.clone();
        let menu_ref = menu.clone();
        let btn = menu_button(hide_label(widgets.desktop.hidden.get()));
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let _ = cmd_tx.send(HubCmd::ToggleHidden);
        });
        widgets.desktop.hide_buttons.borrow_mut().push(btn.clone());
        menu.append(&btn);
    }

    // New Group
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("New Group...");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            new_group_dialog(&widgets);
        });
        menu.append(&btn);
    }

    // Open Terminal Here
    {
        let widgets = widgets.clone();
        let menu_ref = menu.clone();
        let btn = menu_button("Open Terminal Here");
        btn.connect_clicked(move |_| {
            menu_ref.set_visible(false);
            let Some(dir) = widgets.desktop.dir.borrow().clone() else { return };
            crate::desktop_ops::open_terminal_in(&dir);
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
            show_about_dialog(&widgets.primary_window());
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
                reload_desktop_field(&widgets);
            });
            sub.append(&btn);
        }

        // Settings: launch the standalone opusdm-settings app. Icon grid
        // metrics and the desktop folder override live there too now.
        {
            let menu_ref = menu.clone();
            let btn = menu_button("Settings");
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
        assert!(css.contains(".opusdm-desktop-menu {")); // other rules kept
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
