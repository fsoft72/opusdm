//! The full-screen overlay: the frozen screen, dimmed, where the user drags the
//! rectangle to keep.

use crate::render::surface_from;
use crate::selection::{self, Area, Edges, Rect};
use gtk4 as gtk;
use gtk::cairo;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use image::RgbaImage;
use std::cell::Cell;
use std::rc::Rc;

/// Opacity of the black layer over the part of the screen that is not selected.
const DIM_ALPHA: f64 = 0.45;

/// Width of the selection border, in pixels.
const BORDER_WIDTH: f64 = 2.0;

/// Colour of the selection border (the OpusDM accent blue).
const BORDER_RGB: (f64, f64, f64) = (0.25, 0.55, 1.0);

/// Side of the square grips drawn on the selection sides, in pixels.
const HANDLE_SIZE: f64 = 8.0;

/// Font size of the "W x H" label near the selection.
const LABEL_FONT_SIZE: f64 = 14.0;

/// Gap between the selection and its size label.
const LABEL_GAP: f64 = 6.0;

/// Show the overlay on `monitor` with `screen` (the capture of that monitor).
/// When the user releases a valid selection, `on_selected` gets the cropped
/// image and the overlay closes; Esc closes it with no call. The selection stays
/// adjustable after the drag: grab any side to resize it, press Enter to keep it.
pub fn open<F>(app: &gtk::Application, monitor: &gdk::Monitor, screen: RgbaImage, on_selected: F)
where
    F: Fn(RgbaImage) + 'static,
{
    let window = gtk::ApplicationWindow::builder().application(app).decorated(false).title("OpusDM Screenshot").build();
    window.add_css_class("shot-overlay");
    let surface = match surface_from(&screen) {
        Ok(s) => s,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-shot", "{e}");
            app.quit();
            return;
        }
    };
    let screen = Rc::new(screen);
    let on_selected = Rc::new(on_selected);
    let selection: Rc<Cell<Option<Area>>> = Rc::new(Cell::new(None));

    let area = gtk::DrawingArea::new();
    area.set_cursor_from_name(Some("crosshair"));
    {
        let selection = selection.clone();
        let (iw, ih) = (screen.width() as f64, screen.height() as f64);
        area.set_draw_func(move |_, cr, w, h| {
            let _ = paint(cr, &surface, (iw, ih), (w as f64, h as f64), selection.get());
        });
    }

    // `Some((edges, start))` while a side of the selection is being dragged,
    // `None` while a new rectangle is being drawn.
    let grab: Rc<Cell<Option<(Edges, Area)>>> = Rc::new(Cell::new(None));
    let origin = Rc::new(Cell::new((0.0, 0.0)));

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    {
        let (origin, selection, grab, area) = (origin.clone(), selection.clone(), grab.clone(), area.clone());
        drag.connect_drag_begin(move |_, x, y| {
            let hit = selection.get().map(|s| (selection::hit_edges(s, (x, y), selection::GRAB), s));
            match hit {
                Some((edges, start)) if edges.any() => grab.set(Some((edges, start))),
                _ => {
                    grab.set(None);
                    origin.set((x, y));
                    selection.set(Some((x, y, x, y)));
                }
            }
            area.queue_draw();
        });
    }
    {
        let (origin, selection, grab, area) = (origin.clone(), selection.clone(), grab.clone(), area.clone());
        drag.connect_drag_update(move |_, dx, dy| {
            match grab.get() {
                Some((edges, start)) => selection.set(Some(selection::resize(start, edges, dx, dy))),
                None => {
                    let (x, y) = origin.get();
                    selection.set(Some((x, y, x + dx, y + dy)));
                }
            }
            area.queue_draw();
        });
    }
    {
        // A selection that is too small is a stray click: drop it.
        let (selection, area, screen) = (selection.clone(), area.clone(), screen.clone());
        drag.connect_drag_end(move |_, _, _| {
            if to_rect(&area, &screen, selection.get()).is_none() { selection.set(None); }
            area.queue_draw();
        });
    }
    area.add_controller(drag);

    // Show the resize cursor of the side under the pointer.
    let motion = gtk::EventControllerMotion::new();
    {
        let (selection, area) = (selection.clone(), area.clone());
        motion.connect_motion(move |_, x, y| {
            let edges = selection.get().map(|s| selection::hit_edges(s, (x, y), selection::GRAB)).unwrap_or_default();
            area.set_cursor_from_name(Some(edges.cursor()));
        });
    }
    area.add_controller(motion);

    let keys = gtk::EventControllerKey::new();
    {
        let (window, app, area, screen, selection) = (window.clone(), app.clone(), area.clone(), screen.clone(), selection.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => {
                    window.close();
                    app.quit();
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let Some(rect) = to_rect(&area, &screen, selection.get()) else { return glib::Propagation::Stop };
                    on_selected(crop(&screen, rect));
                    window.close();
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    window.set_child(Some(&area));
    window.fullscreen_on_monitor(monitor);
    window.present();
}

/// The selection in image pixels, or `None` when there is none or it is too small.
fn to_rect(area: &gtk::DrawingArea, screen: &RgbaImage, selection: Option<Area>) -> Option<Rect> {
    let (x0, y0, x1, y1) = selection?;
    let bounds = (screen.width() as i32, screen.height() as i32);
    let scale = scale_of(area, bounds);
    selection::from_points((x0 * scale.0, y0 * scale.1), (x1 * scale.0, y1 * scale.1), bounds)
}

/// The factor from widget coordinates to image pixels, per axis.
fn scale_of(area: &gtk::DrawingArea, image: (i32, i32)) -> (f64, f64) {
    let (w, h) = (area.width().max(1) as f64, area.height().max(1) as f64);
    (image.0 as f64 / w, image.1 as f64 / h)
}

/// The part of `screen` inside `rect`.
fn crop(screen: &RgbaImage, rect: Rect) -> RgbaImage {
    image::imageops::crop_imm(screen, rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32).to_image()
}

/// Draw the screen, the dim layer, and the selection (undimmed, with border and
/// size label).
fn paint(
    cr: &cairo::Context,
    surface: &cairo::ImageSurface,
    image: (f64, f64),
    widget: (f64, f64),
    selection: Option<Area>,
) -> Result<(), cairo::Error> {
    cr.save()?;
    cr.scale(widget.0 / image.0, widget.1 / image.1);
    cr.set_source_surface(surface, 0.0, 0.0)?;
    cr.paint()?;
    cr.restore()?;

    cr.set_source_rgba(0.0, 0.0, 0.0, DIM_ALPHA);
    let Some((x0, y0, x1, y1)) = selection else { return cr.paint() };
    let (x, y, w, h) = (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
    // The dim layer is the whole widget minus the selection (even-odd).
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, widget.0, widget.1);
    cr.rectangle(x, y, w, h);
    cr.fill()?;

    let (r, g, b) = BORDER_RGB;
    cr.set_source_rgb(r, g, b);
    cr.set_line_width(BORDER_WIDTH);
    cr.rectangle(x, y, w, h);
    cr.stroke()?;

    // A grip at the middle of each side, to show they can be dragged.
    for (hx, hy) in [(x, y + h / 2.0), (x + w, y + h / 2.0), (x + w / 2.0, y), (x + w / 2.0, y + h)] {
        cr.rectangle(hx - HANDLE_SIZE / 2.0, hy - HANDLE_SIZE / 2.0, HANDLE_SIZE, HANDLE_SIZE);
    }
    cr.fill()?;

    let label = format!("{} x {}  (Enter: keep, Esc: cancel)", (w * image.0 / widget.0).round(), (h * image.1 / widget.1).round());
    cr.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(LABEL_FONT_SIZE);
    let ly = if y > LABEL_FONT_SIZE + 2.0 * LABEL_GAP { y - LABEL_GAP } else { y + h + LABEL_FONT_SIZE + LABEL_GAP };
    cr.move_to(x, ly);
    cr.show_text(&label)
}
