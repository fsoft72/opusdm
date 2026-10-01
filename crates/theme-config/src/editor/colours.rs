//! Colours and Appearance tab. Behaviour and desktop metrics are not edited
//! here; they are carried through the draft untouched.

use super::optional::OptionalSection;
use crate::ui::{AppearanceSection, ColorsSection, PresetSection, ThemeModeSection};
use crate::writer::ThemeDraft;
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::{Appearance, ColorSettings};
use std::rc::Rc;

/// The two optional sections of the tab.
pub struct ColoursTab {
    pub root: gtk::ScrolledWindow,
    colours: Rc<ColorsSection>,
    colours_opt: OptionalSection,
    mode: ThemeModeSection,
    appearance: AppearanceSection,
    appearance_opt: OptionalSection,
    initial: (Option<ColorSettings>, Option<Appearance>),
}

impl ColoursTab {
    /// Build the tab seeded from `draft`; `window` parents the colour dialogs.
    pub fn new(window: &gtk::ApplicationWindow, draft: &ThemeDraft) -> Self {
        let colours_value = draft.colors.clone().unwrap_or_default();
        let colours = Rc::new(ColorsSection::new(window, &colours_value));
        let preset = PresetSection::new();
        preset.connect_selected(colours.clone());
        let colours_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        colours_box.append(&preset.root);
        colours_box.append(&colours.root);
        let colours_opt = OptionalSection::new("COLOURS", &colours_box, draft.colors.is_some());

        let appearance_value = draft.appearance.clone().unwrap_or_default();
        let mode = ThemeModeSection::new(appearance_value.theme_mode);
        let appearance = AppearanceSection::new(&appearance_value);
        let appearance_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        appearance_box.append(&mode.root);
        appearance_box.append(&appearance.root);
        let appearance_opt = OptionalSection::new("APPEARANCE", &appearance_box, draft.appearance.is_some());

        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);
        page.set_margin_start(12);
        page.set_margin_end(12);
        page.append(&colours_opt.root);
        page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        page.append(&appearance_opt.root);
        let root = gtk::ScrolledWindow::builder().child(&page).build();

        let mut tab = Self { root, colours, colours_opt, mode, appearance, appearance_opt, initial: (None, None) };
        tab.initial = tab.read();
        tab
    }

    fn read(&self) -> (Option<ColorSettings>, Option<Appearance>) {
        (
            self.colours_opt.is_set().then(|| self.colours.read()),
            self.appearance_opt.is_set().then(|| self.appearance.read(self.mode.value())),
        )
    }

    /// Write the tab's values into `draft` when they changed.
    pub fn collect_into(&self, draft: &mut ThemeDraft) {
        let now = self.read();
        if now.0 != self.initial.0 {
            draft.colors = now.0;
        }
        if now.1 != self.initial.1 {
            draft.appearance = now.1;
        }
    }
}
