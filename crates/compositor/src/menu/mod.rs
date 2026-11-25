//! The window menu (Alt+Space), drawn by the compositor.
//!
//! `model` holds the items and the keyboard navigation, `theme` the
//! OpusDMTheme colours. This module holds the geometry shared by the hit
//! test and the renderer (fixed metrics, pure functions) and the `State`
//! side: open, close, keys, clicks and the actions.
//!
//! Ownership: `State::menu` is the only owner of the open `WindowMenu`. It
//! refers to its window by `WindowId` only, never by a handle, so a window
//! that goes away cannot be kept alive or used by the menu: the menu is
//! closed when its window is destroyed or hidden (`toplevel_destroyed`,
//! `refresh`), and an action looks the window up again when it runs (an
//! unknown id is logged and nothing runs).

pub mod model;
pub mod theme;

use smithay::{
    desktop::find_popup_root_surface,
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point},
    wayland::shell::xdg::XdgShellHandler,
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;
use model::{menu_items, MenuAction, MenuItem, MenuKey, MenuNav};

/// Height of a menu item, in pixels.
pub const MENU_ITEM_HEIGHT: i32 = 24;

/// Width of the menu and of its submenu, in pixels.
pub const MENU_WIDTH: i32 = 200;

/// Height of a separator, in pixels.
pub const MENU_SEPARATOR_HEIGHT: i32 = 9;

/// Height of `item`: `MENU_SEPARATOR_HEIGHT` for a separator, else
/// `MENU_ITEM_HEIGHT`.
pub fn item_height(item: &MenuItem) -> i32 {
    if item.is_separator() { MENU_SEPARATOR_HEIGHT } else { MENU_ITEM_HEIGHT }
}

/// The rectangles of `items` stacked from `origin`, `MENU_WIDTH` wide.
pub fn item_rects(origin: (i32, i32), items: &[MenuItem]) -> Vec<Rect> {
    let mut y = origin.1;
    items
        .iter()
        .map(|item| {
            let h = item_height(item);
            let rect = Rect { x: origin.0, y, w: MENU_WIDTH, h };
            y += h;
            rect
        })
        .collect()
}

/// Size of a menu of `items`.
pub fn menu_size(items: &[MenuItem]) -> (i32, i32) {
    (MENU_WIDTH, items.iter().map(item_height).sum())
}

/// Origin of a menu of `size` placed at `anchor` and moved inside `output`
/// (a menu larger than the output sticks to its top-left corner).
pub fn place_menu(anchor: (i32, i32), size: (i32, i32), output: Rect) -> (i32, i32) {
    (_clamp_axis(anchor.0, size.0, output.x, output.w), _clamp_axis(anchor.1, size.1, output.y, output.h))
}

/// `pos` moved so `[pos, pos + len)` stays inside `[start, start + span)`;
/// `start` when `len` is larger than `span`.
fn _clamp_axis(pos: i32, len: i32, start: i32, span: i32) -> i32 {
    let max = start + span - len;
    if max < start { return start; }
    pos.clamp(start, max)
}

/// Origin of a submenu of `size` opened from the item `item` of the menu
/// `menu`: right of the menu, at the item's top; left of the menu when it
/// does not fit on the right; moved inside `output` vertically.
pub fn submenu_origin(menu: Rect, item: Rect, size: (i32, i32), output: Rect) -> (i32, i32) {
    let right = menu.x + menu.w;
    let fits_right = right + size.0 <= output.x + output.w;
    let x = if fits_right { right } else { menu.x - size.0 };
    place_menu((x, item.y), size, output)
}

/// Where the window menu and its open submenu are, in output coordinates.
// `menu` and the submenu box are read by the renderer (stage 2, task 9).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuLayout {
    /// The menu box.
    pub menu: Rect,
    /// The rectangles of the menu items.
    pub items: Vec<Rect>,
    /// The open submenu: the index of its parent item, its box and the
    /// rectangles of its items.
    pub submenu: Option<(usize, Rect, Vec<Rect>)>,
}

/// What a point of the screen is in the window menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuHit {
    /// The menu item with this index (a separator included).
    Item(usize),
    /// The item `.1` of the submenu of the item `.0`.
    SubItem(usize, usize),
    /// Outside the menu and its submenu.
    Outside,
}

