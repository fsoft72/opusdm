//! The line numbers of a text view, drawn in its left gutter.
//!
//! The numbers are drawn from the view's own line positions, so they stay
//! next to their lines in a big file whose line heights are still estimated
//! (a second text view of numbers drifts away from the text there).

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// Pixels left and right of the numbers.
const PAD: i32 = 8;
/// Colour of the numbers, on both themes.
const NUMBER_COLOR: gdk::RGBA = gdk::RGBA::new(0.53, 0.53, 0.53, 1.0);

mod imp {
    use super::*;
    use std::cell::Cell;

    /// The text view whose lines are numbered and the digits of its last line.
    #[derive(Default)]
    pub struct LineNumbers {
        pub view: glib::WeakRef<gtk::TextView>,
        pub digits: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LineNumbers {
        const NAME: &'static str = "OpusdmLineNumbers";
        type Type = super::LineNumbers;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for LineNumbers {}

    impl WidgetImpl for LineNumbers {
        /// Wide enough for the biggest number; as tall as the view makes it.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Vertical { return (0, 0, -1, -1); }
            let digits = "0".repeat(self.digits.get().max(1));
            let (width, _) = self.obj().create_pango_layout(Some(&digits)).pixel_size();
            let size = width + PAD * 2;
            (size, size, -1, -1)
        }

        /// The number of every line on screen, right aligned, centred on its line.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(view) = self.view.upgrade() else { return };
            let obj = self.obj();
            let (width, height) = (obj.width(), obj.height());
            let layout = obj.create_pango_layout(None);
            let (_, top) = view.window_to_buffer_coords(gtk::TextWindowType::Left, 0, 0);
            let (mut line, _) = view.line_at_y(top);
            loop {
                let (y, line_height) = view.line_yrange(&line);
                let (_, window_y) = view.buffer_to_window_coords(gtk::TextWindowType::Left, 0, y);
                if window_y > height { break; }
                layout.set_text(&(line.line() + 1).to_string());
                let (text_width, text_height) = layout.pixel_size();
                snapshot.save();
                snapshot.translate(&graphene::Point::new((width - PAD - text_width) as f32, (window_y + (line_height - text_height) / 2) as f32));
                snapshot.append_layout(&layout, &NUMBER_COLOR);
                snapshot.restore();
                if !line.forward_line() { break; }
            }
        }
    }
}

glib::wrapper! {
    /// Draws the line numbers of a text view; put it in the view's left gutter.
    pub struct LineNumbers(ObjectSubclass<imp::LineNumbers>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LineNumbers {
    /// Numbers for `view`, set as its left gutter and redrawn when it scrolls.
    pub fn attach(view: &gtk::TextView) -> LineNumbers {
        let numbers: LineNumbers = glib::Object::new();
        numbers.add_css_class("monospace");
        numbers.imp().view.set(Some(view));
        view.set_gutter(gtk::TextWindowType::Left, Some(&numbers));
        let weak = numbers.downgrade();
        let redraw = move |_: &gtk::Adjustment| if let Some(n) = weak.upgrade() { n.queue_draw(); };
        if let Some(adj) = view.vadjustment() { adj.connect_value_changed(redraw); }
        numbers
    }

    /// The text now has `lines` lines: size the column for the biggest number.
    pub fn set_line_count(&self, lines: i32) {
        self.imp().digits.set(lines.max(1).to_string().len());
        self.queue_resize();
        self.queue_draw();
    }
}
