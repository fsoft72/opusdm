//! `opusdm-settings`: pick the OpusDM theme and edit the user preferences
//! (light / dark, wallpaper, icon grid, panel, dock, menu, Lister behaviour),
//! and apply them live to the running desktop. The look itself (colours,
//! fonts, pictures, icons) is edited only in the Theme Editor.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::gio;
use opusdm_desktop_config::dock_ui::{DockAppsSection, DockSection};
use opusdm_ipc::IconSetId;
use opusdm_desktop_config::background_ui::BackgroundSection;
use opusdm_desktop_config::ui::{BrightnessSection, FolderSection, GridSection, PanelSection, StartupAppsSection, VirtualScreensSection, WindowsSection};
use std::cell::RefCell;
use std::rc::Rc;

mod apply;
mod background;
mod builder;
mod draft;
mod editor;
mod exporter;
mod importer;
mod migrate;
mod model;
mod tools;
mod ui;
mod writer;

fn main() -> gtk::glib::ExitCode {
    // `--edit-theme` opens the Theme Editor right away. The arguments are
    // handled here and not by GApplication, which would reject the unknown flag.
    let edit_theme = std::env::args().any(|a| a == EDIT_THEME_FLAG);
    let app = gtk::Application::builder().application_id("org.opusdm.Settings").build();
    app.connect_startup(|_| opusdm_ui::set_app_icon());
    app.connect_activate(move |app| if edit_theme { build_editor_only(app) } else { build_window(app) });
    app.run_with_args::<&str>(&[])
}

/// The archives Import Theme offers.
const THEME_ARCHIVE_PATTERNS: [&str; 4] = ["*.zip", "*.tar.gz", "*.tgz", "*.tar.bz2"];

/// Command line flag that makes the settings app open only the Theme Editor.
const EDIT_THEME_FLAG: &str = "--edit-theme";

/// Open only the Theme Editor, without the Settings window. The application
/// exits when the editor is closed.
fn build_editor_only(app: &gtk::Application) {
    let settings = model::load_settings();
    let IconSetId::Custom(active_slug) = &settings.theme.icon_set;
    opusdm_ui::register_theme_fonts(active_slug);
    opusdm_ui::apply_theme_mode(settings.appearance.theme_mode);
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!("{}{}", opusdm_ui::chrome_css(settings.appearance.theme_mode), opusdm_ui::ui_look_css(&settings)));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("no default GDK display: cannot run without one"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    editor::open(app, None, Rc::new(|_| {}));
}

/// The controls of the Settings window whose values Apply reads back.
struct Sections {
    theme_mode: Rc<ui::ThemeModeSection>,
    /// Replaced by a new picker when a theme is imported or edited.
    icon_set: Rc<RefCell<Rc<ui::IconSetSection>>>,
    grid: Rc<GridSection>,
    folder: Rc<FolderSection>,
    panel: Rc<PanelSection>,
    screens: Rc<VirtualScreensSection>,
    startup_apps: Rc<StartupAppsSection>,
    windows: Rc<WindowsSection>,
    bg: Rc<BackgroundSection>,
    dock: Rc<DockSection>,
    dock_apps: Rc<DockAppsSection>,
    behavior: Rc<ui::BehaviorSection>,
}

impl Sections {
    /// The preferences shown by the controls. The look fields are left at
    /// their defaults: Apply resolves them from the theme.
    fn read(&self) -> (opusdm_ipc::Settings, opusdm_ipc::DesktopSettings) {
        let prefs = opusdm_ipc::Settings {
            theme: opusdm_ipc::Theme { background: self.bg.read(), ..Default::default() },
            appearance: opusdm_ipc::Appearance { theme_mode: self.theme_mode.value(), ..Default::default() },
            behavior: self.behavior.read(),
            ..Default::default()
        };
        let desktop = opusdm_ipc::DesktopSettings {
            icon_size: self.grid.icon_size(),
            grid_spacing: self.grid.grid_spacing(),
            grid_gap: self.grid.grid_gap(),
            desktop_dir: self.folder.override_dir(),
            panel_position: self.panel.position(),
            dock_position: self.dock.position(),
            dock_icon_size: self.dock.icon_size(),
            virtual_screens: self.screens.count(),
            transparency_enabled: self.windows.enabled(),
            inactive_opacity: self.windows.inactive_opacity(),
            active_opacity: self.windows.active_opacity(),
            startup_apps: self.startup_apps.apps(),
            open_lister_at_start: self.startup_apps.open_lister_at_start(),
            snap_enabled: self.windows.snap_enabled(),
        }
        .sanitized();
        (prefs, desktop)
    }
}

