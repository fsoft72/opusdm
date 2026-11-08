//! The `opusdm-dock` process: the dock on its own GTK application.
//!
//! It never needs the hub. It reads `dock.json` (its settings),
//! `desktop.json` (only the panel edge) and `settings.json` (only the theme
//! mode), and follows all three while it runs. The hub only starts it,
//! restarts it when it dies, and sends "Hide All" over the dock socket.
//!
//! Exit codes: `EXIT_DISABLED` when `dock.json` turns the dock off (at start
//! or later), `EXIT_ALREADY_RUNNING` when another dock owns the socket.
//!
//! Ownership for the whole run: the `Follow` state below is owned by the
//! three file monitor handlers and by the request handler future; the file
//! monitors are owned by [`MONITORS`] (a thread-local of the main thread),
//! so they live until the process ends. The dock window is owned by the
//! application, as before.

use crate::{attach, instance, server, DockLayout};
use gtk4 as gtk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::dock::{DockConfig, DockReply, DockRequest};
use opusdm_ipc::instance::{EXIT_ALREADY_RUNNING, EXIT_DISABLED};
use opusdm_ipc::PanelPosition;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// GTK application id of the dock process.
pub const APP_ID: &str = "org.opusdm.Dock";

/// Log tag.
const TAG: &str = "opusdm-dock";

thread_local! {
    /// The file monitors of the running dock. GTK runs on one thread, so a
    /// thread-local is enough; dropping a monitor would stop its watch.
    static MONITORS: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
}

/// The layout the dock shows for `cfg`, keeping clear of a panel on `panel`.
pub fn layout_of(cfg: &DockConfig, panel: PanelPosition) -> DockLayout {
    DockLayout {
        position: cfg.position,
        icon_size: cfg.icon_size,
        panel,
        panel_height: opusdm_ipc::PANEL_HEIGHT,
        visible: cfg.visible,
        style: cfg.style.clone(),
    }
}

/// The panel edge from `desktop.json`.
fn panel_position() -> PanelPosition {
    opusdm_ipc::settings::load_desktop_settings(TAG).panel_position
}

/// Run the dock process and return its exit code.
pub fn run() -> glib::ExitCode {
    let cfg = opusdm_ipc::dock::load_dock_config();
    if !cfg.visible {
        opusdm_ipc::debug_log!(TAG, "the dock is turned off in dock.json: not started");
        return glib::ExitCode::from(EXIT_DISABLED as u8);
    }
    let listener = match instance::acquire() {
        Ok(l) => Some(l),
        Err(instance::Exit::AlreadyRunning) => {
            opusdm_ipc::err_log!(TAG, "another opusdm-dock owns the dock socket: exiting");
            return glib::ExitCode::from(EXIT_ALREADY_RUNNING as u8);
        }
        Err(instance::Exit::Failed(msg)) => {
            opusdm_ipc::err_log!(TAG, "cannot own the dock socket ({msg}): the dock runs without Hide All");
            None
        }
    };

    // NON_UNIQUE: the dock socket decides about a second instance, not D-Bus.
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    let exit_code: Rc<Cell<Option<u8>>> = Rc::new(Cell::new(None));
    let pending = RefCell::new(Some((cfg, listener)));
    {
        let exit_code = exit_code.clone();
        app.connect_activate(move |app| {
            // Activate runs once per run; a second call finds nothing to start.
            let Some((cfg, listener)) = pending.borrow_mut().take() else { return };
            start(app, cfg, listener, exit_code.clone());
        });
    }
    // No arguments for GTK: the dock takes none.
    let code = app.run_with_args::<&str>(&[]);
    match exit_code.get() {
        Some(c) => glib::ExitCode::from(c),
        None => code,
    }
}

/// What the file watches and the request handler share.
struct Follow {
    app: gtk::Application,
    /// Applies a new layout to the dock window.
    set_layout: Rc<dyn Fn(DockLayout)>,
    /// The last `dock.json` read.
    cfg: RefCell<DockConfig>,
    /// The last panel edge read.
    panel: Cell<PanelPosition>,
    /// Set before quitting, returned by [`run`].
    exit_code: Rc<Cell<Option<u8>>>,
}

