//! `opus-desktop-config`: configure the OpusDM desktop icon grid and which
//! directory is shown as the desktop. Background editing lives in
//! `opusdm-theme`.

use gtk4 as gtk;
use gtk::prelude::*;
use opusdm_desktop_config::{model, ui};

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.opusdm.DesktopConfig")
        .build();
    app.connect_activate(build_window);
    app.run()
}

/// Build and show the single configuration window.
fn build_window(app: &gtk::Application) {
    let settings = model::load();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Desktop")
        .default_width(480)
        .default_height(400)
        .build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    let grid = std::rc::Rc::new(ui::GridSection::new(
        settings.icon_size,
        settings.grid_spacing,
        settings.grid_gap,
    ));
    let folder = std::rc::Rc::new(ui::FolderSection::new(settings.desktop_dir.as_deref()));

    outer.append(&grid.root);
    outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    outer.append(&folder.root);

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
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    buttons.append(&save);
    outer.append(&buttons);

    window.set_child(Some(&outer));

    let collect: std::rc::Rc<dyn Fn() -> opusdm_ipc::DesktopSettings> = {
        let grid = grid.clone();
        let folder = folder.clone();
        std::rc::Rc::new(move || {
            opusdm_ipc::DesktopSettings {
                icon_size: grid.icon_size(),
                grid_spacing: grid.grid_spacing(),
                grid_gap: grid.grid_gap(),
                desktop_dir: folder.override_dir(),
            }
            .sanitized()
        })
    };

    let run_apply: std::rc::Rc<dyn Fn(&opusdm_ipc::DesktopSettings)> = {
        let note = note.clone();
        std::rc::Rc::new(move |settings: &opusdm_ipc::DesktopSettings| {
            match model::apply(settings) {
                model::ApplyOutcome::SentToHub => note.set_visible(false),
                model::ApplyOutcome::WroteFile => {
                    note.set_text(
                        "OpusDM non in esecuzione: le modifiche si applicheranno al prossimo avvio.",
                    );
                    note.set_visible(true);
                }
                model::ApplyOutcome::Failed(e) => {
                    note.set_text(&format!("Impossibile salvare: {e}"));
                    note.set_visible(true);
                }
            }
        })
    };

    {
        let collect = collect.clone();
        let run_apply = run_apply.clone();
        apply.connect_clicked(move |_| run_apply(&collect()));
    }
    {
        let collect = collect.clone();
        let run_apply = run_apply.clone();
        let window = window.clone();
        save.connect_clicked(move |_| {
            run_apply(&collect());
            window.close();
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }

    window.present();
}
