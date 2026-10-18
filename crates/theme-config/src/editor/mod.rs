//! The Theme Editor: a window that edits the folder of an installed theme.
//! Widgets read and write a `ThemeDraft` plus a list of pending file
//! operations; nothing on disk changes until Save. The signal handlers and
//! the preview timer hold the `Editor` weakly; the close handler holds the
//! only strong reference and drops it when the window closes, so the editor
//! state goes away with its window.
//!
//! While the editor is open, the theme it shows is previewed live on the
//! running desktop: a timer sends the look the widgets describe to the hub
//! whenever it changes, and closing the editor sends back the settings that
//! were in force when it opened (or the ones Save / Save & Apply made).

mod asset_list;
mod colours;
mod fonts;
mod general;
mod hub;
mod icons;
mod optional;
mod pictures;
mod sounds;

use crate::draft;
use crate::apply::resolve_draft;
use crate::model::{apply_full, load_settings, resolve_selected, send_preview, ApplyOutcome};
use crate::writer::{is_read_only, write_theme, AssetOp, ThemeDraft};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::{IconSetId, Settings};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How often the live preview checks the widgets for changes.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(250);

/// Shown instead of the tabs when the open theme cannot be changed.
const READ_ONLY_MESSAGE: &str = "The default theme cannot be changed.\nUse Duplicate or Save as copy to make an editable copy.";

/// The tabs of one open theme.
struct Tabs {
    notebook: gtk::Notebook,
    general: general::GeneralTab,
    colours: colours::ColoursTab,
    fonts: fonts::FontsTab,
    pictures: pictures::PicturesTab,
    hub: hub::HubTab,
}

impl Tabs {
    /// Build every tab for theme `slug` and `draft`; file changes are queued in `ops`.
    fn new(window: &gtk::ApplicationWindow, slug: &str, draft: &ThemeDraft, ops: &Rc<RefCell<Vec<AssetOp>>>) -> Self {
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.set_hexpand(true);
        let general = general::GeneralTab::new(draft);
        notebook.append_page(&general.root, Some(&gtk::Label::new(Some("General"))));
        let colours = colours::ColoursTab::new(window, draft);
        notebook.append_page(&colours.root, Some(&gtk::Label::new(Some("Colours"))));
        let fonts = fonts::FontsTab::new(window, slug, draft, ops);
        notebook.append_page(&fonts.root, Some(&gtk::Label::new(Some("Fonts"))));
        let pictures = pictures::PicturesTab::new(window, slug, draft, ops);
        notebook.append_page(&pictures.root, Some(&gtk::Label::new(Some("Pictures"))));
        let hub = hub::HubTab::new(window, draft);
        notebook.append_page(&hub.root, Some(&gtk::Label::new(Some("Hub"))));
        let icons = icons::IconsTab::new(window, slug, ops);
        notebook.append_page(&icons.root, Some(&gtk::Label::new(Some("Icons"))));
        let sounds = sounds::SoundsTab::new(window, slug, ops);
        notebook.append_page(&sounds.root, Some(&gtk::Label::new(Some("Sounds"))));
        Self { notebook, general, colours, fonts, pictures, hub }
    }

    /// The draft the widgets describe now: `base` with each tab's changes
    /// written over it.
    fn collect(&self, base: &ThemeDraft) -> ThemeDraft {
        let mut draft = base.clone();
        self.general.collect_into(&mut draft);
        self.colours.collect_into(&mut draft);
        self.fonts.collect_into(&mut draft);
        self.pictures.collect_into(&mut draft);
        self.hub.collect_into(&mut draft);
        draft
    }
}

/// The theme currently open.
struct Current {
    slug: String,
    saved: ThemeDraft,
    ops: Rc<RefCell<Vec<AssetOp>>>,
    tabs: Tabs,
}

/// State of the live preview on the running desktop.
struct Preview {
    /// The settings to go back to when the editor closes: those in force when
    /// it opened, updated by Save of the active theme and by Save & Apply.
    baseline: Settings,
    /// The settings last sent to the hub (the baseline before any send).
    sent: Settings,
    /// Set once the editor closed and the baseline was restored.
    ended: bool,
}

