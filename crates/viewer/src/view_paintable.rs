//! The paintable the picture area shows: a texture drawn at the size it has
//! on screen, smooth or with sharp pixels (nearest neighbour).
//!
//! Its intrinsic size is the size on screen, so `gtk::Picture` never gets a
//! natural size larger than the zoomed picture: with the texture's own size a
//! picture zoomed out below the window was given the whole window and stretched.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use gdk::subclass::prelude::PaintableImpl;
    use std::cell::{Cell, RefCell};

    /// State of the paintable: the texture, its size on screen and the filter.
    pub struct ViewPaintable {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub size: Cell<(i32, i32)>,
        pub filter: Cell<gsk::ScalingFilter>,
        /// Draw a checkerboard behind the texture (pictures with transparency).
        pub checkered: Cell<bool>,
    }

    impl Default for ViewPaintable {
        fn default() -> Self {
            ViewPaintable { texture: RefCell::new(None), size: Cell::new((0, 0)), filter: Cell::new(gsk::ScalingFilter::Trilinear), checkered: Cell::new(false) }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ViewPaintable {
        const NAME: &'static str = "OpusdmViewPaintable";
        type Type = super::ViewPaintable;
        type ParentType = glib::Object;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for ViewPaintable {}

    impl PaintableImpl for ViewPaintable {
        fn flags(&self) -> gdk::PaintableFlags {
            gdk::PaintableFlags::SIZE | gdk::PaintableFlags::CONTENTS
        }

        fn intrinsic_width(&self) -> i32 {
            self.size.get().0
        }

        fn intrinsic_height(&self) -> i32 {
            self.size.get().1
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some(texture) = self.texture.borrow().clone() else { return };
            let Some(snapshot) = snapshot.downcast_ref::<gtk::Snapshot>() else { return };
            let area = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            if self.checkered.get() { super::snapshot_checkers(snapshot, &area); }
            snapshot.append_scaled_texture(&texture, self.filter.get(), &area);
        }
    }
}

glib::wrapper! {
    /// Draws a texture at its size on screen with the chosen filter.
    pub struct ViewPaintable(ObjectSubclass<imp::ViewPaintable>) @implements gdk::Paintable;
}

/// Side of one square of the checkerboard, in pixels on screen.
const CHECK_SIZE: f32 = 8.0;
/// The two greys of the checkerboard.
const CHECK_LIGHT: gdk::RGBA = gdk::RGBA::new(0.80, 0.80, 0.80, 1.0);
const CHECK_DARK: gdk::RGBA = gdk::RGBA::new(0.55, 0.55, 0.55, 1.0);

/// Fill `area` with a checkerboard: one tile of 2x2 squares, repeated.
fn snapshot_checkers(snapshot: &gtk::Snapshot, area: &graphene::Rect) {
    let tile = CHECK_SIZE * 2.0;
    snapshot.push_repeat(area, Some(&graphene::Rect::new(area.x(), area.y(), tile, tile)));
    let (x, y) = (area.x(), area.y());
    snapshot.append_color(&CHECK_LIGHT, &graphene::Rect::new(x, y, tile, tile));
    snapshot.append_color(&CHECK_DARK, &graphene::Rect::new(x, y, CHECK_SIZE, CHECK_SIZE));
    snapshot.append_color(&CHECK_DARK, &graphene::Rect::new(x + CHECK_SIZE, y + CHECK_SIZE, CHECK_SIZE, CHECK_SIZE));
    snapshot.pop();
}

impl ViewPaintable {
    /// Wrap `texture`, `size` pixels on screen; `sharp` draws with the nearest
    /// neighbour filter, otherwise the picture is smoothed (trilinear, which
    /// also keeps small text readable when zoomed out). `checkered` draws a
    /// checkerboard behind it, to show the transparent parts.
    pub fn new(texture: &gdk::Texture, size: (i32, i32), sharp: bool, checkered: bool) -> ViewPaintable {
        let paintable: ViewPaintable = glib::Object::new();
        let imp = paintable.imp();
        *imp.texture.borrow_mut() = Some(texture.clone());
        imp.size.set((size.0.max(1), size.1.max(1)));
        imp.checkered.set(checkered);
        imp.filter.set(if sharp { gsk::ScalingFilter::Nearest } else { gsk::ScalingFilter::Trilinear });
        paintable
    }
}
