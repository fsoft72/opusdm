//! The annotations drawn over a screenshot: model, styles, hit-testing and
//! undo/redo. No drawing and no GUI here, see `render.rs` for that.

/// A point in image pixels.
pub type Point = (f64, f64);

/// An RGB colour, each channel 0.0 to 1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

/// The palette offered by the tool bar: name and colour.
pub const PALETTE: [(&str, Color); 6] = [
    ("Red", Color { r: 0.90, g: 0.20, b: 0.20 }),
    ("Yellow", Color { r: 0.98, g: 0.80, b: 0.10 }),
    ("Green", Color { r: 0.20, g: 0.70, b: 0.30 }),
    ("Blue", Color { r: 0.20, g: 0.45, b: 0.90 }),
    ("Black", Color { r: 0.0, g: 0.0, b: 0.0 }),
    ("White", Color { r: 1.0, g: 1.0, b: 1.0 }),
];

/// Colour of a new annotation (the first of the palette, red).
pub const DEFAULT_COLOR: Color = PALETTE[0].1;

/// The three sizes of the tool bar. A line or arrow gets [`Size::line_width`],
/// a text gets [`Size::font_size`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}

/// Size of a new annotation.
pub const DEFAULT_SIZE: Size = Size::Medium;

impl Size {
    /// Stroke width of a line or arrow, in pixels.
    pub fn line_width(self) -> f64 {
        match self {
            Size::Small => 2.0,
            Size::Medium => 4.0,
            Size::Large => 8.0,
        }
    }

    /// Font size of a text, in pixels.
    pub fn font_size(self) -> f64 {
        match self {
            Size::Small => 16.0,
            Size::Medium => 24.0,
            Size::Large => 40.0,
        }
    }
}

/// Height of a text line as a multiple of the font size.
pub const LINE_HEIGHT_FACTOR: f64 = 1.25;

/// Average glyph width as a multiple of the font size, used to estimate the box
/// of a text for hit-testing.
const GLYPH_WIDTH_FACTOR: f64 = 0.6;

/// Snap step of a Shift-drag, in degrees.
const ANGLE_STEP_DEG: f64 = 15.0;

/// Distance in pixels from a line within which a click selects it, at least.
const HIT_SLOP: f64 = 6.0;

/// Arrow head length as a multiple of the stroke width, and its minimum.
const ARROW_HEAD_FACTOR: f64 = 4.0;
const ARROW_HEAD_MIN: f64 = 14.0;

/// Half-angle of the arrow head, in radians.
const ARROW_HEAD_HALF_ANGLE: f64 = 0.45;

/// What an annotation is.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line { from: Point, to: Point },
    Arrow { from: Point, to: Point },
    /// Text whose top left corner is `at`; lines are separated by `\n`.
    Text { at: Point, text: String },
}

/// A shape with its style.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub shape: Shape,
    pub color: Color,
    pub size: Size,
}

impl Annotation {
    /// A new annotation with the default colour and size.
    #[cfg(test)]
    pub fn new(shape: Shape) -> Self {
        Annotation { shape, color: DEFAULT_COLOR, size: DEFAULT_SIZE }
    }

    /// Whether `p` is on the annotation (near the stroke, or in the text box).
    pub fn hit(&self, p: Point) -> bool {
        match &self.shape {
            Shape::Line { from, to } | Shape::Arrow { from, to } => {
                distance_to_segment(p, *from, *to) <= (self.size.line_width() / 2.0).max(HIT_SLOP)
            }
            Shape::Text { at, text } => {
                let (w, h) = text_extent(text, self.size.font_size());
                p.0 >= at.0 && p.0 <= at.0 + w && p.1 >= at.1 && p.1 <= at.1 + h
            }
        }
    }

    /// The box around the annotation as (left, top, right, bottom), stroke
    /// width included.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        match &self.shape {
            Shape::Line { from, to } | Shape::Arrow { from, to } => {
                let pad = self.size.line_width() / 2.0;
                (from.0.min(to.0) - pad, from.1.min(to.1) - pad, from.0.max(to.0) + pad, from.1.max(to.1) + pad)
            }
            Shape::Text { at, text } => {
                let (w, h) = text_extent(text, self.size.font_size());
                (at.0, at.1, at.0 + w, at.1 + h)
            }
        }
    }

    /// Move the annotation by `(dx, dy)`.
    pub fn translate(&mut self, dx: f64, dy: f64) {
        match &mut self.shape {
            Shape::Line { from, to } | Shape::Arrow { from, to } => {
                *from = (from.0 + dx, from.1 + dy);
                *to = (to.0 + dx, to.1 + dy);
            }
            Shape::Text { at, .. } => *at = (at.0 + dx, at.1 + dy),
        }
    }
}

