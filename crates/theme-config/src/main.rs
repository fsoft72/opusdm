//! `opusdm-theme`: edit the OpusDM theme - icon set, desktop background, and
//! Lister colours - and apply it live to the running desktop.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::gio;
use opusdm_desktop_config::ui::{BackgroundSection, FolderSection, GridSection};
use std::cell::RefCell;
use std::rc::Rc;

mod apply;
mod importer;
mod model;
mod ui;

/// Shared across every test module in this crate that mutates the
/// process-wide `XDG_CONFIG_HOME`/`OPUSDM_HUB_SOCK` env vars - libtest runs
/// `#[test]` fns in different files concurrently, so a per-module lock does
/// not stop `model::tests` and `importer::tests` racing each other.
#[cfg(test)]
pub(crate) mod test_support {
    pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Theme").build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let settings_data = model::load_settings();
    let theme = settings_data.theme.clone();
    let desktop_data = opusdm_desktop_config::model::load();

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

    let icon_set = Rc::new(RefCell::new(Rc::new(ui::IconSetSection::new(&theme.icon_set))));
    desktop_page.append(&icon_set.borrow().root);

    let import_btn = gtk::Button::with_label("Import theme...");
    desktop_page.append(&import_btn);
    desktop_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let bg = Rc::new(BackgroundSection::new(&window, &theme.background));
    desktop_page.append(&bg.root);
    desktop_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let grid = Rc::new(GridSection::new(
        desktop_data.icon_size,
        desktop_data.grid_spacing,
        desktop_data.grid_gap,
    ));
    desktop_page.append(&grid.root);
    desktop_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let folder = Rc::new(FolderSection::new(desktop_data.desktop_dir.as_deref()));
    desktop_page.append(&folder.root);

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
    lister_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let appearance_section = Rc::new(ui::AppearanceSection::new(&settings_data.appearance));
    lister_page.append(&appearance_section.root);
    lister_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let behavior_section = Rc::new(ui::BehaviorSection::new(&settings_data.behavior));
    lister_page.append(&behavior_section.root);

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
        let grid = grid.clone();
        let folder = folder.clone();
        let appearance_section = appearance_section.clone();
        let behavior_section = behavior_section.clone();
        let note = note.clone();
        apply.connect_clicked(move |_| {
            let selected = icon_set.borrow().value();
            let ui_settings = opusdm_ipc::Settings {
                theme: opusdm_ipc::Theme {
                    icon_set: opusdm_ipc::IconSetId::None,
                    colors: colors.read(),
                    background: bg.read(),
                },
                appearance: appearance_section.read(),
                behavior: behavior_section.read(),
            };
            let ui_desktop = opusdm_ipc::DesktopSettings {
                icon_size: grid.icon_size(),
                grid_spacing: grid.grid_spacing(),
                grid_gap: grid.grid_gap(),
                desktop_dir: folder.override_dir(),
            }
            .sanitized();
            let result = model::apply_icon_set(&selected, &ui_settings, &ui_desktop);
            match model::apply_full(&result.settings, &result.desktop) {
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
    {
        let window = window.clone();
        let icon_set = icon_set.clone();
        let desktop_page = desktop_page.clone();
        let note = note.clone();
        import_btn.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.zip");
            filter.add_pattern("*.tar.gz");
            filter.add_pattern("*.tgz");
            filter.add_pattern("*.tar.bz2");
            filter.set_name(Some("Theme archives"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);

            let dialog = gtk::FileDialog::builder().title("Import Theme").filters(&filters).build();
            let icon_set = icon_set.clone();
            let desktop_page = desktop_page.clone();
            let note = note.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                match crate::importer::import_theme_archive(&path) {
                    Ok(slug) => {
                        note.set_text(&format!("Imported theme \"{slug}\". Select it above and click Apply."));
                        note.set_visible(true);
                        let old_root = icon_set.borrow().root.clone();
                        let new_section = Rc::new(ui::IconSetSection::new(&opusdm_ipc::IconSetId::Custom(slug)));
                        desktop_page.insert_child_after(&new_section.root, Some(&old_root));
                        desktop_page.remove(&old_root);
                        *icon_set.borrow_mut() = new_section;
                    }
                    Err(e) => {
                        note.set_text(&format!("Import failed: {e}"));
                        note.set_visible(true);
                    }
                }
            });
        });
    }

    window.present();
}
