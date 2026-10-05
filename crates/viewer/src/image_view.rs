//! The picture area: a scrolled `gtk::Picture` with wheel zoom and drag pan.

use crate::loader::RawImage;
use crate::zoom::{fit_scale, rotated_size, Mode, Zoom};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Shows one picture at a time.
pub struct ImageView {
    pub root: gtk::ScrolledWindow,
    picture: gtk::Picture,
    raw: RefCell<Option<RawImage>>,
    zoom: Cell<Zoom>,
    /// Rotation of the texture now in the picture, to rebuild it only when it changes.
    shown_rotation: Cell<Option<u16>>,
}

impl ImageView {
    /// Build the view; the wheel zooms and a drag pans.
    pub fn new() -> Rc<Self> {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::ScaleDown);
        let root = gtk::ScrolledWindow::new();
        root.set_child(Some(&picture));
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.add_css_class("viewer-stage");
        let this = Rc::new(ImageView { root, picture, raw: RefCell::new(None), zoom: Cell::new(Zoom::default()), shown_rotation: Cell::new(None) });
        this._wire_wheel();
        this._wire_drag();
        this
    }

    /// Wheel up zooms in, wheel down zooms out; the scrolled window never scrolls with it.
    fn _wire_wheel(self: &Rc<Self>) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        scroll.connect_scroll(move |_, _, dy| {
            if dy < 0.0 { this.zoom_in(); } else if dy > 0.0 { this.zoom_out(); }
            glib::Propagation::Stop
        });
        self.root.add_controller(scroll);
    }

    /// Dragging moves the scroll bars the other way, like grabbing the picture.
    fn _wire_drag(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        let (hadj, vadj) = (self.root.hadjustment(), self.root.vadjustment());
        let (s, h, v) = (start.clone(), hadj.clone(), vadj.clone());
        drag.connect_drag_begin(move |_, _, _| s.set((h.value(), v.value())));
        drag.connect_drag_update(move |_, dx, dy| {
            let (x, y) = start.get();
            hadj.set_value(x - dx);
            vadj.set_value(y - dy);
        });
        self.root.add_controller(drag);
    }

    /// Show a new picture: fit mode, no rotation.
    pub fn set_image(&self, raw: RawImage) {
        *self.raw.borrow_mut() = Some(raw);
        self.zoom.set(Zoom::default());
        self.shown_rotation.set(None);
        self._apply();
    }

    /// Size of the picture as decoded, if one is shown.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.raw.borrow().as_ref().map(|r| (r.width, r.height))
    }

    /// One zoom step in.
    pub fn zoom_in(&self) {
        let mut z = self.zoom.get();
        z.zoom_in(self._fit());
        self._set_zoom(z);
    }

    /// One zoom step out.
    pub fn zoom_out(&self) {
        let mut z = self.zoom.get();
        z.zoom_out(self._fit());
        self._set_zoom(z);
    }

    /// Fit the window.
    pub fn fit(&self) {
        let mut z = self.zoom.get();
        z.fit();
        self._set_zoom(z);
    }

    /// 100%.
    pub fn actual(&self) {
        let mut z = self.zoom.get();
        z.actual();
        self._set_zoom(z);
    }

    /// Turn 90 degrees clockwise.
    pub fn rotate_cw(&self) {
        let mut z = self.zoom.get();
        z.rotate_cw();
        self._set_zoom(z);
    }

    /// Turn 90 degrees counter-clockwise.
    pub fn rotate_ccw(&self) {
        let mut z = self.zoom.get();
        z.rotate_ccw();
        self._set_zoom(z);
    }

    /// Zoom text for the info bar.
    pub fn zoom_label(&self) -> String {
        self.zoom.get().label(self._fit())
    }

    /// Store a new zoom and redraw.
    fn _set_zoom(&self, zoom: Zoom) {
        self.zoom.set(zoom);
        self._apply();
    }

    /// Scale the fit mode has right now.
    fn _fit(&self) -> f64 {
        let Some(size) = self.size() else { return 1.0 };
        let view = (self.root.width().max(0) as u32, self.root.height().max(0) as u32);
        fit_scale(size, view, self.zoom.get().rotation)
    }

    /// Put texture, size and scroll policy in line with the zoom state.
    fn _apply(&self) {
        let raw = self.raw.borrow();
        let Some(raw) = raw.as_ref() else { return };
        let zoom = self.zoom.get();
        if self.shown_rotation.get() != Some(zoom.rotation) {
            self.picture.set_paintable(Some(&raw.texture(zoom.rotation)));
            self.shown_rotation.set(Some(zoom.rotation));
        }
        match zoom.mode {
            Mode::Fit => {
                self.picture.set_content_fit(gtk::ContentFit::ScaleDown);
                self.picture.set_halign(gtk::Align::Fill);
                self.picture.set_valign(gtk::Align::Fill);
                self.picture.set_size_request(-1, -1);
                self.root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            }
            Mode::Scale(s) => {
                let (w, h) = rotated_size(raw.width, raw.height, zoom.rotation);
                self.picture.set_content_fit(gtk::ContentFit::Fill);
                self.picture.set_halign(gtk::Align::Center);
                self.picture.set_valign(gtk::Align::Center);
                self.picture.set_size_request(((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1));
                self.root.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
            }
        }
    }
}
