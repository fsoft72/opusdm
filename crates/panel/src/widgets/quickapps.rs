//! Quick apps: a strip of icon-only launchers next to the pager, backed by
//! `quickapps.json`. Left click launches, right click offers "Remove", and
//! dragging an icon onto another one reorders them. The application menu adds
//! entries through [`add_app`]. Every bar (one per monitor) owns a strip; they
//! are all refreshed after any change.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_apps::apps;
use opusdm_ipc::quickapps::{load_quickapps, save_quickapps};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// Icon size of one launcher, in pixels.
const ICON_SIZE: i32 = 20;

thread_local! {
    /// Every live strip, so a change made anywhere refreshes all of them.
    static STRIPS: RefCell<Vec<Weak<QuickApps>>> = const { RefCell::new(Vec::new()) };
}

/// One strip of launchers (the widget of a single bar).
pub struct QuickApps {
    /// Strip plus its trailing separator; hidden while the list is empty.
    root: gtk::Box,
    icons: gtk::Box,
}

impl QuickApps {
    /// Build a strip, fill it from `quickapps.json` and register it for
    /// refreshes.
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        icons.add_css_class("opusdm-panel-quickapps");
        root.append(&icons);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let strip = Rc::new(QuickApps { root, icons });
        STRIPS.with(|s| {
            let mut s = s.borrow_mut();
            s.retain(|w| w.strong_count() > 0);
            s.push(Rc::downgrade(&strip));
        });
        strip.refresh();
        strip
    }

    /// The GTK widget to pack into the panel.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Rebuild the launchers from `quickapps.json`; ids that are not
    /// installed are skipped, and the strip hides when nothing is left.
    fn refresh(&self) {
        while let Some(child) = self.icons.first_child() {
            self.icons.remove(&child);
        }
        let ids = load_quickapps().apps;
        for entry in apps::resolve(&ids) {
            self.icons.append(&launcher(&entry));
        }
        self.root.set_visible(self.icons.first_child().is_some());
    }
}

/// Refresh every registered strip.
fn refresh_all() {
    let strips: Vec<Rc<QuickApps>> = STRIPS.with(|s| s.borrow().iter().filter_map(Weak::upgrade).collect());
    for strip in strips {
        strip.refresh();
    }
}

/// Add the app `id` to the quick apps (a no-op when already there) and
/// refresh every strip.
pub fn add_app(id: &str) {
    let mut q = load_quickapps();
    if !q.add(id) {
        return;
    }
    save_and_refresh(&q);
}

/// Whether `id` is already a quick app.
pub fn contains(id: &str) -> bool {
    load_quickapps().contains(id)
}

/// Persist `q` and refresh every strip; a save error is logged.
fn save_and_refresh(q: &opusdm_ipc::QuickApps) {
    if let Err(e) = save_quickapps(q) {
        opusdm_ipc::err_log!("panel", "cannot write quickapps.json: {e}");
    }
    refresh_all();
}

/// One launcher button with its click, context menu and drag and drop wiring.
fn launcher(entry: &apps::AppEntry) -> gtk::Button {
    let image = match entry.info.icon() {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(ICON_SIZE);
    let btn = gtk::Button::new();
    btn.set_child(Some(&image));
    btn.add_css_class("flat");
    btn.set_tooltip_text(Some(entry.info.name().as_str()));

    let info = entry.info.clone();
    btn.connect_clicked(move |_| apps::launch(&info));

    attach_remove_menu(&btn, &entry.id);
    attach_reorder(&btn, &entry.id);
    btn
}

/// Right click on `btn` opens a popover with "Remove".
fn attach_remove_menu(btn: &gtk::Button, id: &str) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    let remove = gtk::Button::with_label("Remove");
    remove.add_css_class("flat");
    popover.set_child(Some(&remove));
    popover.set_parent(btn);
    {
        let popover = popover.clone();
        btn.connect_destroy(move |_| popover.unparent());
    }
    {
        let (popover, id) = (popover.clone(), id.to_string());
        remove.connect_clicked(move |_| {
            popover.popdown();
            let mut q = load_quickapps();
            q.remove(&id);
            // Deferred: this refresh destroys the button that owns the popover.
            glib::idle_add_local_once(move || save_and_refresh(&q));
        });
    }
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    click.connect_pressed(move |_, _, _, _| popover.popup());
    btn.add_controller(click);
}

/// Dragging `btn` carries its id; dropping an id on `btn` moves it there.
fn attach_reorder(btn: &gtk::Button, id: &str) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    let dragged = id.to_string();
    drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&dragged.to_value())));
    btn.add_controller(drag);

    let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    let target = id.to_string();
    drop.connect_drop(move |_, value, _, _| {
        let Ok(dragged) = value.get::<String>() else { return false };
        let mut q = load_quickapps();
        q.move_to(&dragged, &target);
        // Deferred: the refresh destroys the buttons the drag is running on.
        glib::idle_add_local_once(move || save_and_refresh(&q));
        true
    });
    btn.add_controller(drop);
}
