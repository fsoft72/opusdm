//! The window switcher model: which windows it shows, in which order, and
//! how the selection moves over them.
//!
//! Pure code with no Smithay types (Openbox `NextWindow` / `PreviousWindow`
//! with the focus order of `State::mru`).

use crate::ipc::protocol::WindowView;
use crate::wm::WindowId;

/// The windows the switcher shows: the windows of `windows` (every mapped
/// toplevel, in stacking order, bottom first) on the desktop `current`, or
/// on every desktop with `all_desktops`; minimized windows included. They
/// come in focus order (`mru`, most recent first); the windows never
/// focused follow, topmost first. An id of `mru` with no window is skipped.
pub fn switcher_entries(mru: &[WindowId], windows: &[WindowView], all_desktops: bool, current: u32) -> Vec<WindowId> {
    let wanted = |v: &&WindowView| all_desktops || v.desktop == current;
    let mut entries: Vec<WindowId> = mru
        .iter()
        .copied()
        .filter(|id| windows.iter().filter(wanted).any(|v| v.id == id.0))
        .collect();
    for view in windows.iter().rev().filter(wanted) {
        let id = WindowId(view.id);
        if !entries.contains(&id) {
            entries.push(id);
        }
    }
    entries
}

/// The entry selected when the switcher opens on `len` entries: the second
/// one (the window focused before the active one), the last one when
/// `backwards`, the first when there is only one.
pub fn initial_selection(len: usize, backwards: bool) -> usize {
    if len < 2 { return 0; }
    if backwards { len - 1 } else { 1 }
}

/// The entry after `selected` among `len` entries (before it when
/// `forward` is false), wrapping; 0 when there is no entry.
pub fn step_selection(selected: usize, len: usize, forward: bool) -> usize {
    if len == 0 { return 0; }
    let selected = selected % len;
    if forward { (selected + 1) % len } else { (selected + len - 1) % len }
}

/// `entries` without the ids `keep` refuses, and the new index of the
/// selection: the same entry when it is kept, else the entry that followed
/// it (the first one when it was the last); 0 when nothing is left.
pub fn retain_entries(entries: &[WindowId], selected: usize, keep: impl Fn(WindowId) -> bool) -> (Vec<WindowId>, usize) {
    let kept_before = entries.iter().take(selected).filter(|id| keep(**id)).count();
    let kept: Vec<WindowId> = entries.iter().copied().filter(|id| keep(*id)).collect();
    // The selected entry kept or not, the entry now at `kept_before` is it or the one that followed it.
    let selected = if kept_before < kept.len() { kept_before } else { 0 };
    (kept, selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    /// A view of the window `id` on `desktop`.
    fn view(id: u32, desktop: u32, minimized: bool) -> WindowView {
        WindowView {
            id,
            app_id: String::new(),
            title: String::new(),
            pid: None,
            geometry: Rect::default(),
            desktop,
            minimized,
            maximized: false,
            fullscreen: false,
            activated: false,
            opacity: 100,
            urgent: false,
        }
    }

    /// The ids `ids` as `WindowId`s.
    fn ids(ids: &[u32]) -> Vec<WindowId> {
        ids.iter().map(|i| WindowId(*i)).collect()
    }

    #[test]
    fn entries_follow_the_focus_order() {
        let windows = [view(1, 0, false), view(2, 0, false), view(3, 0, false)];
        assert_eq!(switcher_entries(&ids(&[2, 3, 1]), &windows, false, 0), ids(&[2, 3, 1]));
    }

    #[test]
    fn windows_never_focused_follow_topmost_first_and_dead_ids_are_skipped() {
        let windows = [view(1, 0, false), view(2, 0, false), view(3, 0, false), view(4, 0, false)];
        assert_eq!(switcher_entries(&ids(&[9, 2]), &windows, false, 0), ids(&[2, 4, 3, 1]));
    }

    #[test]
    fn other_desktops_are_left_out_minimized_windows_are_kept() {
        let windows = [view(1, 0, false), view(2, 1, false), view(3, 0, true)];
        assert_eq!(switcher_entries(&ids(&[1, 2, 3]), &windows, false, 0), ids(&[1, 3]));
        assert_eq!(switcher_entries(&ids(&[1, 2, 3]), &windows, false, 1), ids(&[2]));
    }

    #[test]
    fn all_desktops_keeps_every_window() {
        let windows = [view(1, 0, false), view(2, 1, false), view(3, 2, true)];
        assert_eq!(switcher_entries(&ids(&[2, 1, 3]), &windows, true, 0), ids(&[2, 1, 3]));
    }

    #[test]
    fn the_switcher_opens_on_the_previous_window() {
        assert_eq!(initial_selection(3, false), 1);
        assert_eq!(initial_selection(3, true), 2);
        assert_eq!(initial_selection(1, false), 0);
        assert_eq!(initial_selection(1, true), 0);
        assert_eq!(initial_selection(0, false), 0);
    }

    #[test]
    fn the_selection_wraps_both_ways() {
        assert_eq!(step_selection(1, 3, true), 2);
        assert_eq!(step_selection(2, 3, true), 0);
        assert_eq!(step_selection(0, 3, false), 2);
        assert_eq!(step_selection(0, 0, true), 0);
    }

    #[test]
    fn a_removed_selected_entry_moves_the_selection_to_the_next_one() {
        let entries = ids(&[3, 2, 1]);
        assert_eq!(retain_entries(&entries, 1, |id| id != WindowId(2)), (ids(&[3, 1]), 1));
        assert_eq!(retain_entries(&entries, 2, |id| id != WindowId(3)), (ids(&[2, 1]), 1), "an earlier entry gone");
        assert_eq!(retain_entries(&entries, 2, |id| id != WindowId(1)), (ids(&[3, 2]), 0), "the last one wraps");
        assert_eq!(retain_entries(&entries, 0, |_| false), (Vec::new(), 0));
    }
}
