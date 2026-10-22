//! Sharp letters when a PDF page is zoomed in: after the zoom or the scroll
//! settles, the part of the page on screen (and a margin) is rendered again at
//! the pixels of the zoom by the detail worker and drawn over the stretched
//! page. Search marks and the picked text are drawn on it too.

use super::State;
use crate::loader::Mark;
use crate::pdf_detail::{DetailAnswer, DetailRequest, DetailTile, MAX_TILE_SIDE};
use crate::zoom::unrotate_point;
use gtk4 as gtk;
use gtk::glib;
use std::rc::Rc;
use std::time::Duration;

/// The page needs detail only above this zoom: up to 100% its own picture is sharp.
const DETAIL_MIN_ZOOM: f64 = 1.0;
/// Quiet time after the last zoom or scroll before rendering.
const DETAIL_DELAY: Duration = Duration::from_millis(150);
/// Share of the view added on every side of the window, for small scrolls.
const MARGIN: f64 = 0.3;
/// Most pixels a window may have (a window of 4 bytes per pixel is kept twice).
const MAX_PIXELS: f64 = 6_000_000.0;

/// The window to render, in pixels of the turned picture on screen: the
/// `visible` part of a picture of `full` size plus a margin, kept inside the
/// picture and under `MAX_PIXELS` and `MAX_TILE_SIDE` (it shrinks around the
/// visible part first).
pub(super) fn tile_rect(visible: (f64, f64, f64, f64), full: (f64, f64)) -> (f64, f64, f64, f64) {
    let (w, h) = (visible.2 - visible.0, visible.3 - visible.1);
    let (mut tw, mut th) = (w * (1.0 + 2.0 * MARGIN), h * (1.0 + 2.0 * MARGIN));
    let shrink = (MAX_PIXELS / (tw * th)).sqrt().min(MAX_TILE_SIDE as f64 / tw).min(MAX_TILE_SIDE as f64 / th).min(1.0);
    tw *= shrink;
    th *= shrink;
    let (cx, cy) = ((visible.0 + visible.2) / 2.0, (visible.1 + visible.3) / 2.0);
    let (x0, y0) = ((cx - tw / 2.0).max(0.0), (cy - th / 2.0).max(0.0));
    (x0, y0, (x0 + tw).min(full.0), (y0 + th).min(full.1))
}

/// Whether `outer` holds all of `inner`.
fn _covers(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && outer.2 >= inner.2 && outer.3 >= inner.3
}