/// The layout of the menu `nav` placed at `origin`, its open submenu kept
/// inside `output`.
pub fn layout(origin: (i32, i32), nav: &MenuNav, output: Rect) -> MenuLayout {
    let items = nav.items();
    let (w, h) = menu_size(items);
    let menu = Rect { x: origin.0, y: origin.1, w, h };
    let rects = item_rects(origin, items);
    let (top, sub) = nav.selected();
    let submenu = sub.and_then(|_| {
        let parent = items.get(top)?;
        let item = rects.get(top)?;
        let size = menu_size(&parent.submenu);
        let sub_origin = submenu_origin(menu, *item, size, output);
        let sub_rect = Rect { x: sub_origin.0, y: sub_origin.1, w: size.0, h: size.1 };
        Some((top, sub_rect, item_rects(sub_origin, &parent.submenu)))
    });
    MenuLayout { menu, items: rects, submenu }
}

/// What the point `(x, y)` hits in `layout`: the submenu is above the menu.
pub fn hit_test(layout: &MenuLayout, x: i32, y: i32) -> MenuHit {
    if let Some((parent, _, rects)) = &layout.submenu {
        if let Some(i) = rects.iter().position(|r| _contains(*r, x, y)) { return MenuHit::SubItem(*parent, i); }
    }
    match layout.items.iter().position(|r| _contains(*r, x, y)) {
        Some(i) => MenuHit::Item(i),
        None => MenuHit::Outside,
    }
}

/// True when `(x, y)` is inside `rect` (right and bottom edges excluded).
fn _contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

/// The open window menu of one window.
#[derive(Debug, Clone)]
pub struct WindowMenu {
    /// The window the menu acts on, looked up again when an action runs.
    pub window: WindowId,
    /// The items and the selection.
    pub nav: MenuNav,
    /// Top-left corner of the menu, inside `output`.
    pub origin: (i32, i32),
    /// The output area the menu and its submenu stay in.
    pub output: Rect,
}

impl WindowMenu {
    /// The layout of the menu and of its open submenu.
    pub fn layout(&self) -> MenuLayout {
        layout(self.origin, &self.nav, self.output)
    }
}

