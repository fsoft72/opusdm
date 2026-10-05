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
