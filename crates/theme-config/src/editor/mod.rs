//! The Theme Editor: a window that edits the folder of an installed theme.
//! Widgets read and write a `ThemeDraft` plus a list of pending file
//! operations; nothing on disk changes until Save. The window and its
//! signal handlers share one `Rc<Editor>`. That is a reference cycle which is
//! never broken, so the editor state leaks after the window closes; it is
//! small and the editor is opened rarely, so this is accepted.

mod asset_list;
mod colours;
mod fonts;
mod general;
mod optional;
mod sounds;

use crate::draft;
use crate::model::{apply_full, apply_icon_set, load_settings, ApplyOutcome};
use crate::writer::{write_theme, AssetOp, ThemeDraft};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::IconSetId;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The tabs of one open theme.
struct Tabs {
    notebook: gtk::Notebook,
    general: general::GeneralTab,
    colours: colours::ColoursTab,
    fonts: fonts::FontsTab,
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
        let sounds = sounds::SoundsTab::new(window, slug, ops);
        notebook.append_page(&sounds.root, Some(&gtk::Label::new(Some("Sounds"))));
        Self { notebook, general, colours, fonts }
    }

    /// The draft the widgets describe now: `base` with each tab's changes
    /// written over it.
    fn collect(&self, base: &ThemeDraft) -> ThemeDraft {
        let mut draft = base.clone();
        self.general.collect_into(&mut draft);
        self.colours.collect_into(&mut draft);
        self.fonts.collect_into(&mut draft);
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

/// The editor window and its state.
struct Editor {
    window: gtk::ApplicationWindow,
    list: gtk::ListBox,
    slugs: RefCell<Vec<String>>,
    /// Holds the notebook of the open theme.
    right: gtk::Box,
    status: gtk::Label,
    current: RefCell<Option<Current>>,
    /// True while code, not the user, changes the list selection.
    programmatic: Cell<bool>,
    /// Set once the user agreed to discard unsaved changes and close.
    closing: Cell<bool>,
    on_saved: Rc<dyn Fn(String)>,
}

/// Open the Theme Editor over `parent`. `on_saved` is called with the slug of
/// every theme that was saved or created, so the caller can refresh its theme
/// picker.
pub fn open(parent: &gtk::ApplicationWindow, on_saved: Rc<dyn Fn(String)>) {
    let app = parent.application().expect("the settings window belongs to an application");
    let window = gtk::ApplicationWindow::builder()
        .application(&app)
        .transient_for(parent)
        .title("OpusDM Theme Editor")
        .default_width(980)
        .default_height(720)
        .build();

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let list_scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_width(200).build();
    let new_btn = gtk::Button::with_label("New...");
    let dup_btn = gtk::Button::with_label("Duplicate...");
    let del_btn = gtk::Button::with_label("Delete...");
    let list_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    list_buttons.append(&new_btn);
    list_buttons.append(&dup_btn);
    list_buttons.append(&del_btn);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    left.append(&list_scroll);
    left.append(&list_buttons);

    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.set_vexpand(true);
    right.set_hexpand(true);
    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
    let save_btn = gtk::Button::with_label("Save");
    let copy_btn = gtk::Button::with_label("Save as copy...");
    let revert_btn = gtk::Button::with_label("Revert");
    let apply_btn = gtk::Button::with_label("Save & Apply");
    let close_btn = gtk::Button::with_label("Close");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    for b in [&copy_btn, &revert_btn, &apply_btn, &save_btn, &close_btn] {
        buttons.append(b);
    }
    let right_outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    right_outer.append(&right);
    right_outer.append(&status);
    right_outer.append(&buttons);

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

    let editor = Rc::new(Editor {
        window: window.clone(),
        list,
        slugs: RefCell::new(Vec::new()),
        right,
        status,
        current: RefCell::new(None),
        programmatic: Cell::new(false),
        closing: Cell::new(false),
        on_saved,
    });

    {
        let editor = editor.clone();
        editor.list.clone().connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
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
    {
        let editor = editor.clone();
        save_btn.connect_clicked(move |_| editor.save(false));
    }
    {
        let editor = editor.clone();
        apply_btn.connect_clicked(move |_| editor.save(true));
    }
    {
        let editor = editor.clone();
        copy_btn.connect_clicked(move |_| editor.save_as_copy());
    }
    {
        let editor = editor.clone();
        revert_btn.connect_clicked(move |_| {
            if let Some(slug) = editor.current_slug() {
                if editor.reload(&slug) {
                    editor.status.set_text("Reverted to the saved theme.");
                }
            }
        });
    }
    {
        let window = window.clone();
        close_btn.connect_clicked(move |_| window.close());
    }
    {
        let editor = editor.clone();
        new_btn.connect_clicked(move |_| {
            let go = editor.clone();
            editor.confirm_discard(move || go.prompt_new(), || {});
        });
    }
    {
        let editor = editor.clone();
        dup_btn.connect_clicked(move |_| {
            let go = editor.clone();
            editor.confirm_discard(move || go.prompt_duplicate(), || {});
        });
    }
    {
        let editor = editor.clone();
        del_btn.connect_clicked(move |_| {
            let go = editor.clone();
            editor.confirm_discard(move || go.prompt_delete(), || {});
        });
    }
    {
        let editor = editor.clone();
        window.connect_close_request(move |_| {
            if editor.closing.get() || !editor.is_dirty() {
                return glib::Propagation::Proceed;
            }
            let again = editor.clone();
            opusdm_ui::requester(
                &editor.window,
                "Unsaved Changes",
                "This theme has unsaved changes. Close without saving?",
                None,
                &[("Close", true), ("Cancel", false)],
                move |close| {
                    if close {
                        again.closing.set(true);
                        again.window.close();
                    }
                },
            );
            glib::Propagation::Stop
        });
    }

    editor.refresh_list(None);
    let first = editor.slugs.borrow().first().cloned();
    if let Some(slug) = first {
        editor.reload(&slug);
    }
    window.present();
}

impl Editor {
    /// Slug of the open theme.
    fn current_slug(&self) -> Option<String> {
        self.current.borrow().as_ref().map(|c| c.slug.clone())
    }

    /// Rebuild the theme list and select `select` (the first row when `None`).
    fn refresh_list(&self, select: Option<&str>) {
        self.programmatic.set(true);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
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
        while let Some(child) = self.right.first_child() {
            self.right.remove(&child);
        }
        self.right.append(&tabs.notebook);
        *self.current.borrow_mut() = Some(Current { slug: slug.to_string(), saved: draft, ops, tabs });
        true
    }

    /// Close the open theme: no tabs, no current theme, so nothing can be saved.
    fn clear(&self) {
        while let Some(child) = self.right.first_child() {
            self.right.remove(&child);
        }
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
        if !self.is_dirty() {
            proceed();
            return;
        }
        opusdm_ui::requester(
            &self.window,
            "Unsaved Changes",
            "This theme has unsaved changes. Discard them?",
            None,
            &[("Discard", true), ("Cancel", false)],
            move |discard| {
                if discard {
                    proceed();
                } else {
                    cancel();
                }
            },
        );
    }

    /// The open theme's slug, the draft the widgets describe, and the pending
    /// operations. `None` when no theme is open.
    fn snapshot(&self) -> Option<(String, ThemeDraft, Vec<AssetOp>)> {
        let current = self.current.borrow();
        let c = current.as_ref()?;
        let ops = c.ops.borrow().clone();
        Some((c.slug.clone(), c.tabs.collect(&c.saved), ops))
    }

    /// Save the open theme in place; with `apply`, also apply it to the
    /// running desktop (the editor's preview).
    fn save(self: &Rc<Self>, apply: bool) {
        let Some((slug, draft, ops)) = self.snapshot() else { return };
        if let Err(e) = write_theme(&slug, &draft, Some(&slug), &ops) {
            self.status.set_text(&format!("Save failed: {e}"));
            return;
        }
        let loaded = self.reload(&slug);
        (self.on_saved)(slug.clone());
        let message = if apply { apply_theme(&slug) } else { "Saved.".to_string() };
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
                match write_theme(&slug, &copy, Some(&source), &ops) {
                    Ok(_) => {
                        editor.refresh_list(Some(&slug));
                        let loaded = editor.reload(&slug);
                        (editor.on_saved)(slug);
                        if loaded {
                            editor.status.set_text("Saved as a copy.");
                        }
                    }
                    Err(e) => editor.status.set_text(&format!("Save failed: {e}")),
                }
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
            match draft::duplicate_theme(&source, &label) {
                Ok(slug) => editor.opened(slug, "Duplicated."),
                Err(e) => editor.status.set_text(&format!("Cannot duplicate: {e}")),
            }
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
                        editor.refresh_list(None);
                        let first = editor.slugs.borrow().first().cloned();
                        let loaded = first.is_some_and(|first| editor.load(&first));
                        if !loaded {
                            editor.clear();
                        }
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

/// Apply theme `slug` over the user's current settings and desktop, the same
/// way the Settings window's Apply does. Returns the status message.
fn apply_theme(slug: &str) -> String {
    let current = load_settings();
    let desktop = opusdm_desktop_config::model::load();
    let result = apply_icon_set(&IconSetId::Custom(slug.to_string()), &current, &desktop);
    match apply_full(&result.settings, &result.desktop) {
        ApplyOutcome::SentToHub => "Saved and applied to the running desktop.".to_string(),
        ApplyOutcome::WroteFile => "Saved. No hub running: effective on next login.".to_string(),
        ApplyOutcome::Failed(e) => format!("Saved, but could not apply: {e}"),
    }
}
