//! `opusdm-settings`: edit the OpusDM theme - icon set, desktop background,
//! and Lister colours - and apply it live to the running desktop.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::gio;
use opusdm_desktop_config::dock_ui::{DockAppsSection, DockSection};
use opusdm_desktop_config::menu_ui::MenuSection;
use opusdm_ipc::IconSetId;
use opusdm_desktop_config::ui::{BackgroundSection, FolderSection, GridSection, PanelSection};
use std::cell::RefCell;
use std::rc::Rc;

mod apply;
mod builder;
mod draft;
mod editor;
mod exporter;
mod importer;
mod model;
mod ui;
mod writer;

/// Shared across every test module in this crate that mutates the
/// process-wide `XDG_CONFIG_HOME`/`OPUSDM_HUB_SOCK` env vars - libtest runs
/// `#[test]` fns in different files concurrently, so a per-module lock does
/// not stop `model::tests` and `importer::tests` racing each other.
#[cfg(test)]
pub(crate) mod test_support {
    pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point `XDG_CONFIG_HOME` at a fresh temporary folder. Hold `ENV_LOCK` first.
    pub fn scratch_config() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", td.path());
        td
    }

    /// Install a minimal theme `slug` with an icon, a font and a sound.
    pub fn install_source(slug: &str) {
        let dir = opusdm_ipc::themes::theme_dir(slug);
        let manifest = format!(r#"{{"name":"{slug}","label":"Src"}}"#);
        for (rel, body) in [("manifest.json", manifest.as_bytes()), ("icons/dir.png", b"icon"), ("fonts/f.ttf", b"font"), ("sounds/a.wav", b"snd")] {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }

    /// Write a tiny file named `name` under `dir` whose content is its name.
    pub fn picture(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, name.as_bytes()).unwrap();
        p
    }
}

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("org.opusdm.Settings").build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &gtk::Application) {
    let settings_data = model::load_settings();
    let IconSetId::Custom(active_slug) = &settings_data.theme.icon_set;
    opusdm_ui::register_theme_fonts(active_slug);
    let look_css = opusdm_ui::ui_look_css(&settings_data);
    opusdm_ui::apply_theme_mode(settings_data.appearance.theme_mode);
    let theme = settings_data.theme.clone();
    let desktop_data = opusdm_desktop_config::model::load();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Settings")
        .default_width(480)
        .default_height(680)
        .build();

    // Same fixed dark/light chrome palette as opusdm-lister
    // (`opusdm_ui::chrome_css`), so this window matches the rest of
    // OpusDM regardless of what GTK theme is or isn't installed.
    let chrome_provider = gtk::CssProvider::new();
    chrome_provider.load_from_data(&format!("{}{look_css}", opusdm_ui::chrome_css(settings_data.appearance.theme_mode)));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("no default GDK display: cannot run without one"),
        &chrome_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

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

    let theme_mode_section = Rc::new(ui::ThemeModeSection::new(settings_data.appearance.theme_mode));
    desktop_page.append(&theme_mode_section.root);
    {
        let chrome_provider = chrome_provider.clone();
        let look_css = look_css.clone();
        theme_mode_section.connect_mode_changed(move |mode| {
            opusdm_ui::apply_theme_mode(mode);
            chrome_provider.load_from_data(&format!("{}{look_css}", opusdm_ui::chrome_css(mode)));
        });
    }

    let icon_set = Rc::new(RefCell::new(Rc::new(ui::IconSetSection::new(&theme.icon_set))));
    desktop_page.append(&icon_set.borrow().root);

    let theme_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let import_btn = gtk::Button::with_label("Import theme...");
    let save_btn = gtk::Button::with_label("Save theme...");
    let export_btn = gtk::Button::with_label("Export theme...");
    theme_buttons.append(&import_btn);
    theme_buttons.append(&save_btn);
    theme_buttons.append(&export_btn);
    let edit_btn = gtk::Button::with_label("Edit theme...");
    theme_buttons.append(&edit_btn);
    desktop_page.append(&theme_buttons);
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
    desktop_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let panel = Rc::new(PanelSection::new(desktop_data.panel_position));
    desktop_page.append(&panel.root);

    notebook.append_page(&desktop_page, Some(&gtk::Label::new(Some("Desktop"))));

    // --- Dock tab: edge, icon size, pinned apps ---
    let dock_page_outer = gtk::ScrolledWindow::new();
    let dock_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    dock_page.set_margin_top(12);
    dock_page.set_margin_bottom(12);
    dock_page.set_margin_start(12);
    dock_page.set_margin_end(12);

    let dock_section = DockSection::new(
        desktop_data.dock_position,
        desktop_data.dock_icon_size,
        desktop_data.panel_position,
    );
    dock_page.append(&dock_section.root);
    dock_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let dock_apps = opusdm_ipc::dock::load_dock_config().map(|c| c.apps).unwrap_or_default();
    let dock_apps_section = Rc::new(DockAppsSection::new(&window, dock_apps));
    dock_page.append(&dock_apps_section.root);

    dock_page_outer.set_child(Some(&dock_page));
    notebook.append_page(&dock_page_outer, Some(&gtk::Label::new(Some("Dock"))));

    // The panel edge and dock edge stay mutually exclusive live.
    {
        let dock_section = dock_section.clone();
        panel.connect_position_changed(move |position| dock_section.set_panel(position));
    }

    // --- Menu tab: custom folders and apps for the application menu ---
    // No outer ScrolledWindow here: MenuSection scrolls only its tree, so
    // "Add Folder"/"Add App"/"Reset Default" stay fixed above it.
    let menu_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    menu_page.set_margin_top(12);
    menu_page.set_margin_bottom(12);
    menu_page.set_margin_start(12);
    menu_page.set_margin_end(12);

    let menu_items = opusdm_ipc::menu::load_menu_config().map(|c| c.items).unwrap_or_default();
    let menu_section = Rc::new(MenuSection::new(&window, menu_items));
    menu_page.append(&menu_section.root);

    notebook.append_page(&menu_page, Some(&gtk::Label::new(Some("Menu"))));

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
    // Fonts and surfaces are not edited by this window, only carried.
    let look = Rc::new(RefCell::new((settings_data.fonts.clone(), settings_data.surfaces.clone())));

    // Reads every editable control back into settings, shared by Apply and Save theme.
    let read_ui: Rc<dyn Fn() -> (opusdm_ipc::Settings, opusdm_ipc::DesktopSettings)> = {
        let colors = colors.clone();
        let bg = bg.clone();
        let grid = grid.clone();
        let folder = folder.clone();
        let panel = panel.clone();
        let dock_section = dock_section.clone();
        let appearance_section = appearance_section.clone();
        let behavior_section = behavior_section.clone();
        let theme_mode_section = theme_mode_section.clone();
        let look = look.clone();
        Rc::new(move || {
            let settings = opusdm_ipc::Settings {
                theme: opusdm_ipc::Theme {
                    icon_set: opusdm_ipc::IconSetId::default(),
                    colors: colors.read(),
                    background: bg.read(),
                },
                appearance: appearance_section.read(theme_mode_section.value()),
                behavior: behavior_section.read(),
                fonts: look.borrow().0.clone(),
                surfaces: look.borrow().1.clone(),
            };
            let desktop = opusdm_ipc::DesktopSettings {
                icon_size: grid.icon_size(),
                grid_spacing: grid.grid_spacing(),
                grid_gap: grid.grid_gap(),
                desktop_dir: folder.override_dir(),
                panel_position: panel.position(),
                dock_position: dock_section.position(),
                dock_icon_size: dock_section.icon_size(),
            }
            .sanitized();
            (settings, desktop)
        })
    };

    // Rebuilds the icon-set picker with `slug` selected, keeping its place above the theme buttons.
    let select_theme: Rc<dyn Fn(String)> = {
        let icon_set = icon_set.clone();
        let desktop_page = desktop_page.clone();
        Rc::new(move |slug: String| {
            let old_root = icon_set.borrow().root.clone();
            let new_section = Rc::new(ui::IconSetSection::new(&opusdm_ipc::IconSetId::Custom(slug)));
            desktop_page.insert_child_after(&new_section.root, Some(&old_root));
            desktop_page.remove(&old_root);
            *icon_set.borrow_mut() = new_section;
        })
    };

    {
        let icon_set = icon_set.clone();
        let read_ui = read_ui.clone();
        let dock_apps_section = dock_apps_section.clone();
        let menu_section = menu_section.clone();
        let note = note.clone();
        let look = look.clone();
        apply.connect_clicked(move |_| {
            let selected = icon_set.borrow().value();
            let (ui_settings, ui_desktop) = read_ui();
            if let Err(e) = opusdm_ipc::dock::save_dock_config(&opusdm_ipc::DockConfig {
                apps: dock_apps_section.apps(),
            }) {
                opusdm_ipc::err_log!("theme-config", "cannot write dock.json: {e}");
            }
            if let Err(e) = opusdm_ipc::menu::save_menu_config(&opusdm_ipc::MenuConfig {
                items: menu_section.items(),
            }) {
                opusdm_ipc::err_log!("theme-config", "cannot write menu.json: {e}");
            }
            let result = model::apply_icon_set(&selected, &ui_settings, &ui_desktop);
            *look.borrow_mut() = (result.settings.fonts.clone(), result.settings.surfaces.clone());
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
        let select_theme = select_theme.clone();
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
            let select_theme = select_theme.clone();
            let note = note.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                match crate::importer::import_theme_archive(&path) {
                    Ok(slug) => {
                        note.set_text(&format!("Imported theme \"{slug}\". Select it above and click Apply."));
                        note.set_visible(true);
                        select_theme(slug.clone());
                    }
                    Err(e) => {
                        note.set_text(&format!("Import failed: {e}"));
                        note.set_visible(true);
                    }
                }
            });
        });
    }

    {
        let window = window.clone();
        let icon_set = icon_set.clone();
        let read_ui = read_ui.clone();
        let select_theme = select_theme.clone();
        let note = note.clone();
        save_btn.connect_clicked(move |_| {
            let window2 = window.clone();
            let icon_set = icon_set.clone();
            let read_ui = read_ui.clone();
            let select_theme = select_theme.clone();
            let note = note.clone();
            opusdm_ui::requester_entry(&window, "Save Theme", "", &[("Save", true), ("Cancel", false)], move |save, label| {
                if !save {
                    return;
                }
                let Some(slug) = builder::slugify(&label) else {
                    note.set_text("Save failed: the name needs at least one letter or digit.");
                    note.set_visible(true);
                    return;
                };
                let do_save = {
                    let icon_set = icon_set.clone();
                    let read_ui = read_ui.clone();
                    let select_theme = select_theme.clone();
                    let note = note.clone();
                    let slug = slug.clone();
                    move || {
                        let opusdm_ipc::IconSetId::Custom(source) = icon_set.borrow().value();
                        let (settings, desktop) = read_ui();
                        let req = builder::BuildRequest { label: &label, slug: &slug, source_slug: &source, settings: &settings, desktop: &desktop };
                        match builder::save_theme(&req) {
                            Ok(_) => {
                                note.set_text(&format!("Saved theme \"{slug}\"."));
                                select_theme(slug.clone());
                            }
                            Err(e) => note.set_text(&format!("Save failed: {e}")),
                        }
                        note.set_visible(true);
                    }
                };
                if !opusdm_ipc::themes::theme_dir(&slug).exists() {
                    do_save();
                    return;
                }
                opusdm_ui::requester(&window2, "Replace Theme", &format!("A theme named \"{slug}\" already exists. Replace it?"), None, &[("Replace", true), ("Cancel", false)], move |replace| {
                    if replace {
                        do_save();
                    }
                });
            });
        });
    }
    {
        let window = window.clone();
        let select_theme = select_theme.clone();
        edit_btn.connect_clicked(move |_| editor::open(&window, select_theme.clone()));
    }
    {
        let window = window.clone();
        let icon_set = icon_set.clone();
        let note = note.clone();
        export_btn.connect_clicked(move |_| {
            let opusdm_ipc::IconSetId::Custom(slug) = icon_set.borrow().value();
            let dialog = gtk::FileDialog::builder().title("Export Theme").initial_name(format!("{slug}.zip")).build();
            let note = note.clone();
            dialog.save(Some(&window), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(mut path) = file.path() else { return };
                if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip")) {
                    path.set_extension("zip");
                }
                match exporter::export_theme(&slug, &path) {
                    Ok(()) => note.set_text(&format!("Exported to {}.", path.display())),
                    Err(e) => note.set_text(&format!("Export failed: {e}")),
                }
                note.set_visible(true);
            });
        });
    }

    window.present();
}