/// The Edit / Import / Export theme buttons of the Desktop tab.
struct ThemeButtons {
    edit: gtk::Button,
    import: gtk::Button,
    export: gtk::Button,
}

/// The Desktop tab and the controls on it.
struct DesktopTab {
    page: gtk::Box,
    theme_mode: Rc<ui::ThemeModeSection>,
    icon_set: Rc<RefCell<Rc<ui::IconSetSection>>>,
    buttons: ThemeButtons,
    grid: Rc<GridSection>,
    folder: Rc<FolderSection>,
    panel: Rc<PanelSection>,
    screens: Rc<VirtualScreensSection>,
}

/// A vertical page box with the standard 12 px spacing and margins.
fn page_box() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_margin_top(12);
    page.set_margin_bottom(12);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page
}

/// `page` inside a scrolled window.
fn scrolled(page: &gtk::Box) -> gtk::ScrolledWindow {
    let outer = gtk::ScrolledWindow::new();
    outer.set_child(Some(page));
    outer
}

/// Install the shared chrome and look CSS for the default display and return
/// the provider, so a theme mode change can reload it.
fn install_chrome(mode: opusdm_ipc::ThemeMode, look_css: &str) -> gtk::CssProvider {
    // Same fixed dark/light chrome palette as opusdm-lister
    // (`opusdm_ui::chrome_css`), so this window matches the rest of
    // OpusDM regardless of what GTK theme is or isn't installed.
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!("{}{look_css}", opusdm_ui::chrome_css(mode)));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("no default GDK display: cannot run without one"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    provider
}

/// Desktop tab: theme mode, icon set and theme buttons, grid, folder, panel,
/// virtual screens and brightness.
fn build_desktop_page(
    settings: &opusdm_ipc::Settings,
    desktop: &opusdm_ipc::DesktopSettings,
    chrome: &gtk::CssProvider,
    look_css: &str,
) -> DesktopTab {
    let page = page_box();
    let theme_mode = Rc::new(ui::ThemeModeSection::new(settings.appearance.theme_mode));
    page.append(&theme_mode.root);
    {
        let chrome = chrome.clone();
        let look_css = look_css.to_string();
        theme_mode.connect_mode_changed(move |mode| {
            opusdm_ui::apply_theme_mode(mode);
            chrome.load_from_data(&format!("{}{look_css}", opusdm_ui::chrome_css(mode)));
        });
    }

    let icon_set = Rc::new(RefCell::new(Rc::new(ui::IconSetSection::new(&settings.theme.icon_set))));
    page.append(&icon_set.borrow().root);

    let buttons = ThemeButtons {
        edit: gtk::Button::with_label("Edit themes..."),
        import: gtk::Button::with_label("Import theme..."),
        export: gtk::Button::with_label("Export theme..."),
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&buttons.edit);
    row.append(&buttons.import);
    row.append(&buttons.export);
    page.append(&row);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let grid = Rc::new(GridSection::new(desktop.icon_size, desktop.grid_spacing, desktop.grid_gap));
    let folder = Rc::new(FolderSection::new(desktop.desktop_dir.as_deref()));
    let panel = Rc::new(PanelSection::new(desktop.panel_position));
    let screens = Rc::new(VirtualScreensSection::new(desktop.virtual_screens));
    for root in [&grid.root, &folder.root, &panel.root, &screens.root] {
        page.append(root);
        page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }

    // Applied live by its own worker: it is not part of desktop.json.
    let brightness = BrightnessSection::new();
    page.append(&brightness.root);
    DesktopTab { page, theme_mode, icon_set, buttons, grid, folder, panel, screens }
}

