//! Zoom and rotation state of a picture, with no widget in it.

/// Factor of one zoom step (key or wheel notch).
pub const ZOOM_STEP: f64 = 1.25;
/// Smallest scale: 5%.
pub const ZOOM_MIN: f64 = 0.05;
/// Largest scale: 1600%.
pub const ZOOM_MAX: f64 = 16.0;

/// Fit the window, or a fixed scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fit,
    Scale(f64),
}

/// The zoom mode and the rotation (0, 90, 180 or 270 degrees, clockwise).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub mode: Mode,
    pub rotation: u16,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { mode: Mode::Fit, rotation: 0 }
    }
}

/// Size of a `w` x `h` picture after turning it by `rotation` degrees.
pub fn rotated_size(w: u32, h: u32, rotation: u16) -> (u32, u32) {
    if rotation % 180 == 90 { (h, w) } else { (w, h) }
}

/// Scale that makes a picture fit a view: never above 100%, never below the
/// minimum. A view or picture with no size gives 1.0.
pub fn fit_scale(picture: (u32, u32), view: (u32, u32), rotation: u16) -> f64 {
    let (w, h) = rotated_size(picture.0, picture.1, rotation);
    if w == 0 || h == 0 || view.0 == 0 || view.1 == 0 { return 1.0; }
    let scale = (view.0 as f64 / w as f64).min(view.1 as f64 / h as f64);
    scale.clamp(ZOOM_MIN, 1.0)
}

/// Where a point of the unturned picture (`size` pixels) lands once the picture
/// is turned clockwise by `rotation` degrees.
pub fn rotate_point(p: (f64, f64), size: (u32, u32), rotation: u16) -> (f64, f64) {
    let (w, h) = (size.0 as f64, size.1 as f64);
    match rotation % 360 {
        90 => (h - p.1, p.0),
        180 => (w - p.0, h - p.1),
        270 => (p.1, w - p.0),
        _ => p,
    }
}

/// The opposite of [`rotate_point`]: from the turned picture back to the unturned one.
pub fn unrotate_point(p: (f64, f64), size: (u32, u32), rotation: u16) -> (f64, f64) {
    let (w, h) = (size.0 as f64, size.1 as f64);
    match rotation % 360 {
        90 => (p.1, h - p.0),
        180 => (w - p.0, h - p.1),
        270 => (w - p.1, p.0),
        _ => p,
    }
}

/// Scale and top-left offset of the content inside the picture widget: a
/// turned picture of `shown` pixels in a widget of `widget` pixels. Fit mode
/// scales down only and centres; a fixed scale fills the widget, which has
/// exactly the scaled size.
pub fn content_box(shown: (u32, u32), widget: (f64, f64), mode: Mode) -> (f64, f64, f64) {
    let (w, h) = (shown.0.max(1) as f64, shown.1.max(1) as f64);
    let scale = match mode {
        Mode::Fit => (widget.0 / w).min(widget.1 / h).min(1.0),
        Mode::Scale(_) => widget.0 / w,
    };
    (scale, (widget.0 - w * scale) / 2.0, (widget.1 - h * scale) / 2.0)
}