/// The editor window and its state.
struct Editor {
    window: gtk::ApplicationWindow,
    list: gtk::ListBox,
    slugs: RefCell<Vec<String>>,
    /// Holds the notebook of the open theme.
    right: gtk::Box,
    status: gtk::Label,
    /// Save, Revert, Save & Apply and Delete: off while a read-only theme is open.
    edit_buttons: Vec<gtk::Button>,
    current: RefCell<Option<Current>>,
    preview: RefCell<Preview>,
    /// True while code, not the user, changes the list selection.
    programmatic: Cell<bool>,
    on_saved: Rc<dyn Fn(String)>,
}

/// The buttons of the Theme Editor window.
struct Buttons {
    new: gtk::Button,
    duplicate: gtk::Button,
    delete: gtk::Button,
    save: gtk::Button,
    save_copy: gtk::Button,
    revert: gtk::Button,
    apply: gtk::Button,
    close: gtk::Button,
}

/// The widgets [`build_layout`] creates.
struct Layout {
    list: gtk::ListBox,
    right: gtk::Box,
    status: gtk::Label,
    buttons: Buttons,
}

/// Fill `window`: the theme list with New / Duplicate / Delete on the left,
/// the editing area, status line and action buttons on the right.
fn build_layout(window: &gtk::ApplicationWindow) -> Layout {
    let buttons = Buttons {
        new: gtk::Button::with_label("New..."),
        duplicate: gtk::Button::with_label("Duplicate..."),
        delete: gtk::Button::with_label("Delete..."),
        save: gtk::Button::with_label("Save"),
        save_copy: gtk::Button::with_label("Save as copy..."),
        revert: gtk::Button::with_label("Revert"),
        apply: gtk::Button::with_label("Save & Apply"),
        close: gtk::Button::with_label("Close"),
    };
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_width(200).build();
    let list_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for b in [&buttons.new, &buttons.duplicate, &buttons.delete] {
        list_buttons.append(b);
    }
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.append(&list_scroll);
    left.append(&list_buttons);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.set_vexpand(true);
    right.set_hexpand(true);
    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    action_row.set_halign(gtk::Align::End);
    for b in [&buttons.save_copy, &buttons.revert, &buttons.apply, &buttons.save, &buttons.close] {
        action_row.append(b);
    }
    let right_outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    right_outer.append(&right);
    right_outer.append(&status);
    right_outer.append(&action_row);

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    for side in [&left, &right_outer] {
        side.set_margin_top(12);
        side.set_margin_bottom(12);
    }
    left.set_margin_start(12);
    right_outer.set_margin_end(12);
    outer.append(&left);
    outer.append(&right_outer);
    window.set_child(Some(&outer));
    Layout { list, right, status, buttons }
}

/// Run `action` on `editor` when `btn` is clicked, while the editor lives.
fn on_click(editor: &Rc<Editor>, btn: &gtk::Button, action: impl Fn(&Rc<Editor>) + 'static) {
    let editor = Rc::downgrade(editor);
    btn.connect_clicked(move |_| if let Some(editor) = editor.upgrade() { action(&editor) });
}

/// Run `action` on `editor` when `btn` is clicked, once unsaved changes are
/// saved or discarded.
fn on_click_after_discard(editor: &Rc<Editor>, btn: &gtk::Button, action: fn(&Rc<Editor>)) {
    on_click(editor, btn, move |editor| {
        let go = editor.clone();
        editor.confirm_discard(move || action(&go), || {});
    });
}

