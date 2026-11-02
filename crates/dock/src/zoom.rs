//! Pure geometry of the icon zoom (macOS style magnification): how big each
//! icon is for a pointer position, where every icon and the dock strip sit,
//! and how much room the dock window needs around its strip for the biggest
//! zoom. No GTK here, so it is unit-testable.
//!
//! Coordinates are along the dock ("axis", left to right or top to bottom)
//! and across it ("cross", from the screen edge inward). Every value is in
//! pixels of the dock window.
//!
//! The icon under the pointer reaches the full zoom, its neighbours less on a
//! cosine bell [`RADIUS_SLOTS`] slots wide on each side. The row grows around
//! the pointer: the point of the row under the pointer stays under it, so the
//! icon being aimed at does not slide away while it grows.

use crate::placement::{slot_size, BORDER, DOCK_PAD, ITEM_PAD};

/// Half width of the zoom bell, in icon slots.
pub const RADIUS_SLOTS: f64 = 3.0;

/// Space between the strip border and the first and last slot.
const INSET: i32 = DOCK_PAD + BORDER;

/// The zoom of one dock: icon count, icon size and the biggest scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub count: usize,
    pub icon_size: i32,
    /// Scale of the icon right under the pointer (1.6 = 160 %); 1.0 = off.
    pub max_scale: f64,
}

/// Where one icon is drawn: its start along the axis, its start across it
/// (from the window side nearest the screen edge) and its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconBox {
    pub axis: f64,
    pub cross: f64,
    pub size: f64,
}

/// The whole frame: every icon and the strip `(start, length)` along the axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub icons: Vec<IconBox>,
    pub strip_axis: f64,
    pub strip_length: f64,
}

/// Room the dock window needs around the strip for the biggest zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    /// Extra thickness on the side away from the screen edge.
    pub headroom: i32,
    /// Extra length on each end of the strip.
    pub side: i32,
}

/// The bell: 1 at distance 0, falling to 0 at distance 1 and beyond.
fn bell(distance: f64) -> f64 {
    if distance >= 1.0 { return 0.0; }
    (1.0 + (std::f64::consts::PI * distance).cos()) / 2.0
}

impl Zoom {
    /// Size of one slot at rest.
    fn slot(&self) -> f64 {
        slot_size(self.icon_size) as f64
    }

    /// Thickness of the strip (as at rest, without the zoom).
    pub fn strip_thickness(&self) -> i32 {
        slot_size(self.icon_size) + 2 * INSET
    }

    /// Length of the strip at rest; an empty dock keeps one slot.
    pub fn rest_length(&self) -> i32 {
        self.count.max(1) as i32 * slot_size(self.icon_size) + 2 * INSET
    }

    /// The scale each icon aims at with the pointer at `pointer` along the
    /// axis, measured from the start of the rest row (the first slot).
    /// `None` (pointer away) means every icon at rest.
    pub fn target_scales(&self, pointer: Option<f64>) -> Vec<f64> {
        let slot = self.slot();
        (0..self.count)
            .map(|i| match pointer {
                Some(p) => {
                    let center = (i as f64 + 0.5) * slot;
                    1.0 + (self.max_scale - 1.0) * bell((p - center).abs() / (RADIUS_SLOTS * slot))
                }
                None => 1.0,
            })
            .collect()
    }

    /// How far the row moves toward its start so that the point under
    /// `pointer` (rest row coordinates) stays under it with `scales`.
    fn shift(&self, scales: &[f64], pointer: f64) -> f64 {
        let slot = self.slot();
        let extra = |s: f64| (s - 1.0) * self.icon_size as f64;
        let total: f64 = scales.iter().copied().map(extra).sum();
        if pointer <= 0.0 { return 0.0; }
        if pointer >= scales.len() as f64 * slot { return total; }
        let index = (pointer / slot).floor() as usize;
        let frac = pointer / slot - index as f64;
        scales[..index].iter().copied().map(extra).sum::<f64>() + frac * extra(scales[index])
    }

    /// Lay out the icons at `scales` with the row anchored at `pointer`
    /// (rest row coordinates; `None` keeps the start of the row in place), in
    /// a window with `extent` of room around the strip.
    pub fn frame(&self, scales: &[f64], pointer: Option<f64>, extent: Extent) -> Frame {
        let slot = self.slot();
        let icon = self.icon_size as f64;
        let rest_start = (extent.side + INSET) as f64;
        let shift = pointer.map(|p| self.shift(scales, p)).unwrap_or(0.0);
        let mut axis = rest_start - shift;
        let strip_axis = axis - INSET as f64;
        let cross = (INSET + ITEM_PAD) as f64;
        let mut icons = Vec::with_capacity(scales.len());
        for &scale in scales {
            let size = icon * scale;
            icons.push(IconBox { axis: axis + ITEM_PAD as f64, cross, size });
            axis += size + 2.0 * ITEM_PAD as f64;
        }
        let row_length = if scales.is_empty() { slot } else { axis - (strip_axis + INSET as f64) };
        Frame { icons, strip_axis, strip_length: row_length + 2.0 * INSET as f64 }
    }

