//! Text of a PDF page with the place of every character, for the search. The
//! page is interpreted with a device that draws nothing and only keeps the
//! glyphs that map to Unicode. Places are in page points with y going down
//! (`initial_transform(true)`), the space the page is rendered in.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{interpret_page, BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{Affine, BezPath, Point, Rect};

/// Glyph box in glyph space (1000 units per em): from the descender to the ascender.
const GLYPH_BOTTOM: f64 = -200.0;
const GLYPH_TOP: f64 = 800.0;
/// Advance used when the font does not give one.
const DEFAULT_ADVANCE: f64 = 500.0;
/// A gap wider than this share of the glyph height between two characters is a space.
const SPACE_GAP: f64 = 0.2;

/// One character and its box on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct TextChar {
    pub ch: char,
    pub rect: Rect,
}

/// The characters of one page, in drawing order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<TextChar>,
}

/// A run of characters picked with the pointer: their text and their boxes,
/// one per line, and the first and last character index.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub text: String,
    pub rects: Vec<Rect>,
    pub range: (usize, usize),
}

/// Device that collects the characters of the glyphs and ignores the rest.
struct TextCollector {
    chars: Vec<TextChar>,
}

impl<'a> Device<'a> for TextCollector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, glyph_run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in glyph_run.glyphs() {
            let Some(text) = glyph.as_unicode() else { continue };
            let advance = match &**glyph {
                Glyph::Outline(g) => g.advance_width().map(f64::from).filter(|a| *a > 0.0).unwrap_or(DEFAULT_ADVANCE),
                Glyph::Type3(_) => DEFAULT_ADVANCE,
            };
            let rect = _glyph_rect(props.transform * glyph.transform(), advance);
            let chars: Vec<char> = match text {
                BfString::Char(c) => vec![c],
                BfString::String(s) => s.chars().collect(),
            };
            _push_split(&mut self.chars, &chars, rect);
        }
    }
}

/// Box of a glyph of `advance` units placed by `transform`.
fn _glyph_rect(transform: Affine, advance: f64) -> Rect {
    transform.transform_rect_bbox(Rect::new(0.0, GLYPH_BOTTOM, advance, GLYPH_TOP))
}

/// Add the characters of one glyph (a ligature gives several), sharing its box
/// in equal slices from left to right.
fn _push_split(out: &mut Vec<TextChar>, chars: &[char], rect: Rect) {
    let n = chars.len().max(1) as f64;
    let step = rect.width() / n;
    for (i, ch) in chars.iter().enumerate() {
        let x0 = rect.x0 + step * i as f64;
        out.push(TextChar { ch: *ch, rect: Rect::new(x0, rect.y0, x0 + step, rect.y1) });
    }
}

/// Collect the characters of page `index` (0-based); an unknown page has no text.
pub fn extract(pdf: &Pdf, index: usize) -> PageText {
    let Some(page) = pdf.pages().get(index) else { return PageText::default() };
    let (width, height) = page.render_dimensions();
    let transform = page.initial_transform(true).to_kurbo();
    let cache = InterpreterCache::new();
    let bbox = Rect::new(0.0, 0.0, width as f64, height as f64);
    let mut context = Context::new(transform, bbox, &cache, page.xref(), InterpreterSettings::default());
    let mut device = TextCollector { chars: Vec::new() };
    interpret_page(page, &mut context, &mut device);
    PageText { chars: device.chars }
}

impl PageText {
    /// The page as one string: a space where two characters are far apart, a
    /// new line where the next one starts a new line. Each char of the string
    /// carries the index of its character, or `None` for the added ones.
    fn _layout(&self) -> Vec<(char, Option<usize>)> {
        self._layout_range(0, self.chars.len())
    }