/// Open the Theme Editor of `app`, over `parent` when there is one (it is
/// standalone otherwise). `on_saved` is called with the slug of
/// every theme that was saved or created, so the caller can refresh its theme
/// picker.
pub fn open(app: &gtk::Application, parent: Option<&gtk::ApplicationWindow>, on_saved: Rc<dyn Fn(String)>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("OpusDM Theme Editor")
        .default_width(980)
        .default_height(720)
        .build();
    window.set_transient_for(parent);
    let Layout { list, right, status, buttons } = build_layout(&window);

    let baseline = load_settings();
    let editor = Rc::new(Editor {
        window: window.clone(),
        list,
        slugs: RefCell::new(Vec::new()),
        right,
        status,
        edit_buttons: vec![buttons.save.clone(), buttons.revert.clone(), buttons.apply.clone(), buttons.delete.clone()],
        current: RefCell::new(None),
        preview: RefCell::new(Preview { sent: baseline.clone(), baseline, ended: false }),
        programmatic: Cell::new(false),
        on_saved,
    });

    wire_list_selection(&editor);
    on_click(&editor, &buttons.save, |e| e.save(false));
    on_click(&editor, &buttons.apply, |e| e.save(true));
    on_click(&editor, &buttons.save_copy, |e| e.save_as_copy());
    on_click(&editor, &buttons.revert, |e| {
        if let Some(slug) = e.current_slug() {
            if e.reload(&slug) {
                e.status.set_text("Reverted to the saved theme.");
            }
        }
    });
    on_click(&editor, &buttons.close, |e| e.window.close());
    on_click_after_discard(&editor, &buttons.new, |e| e.prompt_new());
    on_click_after_discard(&editor, &buttons.duplicate, |e| e.prompt_duplicate());
    on_click_after_discard(&editor, &buttons.delete, |e| e.prompt_delete());
    wire_close_guard(&editor);

    editor.open_active();
    {
        let editor = Rc::downgrade(&editor);
        glib::timeout_add_local(PREVIEW_INTERVAL, move || match editor.upgrade() {
            Some(editor) => editor.update_preview(),
            None => glib::ControlFlow::Break,
        });
    }
    window.present();
}

/// Selecting a theme in the list opens it, once unsaved changes of the open
/// one are saved or discarded; a refused discard puts the selection back.
fn wire_list_selection(editor: &Rc<Editor>) {
    let list = editor.list.clone();
    let editor = Rc::downgrade(editor);
    list.connect_row_selected(move |_, row| {
        let (Some(row), Some(editor)) = (row, editor.upgrade()) else { return };
        if editor.programmatic.get() {
            return;
        }
        let Some(slug) = editor.slugs.borrow().get(row.index() as usize).cloned() else { return };
        if editor.current_slug().as_deref() == Some(slug.as_str()) {
            return;
        }
        let go = editor.clone();
        let back = editor.clone();
        editor.confirm_discard(move || {
            go.reload(&slug);
        },
        move || back.refresh_list(back.current_slug().as_deref()));
    });
}

/// Closing the window with unsaved changes asks first; a close that goes
/// through ends the live preview and drops the editor, whose only strong
/// reference this guard holds.
fn wire_close_guard(editor: &Rc<Editor>) {
    let weak = Rc::downgrade(editor);
    let owner = RefCell::new(Some(editor.clone()));
    opusdm_ui::DirtyGuard::attach(
        &editor.window,
        "This theme has unsaved changes. Close without saving?",
        None,
        move || weak.upgrade().is_some_and(|e| e.is_dirty()),
        move || {
            let Some(editor) = owner.borrow_mut().take() else { return };
            editor.end_preview();
        },
    );
}

impl Editor {
    /// Slug of the open theme.
    fn current_slug(&self) -> Option<String> {
        self.current.borrow().as_ref().map(|c| c.slug.clone())
    }

    /// Slug of the theme active on the desktop (the preview's baseline).
    fn active_slug(&self) -> String {
        let IconSetId::Custom(slug) = &self.preview.borrow().baseline.theme.icon_set;
        slug.clone()
    }

    /// Select and open the active theme, or the first one when it is not
    /// installed. Leaves no theme open when none can be loaded.
    fn open_active(self: &Rc<Self>) {
        let active = self.active_slug();
        self.refresh_list(Some(&active));
        let selected = self.list.selected_row().and_then(|row| self.slugs.borrow().get(row.index() as usize).cloned());
        let loaded = selected.is_some_and(|slug| self.load(&slug));
        if !loaded {
            self.clear();
        }
    }

