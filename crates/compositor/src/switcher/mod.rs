//! The window switcher (Alt+Tab), drawn by the compositor.
//!
//! `model` holds the pure parts (entries in focus order, selection moves),
//! `render` the drawing. This module holds the `State` side: the focus
//! history (`State::mru`), open, keys, the release of the held modifiers
//! and the activation of the chosen window.
//!
//! Ownership: `State::switcher` is the only owner of the open
//! `WindowSwitcher`. It names its windows by `WindowId` only, never by a
//! handle, so a window that goes away is never kept alive by it: its entry
//! is dropped by `check_switcher` (called by `refresh` and before every key
//! the switcher takes) and an empty switcher closes. The chosen window is
//! looked up again when it is activated (an unknown id is logged).

pub mod model;
pub mod render;

use opusdm_ipc::bindings::Mods;
use smithay::{backend::renderer::element::memory::MemoryRenderBuffer, reexports::wayland_server::Resource};

use crate::draw::RenderCache;
use crate::hotkeys::mods_of;
use crate::menu::theme::MenuTheme;
use crate::state::{State, LOG_TAG};
use crate::wm::actions::Action;
use crate::wm::WindowId;
use model::{initial_selection, retain_entries, step_selection, switcher_entries};
use render::{render_switcher, SwitcherRow};

/// The image cache of the switcher: rows, selection and scale of the image.
pub type SwitcherImageCache = RenderCache<(Vec<SwitcherRow>, usize, i32), MemoryRenderBuffer>;

/// A key the open switcher understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitcherKey {
    /// The binding key: the next entry (`true`) or the previous one.
    Step(bool),
    /// Escape: closes with no action.
    Cancel,
}

/// The open window switcher.
#[derive(Debug, Clone)]
pub struct WindowSwitcher {
    /// The windows shown, in focus order (most recent first).
    pub entries: Vec<WindowId>,
    /// Index of the selected entry in `entries`.
    pub selected: usize,
    /// The modifiers of the combo that opened the switcher: it stays open
    /// while one of them is held and activates the selection when the last
    /// one is released.
    pub hold_mods: Mods,
    /// Key name of the combo that opened the switcher (`Tab`): pressed
    /// again it moves the selection.
    pub key: String,
    /// True when the switcher was opened backwards (`switcher_prev`): the
    /// key moves backwards, and forwards with Shift toggled.
    pub backwards: bool,
    /// The image last drawn (nested backend), drawn again only when the
    /// rows, the selection or the scale change.
    pub images: SwitcherImageCache,
}

impl WindowSwitcher {
    /// The id of the selected window, if any.
    pub fn selected_id(&self) -> Option<WindowId> {
        self.entries.get(self.selected).copied()
    }

    /// True when the key of the switcher pressed with Shift `shift` moves
    /// forwards: the direction of the opening combo, the other way when
    /// Shift differs from the opening combo.
    pub fn steps_forward(&self, shift: bool) -> bool {
        let toggled = shift != self.hold_mods.shift;
        self.backwards == toggled
    }

    /// The image of the switcher showing `rows` at `scale`: the cached one
    /// while the rows, the selection and the scale are the same, else drawn
    /// again (a failure is cached the same way, see `RenderCache`).
    pub fn image(&mut self, rows: Vec<SwitcherRow>, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
        let selected = self.selected;
        let key = (rows, selected, scale);
        let draw_rows = key.0.clone();
        self.images.get_or_draw(key, || render_switcher(&draw_rows, selected, theme, scale))
    }
}

impl State {
    /// Puts the window `id` at the front of the focus history (`mru`): it
    /// just became the active window.
    pub fn remember_active(&mut self, id: WindowId) {
        self.mru.retain(|i| *i != id);
        self.mru.insert(0, id);
    }

    /// Opens the window switcher over the windows of the current desktop
    /// (every desktop with `all`), in focus order, the second entry
    /// selected (the last one when `backwards`). The modifiers held to keep
    /// it open are those of the configured combo of the action. Nothing
    /// opens (`debug_log!`) with no window, under an exclusive keyboard
    /// layer, or when the action has no key; already open, it moves the
    /// selection instead.
    pub fn open_switcher(&mut self, all: bool, backwards: bool) {
        if self.switcher.is_some() {
            opusdm_ipc::debug_log!(LOG_TAG, "window switcher: already open, selection moved");
            self.switcher_key(SwitcherKey::Step(!backwards));
            return;
        }
        if let Some(layer) = &self.exclusive_layer {
            opusdm_ipc::debug_log!(LOG_TAG, "window switcher: exclusive keyboard layer {} holds the keyboard, not opened", layer.id());
            return;
        }
        let action = match (all, backwards) {
            (true, _) => Action::SwitcherAll,
            (false, false) => Action::SwitcherNext,
            (false, true) => Action::SwitcherPrev,
        };
        let Some(combo) = self.hotkeys.combo_of(action) else {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher: {action:?} has no key, not opened");
            return;
        };
        let entries = switcher_entries(&self.mru, &self.window_views(), all, self.desktops.current());
        if entries.is_empty() {
            opusdm_ipc::debug_log!(LOG_TAG, "window switcher: no window to switch to, not opened");
            return;
        }
        let selected = initial_selection(entries.len(), backwards);
        opusdm_ipc::debug_log!(LOG_TAG, "window switcher opened (all {all}, backwards {backwards}): {} entries, selected {selected}", entries.len());
        self.switcher = Some(WindowSwitcher {
            entries,
            selected,
            hold_mods: combo.mods,
            key: combo.name,
            backwards,
            images: SwitcherImageCache::default(),
        });
        self.check_switcher_mods();
    }

