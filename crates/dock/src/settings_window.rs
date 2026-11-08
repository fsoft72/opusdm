//! The dock Settings window, opened from the dock background menu: edge and
//! icon size, look (with "Show the dock") and the pinned apps.
//!
//! Apply writes the fields of the window into `dock.json` with
//! `update_dock_config` (fresh read, atomic save); the dock follows the file
//! like any other change. Turning "Show the dock" off and applying ends the
//! dock process. Cancel closes the window without saving.

use crate::settings_ui::{DockAppsSection, DockLookSection, DockSection};
use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_ipc::dock::DockConfig;
use opusdm_ipc::PanelPosition;
use std::rc::Rc;

/// Title of the window.
pub const WINDOW_TITLE: &str = "Dock Settings";

/// Default size of the window, in pixels.
const DEFAULT_SIZE: (i32, i32) = (560, 680);

/// The controls Apply reads back.
struct Sections {
    dock: Rc<DockSection>,
    look: Rc<DockLookSection>,
    apps: DockAppsSection,
}

impl Sections {
    /// Copy the values of the controls into `cfg`.
    fn write_into(&self, cfg: &mut DockConfig) {
        cfg.position = self.dock.position();
        cfg.icon_size = self.dock.icon_size();
        cfg.visible = self.look.visible();
        cfg.style = self.look.style();
        cfg.apps = Some(self.apps.apps());
    }
}

/// Build the window on `app`, seeded with `cfg`; `apps` is the pinned list
/// shown (the dock's own when `cfg` has none yet) and `panel` the edge the
/// dock must keep clear of.
pub fn build(app: &gtk::Application, cfg: &DockConfig, apps: Vec<String>, panel: PanelPosition) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .default_width(DEFAULT_SIZE.0)
        .default_height(DEFAULT_SIZE.1)
        .build();

    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    for set_margin in [gtk::Widget::set_margin_top, gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set_margin(page.upcast_ref(), 12);
    }
    let sections = Rc::new(Sections {
        dock: DockSection::new(cfg.position, cfg.icon_size, panel),
        look: DockLookSection::new(&window, cfg.visible, &cfg.style),
        apps: DockAppsSection::new(&window, apps),
    });
    page.append(&sections.dock.root);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&sections.look.root);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&sections.apps.root);
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&page));
    scrolled.set_vexpand(true);

    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_visible(false);

    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&apply);

    let footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    for set_margin in [gtk::Widget::set_margin_bottom, gtk::Widget::set_margin_start, gtk::Widget::set_margin_end] {
        set_margin(footer.upcast_ref(), 12);
    }
    footer.append(&note);
    footer.append(&buttons);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scrolled);
    content.append(&footer);
    window.set_child(Some(&content));

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    apply.connect_clicked(move |_| {
        match opusdm_ipc::dock::update_dock_config(|cfg| sections.write_into(cfg)) {
            Ok(saved) => {
                opusdm_ipc::debug_log!("opusdm-dock", "dock settings applied");
                note.set_text(if saved.visible { "Applied." } else { "Applied: the dock is turned off and closes now." });
            }
            Err(e) => {
                opusdm_ipc::err_log!("opusdm-dock", "cannot save the dock settings: {e}");
                note.set_text(&format!("Cannot save the dock settings: {e}"));
            }
        }
        note.set_visible(true);
    });
    window
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::{DockPosition, DockStyle};

    #[test]
    fn the_sections_write_back_what_they_were_seeded_with() {
        crate::gtk_test::run(|| {
            let window = gtk::Window::new();
            let style = DockStyle { opacity: 30, zoom_enabled: true, ..Default::default() };
            let sections = Sections {
                dock: DockSection::new(DockPosition::Left, 64, PanelPosition::Top),
                look: DockLookSection::new(&window, false, &style),
                apps: DockAppsSection::new(&window, vec!["a.desktop".into(), "b.desktop".into()]),
            };
            let mut cfg = DockConfig { layout_migrated: true, ..Default::default() };
            sections.write_into(&mut cfg);
            assert_eq!((cfg.position, cfg.icon_size, cfg.visible), (DockPosition::Left, 64, false));
            assert_eq!(cfg.style, style.sanitized());
            assert_eq!(cfg.apps, Some(vec!["a.desktop".to_string(), "b.desktop".to_string()]));
            assert!(cfg.layout_migrated, "fields the window does not own stay as they were");
        });
    }
}