    /// Room around the strip the window needs so the biggest zoom fits:
    /// the headroom for the tallest icon, and on each end the most the row
    /// can grow (the whole growth, should it all go to one side).
    pub fn extent(&self) -> Extent {
        if self.max_scale <= 1.0 || self.count == 0 { return Extent::default(); }
        let headroom = ((self.max_scale - 1.0) * self.icon_size as f64).ceil() as i32;
        let slot = self.slot();
        let length = self.count as f64 * slot;
        let mut most = 0.0_f64;
        let mut p = 0.0;
        while p <= length {
            let grown: f64 = self.target_scales(Some(p)).iter().map(|s| (s - 1.0) * self.icon_size as f64).sum();
            most = most.max(grown);
            p += 1.0;
        }
        Extent { headroom, side: most.ceil() as i32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom(count: usize) -> Zoom {
        Zoom { count, icon_size: 48, max_scale: 2.0 }
    }

    #[test]
    fn the_icon_under_the_pointer_gets_the_full_zoom_and_far_ones_none() {
        let z = zoom(10);
        let slot = slot_size(48) as f64;
        let s = z.target_scales(Some(4.5 * slot));
        assert!((s[4] - 2.0).abs() < 1e-9, "{s:?}");
        assert!(s[3] > 1.0 && s[3] < 2.0 && (s[3] - s[5]).abs() < 1e-9, "symmetric bell: {s:?}");
        assert_eq!(s[0], 1.0);
        assert_eq!(s[9], 1.0);
        assert!(z.target_scales(None).iter().all(|&s| s == 1.0));
    }

    #[test]
    fn at_rest_the_frame_is_the_classic_row() {
        let z = zoom(3);
        let f = z.frame(&z.target_scales(None), None, Extent::default());
        assert_eq!(f.strip_axis, 0.0);
        assert_eq!(f.strip_length, z.rest_length() as f64);
        let slot = slot_size(48) as f64;
        for (i, icon) in f.icons.iter().enumerate() {
            assert_eq!(icon.axis, (DOCK_PAD + BORDER + ITEM_PAD) as f64 + i as f64 * slot);
            assert_eq!(icon.cross, (DOCK_PAD + BORDER + ITEM_PAD) as f64);
            assert_eq!(icon.size, 48.0);
        }
    }

    #[test]
    fn the_point_under_the_pointer_stays_under_it() {
        let z = zoom(8);
        let extent = z.extent();
        let slot = slot_size(48) as f64;
        let rest_start = (extent.side + DOCK_PAD + BORDER) as f64;
        for p in [0.3 * slot, 2.5 * slot, 3.9 * slot, 7.7 * slot] {
            let scales = z.target_scales(Some(p));
            let f = z.frame(&scales, Some(p), extent);
            let index = (p / slot).floor() as usize;
            let frac = p / slot - index as f64;
            let icon = f.icons[index];
            let slot_start = icon.axis - ITEM_PAD as f64;
            let under = slot_start + frac * (icon.size + 2.0 * ITEM_PAD as f64);
            assert!((under - (rest_start + p)).abs() < 1e-6, "p={p}: {under} vs {}", rest_start + p);
        }
    }

    #[test]
    fn every_zoomed_frame_fits_the_window_extent() {
        for count in [1, 2, 5, 12] {
            let z = zoom(count);
            let extent = z.extent();
            let window_length = (z.rest_length() + 2 * extent.side) as f64;
            let slot = slot_size(48) as f64;
            let mut p = -20.0;
            while p < count as f64 * slot + 20.0 {
                let f = z.frame(&z.target_scales(Some(p)), Some(p), extent);
                assert!(f.strip_axis >= -1e-6, "count={count} p={p}: {}", f.strip_axis);
                assert!(f.strip_axis + f.strip_length <= window_length + 1e-6, "count={count} p={p}");
                for icon in &f.icons {
                    assert!(icon.cross + icon.size <= (z.strip_thickness() + extent.headroom) as f64 + 1e-6);
                }
                p += 3.0;
            }
        }
    }

    #[test]
    fn no_zoom_needs_no_room() {
        assert_eq!(Zoom { count: 5, icon_size: 48, max_scale: 1.0 }.extent(), Extent::default());
        assert_eq!(Zoom { count: 0, icon_size: 48, max_scale: 2.0 }.extent(), Extent::default());
        assert_eq!(zoom(5).extent().headroom, 48);
    }

    #[test]
    fn the_strip_grows_with_the_icons() {
        let z = zoom(6);
        let slot = slot_size(48) as f64;
        let p = 3.0 * slot;
        let scales = z.target_scales(Some(p));
        let grown: f64 = scales.iter().map(|s| (s - 1.0) * 48.0).sum();
        let f = z.frame(&scales, Some(p), z.extent());
        assert!((f.strip_length - (z.rest_length() as f64 + grown)).abs() < 1e-6);
    }
}