/// Desktop Background tab: the rotating list of backgrounds, which can also
/// pick the pictures of the theme selected in `icon_set`.
fn build_background_page(window: &gtk::ApplicationWindow, theme: &opusdm_ipc::Theme, icon_set: &Rc<RefCell<Rc<ui::IconSetSection>>>) -> (gtk::Box, Rc<BackgroundSection>) {
    let page = page_box();
    let bg = Rc::new(BackgroundSection::new(window, &theme.background));
    {
        let icon_set = icon_set.clone();
        bg.connect_theme_pictures(window, move || {
            let IconSetId::Custom(slug) = icon_set.borrow().value();
            opusdm_ipc::themes::theme_wallpapers(&slug)
        });
    }
    page.append(&bg.root);
    (page, bg)
}

/// Dock tab: edge, icon size and pinned apps.
fn build_dock_page(window: &gtk::ApplicationWindow, desktop: &opusdm_ipc::DesktopSettings) -> (gtk::ScrolledWindow, Rc<DockSection>, Rc<DockAppsSection>) {
    let page = page_box();
    let dock = DockSection::new(desktop.dock_position, desktop.dock_icon_size, desktop.panel_position);
    page.append(&dock.root);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let apps = opusdm_ipc::dock::load_dock_config().map(|c| c.apps).unwrap_or_default();
    let dock_apps = Rc::new(DockAppsSection::new(window, apps));
    page.append(&dock_apps.root);
    (scrolled(&page), dock, dock_apps)
}

/// The note line under the tabs, showing `text` when there is one.
fn build_note(text: Option<&str>) -> gtk::Label {
    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_visible(false);
    if let Some(text) = text {
        note.set_text(text);
        note.set_visible(true);
    }
    note
}

/// Show `text` in the note line.
fn show_note(note: &gtk::Label, text: &str) {
    note.set_text(text);
    note.set_visible(true);
}