    /// [`Self::_layout`] of the characters `from..to` only; the indexes it
    /// carries are those of the whole page.
    fn _layout_range(&self, from: usize, to: usize) -> Vec<(char, Option<usize>)> {
        let mut out: Vec<(char, Option<usize>)> = Vec::with_capacity((to - from) * 2);
        let mut prev: Option<&TextChar> = None;
        for (i, c) in self.chars.iter().enumerate().take(to).skip(from) {
            if let Some(p) = prev {
                let height = p.rect.height().max(c.rect.height()).max(0.1);
                let same_line = (c.rect.center().y - p.rect.center().y).abs() < height / 2.0;
                if !same_line {
                    out.push(('\n', None));
                } else if c.rect.x0 - p.rect.x1 > height * SPACE_GAP && !p.ch.is_whitespace() && !c.ch.is_whitespace() {
                    out.push((' ', None));
                }
            }
            out.push((c.ch, Some(i)));
            prev = Some(c);
        }
        out
    }

    /// Every match of `query` (case-insensitive, any run of white space matches
    /// any other), each as the boxes of its characters merged line by line.
    pub fn find(&self, query: &str) -> Vec<Vec<Rect>> {
        let needle = _normalize(query.chars());
        if needle.is_empty() { return Vec::new(); }
        let layout = self._layout();
        let (hay, owners) = _normalize_with_owners(&layout);
        let mut found = Vec::new();
        let mut start = 0;
        while start + needle.len() <= hay.len() {
            if hay[start..start + needle.len()] == needle[..] {
                let indexes: Vec<usize> = owners[start..start + needle.len()].iter().flatten().copied().collect();
                found.push(self._merge_boxes(&indexes));
                start += needle.len();
            } else {
                start += 1;
            }
        }
        found
    }

    /// Index of the character nearest to `point` (the one under it when there
    /// is one); `None` on a page without text.
    pub fn nearest(&self, point: Point) -> Option<usize> {
        let distance = |r: &Rect| {
            let dx = (r.x0 - point.x).max(0.0).max(point.x - r.x1);
            let dy = (r.y0 - point.y).max(0.0).max(point.y - r.y1);
            dx * dx + dy * dy
        };
        self.chars.iter().enumerate().min_by(|a, b| distance(&a.1.rect).total_cmp(&distance(&b.1.rect))).map(|(i, _)| i)
    }

    /// The characters from `a` to `b` (either order, both included) as text
    /// laid out like the search sees it, with their boxes; `None` when the
    /// page has no such character.
    pub fn select(&self, a: usize, b: usize) -> Option<Selection> {
        let (lo, hi) = (a.min(b), a.max(b));
        if hi >= self.chars.len() { return None; }
        let text: String = self._layout_range(lo, hi + 1).into_iter().map(|(c, _)| c).collect();
        let indexes: Vec<usize> = (lo..=hi).collect();
        Some(Selection { text, rects: self._merge_boxes(&indexes), range: (lo, hi) })
    }

    /// Boxes of the characters `indexes`, one box per line.
    fn _merge_boxes(&self, indexes: &[usize]) -> Vec<Rect> {
        let mut boxes: Vec<Rect> = Vec::new();
        for &i in indexes {
            let rect = self.chars[i].rect;
            match boxes.last_mut() {
                Some(last) if (rect.center().y - last.center().y).abs() < last.height().max(rect.height()) / 2.0 => *last = last.union(rect),
                _ => boxes.push(rect),
            }
        }
        boxes
    }
}

/// Lower case, white space runs folded into one space, no leading or trailing space.
fn _normalize(chars: impl Iterator<Item = char>) -> Vec<char> {
    let layout: Vec<(char, Option<usize>)> = chars.map(|c| (c, None)).collect();
    _normalize_with_owners(&layout).0
}

