//! Messages from the hub: [`Ui::apply_inbound`] only dispatches, and each
//! group of messages (roles and window, caches, prompts and jobs, config,
//! functions) has its own handler below.

use crate::client::Roles;
use crate::window::Ui;
use gtk4::prelude::*;
use opusdm_ipc::{Settings, ToHub, ToLister};
use std::path::Path;
use std::rc::Rc;

impl Ui {
    /// Handle a message from the hub.
    pub fn apply_inbound(self: &Rc<Self>, msg: ToLister) {
        crate::dlog!("hub msg: {msg:?}");
        match msg {
            // Roles and the window itself.
            ToLister::RoleChanged { source, dests, source_locked } => self.on_roles(Roles { source, dests, source_locked }),
            ToLister::Present => self.window.present(),
            ToLister::SetHidden(hidden) => self.set_hidden(hidden),
            ToLister::Shutdown => self.window.close(),
            ToLister::ScanDir { path, select, mode } => self.on_scan_dir(&path, select, mode.as_deref()),
            ToLister::GoTo { path } => {
                self.window.present();
                self.navigate_to(&path);
            }
            ToLister::QuerySelection => {
                self.send_hub(&ToHub::SelectionReply { paths: self.selected_paths(), cwd: self.cwd_path() });
            }
            ToLister::Pong => {}

            // Directory caches.
            ToLister::InvalidatePath(p) => self.on_invalidate_path(&p),
            ToLister::FreeCaches => self.free_caches(),
            ToLister::CacheList(entries) => crate::cache_list::show(self, entries),

            // Jobs and the questions they ask.
            // Progress is shown centrally by the hub's progress window now.
            ToLister::JobProgress { .. } => {}
            ToLister::ConfirmPrompt { req_id, message } => self.ask_for_hub("Confirm", &message, &[("OK", true), ("Cancel", false)], req_id),
            ToLister::AlertPrompt { req_id, message } => self.ask_for_hub("Alert", &message, &[("OK", true)], req_id),
            ToLister::ConflictPrompt { req_id, path, src, dst, .. } => crate::actions::on_conflict(self, req_id, &path, src, dst),
            ToLister::JobDone { job_id, outcome, errors } => crate::actions::on_job_done(self, job_id, &outcome, &errors),
            ToLister::RemoteTransfer { op, srcs, dest_dir } => self.run_remote_transfer(op, srcs, dest_dir),
            ToLister::CheckFitResult { dest_dir, needed, free, fits, percent } => {
                crate::checkfit_ui::show_result(self, &dest_dir, needed, free, fits, percent);
            }
            ToLister::CheckFitFailed { message } => crate::actions::flash(self, &format!("check fit failed: {message}")),

            // Configuration pushed by the hub.
            ToLister::SettingsUpdate(new_settings) => self.on_settings(*new_settings),
            ToLister::FormatsUpdate(new_formats) => {
                *self.hub.formats.borrow_mut() = new_formats;
                self.on_formats_changed();
            }
            ToLister::FunctionsUpdate(new_functions) => *self.hub.functions.borrow_mut() = new_functions,
            ToLister::BindingsUpdate(new_bindings) => {
                *self.hub.hotkeys.borrow_mut() = crate::hotkeys::HotkeyMap::from_hotkeys(&new_bindings.hotkeys);
                *self.hub.bindings.borrow_mut() = new_bindings;
            }

            // Functions started here and their output windows.
            ToLister::OutputOpen { id, title } => crate::output_window::open(self, id, &title),
            ToLister::OutputLines { id, lines } => crate::output_window::append(self, id, &crate::output_window::lines_text(&lines)),
            ToLister::OutputEnd { id, exit } => crate::output_window::end(self, id, exit),
            // Step output is shown by the Manage Functions window's Test;
            // a run from here only reports how it ended.
            ToLister::FunctionOutput { .. } | ToLister::FunctionStepDone { .. } => {}
            ToLister::FunctionFailed { step_idx, error } => {
                self.on_function_end(&format!("function failed at step {step_idx}: {error}"));
            }
            ToLister::FunctionDone => self.on_function_end("function finished"),
        }
    }

    /// New global roles: repaint the badge and the role buttons.
    fn on_roles(self: &Rc<Self>, roles: Roles) {
        *self.hub.roles.borrow_mut() = roles;
        self.update_status();
        crate::actions::sync_role_buttons(self);
    }

    /// Show `path` (raising the window), optionally in another view mode and
    /// with only `select` selected.
    fn on_scan_dir(self: &Rc<Self>, path: &Path, select: Option<String>, mode: Option<&str>) {
        self.window.present();
        // The toggle button drives the view, so its state follows.
        match mode {
            Some("icons") => self.view.mode_toggle.set_active(true),
            Some("details") => self.view.mode_toggle.set_active(false),
            _ => {}
        }
        let ui = self.clone();
        self.navigate(path, true, move || {
            if let Some(name) = &select {
                ui.view.selection.unselect_all();
                ui.select_names(std::slice::from_ref(name));
            }
        });
    }

    /// `path` changed on disk: forget its cached listing, and reload it when
    /// it is on screen.
    fn on_invalidate_path(self: &Rc<Self>, path: &Path) {
        if self.caches.dirs.borrow_mut().remove(path) {
            self.report_cache();
        }
        if path == self.cwd.borrow().as_path() {
            self.reload();
        }
    }

    /// A question the hub asks for a running job or script: show it and send
    /// back the button chosen.
    fn ask_for_hub(self: &Rc<Self>, title: &str, message: &str, buttons: &[(&str, bool)], req_id: u64) {
        let ui = self.clone();
        opusdm_ui::requester(&self.window, title, message, None, buttons, move |ok| {
            ui.send_hub(&ToHub::ConfirmReply { req_id, ok });
        });
    }

    /// New settings: apply everything that depends on them.
    fn on_settings(self: &Rc<Self>, new_settings: Settings) {
        let names_changed = {
            let old = self.hub.settings.borrow();
            old.theme.icon_set != new_settings.theme.icon_set
                || old.appearance.type_indicators != new_settings.appearance.type_indicators
        };
        *self.hub.settings.borrow_mut() = new_settings;
        let capacity = self.hub.settings.borrow().behavior.cache_capacity();
        self.caches.dirs.borrow_mut().set_capacity(capacity);
        self.report_cache();
        self.update_status();
        opusdm_ui::apply_theme_mode(self.hub.settings.borrow().appearance.theme_mode);
        self.update_css_for_colors();
        self.view.icon_field.set_icon_set(self.hub.settings.borrow().theme.icon_set.clone());
        // Auto-refresh may have just been switched on or off.
        let cwd = self.cwd.borrow().clone();
        self.sync_watch(&cwd);
        // Name-column text is built at bind time, so glyph and
        // indicator changes only show after the rows are rebound.
        if names_changed {
            self.touch_rows();
        }
    }

    /// A function started here ended: stop the busy indicator and say how.
    fn on_function_end(self: &Rc<Self>, message: &str) {
        self.run_finished();
        crate::actions::flash(self, message);
    }
}