    /// Closes the open switcher with no action, logging `why`; nothing when
    /// it is closed.
    pub fn close_switcher(&mut self, why: &str) {
        let Some(switcher) = self.switcher.take() else { return };
        opusdm_ipc::debug_log!(LOG_TAG, "window switcher closed: {why} ({} entries)", switcher.entries.len());
    }

    /// Drops the entries whose window is gone (the selection moves to the
    /// entry that followed); closes the switcher when no entry is left or
    /// an exclusive keyboard layer holds the keyboard. Called by `refresh`
    /// and before every key the switcher takes.
    pub fn check_switcher(&mut self) {
        let Some(switcher) = &self.switcher else { return };
        if self.exclusive_layer.is_some() {
            self.close_switcher("an exclusive keyboard layer holds the keyboard");
            return;
        }
        let (entries, selected) = retain_entries(&switcher.entries, switcher.selected, |id| self.window_by_id(id).is_some());
        let dropped = switcher.entries.len() - entries.len();
        if entries.is_empty() {
            self.close_switcher("no window left");
            return;
        }
        if dropped == 0 { return; }
        opusdm_ipc::debug_log!(LOG_TAG, "window switcher: {dropped} entries gone, selected {selected}");
        if let Some(switcher) = self.switcher.as_mut() {
            switcher.entries = entries;
            switcher.selected = selected;
        }
    }

    /// A key for the open switcher: moves the selection or cancels.
    pub fn switcher_key(&mut self, key: SwitcherKey) {
        self.check_switcher();
        let Some(switcher) = self.switcher.as_mut() else {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher key {key:?}: the switcher closed, key dropped");
            return;
        };
        match key {
            SwitcherKey::Step(forward) => {
                switcher.selected = step_selection(switcher.selected, switcher.entries.len(), forward);
                opusdm_ipc::debug_log!(LOG_TAG, "window switcher: selected {}", switcher.selected);
            }
            SwitcherKey::Cancel => self.close_switcher("Escape"),
        }
    }

    /// After every key event while the switcher is open: when none of its
    /// hold modifiers is held any more, activates the selected window
    /// (`activate_window`: shows its desktop, restores, raises, focuses)
    /// and closes the switcher.
    pub fn check_switcher_mods(&mut self) {
        let Some(switcher) = &self.switcher else { return };
        let hold = switcher.hold_mods;
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher: the seat has no keyboard, closed");
            self.close_switcher("no keyboard");
            return;
        };
        let held = mods_of(&keyboard.modifier_state());
        let still = (hold.ctrl && held.ctrl) || (hold.alt && held.alt) || (hold.shift && held.shift) || (hold.sup && held.sup);
        if still { return; }
        self.check_switcher();
        let Some(switcher) = self.switcher.take() else { return };
        let Some(id) = switcher.selected_id() else {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher: no selected entry ({} entries), nothing activated", switcher.entries.len());
            return;
        };
        opusdm_ipc::debug_log!(LOG_TAG, "window switcher: modifiers released, activating window {}", id.0);
        if let Err(e) = self.activate_window(id) {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher: {e}");
        }
    }

    /// The rows of the open switcher (title and app id of each entry, in
    /// entry order); `None` when it is closed. A window missing from the
    /// views gets an empty row (it is dropped at the next check).
    pub fn switcher_rows(&self) -> Option<Vec<SwitcherRow>> {
        let switcher = self.switcher.as_ref()?;
        let views = self.window_views();
        let rows = switcher
            .entries
            .iter()
            .map(|id| match views.iter().find(|v| v.id == id.0) {
                Some(v) => SwitcherRow { title: v.title.clone(), app_id: v.app_id.clone() },
                None => SwitcherRow { title: String::new(), app_id: String::new() },
            })
            .collect();
        Some(rows)
    }
}
