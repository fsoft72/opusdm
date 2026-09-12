//! `opusdm-theme`: edit the OpusDM theme - icon set, desktop background, and
//! Lister colours - and apply it live to the running desktop.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::ui::BackgroundSection;
use std::rc::Rc;

mod model;
mod ui;

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Theme").build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let theme = model::load();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Theme")
        .default_width(480)
        .default_height(680)
        .build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);

    // --- Desktop tab: icon set + background ---
    let desktop_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    desktop_page.set_margin_top(12);
    desktop_page.set_margin_bottom(12);
    desktop_page.set_margin_start(12);
    desktop_page.set_margin_end(12);

    let icon_set = Rc::new(ui::IconSetSection::new(theme.icon_set));
    desktop_page.append(&icon_set.root);
    desktop_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let bg = Rc::new(BackgroundSection::new(&window, &theme.background));
    desktop_page.append(&bg.root);

    notebook.append_page(&desktop_page, Some(&gtk::Label::new(Some("Desktop"))));

    // --- Lister tab: preset picker + colours ---
    let lister_page_outer = gtk::ScrolledWindow::new();
    let lister_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    lister_page.set_margin_top(12);
    lister_page.set_margin_bottom(12);
    lister_page.set_margin_start(12);
    lister_page.set_margin_end(12);

    let colors = Rc::new(ui::ColorsSection::new(&window, &theme.colors));
    let preset = ui::PresetSection::new();
    preset.connect_selected(colors.clone());
    lister_page.append(&preset.root);
    lister_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    lister_page.append(&colors.root);
    lister_page_outer.set_child(Some(&lister_page));

    notebook.append_page(&lister_page_outer, Some(&gtk::Label::new(Some("Lister"))));

    outer.append(&notebook);

    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_visible(false);
    outer.append(&note);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    buttons.append(&cancel);
    buttons.append(&apply);
    outer.append(&buttons);

    window.set_child(Some(&outer));

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let icon_set = icon_set.clone();
        let colors = colors.clone();
        let bg = bg.clone();
        let note = note.clone();
        apply.connect_clicked(move |_| {
            let theme = opusdm_ipc::Theme {
                icon_set: icon_set.value(),
                colors: colors.read(),
                background: bg.read(),
            };
            match model::apply(&theme) {
                model::ApplyOutcome::SentToHub => {
                    note.set_text("Applied to the running desktop.");
                    note.set_visible(true);
                }
                model::ApplyOutcome::WroteFile => {
                    note.set_text("No hub running - saved, effective on next login.");
                    note.set_visible(true);
                }
                model::ApplyOutcome::Failed(e) => {
                    note.set_text(&format!("Could not apply: {e}"));
                    note.set_visible(true);
                }
            }
        });
    }

    window.present();
}
