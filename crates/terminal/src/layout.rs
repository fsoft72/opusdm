//! The split layout of a tab: a tree of `gtk::Paned` whose leaves are pane
//! boxes (terminal + scrollbar). The root of the tree is the only child of
//! the tab's root box, which is the notebook page.

use crate::shared::Shared;
use gtk4 as gtk;
use gtk::prelude::*;
use std::rc::Rc;

/// CSS class of the box that is a notebook page and holds a tab's layout.
pub const TAB_ROOT_CLASS: &str = "tab-root";

/// Where a widget sat inside its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    PanedStart,
    PanedEnd,
    /// The only child of a tab root box.
    TabRoot,
}

/// A direction to move the focus in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// An axis-aligned rectangle in tab coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Every terminal under `root`, in layout order (left/top first).
pub fn terminals_in(root: &gtk::Widget) -> Vec<vte4::Terminal> {
    let mut found = Vec::new();
    collect_terminals(root, &mut found);
    found
}

/// Depth-first walk of `widget` collecting terminals into `found`.
fn collect_terminals(widget: &gtk::Widget, found: &mut Vec<vte4::Terminal>) {
    if let Some(term) = widget.downcast_ref::<vte4::Terminal>() {
        found.push(term.clone());
        return;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        collect_terminals(&c, found);
        child = c.next_sibling();
    }
}

/// The tab root box holding `widget`, if any.
pub fn tab_root_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let mut current = widget.parent();
    while let Some(w) = current {
        if w.has_css_class(TAB_ROOT_CLASS) { return Some(w); }
        current = w.parent();
    }
    None
}

/// Take `child` out of `container`, returning where it was.
fn detach(container: &gtk::Widget, child: &gtk::Widget) -> Option<Slot> {
    if let Some(paned) = container.downcast_ref::<gtk::Paned>() {
        if paned.start_child().as_ref() == Some(child) {
            paned.set_start_child(None::<&gtk::Widget>);
            return Some(Slot::PanedStart);
        }
        if paned.end_child().as_ref() == Some(child) {
            paned.set_end_child(None::<&gtk::Widget>);
            return Some(Slot::PanedEnd);
        }
        return None;
    }
    let root = container.downcast_ref::<gtk::Box>()?;
    root.remove(child);
    Some(Slot::TabRoot)
}

/// Put `child` into `container` at `slot`.
fn attach(container: &gtk::Widget, slot: Slot, child: &gtk::Widget) {
    match (slot, container.downcast_ref::<gtk::Paned>(), container.downcast_ref::<gtk::Box>()) {
        (Slot::PanedStart, Some(paned), _) => paned.set_start_child(Some(child)),
        (Slot::PanedEnd, Some(paned), _) => paned.set_end_child(Some(child)),
        (Slot::TabRoot, _, Some(root)) => root.append(child),
        _ => opusdm_ipc::err_log!("opusdm-terminal", "layout: cannot attach a pane to {}", container.type_().name()),
    }
}

/// Split the pane of `term` in two. `orientation` is the one of the new
/// `gtk::Paned`: horizontal puts the panes side by side. The new terminal
/// starts in the directory of `term` and gets the focus.
pub fn split(shared: &Rc<Shared>, term: &vte4::Terminal, orientation: gtk::Orientation) {
    let Some(pane) = term.parent() else { return };
    let Some(container) = pane.parent() else { return };
    let size = if orientation == gtk::Orientation::Horizontal { pane.width() } else { pane.height() };
    let cwd = crate::pane::working_directory(term);
    let new_term = crate::pane::new_terminal(shared, cwd.as_deref(), None);
    let Some(new_pane) = new_term.parent() else { return };

    let Some(slot) = detach(&container, &pane) else { return };
    let paned = gtk::Paned::new(orientation);
    paned.set_wide_handle(true);
    paned.set_start_child(Some(&pane));
    paned.set_end_child(Some(&new_pane));
    attach(&container, slot, paned.upcast_ref());
    if size > 0 { paned.set_position(size / 2); }
    new_term.grab_focus();
}