fn build_window(app: &gtk::Application) {
    let desktop_data = opusdm_desktop_config::model::load();
    let (settings_data, migration_note) = migrated_settings(model::load_settings(), &desktop_data);
    let IconSetId::Custom(active_slug) = &settings_data.theme.icon_set;
    opusdm_ui::register_theme_fonts(active_slug);
    let look_css = opusdm_ui::ui_look_css(&settings_data);
    opusdm_ui::apply_theme_mode(settings_data.appearance.theme_mode);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Settings")
        .default_width(480)
        .default_height(680)
        .build();
    let chrome = install_chrome(settings_data.appearance.theme_mode, &look_css);

    let notebook = gtk::Notebook::new();
    notebook.set_vexpand(true);
    let tab = |page: &gtk::Widget, title: &str| {
        notebook.append_page(page, Some(&gtk::Label::new(Some(title))));
    };

    let DesktopTab { page: desktop_page, theme_mode, icon_set, buttons: theme_buttons, grid, folder, panel, screens } =
        build_desktop_page(&settings_data, &desktop_data, &chrome, &look_css);
    tab(desktop_page.upcast_ref(), "Desktop");

    let windows_page = page_box();
    let windows = Rc::new(WindowsSection::new(
        desktop_data.transparency_enabled,
        desktop_data.inactive_opacity,
        desktop_data.active_opacity,
        desktop_data.snap_enabled,
    ));
    windows_page.append(&windows.root);
    tab(windows_page.upcast_ref(), "Windows");

    let (background_page, bg) = build_background_page(&window, &settings_data.theme, &icon_set);
    tab(background_page.upcast_ref(), "Desktop Background");

    let (dock_page, dock, dock_apps) = build_dock_page(&window, &desktop_data);
    tab(dock_page.upcast_ref(), "Dock");
    // The panel edge and dock edge stay mutually exclusive live.
    {
        let dock = dock.clone();
        panel.connect_position_changed(move |position| dock.set_panel(position));
    }

    // Lister tab: behaviour (the colours belong to the theme).
    let lister_page = page_box();
    let behavior = Rc::new(ui::BehaviorSection::new(&settings_data.behavior));
    lister_page.append(&behavior.root);
    tab(scrolled(&lister_page).upcast_ref(), "Lister");

    let startup_page = page_box();
    let startup_apps = Rc::new(StartupAppsSection::new(&desktop_data.startup_apps, desktop_data.open_lister_at_start));
    startup_page.append(&startup_apps.root);
    tab(scrolled(&startup_page).upcast_ref(), "Startup Apps");

    // Advanced and System tabs: buttons that open the other tools.
    tab(tools::build_advanced_page().upcast_ref(), "Advanced");
    tab(tools::build_system_page().upcast_ref(), "System");

    let note = build_note(migration_note.as_deref());
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    buttons.append(&cancel);
    buttons.append(&apply);

    let outer = page_box();
    outer.append(&notebook);
    outer.append(&note);
    outer.append(&buttons);
    window.set_child(Some(&outer));

    let sections = Rc::new(Sections { theme_mode, icon_set, grid, folder, panel, screens, startup_apps, windows, bg, dock, dock_apps, behavior });
    let select_theme = theme_selector(&sections.icon_set, &desktop_page);
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    connect_apply(&apply, &sections, &note);
    // Import and Export run off the GTK thread; the theme buttons and Apply
    // wait meanwhile.
    let busy: Vec<gtk::Widget> = [&theme_buttons.edit, &theme_buttons.import, &theme_buttons.export, &apply].into_iter().map(|b| b.clone().upcast()).collect();
    connect_import(&theme_buttons.import, &window, &select_theme, &note, &busy);
    connect_export(&theme_buttons.export, &window, &sections.icon_set, &note, &busy);
    {
        let window = window.clone();
        theme_buttons.edit.connect_clicked(move |_| {
            let Some(app) = window.application() else { return };
            editor::open(&app, Some(&window), select_theme.clone());
        });
    }
    window.present();
}

/// A function that rebuilds the icon-set picker with `slug` selected,
/// keeping its place in `page` above the theme buttons.
fn theme_selector(icon_set: &Rc<RefCell<Rc<ui::IconSetSection>>>, page: &gtk::Box) -> Rc<dyn Fn(String)> {
    let icon_set = icon_set.clone();
    let page = page.clone();
    Rc::new(move |slug: String| {
        let old_root = icon_set.borrow().root.clone();
        let new_section = Rc::new(ui::IconSetSection::new(&opusdm_ipc::IconSetId::Custom(slug)));
        page.insert_child_after(&new_section.root, Some(&old_root));
        page.remove(&old_root);
        *icon_set.borrow_mut() = new_section;
    })
}

/// Apply: save the dock apps, then apply the selected theme with the
/// preferences of the controls and tell how it went in `note`.
fn connect_apply(apply: &gtk::Button, sections: &Rc<Sections>, note: &gtk::Label) {
    let sections = sections.clone();
    let note = note.clone();
    apply.connect_clicked(move |_| {
        let selected = sections.icon_set.borrow().value();
        let (prefs, desktop) = sections.read();
        let dock_error = opusdm_ipc::dock::save_dock_config(&opusdm_ipc::DockConfig { apps: sections.dock_apps.apps() }).err().map(|e| e.to_string());
        if let Some(e) = &dock_error {
            opusdm_ipc::err_log!("theme-config", "cannot write dock.json: {e}");
        }
        let settings = model::resolve_selected(&selected, &prefs);
        let outcome = model::apply_full(&settings, &desktop);
        show_note(&note, &model::apply_message(&outcome, dock_error.as_deref()));
    });
}