    /// Send the look of the open theme (the baseline when none is open) to
    /// the hub when it changed since the last send. Runs on a timer; stops
    /// once the editor closed.
    fn update_preview(&self) -> glib::ControlFlow {
        if self.preview.borrow().ended {
            return glib::ControlFlow::Break;
        }
        let target = {
            let preview = self.preview.borrow();
            match self.snapshot() {
                Some((slug, draft, _)) => resolve_draft(&slug, &draft, &preview.baseline),
                None => preview.baseline.clone(),
            }
        };
        if target != self.preview.borrow().sent {
            send_preview(&target);
            self.preview.borrow_mut().sent = target;
        }
        glib::ControlFlow::Continue
    }

    /// Stop the live preview and put back the baseline settings when the
    /// desktop shows something else.
    fn end_preview(&self) {
        let mut preview = self.preview.borrow_mut();
        if preview.ended {
            return;
        }
        preview.ended = true;
        if preview.sent != preview.baseline {
            send_preview(&preview.baseline);
        }
    }

    /// Rebuild the theme list and select `select` (the first row when `None`).
    fn refresh_list(&self, select: Option<&str>) {
        self.programmatic.set(true);
        opusdm_ui::clear_children(&self.list);
        let mut slugs = Vec::new();
        for theme in opusdm_ipc::themes::scan_themes() {
            let label = gtk::Label::builder().label(&theme.label).xalign(0.0).margin_start(6).margin_end(6).margin_top(4).margin_bottom(4).build();
            self.list.append(&label);
            slugs.push(theme.slug);
        }
        let index = select.and_then(|s| slugs.iter().position(|x| x == s)).unwrap_or(0);
        *self.slugs.borrow_mut() = slugs;
        if let Some(row) = self.list.row_at_index(index as i32) {
            self.list.select_row(Some(&row));
        }
        self.programmatic.set(false);
    }

    /// Like `load`, but when it fails re-sync the list highlight with the
    /// theme that is still open. Returns whether the theme was loaded.
    fn reload(self: &Rc<Self>, slug: &str) -> bool {
        let loaded = self.load(slug);
        if !loaded {
            self.refresh_list(self.current_slug().as_deref());
        }
        loaded
    }

    /// Open theme `slug`: load its draft from disk and rebuild the tabs.
    /// Returns false, with the error in the status line and the previous
    /// theme left open, when it cannot be loaded.
    fn load(self: &Rc<Self>, slug: &str) -> bool {
        let draft = match draft::load_draft(slug) {
            Ok(d) => d,
            Err(e) => {
                self.status.set_text(&format!("Cannot open \"{slug}\": {e}"));
                return false;
            }
        };
        let ops = Rc::new(RefCell::new(Vec::new()));
        let tabs = Tabs::new(&self.window, slug, &draft, &ops);
        opusdm_ui::clear_children(&self.right);
        let read_only = is_read_only(slug);
        if read_only {
            let message = gtk::Label::builder().label(READ_ONLY_MESSAGE).justify(gtk::Justification::Center).wrap(true).vexpand(true).build();
            self.right.append(&message);
        } else {
            self.right.append(&tabs.notebook);
        }
        for button in &self.edit_buttons {
            button.set_sensitive(!read_only);
        }
        *self.current.borrow_mut() = Some(Current { slug: slug.to_string(), saved: draft, ops, tabs });
        self.status.set_text("");
        true
    }

    /// Close the open theme: no tabs, no current theme, so nothing can be saved.
    fn clear(&self) {
        opusdm_ui::clear_children(&self.right);
        *self.current.borrow_mut() = None;
    }

    /// Whether the open theme has unsaved changes.
    fn is_dirty(&self) -> bool {
        let current = self.current.borrow();
        let Some(c) = current.as_ref() else { return false };
        let now = c.tabs.collect(&c.saved);
        let ops = c.ops.borrow();
        draft::is_dirty(&c.saved, &now, &ops)
    }

