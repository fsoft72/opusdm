//! One launcher icon of the dock: a widget that draws an icon paintable at
//! whatever size it is given. The paintable is looked up once, at the biggest
//! size the zoom can reach, so the zoom animation never goes back to the icon
//! theme: every frame only changes the size the texture is drawn at.

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// Icon shown for an app without one of its own.
const FALLBACK_ICON: &str = "application-x-executable";

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// The paintable and the size it is drawn at now.
    #[derive(Default)]
    pub struct DockIcon {
        pub paintable: RefCell<Option<gdk::Paintable>>,
        pub size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DockIcon {
        const NAME: &'static str = "OpusdmDockIcon";
        type Type = super::DockIcon;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for DockIcon {}

    impl WidgetImpl for DockIcon {
        /// Square, of the current size, on both axes.
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.size.get();
            (size, size, -1, -1)
        }

        /// The paintable scaled to the allocation.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(paintable) = self.paintable.borrow().clone() else { return };
            let obj = self.obj();
            paintable.snapshot(snapshot, obj.width() as f64, obj.height() as f64);
        }
    }
}

glib::wrapper! {
    /// A dock launcher icon; see the module doc.
    pub struct DockIcon(ObjectSubclass<imp::DockIcon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl DockIcon {
    /// An icon showing `icon` (or the generic application icon), loaded at
    /// `max_size` pixels for the display scale `scale`, drawn at `size`.
    pub fn new(icon: Option<&gio::Icon>, size: i32, max_size: i32, scale: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().size.set(size);
        *widget.imp().paintable.borrow_mut() = lookup(icon, max_size, scale);
        widget
    }

    /// Draw the icon at `size` pixels from now on. No-op when unchanged.
    pub fn set_size(&self, size: i32) {
        if self.imp().size.get() == size { return; }
        self.imp().size.set(size);
        self.queue_resize();
    }

    /// The size the icon is drawn at now.
    pub fn size(&self) -> i32 {
        self.imp().size.get()
    }
}

/// The icon theme paintable of `icon` at `size`, or of the fallback icon.
/// `None` (logged) when there is no display to take the icon theme from.
fn lookup(icon: Option<&gio::Icon>, size: i32, scale: i32) -> Option<gdk::Paintable> {
    let Some(display) = gdk::Display::default() else {
        opusdm_ipc::err_log!("opusdm-dock", "no display: cannot load a dock icon");
        return None;
    };
    let theme = gtk::IconTheme::for_display(&display);
    let flags = gtk::IconLookupFlags::empty();
    let paintable = match icon {
        Some(icon) => theme.lookup_by_gicon(icon, size, scale, gtk::TextDirection::None, flags),
        None => theme.lookup_icon(FALLBACK_ICON, &[], size, scale, gtk::TextDirection::None, flags),
    };
    Some(paintable.upcast())
}