/// Import theme: pick an archive, unpack it as a theme on a worker thread
/// and select it. `busy` is insensitive while the archive is unpacked.
fn connect_import(button: &gtk::Button, window: &gtk::ApplicationWindow, select_theme: &Rc<dyn Fn(String)>, note: &gtk::Label, busy: &[gtk::Widget]) {
    let window = window.clone();
    let select_theme = select_theme.clone();
    let note = note.clone();
    let busy = busy.to_vec();
    button.connect_clicked(move |_| {
        let pick = opusdm_ui::FilePick {
            title: "Import Theme",
            filter: Some(opusdm_ui::FileFilterSpec { name: "Theme archives", patterns: &THEME_ARCHIVE_PATTERNS, ..Default::default() }),
            ..Default::default()
        };
        let select_theme = select_theme.clone();
        let note = note.clone();
        let busy = busy.clone();
        opusdm_ui::pick_file(Some(window.upcast_ref()), &pick, move |path| {
            show_note(&note, &format!("Importing {}...", path.display()));
            let (note, select_theme) = (note.clone(), select_theme.clone());
            background::run_blocking(
                background::busy_widgets(busy.clone()),
                move || crate::importer::import_theme_archive(&path).map_err(|e| e.to_string()),
                move |result| match result.and_then(|r| r) {
                    Ok(slug) => {
                        show_note(&note, &format!("Imported theme \"{slug}\". Select it above and click Apply."));
                        select_theme(slug);
                    }
                    Err(e) => show_note(&note, &format!("Import failed: {e}")),
                },
            );
        });
    });
}

/// Export theme: save the theme selected in `icon_set` as a zip file, on a
/// worker thread. `busy` is insensitive while the archive is written.
fn connect_export(button: &gtk::Button, window: &gtk::ApplicationWindow, icon_set: &Rc<RefCell<Rc<ui::IconSetSection>>>, note: &gtk::Label, busy: &[gtk::Widget]) {
    let window = window.clone();
    let icon_set = icon_set.clone();
    let note = note.clone();
    let busy = busy.to_vec();
    button.connect_clicked(move |_| {
        let opusdm_ipc::IconSetId::Custom(slug) = icon_set.borrow().value();
        let dialog = gtk::FileDialog::builder().title("Export Theme").initial_name(format!("{slug}.zip")).build();
        let note = note.clone();
        let busy = busy.clone();
        dialog.save(Some(&window), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            let Some(mut path) = file.path() else { return };
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip")) {
                path.set_extension("zip");
            }
            show_note(&note, &format!("Exporting to {}...", path.display()));
            let (note, slug) = (note.clone(), slug.clone());
            let shown = path.display().to_string();
            background::run_blocking(
                background::busy_widgets(busy.clone()),
                move || exporter::export_theme(&slug, &path),
                move |result| match result.and_then(|r| r) {
                    Ok(()) => show_note(&note, &format!("Exported to {shown}.")),
                    Err(e) => show_note(&note, &format!("Export failed: {e}")),
                },
            );
        });
    });
}

/// Run the one-time look migration on `settings` (see [`migrate::migrate_look`])
/// and persist its result together with `desktop`. Returns the settings the
/// window starts from and, when something happened, a line for the note.
fn migrated_settings(settings: opusdm_ipc::Settings, desktop: &opusdm_ipc::DesktopSettings) -> (opusdm_ipc::Settings, Option<String>) {
    let migrated = match migrate::migrate_look(&settings) {
        Ok(Some(m)) => m,
        Ok(None) => return (settings, None),
        Err(e) => return (settings, Some(format!("Could not move your colours into a theme: {e}"))),
    };
    if let model::ApplyOutcome::Failed(e) = model::apply_full(&migrated.settings, desktop) {
        return (settings, Some(format!("Could not save the settings: {e}")));
    }
    let note = migrated.created.map(|slug| {
        format!("Your colours, fonts and pictures are now the theme \"{}\" ({slug}); edit them with Edit themes...", migrate::MIGRATION_LABEL)
    });
    (migrated.settings, note)
}

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
