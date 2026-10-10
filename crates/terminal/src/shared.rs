//! State shared by every window of one `opusdm-terminal` process: the
//! config, the loaded themes and the order in which terminals got the focus.

use crate::config::{self, TerminalConfig};
use crate::theme::{self, TerminalTheme};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Process-wide terminal state.
pub struct Shared {
    /// The application owning every terminal window.
    pub app: gtk::Application,
    config: RefCell<TerminalConfig>,
    themes: RefCell<Vec<TerminalTheme>>,
    /// Terminals by most recent focus, newest first.
    focus_history: RefCell<Vec<glib::WeakRef<vte4::Terminal>>>,
}

impl Shared {
    /// Load the config and the themes for `app`.
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            config: RefCell::new(config::load()),
            themes: RefCell::new(theme::all_themes()),
            focus_history: RefCell::new(Vec::new()),
        })
    }

    /// A copy of the current config.
    pub fn config(&self) -> TerminalConfig {
        self.config.borrow().clone()
    }

    /// Every theme available, presets first.
    pub fn themes(&self) -> Vec<TerminalTheme> {
        self.themes.borrow().clone()
    }

    /// The theme the config selects (or the default one).
    pub fn theme(&self) -> TerminalTheme {
        theme::find(&self.themes.borrow(), &self.config.borrow().theme).clone()
    }

    /// Re-read the user themes from disk.
    pub fn reload_themes(&self) {
        *self.themes.borrow_mut() = theme::all_themes();
    }

    /// Store `config`, save it and apply it to every open terminal.
    pub fn update_config(&self, config: TerminalConfig) {
        config::save(&config);
        *self.config.borrow_mut() = config;
        self.reload_themes();
        self.preview(&self.config());
    }

    /// Apply `config` to every open terminal without storing it, for the
    /// live preview of the Preferences dialog. `preview(&self.config())`
    /// puts the saved options back.
    pub fn preview(&self, config: &TerminalConfig) {
        let theme = theme::find(&self.themes.borrow(), &config.theme).clone();
        self.preview_with(config, &theme);
    }

    /// Apply `config` with the colours of `theme` (which may be an unsaved
    /// edit) to every open terminal.
    pub fn preview_with(&self, config: &TerminalConfig, theme: &TerminalTheme) {
        for term in self.all_terminals() {
            crate::pane::apply_config(&term, config, theme);
        }
    }

    /// Every terminal in every window of the application.
    pub fn all_terminals(&self) -> Vec<vte4::Terminal> {
        self.app
            .windows()
            .iter()
            .flat_map(|w| crate::layout::terminals_in(w.upcast_ref()))
            .collect()
    }

    /// Record that `term` just got the keyboard focus.
    pub fn remember_focus(&self, term: &vte4::Terminal) {
        let mut history = self.focus_history.borrow_mut();
        history.retain(|w| w.upgrade().is_some_and(|t| &t != term));
        history.insert(0, term.downgrade());
    }

    /// The terminal inside `ancestor` that had the focus most recently.
    pub fn last_focused_in(&self, ancestor: &gtk::Widget) -> Option<vte4::Terminal> {
        self.focus_history
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .find(|t| t.is_ancestor(ancestor))
    }
}
