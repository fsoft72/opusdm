//! The picture area: a scrolled `gtk::Picture` with wheel zoom and drag pan;
//! over a PDF page the wheel scrolls and turns the pages, Ctrl+wheel zooms.

use crate::loader::RawImage;
use crate::page_wheel::{notch_pixels, wheel_move, WheelMove};
use crate::view_paintable::ViewPaintable;
use crate::zoom::{anchored_scroll, content_box, fit_scale, rotate_point, rotated_size, screen_size, unrotate_point, Mode, Zoom};
use gtk4 as gtk;
use gtk::{gdk, glib};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Where the scroll goes when another page replaces the one on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Landing {
    /// The scroll stays where it is.
    Keep,
    Top,
    Bottom,
}

/// Shows one picture at a time.
pub struct ImageView {
    pub root: gtk::ScrolledWindow,
    picture: gtk::Picture,
    raw: RefCell<Option<RawImage>>,
    zoom: Cell<Zoom>,
    /// Rotation of the texture now in the picture, to rebuild it only when it changes.
    shown_rotation: Cell<Option<u16>>,
    /// The texture now in the picture, kept to rewrap it when the filter changes.
    texture: RefCell<Option<gdk::Texture>>,
    /// Interpolated (smooth) scaling; off shows sharp pixels.
    smooth: Cell<bool>,
    /// Pointer position over the view, the point a zoom step keeps in place.
    pointer: Cell<Option<(f64, f64)>>,
    /// Called after the wheel changed the zoom, so the window can show it.
    on_wheel_zoom: RefCell<Option<Box<dyn Fn()>>>,
    /// The wheel scrolls and turns pages (a PDF) instead of zooming.
    page_scroll: Cell<bool>,
    /// Pixels the wheel pushed past an edge of the page, toward a page turn.
    overscroll: Cell<f64>,
    /// Called when the wheel goes past the bottom (`true`) or top (`false`) edge.
    on_page_edge: RefCell<Option<Box<dyn Fn(bool)>>>,
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
        let this = Rc::new(ImageView { root, picture, raw: RefCell::new(None), zoom: Cell::new(Zoom::default()), shown_rotation: Cell::new(None), texture: RefCell::new(None), smooth: Cell::new(true), pointer: Cell::new(None), on_wheel_zoom: RefCell::new(None), page_scroll: Cell::new(false), overscroll: Cell::new(0.0), on_page_edge: RefCell::new(None) });
        this._wire_pointer();
        this._wire_wheel();
        this._wire_drag();
        this
    }

    /// Remember where the pointer is over the view; zooming keeps that point in place.
    fn _wire_pointer(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_enter(move |_, x, y| this.pointer.set(Some((x, y))));
        let this = self.clone();
        motion.connect_motion(move |_, x, y| this.pointer.set(Some((x, y))));
        let this = self.clone();
        motion.connect_leave(move |_| this.pointer.set(None));
        self.root.add_controller(motion);
    }

    /// Wheel up zooms in, wheel down zooms out (a picture, or Ctrl+wheel); over
    /// a PDF page the wheel scrolls and turns the pages. The scrolled window
    /// never scrolls by itself.
    fn _wire_wheel(self: &Rc<Self>) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        scroll.connect_scroll(move |controller, _, dy| {
            let ctrl = controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            if this.page_scroll.get() && !ctrl {
                this._wheel_scroll(dy, controller.unit() == gdk::ScrollUnit::Surface);
                return glib::Propagation::Stop;
            }
            if dy < 0.0 { this.zoom_in(); } else if dy > 0.0 { this.zoom_out(); }
            if let Some(notify) = this.on_wheel_zoom.borrow().as_ref() { notify(); }
            glib::Propagation::Stop
        });
        self.root.add_controller(scroll);
    }

    /// Scroll the page by a wheel step of `dy` (notches, or pixels when
    /// `pixels`), or ask for a page turn past an edge.
    fn _wheel_scroll(&self, dy: f64, pixels: bool) {
        let vadj = self.root.vadjustment();
        let notch = notch_pixels(vadj.page_size().max(self.root.height() as f64));
        let delta = if pixels { dy } else { dy * notch };
        let mut overscroll = self.overscroll.get();
        let step = wheel_move(vadj.value(), vadj.upper(), vadj.page_size(), delta, &mut overscroll, notch);
        self.overscroll.set(overscroll);
        match step {
            WheelMove::Scroll(value) => vadj.set_value(value),
            WheelMove::Turn(forward) => {
                if let Some(turn) = self.on_page_edge.borrow().as_ref() { turn(forward); }
            }
            WheelMove::Hold => {}
        }
    }

    /// Run `notify` after every wheel zoom (the keys and buttons go through the window).
    pub fn connect_wheel_zoom(&self, notify: impl Fn() + 'static) {
        *self.on_wheel_zoom.borrow_mut() = Some(Box::new(notify));
    }

    /// Run `turn` when the wheel goes past the bottom (`true`) or the top
    /// (`false`) edge of a page.
    pub fn connect_page_edge(&self, turn: impl Fn(bool) + 'static) {
        *self.on_page_edge.borrow_mut() = Some(Box::new(turn));
    }

    /// `true` while a PDF is shown: the wheel scrolls and turns the pages, Ctrl+wheel zooms.
    pub fn set_page_scroll(&self, on: bool) {
        self.page_scroll.set(on);
        self.overscroll.set(0.0);
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

    /// Show another page of the same document (or the same page with other
    /// marks): zoom and rotation stay; `landing` says where the scroll goes.
    pub fn replace_image(&self, raw: RawImage, landing: Landing) {
        *self.raw.borrow_mut() = Some(raw);
        self.shown_rotation.set(None);
        self._apply();
        let vadj = self.root.vadjustment();
        match landing {
            Landing::Keep => {}
            Landing::Top => vadj.set_value(0.0),
            Landing::Bottom => {
                let Some(full) = self._full_size() else { return };
                let page = self.root.height() as f64;
                _scroll_to(&vadj, (full.1 - page).max(0.0), full.1.max(page));
                // The adjustment learns the new size only after the next layout.
                glib::idle_add_local_once(move || vadj.set_value(vadj.upper() - vadj.page_size()));
            }
        }
    }

    /// The picture widget, for the controllers of the PDF links.
    pub fn picture(&self) -> &gtk::Picture {
        &self.picture
    }

    /// The pixel of the unturned picture under the point `(x, y)` of the
    /// picture widget; `None` outside the picture.
    pub fn image_point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let size = self.size()?;
        let zoom = self.zoom.get();
        let shown = rotated_size(size.0, size.1, zoom.rotation);
        let widget = (self.picture.width() as f64, self.picture.height() as f64);
        let (scale, ox, oy) = content_box(shown, widget, zoom.mode);
        let p = ((x - ox) / scale, (y - oy) / scale);
        if p.0 < 0.0 || p.1 < 0.0 || p.0 > shown.0 as f64 || p.1 > shown.1 as f64 { return None; }
        Some(unrotate_point(p, size, zoom.rotation))
    }

    /// Scroll so that the box `(x0, y0, x1, y1)` (pixels of the unturned
    /// picture) is in the middle of the view; nothing to do when fitting.
    pub fn reveal(&self, rect: (f64, f64, f64, f64)) {
        let Some(size) = self.size() else { return };
        let zoom = self.zoom.get();
        let Mode::Scale(scale) = zoom.mode else { return };
        let (a, b) = (rotate_point((rect.0, rect.1), size, zoom.rotation), rotate_point((rect.2, rect.3), size, zoom.rotation));
        let centre = ((a.0 + b.0) / 2.0 * scale, (a.1 + b.1) / 2.0 * scale);
        let shown = rotated_size(size.0, size.1, zoom.rotation);
        let (hadj, vadj) = (self.root.hadjustment(), self.root.vadjustment());
        // The adjustments learn the new size only after the next layout.
        let full = (shown.0 as f64 * scale, shown.1 as f64 * scale);
        glib::idle_add_local_once(move || {
            _center_on(&hadj, centre.0, full.0);
            _center_on(&vadj, centre.1, full.1);
        });
    }

    /// Size of the picture as decoded, if one is shown.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.raw.borrow().as_ref().map(|r| (r.width, r.height))
    }

    /// One zoom step in, around the pointer (the middle of the view when it is outside).
    pub fn zoom_in(&self) {
        let mut z = self.zoom.get();
        z.zoom_in(self._fit());
        self._set_zoom_anchored(z);
    }

    /// One zoom step out, around the pointer (the middle of the view when it is outside).
    pub fn zoom_out(&self) {
        let mut z = self.zoom.get();
        z.zoom_out(self._fit());
        self._set_zoom_anchored(z);
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

    /// Switch between smooth and sharp pixels; the choice stays for the next pictures.
    pub fn toggle_smooth(&self) {
        self.smooth.set(!self.smooth.get());
        self._show_texture();
    }

    /// True while scaling is interpolated.
    pub fn is_smooth(&self) -> bool {
        self.smooth.get()
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

    /// Store a new zoom and scroll so the picture point under the pointer (or
    /// in the middle of the view) stays where it is.
    fn _set_zoom_anchored(&self, zoom: Zoom) {
        let Some(old_full) = self._full_size() else { return self._set_zoom(zoom) };
        let page = (self.root.width() as f64, self.root.height() as f64);
        let anchor = self.pointer.get().unwrap_or((page.0 / 2.0, page.1 / 2.0));
        let (hadj, vadj) = (self.root.hadjustment(), self.root.vadjustment());
        let value = (hadj.value(), vadj.value());
        self._set_zoom(zoom);
        let Some(new_full) = self._full_size() else { return };
        let target = (anchored_scroll(value.0, anchor.0, page.0, old_full.0, new_full.0), anchored_scroll(value.1, anchor.1, page.1, old_full.1, new_full.1));
        // The adjustments learn the new size only at the next layout: give it
        // to them now so the first frame is already in place, and again after
        // the layout in case it moved the value.
        _scroll_to(&hadj, target.0, new_full.0.max(page.0));
        _scroll_to(&vadj, target.1, new_full.1.max(page.1));
        glib::idle_add_local_once(move || {
            hadj.set_value(target.0);
            vadj.set_value(target.1);
        });
    }

    /// Size on screen of the turned picture at the current zoom, in pixels.
    fn _full_size(&self) -> Option<(f64, f64)> {
        let size = self.size()?;
        let zoom = self.zoom.get();
        let shown = rotated_size(size.0, size.1, zoom.rotation);
        let scale = zoom.scale(self._fit());
        Some((shown.0 as f64 * scale, shown.1 as f64 * scale))
    }

    /// Scale the fit mode has right now.
    fn _fit(&self) -> f64 {
        let Some(size) = self.size() else { return 1.0 };
        let view = (self.root.width().max(0) as u32, self.root.height().max(0) as u32);
        fit_scale(size, view, self.zoom.get().rotation)
    }

    /// Give the picture the current texture at its size on screen, smooth or
    /// with sharp pixels.
    fn _show_texture(&self) {
        let texture = self.texture.borrow();
        let Some(texture) = texture.as_ref() else { return };
        let Some(size) = self.size() else { return };
        let shown = screen_size(size, self.zoom.get());
        self.picture.set_paintable(Some(&ViewPaintable::new(texture, shown, !self.smooth.get())));
    }

    /// Put texture, size and scroll policy in line with the zoom state.
    fn _apply(&self) {
        let raw = self.raw.borrow();
        let Some(raw) = raw.as_ref() else { return };
        let zoom = self.zoom.get();
        if self.shown_rotation.get() != Some(zoom.rotation) {
            *self.texture.borrow_mut() = Some(raw.texture(zoom.rotation));
            self.shown_rotation.set(Some(zoom.rotation));
        }
        self._show_texture();
        match zoom.mode {
            Mode::Fit => {
                self.picture.set_content_fit(gtk::ContentFit::ScaleDown);
                self.picture.set_halign(gtk::Align::Fill);
                self.picture.set_valign(gtk::Align::Fill);
                self.picture.set_size_request(-1, -1);
                self.root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            }
            Mode::Scale(_) => {
                let (w, h) = screen_size((raw.width, raw.height), zoom);
                self.picture.set_content_fit(gtk::ContentFit::Fill);
                self.picture.set_halign(gtk::Align::Center);
                self.picture.set_valign(gtk::Align::Center);
                self.picture.set_size_request(w, h);
                self.root.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
            }
        }
    }
}

/// Scroll `adj` to `value` in a content now `upper` long.
fn _scroll_to(adj: &gtk::Adjustment, value: f64, upper: f64) {
    if adj.upper() < upper { adj.set_upper(upper); }
    adj.set_value(value);
}

/// Put `pos` (of a content `full` long, centred in the view when smaller) in
/// the middle of the view of `adj`.
fn _center_on(adj: &gtk::Adjustment, pos: f64, full: f64) {
    let page = adj.page_size();
    let offset = ((adj.upper() - full) / 2.0).max(0.0);
    let target = offset + pos - page / 2.0;
    adj.set_value(target.clamp(adj.lower(), (adj.upper() - page).max(adj.lower())));
}