/// `rect` of the turned picture as a box of the unturned one (`full` is the
/// unturned size).
fn _unturn(rect: (f64, f64, f64, f64), full: (u32, u32), rotation: u16) -> (f64, f64, f64, f64) {
    let (a, b) = (unrotate_point((rect.0, rect.1), full, rotation), unrotate_point((rect.2, rect.3), full, rotation));
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

impl State {
    /// Ask for the detail once the zoom and the scroll have been still for a
    /// moment; a newer call restarts the wait.
    pub(super) fn _schedule_detail(self: &Rc<Self>) {
        if !self._is_pdf() { return; }
        if let Some(id) = self.pdf.detail_timer.borrow_mut().take() { id.remove(); }
        let s = self.clone();
        let id = glib::timeout_add_local_once(DETAIL_DELAY, move || {
            s.pdf.detail_timer.borrow_mut().take();
            s._request_detail();
        });
        *self.pdf.detail_timer.borrow_mut() = Some(id);
    }

    /// Ask the worker for the window around the part of the page on screen,
    /// unless the one already drawn holds it. Dropped (with its memory) when
    /// the page is not zoomed in.
    fn _request_detail(self: &Rc<Self>) {
        if !self._is_pdf() || self.stack.visible_child_name().as_deref() != Some("image") { return; }
        let view = self.image.view_state().filter(|v| v.zoom > DETAIL_MIN_ZOOM);
        let (Some(view), Some((path, _)), Some(picture)) = (view, self._current(), self.image.size()) else {
            self.pdf.detail.borrow_mut().take();
            return;
        };
        let unturned_full = ((picture.0 as f64 * view.zoom).round() as u32, (picture.1 as f64 * view.zoom).round() as u32);
        let (x0, y0, x1, y1) = _unturn(tile_rect(view.visible, view.full), unturned_full, view.rotation);
        let (x0, y0, x1, y1) = (x0.floor().max(0.0), y0.floor().max(0.0), x1.ceil().min(unturned_full.0 as f64), y1.ceil().min(unturned_full.1 as f64));
        if x1 <= x0 || y1 <= y0 { return; }
        let page = self.page.get();
        let seen = _unturn(view.visible, unturned_full, view.rotation);
        let have = self.pdf.detail.borrow().as_ref().is_some_and(|t| {
            let tile = (t.origin.0 as f64, t.origin.1 as f64, t.origin.0 as f64 + t.raw.width as f64, t.origin.1 as f64 + t.raw.height as f64);
            t.page == page && t.zoom == view.zoom && t.rotation == view.rotation && _covers(tile, seen)
        });
        if have { return; }
        let request = DetailRequest {
            generation: self.generation.get(),
            path,
            page,
            zoom: view.zoom,
            rotation: view.rotation,
            origin: (x0 as i32, y0 as i32),
            size: ((x1 - x0) as u32, (y1 - y0) as u32),
            scale: (view.zoom * self.pdf.scale.get() as f64) as f32,
        };
        if self.pdf.detail_tx.send(request).is_err() { opusdm_ipc::err_log!("viewer", "the PDF detail renderer stopped"); }
    }

    /// The detail worker answered.
    pub(super) fn _detail_done(self: &Rc<Self>, answer: DetailAnswer) {
        let (generation, result) = answer;
        match result {
            Ok(tile) => {
                // An answer for another page or file is not an error: the view moved on.
                if generation != self.generation.get() || tile.page != self.page.get() { return; }
                *self.pdf.detail.borrow_mut() = Some(tile);
                self._show_detail();
            }
            Err(e) => opusdm_ipc::err_log!("viewer", "PDF detail: {e}"),
        }
    }

    /// Draw the detail on the page on screen, with the marks of the search and
    /// of the picked text.
    pub(super) fn _show_detail(&self) {
        if self.stack.visible_child_name().as_deref() != Some("image") { return; }
        let detail = self.pdf.detail.borrow();
        let Some(tile): Option<&DetailTile> = detail.as_ref() else { return };
        let origin = (tile.origin.0 as f64, tile.origin.1 as f64);
        let marks: Vec<Mark> = self._marks(tile.scale as f64, origin);
        if marks.is_empty() {
            self.image.set_detail(&tile.raw, origin, tile.zoom);
        } else {
            self.image.set_detail(&tile.raw.highlighted(&marks), origin, tile.zoom);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_holds_the_view_and_a_margin_inside_the_picture() {
        let r = tile_rect((1000.0, 1000.0, 1800.0, 1600.0), (5000.0, 5000.0));
        assert!(_covers(r, (1000.0, 1000.0, 1800.0, 1600.0)));
        assert!(r.0 < 1000.0 && r.2 > 1800.0, "margin on both sides: {r:?}");
        let corner = tile_rect((0.0, 0.0, 800.0, 600.0), (900.0, 700.0));
        assert_eq!(corner, (0.0, 0.0, 900.0, 700.0));
    }

    #[test]
    fn a_huge_view_gives_a_window_under_the_limits_still_around_the_view() {
        let r = tile_rect((0.0, 0.0, 6000.0, 4000.0), (20000.0, 20000.0));
        let (w, h) = (r.2 - r.0, r.3 - r.1);
        assert!(w <= MAX_TILE_SIDE as f64 && h <= MAX_TILE_SIDE as f64 && w * h <= MAX_PIXELS + 1.0, "{w} x {h}");
        let r = tile_rect((9000.0, 9000.0, 9800.0, 9600.0), (20000.0, 20000.0));
        assert!(((r.0 + r.2) / 2.0 - 9400.0).abs() < 1.0, "centred on the view: {r:?}");
    }

    #[test]
    fn unturn_maps_a_box_back_to_the_unturned_picture() {
        // A 100 x 50 picture turned 90 degrees clockwise is 50 x 100.
        assert_eq!(_unturn((0.0, 0.0, 50.0, 20.0), (100, 50), 90), (0.0, 0.0, 20.0, 50.0));
        assert_eq!(_unturn((1.0, 2.0, 3.0, 4.0), (100, 50), 0), (1.0, 2.0, 3.0, 4.0));
    }
}
