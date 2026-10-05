//! A paintable that scales its texture with sharp pixels (nearest neighbour)
//! instead of the smooth filter `gtk::Picture` uses for a plain texture.

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
    use std::cell::RefCell;

    /// State of the paintable: the texture to draw.
    #[derive(Default)]
    pub struct NearestPaintable {
        pub texture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NearestPaintable {
        const NAME: &'static str = "OpusdmNearestPaintable";
        type Type = super::NearestPaintable;
        type ParentType = glib::Object;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for NearestPaintable {}

    impl PaintableImpl for NearestPaintable {
        fn flags(&self) -> gdk::PaintableFlags {
            gdk::PaintableFlags::SIZE | gdk::PaintableFlags::CONTENTS
        }

        fn intrinsic_width(&self) -> i32 {
            self.texture.borrow().as_ref().map_or(0, |t| t.width())
        }

        fn intrinsic_height(&self) -> i32 {
            self.texture.borrow().as_ref().map_or(0, |t| t.height())
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some(texture) = self.texture.borrow().clone() else { return };
            let Some(snapshot) = snapshot.downcast_ref::<gtk::Snapshot>() else { return };
            snapshot.append_scaled_texture(&texture, gsk::ScalingFilter::Nearest, &graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
        }
    }
}

glib::wrapper! {
    /// Draws a texture with the nearest neighbour filter.
    pub struct NearestPaintable(ObjectSubclass<imp::NearestPaintable>) @implements gdk::Paintable;
}

impl NearestPaintable {
    /// Wrap `texture`.
    pub fn new(texture: &gdk::Texture) -> NearestPaintable {
        let paintable: NearestPaintable = glib::Object::new();
        *paintable.imp().texture.borrow_mut() = Some(texture.clone());
        paintable
    }
}