impl Follow {
    /// `dock.json` changed: quit when the dock was turned off, else show
    /// the new layout.
    fn dock_config_changed(&self) {
        // Already quitting: the events of the same write arrive one by one.
        if self.exit_code.get().is_some() { return; }
        let cfg = opusdm_ipc::dock::load_dock_config();
        if !cfg.visible {
            opusdm_ipc::debug_log!(TAG, "the dock was turned off in dock.json: exiting");
            self.exit_code.set(Some(EXIT_DISABLED as u8));
            self.app.quit();
            return;
        }
        (self.set_layout)(layout_of(&cfg, self.panel.get()));
        *self.cfg.borrow_mut() = cfg;
    }

    /// `desktop.json` changed: follow the panel edge.
    fn desktop_changed(&self) {
        let panel = panel_position();
        if panel == self.panel.get() {
            return;
        }
        self.panel.set(panel);
        (self.set_layout)(layout_of(&self.cfg.borrow(), panel));
    }

    /// Answer one request from the dock socket.
    fn answer(&self, request: DockRequest) -> DockReply {
        match request {
            DockRequest::Ping => DockReply::Hidden(crate::is_hidden()),
            DockRequest::SetHidden(hidden) => {
                opusdm_ipc::debug_log!(TAG, "hub: {}", if hidden { "hide" } else { "reveal" });
                crate::set_hidden(hidden);
                crate::sync_visibility(&self.app);
                DockReply::Hidden(hidden)
            }
        }
    }
}

/// Build the dock and start following the files and the socket.
fn start(app: &gtk::Application, cfg: DockConfig, listener: Option<std::os::unix::net::UnixListener>, exit_code: Rc<Cell<Option<u8>>>) {
    opusdm_ui::apply_theme_mode(opusdm_ipc::settings::load_settings(TAG).appearance.theme_mode);
    let panel = panel_position();
    let set_layout = attach(app, layout_of(&cfg, panel));
    let follow = Rc::new(Follow { app: app.clone(), set_layout, cfg: RefCell::new(cfg), panel: Cell::new(panel), exit_code });

    let f = follow.clone();
    watch_file(&opusdm_ipc::dock::dock_config_path(), move || f.dock_config_changed());
    let f = follow.clone();
    watch_file(&opusdm_ipc::settings::desktop_settings_path(), move || f.desktop_changed());
    watch_file(&opusdm_ipc::settings::settings_path(), || {
        opusdm_ui::apply_theme_mode(opusdm_ipc::settings::load_settings(TAG).appearance.theme_mode);
    });

    let Some(listener) = listener else { return };
    let (tx, rx) = async_channel::unbounded::<server::Incoming>();
    if let Err(e) = server::spawn(listener, tx) {
        opusdm_ipc::err_log!(TAG, "cannot start the dock socket thread ({e}): the dock runs without Hide All");
        return;
    }
    glib::spawn_future_local(async move {
        while let Ok(incoming) = rx.recv().await {
            let reply = follow.answer(incoming.request);
            if incoming.reply.send(reply).is_err() {
                opusdm_ipc::err_log!(TAG, "a dock socket client went away before its answer");
            }
        }
        opusdm_ipc::err_log!(TAG, "the dock socket thread ended: Hide All no longer reaches the dock");
    });
}

/// Call `on_change` whenever the file at `path` is written, replaced or
/// removed. The monitor is kept in [`MONITORS`]; a watch that cannot be set
/// up is logged.
fn watch_file(path: &Path, on_change: impl Fn() + 'static) {
    let file = gio::File::for_path(path);
    let monitor = match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
        Ok(m) => m,
        Err(e) => {
            opusdm_ipc::err_log!(TAG, "cannot watch {}: {e}; changes to it reach the dock only after a restart", path.display());
            return;
        }
    };
    monitor.connect_changed(move |_, _, _, event| {
        use gio::FileMonitorEvent::{ChangesDoneHint, Created, Deleted};
        if matches!(event, ChangesDoneHint | Created | Deleted) {
            on_change();
        }
    });
    MONITORS.with(|m| m.borrow_mut().push(monitor));
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::{DockPosition, DockStyle};

    #[test]
    fn the_layout_comes_from_dock_json_and_the_panel_edge() {
        let cfg = DockConfig { position: DockPosition::Left, icon_size: 32, style: DockStyle { opacity: 50, ..Default::default() }, ..Default::default() };
        let layout = layout_of(&cfg, PanelPosition::Bottom);
        assert_eq!((layout.position, layout.icon_size, layout.panel, layout.visible), (DockPosition::Left, 32, PanelPosition::Bottom, true));
        assert_eq!(layout.panel_height, opusdm_ipc::PANEL_HEIGHT);
        assert_eq!(layout.style.opacity, 50);
    }
}