impl State {
    /// Opens the window menu of the window with the keyboard (the root of
    /// the focused popup), at the window's top-left corner kept inside the
    /// output. Nothing opens (`debug_log!`) when no shown window has the
    /// keyboard (nothing, a layer surface, an exclusive keyboard layer) or
    /// a menu is already open. A client popup grab (an open client menu)
    /// is ended first, as an exclusive layer does, so the client menu and
    /// the window menu never fight for the keyboard and the pointer.
    pub fn open_window_menu(&mut self) {
        if let Some(menu) = &self.menu {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu: already open on window {}", menu.window.0);
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "window menu: the seat has no keyboard, not opened");
            return;
        };
        let Some(focus) = keyboard.current_focus() else {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu: no focused window, not opened");
            return;
        };
        let root = self._popup_root(&focus);
        let Some(info) = self.windows.get(&root) else {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu: no focused window (keyboard on {}), not opened", focus.id());
            return;
        };
        let Some(loc) = self.space.element_location(&info.window) else {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu: window {} is not shown, not opened", info.id.0);
            return;
        };
        let window = info.id;
        let nav = MenuNav::new(menu_items(info.maximized, &self.desktops, info.desktop));
        if let Some(grab_root) = keyboard.grab_start_data().and_then(|d| d.focus) {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu ends the popup grab rooted at {}", grab_root.id());
            self.dismiss_popups_and_grabs(&grab_root);
        }
        let output = self.output_area();
        let origin = place_menu((loc.x, loc.y), menu_size(nav.items()), output);
        self.menu = Some(WindowMenu { window, nav, origin, output });
        opusdm_ipc::debug_log!(LOG_TAG, "window menu opened on window {} at {origin:?}", window.0);
    }

    /// Closes the open window menu, with no action, logging `why`;
    /// nothing when no menu is open.
    pub fn close_window_menu(&mut self, why: &str) {
        let Some(menu) = self.menu.take() else { return };
        opusdm_ipc::debug_log!(LOG_TAG, "window menu closed: {why} (window {})", menu.window.0);
    }

    /// Closes the window menu when its window is gone or no longer shown,
    /// or when an exclusive keyboard layer holds the keyboard. Called by
    /// `refresh` and before every key and click the menu takes.
    pub fn check_window_menu(&mut self) {
        let Some(menu) = &self.menu else { return };
        let why = match self.window_by_id(menu.window) {
            None => Some("its window is gone"),
            Some((_, info)) if self.space.element_location(&info.window).is_none() => Some("its window is hidden"),
            Some(_) if self.exclusive_layer.is_some() => Some("an exclusive keyboard layer holds the keyboard"),
            Some(_) => None,
        };
        if let Some(why) = why {
            self.close_window_menu(why);
        }
    }

    /// A key for the open window menu: moves the selection, or runs the
    /// chosen action on the menu's window (the menu closes first).
    pub fn window_menu_key(&mut self, key: MenuKey) {
        self.check_window_menu();
        let Some(menu) = self.menu.as_mut() else {
            opusdm_ipc::err_log!(LOG_TAG, "window menu key {key:?}: the menu closed, key dropped");
            return;
        };
        let action = menu.nav.key(key);
        let window = menu.window;
        self._after_menu_input(window, action);
    }

    /// A button press at `location` while the window menu is open: on an
    /// item it chooses it (a submenu parent opens its submenu), outside
    /// the menu it closes the menu. The press never reaches a client.
    pub fn window_menu_click(&mut self, location: Point<f64, Logical>) {
        self.check_window_menu();
        let Some(menu) = self.menu.as_mut() else {
            opusdm_ipc::debug_log!(LOG_TAG, "window menu click: the menu closed, click dropped");
            return;
        };
        let at = location.to_i32_floor();
        let action = match hit_test(&menu.layout(), at.x, at.y) {
            MenuHit::Outside => {
                self.close_window_menu("click outside the menu");
                return;
            }
            MenuHit::Item(top) => menu.nav.click(top, None),
            MenuHit::SubItem(top, sub) => menu.nav.click(top, Some(sub)),
        };
        let window = menu.window;
        self._after_menu_input(window, action);
    }

    /// Runs `action` on the window `id`, looked up now: an unknown or dead
    /// window is logged and nothing runs.
    pub fn run_menu_action(&mut self, id: WindowId, action: MenuAction) {
        let result = match action {
            MenuAction::ToggleMaximize => self._toggle_maximize(id),
            MenuAction::Minimize => self.set_minimized(id, true),
            MenuAction::MoveToDesktop(desktop) => self.move_to_desktop(id, desktop),
            MenuAction::Close => self.close_window(id),
            MenuAction::Kill => self.kill_window(id),
        };
        match result {
            Ok(()) => opusdm_ipc::debug_log!(LOG_TAG, "window menu: {action:?} on window {}", id.0),
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "window menu: {action:?} on window {} failed: {e}", id.0),
        }
    }

    /// Drops the menu when the last input closed it, then runs `action`
    /// (if any) on the window `window`.
    fn _after_menu_input(&mut self, window: WindowId, action: Option<MenuAction>) {
        if self.menu.as_ref().is_some_and(|m| m.nav.closed()) {
            self.close_window_menu(if action.is_some() { "item chosen" } else { "Escape" });
        }
        let Some(action) = action else { return };
        self.run_menu_action(window, action);
    }

    /// Maximizes the window `id`, or restores it when it is maximized,
    /// through the same path as a client request (stage 1 `fill_window`).
    fn _toggle_maximize(&mut self, id: WindowId) -> Result<(), String> {
        let (_, info) = self.window_by_id(id).ok_or_else(|| format!("unknown window {}", id.0))?;
        let toplevel = info.window.toplevel().cloned().ok_or_else(|| format!("window {} has no toplevel", id.0))?;
        if info.maximized {
            self.unmaximize_request(toplevel);
        } else {
            self.maximize_request(toplevel);
        }
        self.notify_watchers();
        Ok(())
    }

    /// The root surface of the popup `surface` (a toplevel or a layer
    /// surface); `surface` itself when it is not a popup or its root is gone.
    fn _popup_root(&self, surface: &WlSurface) -> WlSurface {
        let Some(kind) = self.popups.find_popup(surface) else { return surface.clone() };
        match find_popup_root_surface(&kind) {
            Ok(root) => root,
            Err(_) => {
                opusdm_ipc::debug_log!(LOG_TAG, "window menu: the root of popup {} is gone", surface.id());
                surface.clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wm::desktops::Desktops;
    use model::{menu_items, MenuKey};

    /// The output used by the tests.
    const OUTPUT: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    /// Navigation over the menu of a normal window on desktop 0 of 4.
    fn nav() -> MenuNav {
        MenuNav::new(menu_items(false, &Desktops::new(4), 0))
    }

    #[test]
    fn items_are_stacked_with_short_separators() {
        let items = menu_items(false, &Desktops::new(4), 0);
        let rects = item_rects((10, 20), &items);
        assert_eq!(rects[0], Rect { x: 10, y: 20, w: MENU_WIDTH, h: MENU_ITEM_HEIGHT });
        assert_eq!(rects[3], Rect { x: 10, y: 20 + 3 * MENU_ITEM_HEIGHT, w: MENU_WIDTH, h: MENU_SEPARATOR_HEIGHT });
        assert_eq!(rects[4].y, 20 + 3 * MENU_ITEM_HEIGHT + MENU_SEPARATOR_HEIGHT);
        assert_eq!(menu_size(&items), (MENU_WIDTH, 5 * MENU_ITEM_HEIGHT + MENU_SEPARATOR_HEIGHT));
    }

    #[test]
    fn the_menu_is_kept_inside_the_output() {
        assert_eq!(place_menu((100, 50), (200, 129), OUTPUT), (100, 50));
        assert_eq!(place_menu((1900, 1000), (200, 129), OUTPUT), (1720, 951));
        assert_eq!(place_menu((-30, -40), (200, 129), OUTPUT), (0, 0));
        assert_eq!(place_menu((50, 50), (3000, 2000), OUTPUT), (0, 0), "too big: top-left corner");
        let shifted = Rect { x: 100, y: 200, w: 800, h: 600 };
        assert_eq!(place_menu((0, 0), (200, 129), shifted), (100, 200));
    }

    #[test]
    fn the_submenu_opens_right_or_left_and_stays_inside() {
        let menu = Rect { x: 100, y: 100, w: MENU_WIDTH, h: 129 };
        let item = Rect { x: 100, y: 148, w: MENU_WIDTH, h: MENU_ITEM_HEIGHT };
        assert_eq!(submenu_origin(menu, item, (200, 96), OUTPUT), (300, 148));
        let right = Rect { x: 1720, ..menu };
        assert_eq!(submenu_origin(right, Rect { x: 1720, ..item }, (200, 96), OUTPUT), (1520, 148));
        let low = Rect { x: 100, y: 1040, w: MENU_WIDTH, h: MENU_ITEM_HEIGHT };
        assert_eq!(submenu_origin(menu, low, (200, 96), OUTPUT), (300, 984));
    }

    #[test]
    fn the_layout_has_the_submenu_only_while_it_is_open() {
        let mut nav = nav();
        assert_eq!(layout((0, 0), &nav, OUTPUT).submenu, None);
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Right);
        let l = layout((0, 0), &nav, OUTPUT);
        let (parent, sub, rects) = l.submenu.expect("submenu open");
        assert_eq!(parent, 2);
        assert_eq!(sub, Rect { x: MENU_WIDTH, y: 2 * MENU_ITEM_HEIGHT, w: MENU_WIDTH, h: 4 * MENU_ITEM_HEIGHT });
        assert_eq!(rects.len(), 4);
    }

    #[test]
    fn hit_test_finds_items_submenu_items_and_outside() {
        let mut nav = nav();
        let l = layout((0, 0), &nav, OUTPUT);
        assert_eq!(hit_test(&l, 5, 5), MenuHit::Item(0));
        assert_eq!(hit_test(&l, 199, 3 * MENU_ITEM_HEIGHT + 1), MenuHit::Item(3), "the separator");
        assert_eq!(hit_test(&l, 5, 3 * MENU_ITEM_HEIGHT + MENU_SEPARATOR_HEIGHT), MenuHit::Item(4));
        assert_eq!(hit_test(&l, 200, 5), MenuHit::Outside, "right edge is outside");
        assert_eq!(hit_test(&l, 5, 5 * MENU_ITEM_HEIGHT + MENU_SEPARATOR_HEIGHT), MenuHit::Outside, "below");
        assert_eq!(hit_test(&l, -1, 5), MenuHit::Outside);
        nav.click(2, None);
        let l = layout((0, 0), &nav, OUTPUT);
        assert_eq!(hit_test(&l, MENU_WIDTH + 5, 2 * MENU_ITEM_HEIGHT + MENU_ITEM_HEIGHT + 1), MenuHit::SubItem(2, 1));
        assert_eq!(hit_test(&l, MENU_WIDTH + 5, 1), MenuHit::Outside, "above the submenu");
    }
}