/// Estimated (width, height) of `text` at `font_size`, a box around its lines.
pub fn text_extent(text: &str, font_size: f64) -> (f64, f64) {
    let lines = text.split('\n');
    let count = lines.clone().count().max(1);
    let longest = lines.map(|l| l.chars().count()).max().unwrap_or(0).max(1);
    (longest as f64 * font_size * GLYPH_WIDTH_FACTOR, count as f64 * font_size * LINE_HEIGHT_FACTOR)
}

/// Distance from `p` to the segment `a`-`b`.
fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 { return (p.0 - a.0).hypot(p.1 - a.1); }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

/// `to` moved so the segment `from`-`to` has an angle that is a multiple of 15
/// degrees and the same length (the Shift-drag of the line and arrow tools).
pub fn snap_angle(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = dx.hypot(dy);
    if len == 0.0 { return to; }
    let step = ANGLE_STEP_DEG.to_radians();
    let angle = (dy.atan2(dx) / step).round() * step;
    (from.0 + len * angle.cos(), from.1 + len * angle.sin())
}

/// The three corners of the head of an arrow from `from` to `to` drawn with
/// `line_width`: the tip first, then the two wings. `None` for a zero-length
/// arrow.
pub fn arrow_head(from: Point, to: Point, line_width: f64) -> Option<[Point; 3]> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = dx.hypot(dy);
    if len == 0.0 { return None; }
    let head = (line_width * ARROW_HEAD_FACTOR).max(ARROW_HEAD_MIN).min(len);
    let back = dy.atan2(dx) + std::f64::consts::PI;
    let wing = |a: f64| (to.0 + head * a.cos(), to.1 + head * a.sin());
    Some([to, wing(back + ARROW_HEAD_HALF_ANGLE), wing(back - ARROW_HEAD_HALF_ANGLE)])
}

/// The list of annotations with an undo and a redo history. Every change goes
/// through [`Document::checkpoint`], which saves the list before it changes.
#[derive(Debug, Default)]
pub struct Document {
    items: Vec<Annotation>,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
}

impl Document {
    /// The annotations, bottom first.
    pub fn items(&self) -> &[Annotation] {
        &self.items
    }

    /// Save the current list as an undo step and drop the redo history. Call it
    /// right before a change (an add, a remove, the start of a drag).
    pub fn checkpoint(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
    }

    /// Add an annotation on top and return its index.
    pub fn add(&mut self, annotation: Annotation) -> usize {
        self.checkpoint();
        self.items.push(annotation);
        self.items.len() - 1
    }