    /// Run `proceed` now when nothing is unsaved, otherwise after the user
    /// agrees to discard; `cancel` runs when they decline.
    fn confirm_discard(self: &Rc<Self>, proceed: impl Fn() + 'static, cancel: impl Fn() + 'static) {
        opusdm_ui::confirm_discard(&self.window, self.is_dirty(), "This theme has unsaved changes. Discard them?", proceed, cancel);
    }

    /// The open theme's slug, the draft the widgets describe, and the pending
    /// operations. `None` when no theme is open.
    fn snapshot(&self) -> Option<(String, ThemeDraft, Vec<AssetOp>)> {
        let current = self.current.borrow();
        let c = current.as_ref()?;
        let ops = c.ops.borrow().clone();
        Some((c.slug.clone(), c.tabs.collect(&c.saved), ops))
    }

    /// The widgets that wait while a theme is written: the whole window
    /// content, so nothing is edited between the snapshot and the reload.
    fn busy(&self) -> impl Fn(bool) + 'static {
        crate::background::busy_widgets(self.window.child().into_iter().collect())
    }

    /// Save the open theme in place, on a worker thread; with `apply`, also
    /// make it the active theme.
    fn save(self: &Rc<Self>, apply: bool) {
        let Some((slug, draft, ops)) = self.snapshot() else { return };
        self.status.set_text("Saving...");
        let weak = Rc::downgrade(self);
        let target = slug.clone();
        crate::background::run_blocking(
            self.busy(),
            move || write_theme(&target, &draft, Some(&target), &ops).map(|_| ()).map_err(|e| e.to_string()),
            move |result| {
                let Some(editor) = weak.upgrade() else {
                    opusdm_ipc::err_log!("opusdm-theme-config", "the editor was closed before the save finished: the theme '{slug}' is written but not applied");
                    return;
                };
                match result.and_then(|r| r) {
                    Ok(()) => editor.saved(slug, apply),
                    Err(e) => editor.status.set_text(&format!("Save failed: {e}")),
                }
            },
        );
    }

    /// Theme `slug` was written: reopen it and, with `apply`, make it the
    /// active theme. Either way the saved look becomes the preview's baseline
    /// when the theme is (now) the active one, so closing the editor keeps it.
    fn saved(self: &Rc<Self>, slug: String, apply: bool) {
        let loaded = self.reload(&slug);
        (self.on_saved)(slug.clone());
        let message = if apply {
            let (message, applied) = apply_theme(&slug, &self.preview.borrow().baseline);
            let mut preview = self.preview.borrow_mut();
            preview.sent = applied.clone();
            preview.baseline = applied;
            message
        } else {
            if slug == self.active_slug() {
                let mut preview = self.preview.borrow_mut();
                preview.baseline = resolve_selected(&IconSetId::Custom(slug.clone()), &preview.baseline);
            }
            "Saved.".to_string()
        };
        if loaded {
            self.status.set_text(&message);
        }
    }

    /// Save the widgets' state plus pending operations as a new theme and open it.
    fn save_as_copy(self: &Rc<Self>) {
        let Some((source, draft, ops)) = self.snapshot() else { return };
        let editor = self.clone();
        opusdm_ui::requester_entry(
            &self.window,
            "Save as Copy",
            &format!("{} copy", draft.label.trim()),
            &[("Save", true), ("Cancel", false)],
            move |save, label| {
                if !save {
                    return;
                }
                let Some(slug) = crate::builder::slugify(&label) else {
                    editor.status.set_text("The name needs at least one letter or digit.");
                    return;
                };
                if opusdm_ipc::themes::theme_dir(&slug).exists() {
                    editor.status.set_text(&format!("A theme named \"{slug}\" already exists."));
                    return;
                }
                let mut copy = draft.clone();
                copy.label = label.trim().to_string();
                editor.status.set_text("Saving...");
                let (source, ops, target) = (source.clone(), ops.clone(), slug.clone());
                let weak = Rc::downgrade(&editor);
                crate::background::run_blocking(
                    editor.busy(),
                    move || write_theme(&target, &copy, Some(&source), &ops).map(|_| ()).map_err(|e| e.to_string()),
                    move |result| {
                        let Some(editor) = weak.upgrade() else {
                            opusdm_ipc::err_log!("opusdm-theme-config", "the editor was closed before the copy finished: the theme '{slug}' is written but not opened");
                            return;
                        };
                        match result.and_then(|r| r) {
                            Ok(()) => editor.opened(slug, "Saved as a copy."),
                            Err(e) => editor.status.set_text(&format!("Save failed: {e}")),
                        }
                    },
                );
            },
        );
    }

    /// Ask for a name and create an empty theme.
    fn prompt_new(self: &Rc<Self>) {
        let editor = self.clone();
        opusdm_ui::requester_entry(&self.window, "New Theme", "", &[("Create", true), ("Cancel", false)], move |create, label| {
            if !create {
                return;
            }
            match draft::new_theme(&label) {
                Ok(slug) => editor.opened(slug, "Created."),
                Err(e) => editor.status.set_text(&format!("Cannot create: {e}")),
            }
        });
    }

    /// Ask for a name and duplicate the open theme as saved on disk.
    fn prompt_duplicate(self: &Rc<Self>) {
        let Some(source) = self.current_slug() else { return };
        let editor = self.clone();
        opusdm_ui::requester_entry(&self.window, "Duplicate Theme", &format!("{source} copy"), &[("Duplicate", true), ("Cancel", false)], move |go, label| {
            if !go {
                return;
            }
            editor.status.set_text("Duplicating...");
            let (source, weak) = (source.clone(), Rc::downgrade(&editor));
            crate::background::run_blocking(
                editor.busy(),
                move || draft::duplicate_theme(&source, &label).map_err(|e| e.to_string()),
                move |result| {
                    let Some(editor) = weak.upgrade() else {
                        opusdm_ipc::err_log!("opusdm-theme-config", "the editor was closed before the duplicate finished");
                        return;
                    };
                    match result.and_then(|r| r) {
                        Ok(slug) => editor.opened(slug, "Duplicated."),
                        Err(e) => editor.status.set_text(&format!("Cannot duplicate: {e}")),
                    }
                },
            );
        });
    }

    /// Ask for confirmation and delete the open theme.
    fn prompt_delete(self: &Rc<Self>) {
        let Some(slug) = self.current_slug() else { return };
        let editor = self.clone();
        opusdm_ui::requester(
            &self.window,
            "Delete Theme",
            &format!("Delete theme \"{slug}\" and all its files?"),
            None,
            &[("Delete", true), ("Cancel", false)],
            move |delete| {
                if !delete {
                    return;
                }
                match draft::delete_theme(&slug) {
                    Ok(()) => {
                        editor.open_active();
                        let loaded = editor.current_slug().is_some();
                        (editor.on_saved)(opusdm_ipc::theme::DEFAULT_THEME_SLUG.to_string());
                        if loaded {
                            editor.status.set_text("Deleted.");
                        }
                    }
                    Err(e) => editor.status.set_text(&format!("Cannot delete: {e}")),
                }
            },
        );
    }

    /// Select and open a theme that was just created or duplicated.
    fn opened(self: &Rc<Self>, slug: String, message: &str) {
        self.refresh_list(Some(&slug));
        let loaded = self.reload(&slug);
        (self.on_saved)(slug);
        if loaded {
            self.status.set_text(message);
        }
    }
}

/// Make theme `slug` the active one, keeping the user's preferences
/// `prefs`, the same way the Settings window's Apply does. Returns the status
/// message and the settings that were applied.
fn apply_theme(slug: &str, prefs: &Settings) -> (String, Settings) {
    let settings = resolve_selected(&IconSetId::Custom(slug.to_string()), prefs);
    let desktop = opusdm_desktop_config::model::load();
    let message = match apply_full(&settings, &desktop) {
        ApplyOutcome::SentToHub => "Saved and applied to the running desktop.".to_string(),
        ApplyOutcome::WroteFile => "Saved. No hub running: effective on next login.".to_string(),
        ApplyOutcome::Failed(e) => format!("Saved, but could not apply: {e}"),
    };
    (message, settings)
}
