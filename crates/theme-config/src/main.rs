//! `opusdm-theme`: edit the OpusDM theme - icon set, Lister colours, and
//! desktop background - and apply it live to the running desktop.

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

    let icon_set = Rc::new(ui::IconSetSection::new(theme.icon_set));
    outer.append(&icon_set.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let colors = Rc::new(ui::ColorsSection::new(&window, &theme.colors));
    outer.append(&colors.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let bg = Rc::new(BackgroundSection::new(&window, &theme.background));
    outer.append(&bg.root);

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