    /// Remove the annotation at `index`; an invalid index does nothing.
    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() { return; }
        self.checkpoint();
        self.items.remove(index);
    }

    /// Change the annotation at `index` in place, with no undo step (the caller
    /// checkpointed when the change began, like a drag in progress).
    pub fn modify(&mut self, index: usize, change: impl FnOnce(&mut Annotation)) {
        if let Some(item) = self.items.get_mut(index) { change(item); }
    }

    /// The index of the top-most annotation under `p`.
    pub fn hit_test(&self, p: Point) -> Option<usize> {
        self.items.iter().rposition(|a| a.hit(p))
    }

    /// Undo the last change; false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.items, previous));
        true
    }

    /// Redo the last undone change; false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.items, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: Point, to: Point) -> Annotation {
        Annotation::new(Shape::Line { from, to })
    }

    fn text(at: Point, s: &str) -> Annotation {
        Annotation::new(Shape::Text { at, text: s.to_string() })
    }

    #[test]
    fn default_style_is_red_medium() {
        let a = line((0.0, 0.0), (10.0, 10.0));
        assert_eq!(a.color, PALETTE[0].1);
        assert_eq!(a.size, Size::Medium);
        assert_eq!(Size::Medium.line_width(), 4.0);
        assert_eq!(Size::Medium.font_size(), 24.0);
    }

    #[test]
    fn line_hit_is_near_the_stroke_only() {
        let a = line((0.0, 0.0), (100.0, 0.0));
        assert!(a.hit((50.0, 3.0)));
        assert!(!a.hit((50.0, 30.0)));
        assert!(!a.hit((150.0, 0.0)));
    }

    #[test]
    fn text_hit_is_inside_its_box() {
        let a = text((10.0, 10.0), "hello");
        assert!(a.hit((20.0, 20.0)));
        assert!(!a.hit((5.0, 5.0)));
        assert!(!a.hit((500.0, 20.0)));
    }

    #[test]
    fn multiline_text_is_taller() {
        let one = text_extent("ab", 20.0);
        let two = text_extent("ab\ncd", 20.0);
        assert_eq!(two.0, one.0);
        assert_eq!(two.1, one.1 * 2.0);
    }

    #[test]
    fn bounds_include_the_stroke() {
        let a = line((10.0, 20.0), (50.0, 5.0));
        assert_eq!(a.bounds(), (8.0, 3.0, 52.0, 22.0));
        let t = text((1.0, 2.0), "ab");
        let (l, top, r, b) = t.bounds();
        assert_eq!((l, top), (1.0, 2.0));
        assert!(r > l && b > top);
    }

    #[test]
    fn translate_moves_every_shape() {
        let mut l = Annotation::new(Shape::Arrow { from: (0.0, 0.0), to: (10.0, 0.0) });
        l.translate(5.0, 7.0);
        assert_eq!(l.shape, Shape::Arrow { from: (5.0, 7.0), to: (15.0, 7.0) });
        let mut t = text((1.0, 2.0), "x");
        t.translate(3.0, 3.0);
        assert_eq!(t.shape, Shape::Text { at: (4.0, 5.0), text: "x".into() });
    }

    #[test]
    fn snap_angle_locks_to_15_degrees() {
        let (x, y) = snap_angle((0.0, 0.0), (100.0, 3.0));
        assert!((x - 100.0_f64.hypot(3.0)).abs() < 1e-9 && y.abs() < 1e-9);
        let (x, y) = snap_angle((0.0, 0.0), (10.0, 10.0));
        assert!((x - y).abs() < 1e-9);
        assert_eq!(snap_angle((5.0, 5.0), (5.0, 5.0)), (5.0, 5.0));
    }

    #[test]
    fn arrow_head_wings_are_behind_the_tip() {
        let [tip, w1, w2] = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0).unwrap();
        assert_eq!(tip, (100.0, 0.0));
        assert!(w1.0 < 100.0 && w2.0 < 100.0);
        assert!((w1.1 + w2.1).abs() < 1e-9);
        assert_eq!(arrow_head((3.0, 3.0), (3.0, 3.0), 4.0), None);
    }

    #[test]
    fn arrow_head_never_longer_than_the_arrow() {
        let [tip, w1, _] = arrow_head((0.0, 0.0), (5.0, 0.0), 8.0).unwrap();
        assert!((tip.0 - w1.0).hypot(tip.1 - w1.1) <= 5.0 + 1e-9);
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut d = Document::default();
        d.add(line((0.0, 0.0), (1.0, 1.0)));
        d.add(line((0.0, 0.0), (2.0, 2.0)));
        assert_eq!(d.items().len(), 2);
        assert!(d.undo());
        assert_eq!(d.items().len(), 1);
        assert!(d.undo());
        assert!(!d.undo());
        assert!(d.redo());
        assert!(d.redo());
        assert!(!d.redo());
        assert_eq!(d.items().len(), 2);
    }

    #[test]
    fn a_new_change_drops_the_redo_history() {
        let mut d = Document::default();
        d.add(line((0.0, 0.0), (1.0, 1.0)));
        d.undo();
        d.add(line((0.0, 0.0), (3.0, 3.0)));
        assert!(!d.redo());
    }

    #[test]
    fn remove_is_undoable_and_ignores_bad_index() {
        let mut d = Document::default();
        let i = d.add(line((0.0, 0.0), (1.0, 1.0)));
        d.remove(9);
        assert_eq!(d.items().len(), 1);
        d.remove(i);
        assert!(d.items().is_empty());
        d.undo();
        assert_eq!(d.items().len(), 1);
    }

    #[test]
    fn modify_after_checkpoint_undoes_in_one_step() {
        let mut d = Document::default();
        let i = d.add(line((0.0, 0.0), (10.0, 0.0)));
        d.checkpoint();
        d.modify(i, |a| a.translate(1.0, 0.0));
        d.modify(i, |a| a.translate(1.0, 0.0));
        d.undo();
        assert_eq!(d.items()[i].shape, Shape::Line { from: (0.0, 0.0), to: (10.0, 0.0) });
    }

    #[test]
    fn hit_test_returns_the_top_most() {
        let mut d = Document::default();
        d.add(line((0.0, 0.0), (100.0, 0.0)));
        let top = d.add(line((0.0, 0.0), (100.0, 0.0)));
        assert_eq!(d.hit_test((50.0, 0.0)), Some(top));
        assert_eq!(d.hit_test((50.0, 80.0)), None);
    }
}