/// `_normalize` that keeps, for each char out, the character index it came from.
fn _normalize_with_owners(layout: &[(char, Option<usize>)]) -> (Vec<char>, Vec<Option<usize>>) {
    let (mut chars, mut owners) = (Vec::with_capacity(layout.len()), Vec::with_capacity(layout.len()));
    for &(c, owner) in layout {
        if c.is_whitespace() {
            if chars.last().is_none_or(|l: &char| *l == ' ') { continue; }
            chars.push(' ');
            owners.push(None);
            continue;
        }
        for lower in c.to_lowercase() {
            chars.push(lower);
            owners.push(owner);
        }
    }
    if chars.last() == Some(&' ') {
        chars.pop();
        owners.pop();
    }
    (chars, owners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::pdf_with_contents;

    /// A character `ch` with a 6 x 10 box whose left edge is `x` on the line at `y`.
    fn ch(ch: char, x: f64, y: f64) -> TextChar {
        TextChar { ch, rect: Rect::new(x, y, x + 6.0, y + 10.0) }
    }

    /// Characters of `text` side by side from `x`, on the line at `y`.
    fn word(text: &str, x: f64, y: f64) -> Vec<TextChar> {
        text.chars().enumerate().map(|(i, c)| ch(c, x + i as f64 * 6.0, y)).collect()
    }

    #[test]
    fn layout_adds_spaces_and_new_lines() {
        let mut chars = word("Hello", 0.0, 0.0);
        chars.extend(word("world", 40.0, 0.0));
        chars.extend(word("next", 0.0, 20.0));
        let text: String = PageText { chars }._layout().iter().map(|(c, _)| *c).collect();
        assert_eq!(text, "Hello world\nnext");
    }

    #[test]
    fn nearest_picks_the_character_under_or_closest_to_the_point() {
        let page = PageText { chars: word("abc", 0.0, 0.0) };
        assert_eq!(page.nearest(Point::new(7.0, 5.0)), Some(1));
        assert_eq!(page.nearest(Point::new(100.0, 5.0)), Some(2));
        assert_eq!(page.nearest(Point::new(-50.0, -50.0)), Some(0));
        assert_eq!(PageText::default().nearest(Point::new(0.0, 0.0)), None);
    }

    #[test]
    fn select_gives_the_text_between_two_characters_in_either_order() {
        let mut chars = word("Hello", 0.0, 0.0);
        chars.extend(word("world", 40.0, 0.0));
        chars.extend(word("next", 0.0, 20.0));
        let page = PageText { chars };
        let forward = page.select(3, 11).unwrap();
        assert_eq!(forward.text, "lo world\nne");
        assert_eq!(forward.range, (3, 11));
        assert_eq!(forward.rects.len(), 2);
        assert_eq!(page.select(11, 3), Some(forward));
        assert_eq!(page.select(2, 2).unwrap().text, "l");
        assert!(page.select(0, 99).is_none());
    }

    #[test]
    fn find_is_case_insensitive_and_merges_boxes() {
        let mut chars = word("Hello", 0.0, 0.0);
        chars.extend(word("World", 40.0, 0.0));
        let page = PageText { chars };
        let hits = page.find("o w");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], vec![Rect::new(24.0, 0.0, 46.0, 10.0)]);
        assert_eq!(page.find("WORLD").len(), 1);
        assert_eq!(page.find("l").len(), 3);
        assert!(page.find("   ").is_empty());
        assert!(page.find("xyz").is_empty());
    }

    #[test]
    fn a_match_across_lines_has_one_box_per_line() {
        let mut chars = word("end", 100.0, 0.0);
        chars.extend(word("start", 0.0, 20.0));
        let hits = PageText { chars }.find("end start");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].len(), 2);
    }

    #[test]
    fn a_ligature_is_split_into_its_characters() {
        let mut out = Vec::new();
        _push_split(&mut out, &['f', 'i'], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(out, vec![TextChar { ch: 'f', rect: Rect::new(0.0, 0.0, 5.0, 10.0) }, TextChar { ch: 'i', rect: Rect::new(5.0, 0.0, 10.0, 10.0) }]);
    }

    #[test]
    fn extracts_the_text_of_a_real_page_with_places() {
        let data = pdf_with_contents(&["BT /F1 20 Tf 10 50 Td (Hello World) Tj ET"]);
        let pdf = Pdf::new(data).unwrap();
        let page = extract(&pdf, 0);
        let text: String = page.chars.iter().map(|c| c.ch).collect();
        assert_eq!(text, "Hello World");
        // The baseline is at y = 50 from the bottom of a 100 point page: 50 from the top.
        let h = &page.chars[0].rect;
        assert!((h.x0 - 10.0).abs() < 0.5, "{h:?}");
        assert!(h.y0 < 50.0 && h.y1 > 50.0, "{h:?}");
        assert_eq!(page.find("world").len(), 1);
        assert!(extract(&pdf, 5).chars.is_empty());
    }
}