/// Remove the pane of `term`. Its sibling takes the place of the split; the
/// last pane of a tab closes the tab (and the last tab the window).
pub fn close_pane(shared: &Rc<Shared>, term: &vte4::Terminal) {
    let Some(pane) = term.parent() else { return };
    let Some(container) = pane.parent() else { return };
    let Some(paned) = container.downcast_ref::<gtk::Paned>() else {
        return crate::window::close_tab(&container);
    };
    let sibling = if paned.start_child().as_ref() == Some(&pane) { paned.end_child() } else { paned.start_child() };
    let Some(sibling) = sibling else { return };
    paned.set_start_child(None::<&gtk::Widget>);
    paned.set_end_child(None::<&gtk::Widget>);
    let Some(grand) = paned.parent() else { return };
    let Some(slot) = detach(&grand, paned.upcast_ref()) else { return };
    attach(&grand, slot, &sibling);
    let next = shared.last_focused_in(&sibling).or_else(|| terminals_in(&sibling).into_iter().next());
    if let Some(next) = next { next.grab_focus(); }
}

/// Move the focus from `term` to the next (`step` 1) or previous (`step`
/// -1) terminal of its tab, wrapping around.
pub fn focus_cycle(term: &vte4::Terminal, step: isize) {
    let Some(root) = tab_root_of(term.upcast_ref()) else { return };
    let terms = terminals_in(&root);
    let Some(index) = terms.iter().position(|t| t == term) else { return };
    let next = (index as isize + step).rem_euclid(terms.len() as isize) as usize;
    terms[next].grab_focus();
}

/// Move the focus from `term` to the nearest terminal in `dir`.
pub fn focus_direction(term: &vte4::Terminal, dir: Direction) {
    let Some(root) = tab_root_of(term.upcast_ref()) else { return };
    let bounds = |t: &vte4::Terminal| {
        t.compute_bounds(&root).map(|b| Rect { x: b.x(), y: b.y(), width: b.width(), height: b.height() })
    };
    let Some(current) = bounds(term) else { return };
    let others: Vec<(vte4::Terminal, Rect)> = terminals_in(&root)
        .into_iter()
        .filter(|t| t != term)
        .filter_map(|t| bounds(&t).map(|b| (t, b)))
        .collect();
    let rects: Vec<Rect> = others.iter().map(|(_, r)| *r).collect();
    if let Some(i) = pick_neighbour(current, &rects, dir) { others[i].0.grab_focus(); }
}

/// The index of the rectangle in `others` nearest to `current` in `dir`:
/// it must lie past the matching edge of `current`; among those the one
/// whose edge is closest wins, ties going to the best overlap on the other
/// axis.
pub fn pick_neighbour(current: Rect, others: &[Rect], dir: Direction) -> Option<usize> {
    const EPSILON: f32 = 1.0;
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
    others
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let (gap, shared) = match dir {
                Direction::Left => (current.x - (r.x + r.width), overlap(current.y, current.y + current.height, r.y, r.y + r.height)),
                Direction::Right => (r.x - (current.x + current.width), overlap(current.y, current.y + current.height, r.y, r.y + r.height)),
                Direction::Up => (current.y - (r.y + r.height), overlap(current.x, current.x + current.width, r.x, r.x + r.width)),
                Direction::Down => (r.y - (current.y + current.height), overlap(current.x, current.x + current.width, r.x, r.x + r.width)),
            };
            (gap > -EPSILON && shared > 0.0).then_some((i, gap.max(0.0), shared))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
        .map(|(i, _, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    /// Layout: A on the left half, B top right, C bottom right.
    fn three_panes() -> (Rect, Rect, Rect) {
        (rect(0.0, 0.0, 100.0, 200.0), rect(105.0, 0.0, 100.0, 95.0), rect(105.0, 100.0, 100.0, 100.0))
    }

    #[test]
    fn right_of_a_tall_pane_picks_the_most_overlapping() {
        let (a, b, c) = three_panes();
        assert!(pick_neighbour(a, &[b, c], Direction::Right).is_some());
        assert_eq!(pick_neighbour(a, &[b, c], Direction::Left), None);
    }

    #[test]
    fn up_and_down_move_between_stacked_panes() {
        let (a, b, c) = three_panes();
        assert_eq!(pick_neighbour(c, &[a, b], Direction::Up), Some(1));
        assert_eq!(pick_neighbour(b, &[a, c], Direction::Down), Some(1));
        assert_eq!(pick_neighbour(b, &[a, c], Direction::Left), Some(0));
    }

    #[test]
    fn panes_without_overlap_are_ignored() {
        let (a, b, _) = three_panes();
        assert_eq!(pick_neighbour(b, &[a], Direction::Up), None);
    }
}
