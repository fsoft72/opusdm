//! What the compositor tells about its windows and workspaces, as plain data
//! (no Wayland objects), so it can be handed to other threads and tested.

/// Id of a toplevel, unique for the life of the process (never reused).
pub type ToplevelId = u32;

/// A window of any client, as `wlr-foreign-toplevel-management` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toplevel {
    pub id: ToplevelId,
    pub title: String,
    /// The Wayland app id, usually the desktop file name without `.desktop`.
    pub app_id: String,
    pub maximized: bool,
    pub minimized: bool,
    pub activated: bool,
    pub fullscreen: bool,
    /// Names of the outputs the window is on (`eDP-1`, `HDMI-A-1`...).
    pub outputs: Vec<String>,
}

/// A virtual desktop, as `ext-workspace` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub active: bool,
    /// The compositor asks not to show it in pagers.
    pub hidden: bool,
    /// Position in the compositor's workspace grid, when it has one.
    pub coordinates: Vec<u32>,
}

/// Everything known at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Every open window, oldest first.
    pub toplevels: Vec<Toplevel>,
    /// The workspaces to show, in pager order (hidden ones left out).
    pub workspaces: Vec<Workspace>,
}

impl Snapshot {
    /// The window that has the focus.
    pub fn active(&self) -> Option<ToplevelId> {
        self.toplevels.iter().find(|t| t.activated).map(|t| t.id)
    }

    /// Index of the active workspace in [`Snapshot::workspaces`].
    pub fn current_workspace(&self) -> Option<usize> {
        self.workspaces.iter().position(|w| w.active)
    }
}

/// Values of the `state` array of `zwlr_foreign_toplevel_handle_v1`.
const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// Set the state flags of `toplevel` from a raw `state` array (native endian
/// `u32` values; any value missing from it is off).
pub fn apply_states(toplevel: &mut Toplevel, raw: &[u8]) {
    let states: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();
    toplevel.maximized = states.contains(&STATE_MAXIMIZED);
    toplevel.minimized = states.contains(&STATE_MINIMIZED);
    toplevel.activated = states.contains(&STATE_ACTIVATED);
    toplevel.fullscreen = states.contains(&STATE_FULLSCREEN);
}

/// Decode an `ext-workspace` `coordinates` array (native endian `u32`s).
pub fn parse_coordinates(raw: &[u8]) -> Vec<u32> {
    raw.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// The workspaces to show, in pager order: hidden ones dropped, sorted by
/// their coordinates (rows then columns) when every one has them, otherwise
/// in the order the compositor announced them.
pub fn pager_order(workspaces: &[Workspace]) -> Vec<Workspace> {
    let mut shown: Vec<Workspace> = workspaces.iter().filter(|w| !w.hidden).cloned().collect();
    if shown.iter().all(|w| !w.coordinates.is_empty()) {
        shown.sort_by(|a, b| a.coordinates.iter().rev().cmp(b.coordinates.iter().rev()));
    }
    shown
}

/// The index `delta` steps from `current` among `count`, wrapping at both
/// ends. `None` when there is nothing to step through.
pub fn wrapped_index(current: usize, count: usize, delta: i32) -> Option<usize> {
    if count == 0 { return None; }
    let target = (current as i64 + i64::from(delta)).rem_euclid(count as i64);
    usize::try_from(target).ok()
}

/// The index `delta` steps from `current` among `count`. With `wrap` it goes
/// around at both ends (see [`wrapped_index`]); without it stops at the first
/// and the last one. `None` when there is nothing to step through.
pub fn stepped_index(current: usize, count: usize, delta: i32, wrap: bool) -> Option<usize> {
    if wrap { return wrapped_index(current, count, delta); }
    if count == 0 { return None; }
    let target = (current as i64 + i64::from(delta)).clamp(0, count as i64 - 1);
    usize::try_from(target).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn states_come_from_the_raw_array() {
        let mut t = Toplevel::default();
        apply_states(&mut t, &raw(&[STATE_ACTIVATED, STATE_FULLSCREEN]));
        assert!(t.activated && t.fullscreen && !t.minimized && !t.maximized);
        apply_states(&mut t, &raw(&[STATE_MINIMIZED]));
        assert!(t.minimized && !t.activated && !t.fullscreen);
        apply_states(&mut t, &[1, 2]);
        assert!(!t.minimized, "a short tail is ignored");
    }

    #[test]
    fn workspaces_sort_by_coordinates_and_hide_hidden_ones() {
        let ws = |name: &str, coordinates: &[u32], hidden| Workspace { name: name.into(), coordinates: coordinates.to_vec(), hidden, ..Default::default() };
        let list = [ws("c", &[2], false), ws("a", &[0], false), ws("x", &[1], true), ws("b", &[1], false)];
        let names: Vec<_> = pager_order(&list).into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
        let unsorted = [ws("2", &[], false), ws("1", &[0], false)];
        let names: Vec<_> = pager_order(&unsorted).into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["2", "1"], "announce order without coordinates on every one");
        assert_eq!(parse_coordinates(&raw(&[3, 4])), vec![3, 4]);
    }

    #[test]
    fn the_snapshot_knows_the_active_window_and_workspace() {
        let snapshot = Snapshot {
            toplevels: vec![Toplevel { id: 1, ..Default::default() }, Toplevel { id: 2, activated: true, ..Default::default() }],
            workspaces: vec![Workspace::default(), Workspace { active: true, ..Default::default() }],
        };
        assert_eq!(snapshot.active(), Some(2));
        assert_eq!(snapshot.current_workspace(), Some(1));
        assert_eq!(Snapshot::default().active(), None);
    }

    #[test]
    fn workspace_steps_stop_at_the_ends_without_wrap() {
        assert_eq!(stepped_index(3, 4, 1, false), Some(3));
        assert_eq!(stepped_index(0, 4, -1, false), Some(0));
        assert_eq!(stepped_index(1, 4, 1, false), Some(2));
        assert_eq!(stepped_index(3, 4, 1, true), Some(0));
        assert_eq!(stepped_index(0, 0, 1, false), None);
    }

    #[test]
    fn workspace_steps_wrap() {
        assert_eq!(wrapped_index(0, 4, -1), Some(3));
        assert_eq!(wrapped_index(3, 4, 1), Some(0));
        assert_eq!(wrapped_index(1, 4, 1), Some(2));
        assert_eq!(wrapped_index(0, 0, 1), None);
    }
}