/// Size on screen of a `picture` (unturned pixels): the turned size when
/// fitting (the widget scales it down), the scaled size otherwise; at least 1 x 1.
pub fn screen_size(picture: (u32, u32), zoom: Zoom) -> (i32, i32) {
    let (w, h) = rotated_size(picture.0, picture.1, zoom.rotation);
    let s = match zoom.mode {
        Mode::Fit => 1.0,
        Mode::Scale(s) => s,
    };
    (((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1))
}

impl Zoom {
    /// The scale in use; `fit` is what the fit mode would use right now.
    pub fn scale(&self, fit: f64) -> f64 {
        match self.mode {
            Mode::Fit => fit,
            Mode::Scale(s) => s,
        }
    }

    /// One step bigger, starting from the fit scale when fitting.
    pub fn zoom_in(&mut self, fit: f64) {
        self.mode = Mode::Scale((self.scale(fit) * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX));
    }

    /// One step smaller, starting from the fit scale when fitting.
    pub fn zoom_out(&mut self, fit: f64) {
        self.mode = Mode::Scale((self.scale(fit) / ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX));
    }

    /// Back to fitting the window.
    pub fn fit(&mut self) {
        self.mode = Mode::Fit;
    }

    /// 100%: one picture pixel per screen pixel.
    pub fn actual(&mut self) {
        self.mode = Mode::Scale(1.0);
    }

    /// Turn 90 degrees clockwise.
    pub fn rotate_cw(&mut self) {
        self.rotation = (self.rotation + 90) % 360;
    }

    /// Turn 90 degrees counter-clockwise.
    pub fn rotate_ccw(&mut self) {
        self.rotation = (self.rotation + 270) % 360;
    }

    /// Text for the info bar: `Fit` or the percentage.
    pub fn label(&self, fit: f64) -> String {
        match self.mode {
            Mode::Fit => format!("Fit ({}%)", (fit * 100.0).round() as u32),
            Mode::Scale(s) => format!("{}%", (s * 100.0).round() as u32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_size_follows_zoom_and_rotation() {
        assert_eq!(screen_size((1224, 1584), Zoom::default()), (1224, 1584));
        assert_eq!(screen_size((1224, 1584), Zoom { mode: Mode::Scale(0.25), rotation: 0 }), (306, 396));
        assert_eq!(screen_size((1224, 1584), Zoom { mode: Mode::Scale(0.5), rotation: 90 }), (792, 612));
        assert_eq!(screen_size((10, 10), Zoom { mode: Mode::Scale(0.05), rotation: 0 }), (1, 1));
    }

    #[test]
    fn rotate_and_unrotate_points() {
        let size = (4, 2);
        // Clockwise: the top-left corner goes to the top-right one (as the pixbuf rotation).
        assert_eq!(rotate_point((0.0, 0.0), size, 90), (2.0, 0.0));
        assert_eq!(rotate_point((0.0, 0.0), size, 180), (4.0, 2.0));
        assert_eq!(rotate_point((0.0, 0.0), size, 270), (0.0, 4.0));
        for rotation in [0, 90, 180, 270] {
            let p = (1.0, 0.5);
            assert_eq!(unrotate_point(rotate_point(p, size, rotation), size, rotation), p, "{rotation}");
        }
    }

    #[test]
    fn content_box_centres_in_fit_and_fills_at_a_scale() {
        // 200 x 100 picture in a 400 x 400 widget: not enlarged, centred.
        assert_eq!(content_box((200, 100), (400.0, 400.0), Mode::Fit), (1.0, 100.0, 150.0));
        // Too big: scaled down to the width.
        assert_eq!(content_box((800, 100), (400.0, 400.0), Mode::Fit), (0.5, 0.0, 175.0));
        assert_eq!(content_box((200, 100), (400.0, 200.0), Mode::Scale(2.0)), (2.0, 0.0, 0.0));
    }

    #[test]
    fn steps_start_from_the_fit_scale() {
        let mut z = Zoom::default();
        z.zoom_in(0.5);
        assert_eq!(z.mode, Mode::Scale(0.625));
        z.zoom_out(0.5);
        assert_eq!(z.mode, Mode::Scale(0.5));
    }

    #[test]
    fn scale_is_clamped() {
        let mut z = Zoom { mode: Mode::Scale(ZOOM_MAX), rotation: 0 };
        z.zoom_in(1.0);
        assert_eq!(z.mode, Mode::Scale(ZOOM_MAX));
        let mut z = Zoom { mode: Mode::Scale(ZOOM_MIN), rotation: 0 };
        z.zoom_out(1.0);
        assert_eq!(z.mode, Mode::Scale(ZOOM_MIN));
    }

    #[test]
    fn fit_and_actual() {
        let mut z = Zoom::default();
        z.actual();
        assert_eq!(z.scale(0.3), 1.0);
        z.fit();
        assert_eq!(z.scale(0.3), 0.3);
    }

    #[test]
    fn rotation_cycles() {
        let mut z = Zoom::default();
        for expected in [90, 180, 270, 0] {
            z.rotate_cw();
            assert_eq!(z.rotation, expected);
        }
        z.rotate_ccw();
        assert_eq!(z.rotation, 270);
        z.rotate_ccw();
        assert_eq!(z.rotation, 180);
    }

    #[test]
    fn fit_scale_never_enlarges() {
        assert_eq!(fit_scale((400, 200), (800, 600), 0), 1.0);
        assert_eq!(fit_scale((1600, 1200), (800, 600), 0), 0.5);
        assert_eq!(fit_scale((4000, 100), (800, 600), 0), 0.2);
    }

    #[test]
    fn fit_scale_swaps_the_sides_when_rotated() {
        // 1000 wide, 500 tall in a 500 x 1000 view: only fits turned.
        assert_eq!(fit_scale((1000, 500), (500, 1000), 0), 0.5);
        assert_eq!(fit_scale((1000, 500), (500, 1000), 90), 1.0);
        assert_eq!(rotated_size(10, 20, 270), (20, 10));
        assert_eq!(rotated_size(10, 20, 180), (10, 20));
    }

    #[test]
    fn fit_scale_handles_empty_sizes_and_tiny_results() {
        assert_eq!(fit_scale((0, 10), (100, 100), 0), 1.0);
        assert_eq!(fit_scale((10, 10), (0, 100), 0), 1.0);
        assert_eq!(fit_scale((100000, 10), (100, 100), 0), ZOOM_MIN);
    }

    #[test]
    fn labels() {
        let mut z = Zoom::default();
        assert_eq!(z.label(0.5), "Fit (50%)");
        z.actual();
        assert_eq!(z.label(0.5), "100%");
    }
}
