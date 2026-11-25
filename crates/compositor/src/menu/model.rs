//! The window menu model: its items and the keyboard navigation over them.
//!
//! Pure code with no Smithay types: `menu_items` builds the items of a
//! window, `MenuNav` walks them (Up / Down skip separators and disabled
//! items and wrap, Right / Enter open the submenu, Left closes it, Enter
//! returns the action, Escape closes the menu).

use crate::wm::desktops::Desktops;

/// What a window menu item does to its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Maximizes the window, or restores it when it is maximized.
    ToggleMaximize,
    /// Minimizes the window.
    Minimize,
    /// Moves the window to the desktop (0-based).
    MoveToDesktop(u32),
    /// Asks the window to close (`xdg_toplevel.close`).
    Close,
    /// SIGKILL to the client process of the window.
    Kill,
}

/// One item of the window menu. A separator has an empty `label`, no
/// action and no submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// Text shown; empty for a separator.
    pub label: String,
    /// What the item does; `None` for a separator or a submenu parent.
    pub action: Option<MenuAction>,
    /// False when the item is shown greyed out and cannot be chosen.
    pub enabled: bool,
    /// Items of the submenu this item opens (empty: no submenu).
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    /// An enabled item `label` that runs `action`.
    fn action(label: &str, action: MenuAction) -> MenuItem {
        MenuItem { label: label.to_string(), action: Some(action), enabled: true, submenu: Vec::new() }
    }

    /// A separator line.
    fn separator() -> MenuItem {
        MenuItem { label: String::new(), action: None, enabled: false, submenu: Vec::new() }
    }

    /// True for a separator (empty label, no action, no submenu).
    pub fn is_separator(&self) -> bool {
        self.label.is_empty() && self.action.is_none() && self.submenu.is_empty()
    }

    /// True when Up / Down may stop on the item: enabled and not a separator.
    pub fn is_selectable(&self) -> bool {
        self.enabled && !self.is_separator()
    }
}

/// Label of the submenu parent that moves the window to another desktop.
pub const MOVE_TO_DESKTOP_LABEL: &str = "Move to desktop";

/// The items of the window menu of a window that is `maximized`, on the
/// desktop `window_desktop` of `desktops`: "Restore" or "Maximize",
/// "Minimize", "Move to desktop" ("1".."N", the window's own desktop
/// disabled; the parent is disabled with one desktop only), a separator,
/// "Close", "Kill".
pub fn menu_items(maximized: bool, desktops: &Desktops, window_desktop: u32) -> Vec<MenuItem> {
    let desktop_items: Vec<MenuItem> = (0..desktops.count())
        .map(|d| MenuItem { enabled: d != window_desktop, ..MenuItem::action(&(d + 1).to_string(), MenuAction::MoveToDesktop(d)) })
        .collect();
    let move_to = MenuItem {
        label: MOVE_TO_DESKTOP_LABEL.to_string(),
        action: None,
        enabled: desktop_items.iter().any(|i| i.enabled),
        submenu: desktop_items,
    };
    vec![
        MenuItem::action(if maximized { "Restore" } else { "Maximize" }, MenuAction::ToggleMaximize),
        MenuItem::action("Minimize", MenuAction::Minimize),
        move_to,
        MenuItem::separator(),
        MenuItem::action("Close", MenuAction::Close),
        MenuItem::action("Kill", MenuAction::Kill),
    ]
}

/// A key the window menu understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Previous item (wraps).
    Up,
    /// Next item (wraps).
    Down,
    /// Closes the open submenu.
    Left,
    /// Opens the submenu of the selected item.
    Right,
    /// Chooses the selected item (opens its submenu when it has one).
    Enter,
    /// Closes the menu with no action.
    Escape,
}

/// Navigation state of an open window menu: its items, the selected item
/// and the selected item of the open submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuNav {
    /// The items of the menu.
    items: Vec<MenuItem>,
    /// Index of the selected top-level item.
    top: usize,
    /// Index of the selected submenu item; `None` while no submenu is open.
    sub: Option<usize>,
    /// True once the menu is closed (an action chosen or Escape).
    closed: bool,
}

impl MenuNav {
    /// Navigation over `items`, the first selectable item selected.
    pub fn new(items: Vec<MenuItem>) -> MenuNav {
        let top = items.iter().position(MenuItem::is_selectable).unwrap_or(0);
        MenuNav { items, top, sub: None, closed: false }
    }

    /// The items of the menu.
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// Applies `key`; returns the action to run when an item was chosen
    /// (the menu is closed then).
    /// Opening a submenu selects its first item, even a disabled one (the
    /// window's own desktop), so Down from there is the next item.
    pub fn key(&mut self, key: MenuKey) -> Option<MenuAction> {
        if self.closed { return None; }
        match key {
            MenuKey::Up | MenuKey::Down => {
                let forward = key == MenuKey::Down;
                match self.sub {
                    Some(sub) => self.sub = Some(_step(self._submenu(), sub, forward)),
                    None => self.top = _step(&self.items, self.top, forward),
                }
                None
            }
            MenuKey::Left => {
                self.sub = None;
                None
            }
            MenuKey::Right => {
                self._open_submenu();
                None
            }
            MenuKey::Enter => {
                if self.sub.is_none() && self._has_submenu(self.top) {
                    self._open_submenu();
                    return None;
                }
                self._choose()
            }
            MenuKey::Escape => {
                self.closed = true;
                None
            }
        }
    }

    /// A pointer click on the item `top` (and on the item `sub` of its
    /// submenu): returns the action to run when one was chosen. A click on
    /// a submenu parent opens its submenu; a click on a separator, a
    /// disabled item or an index out of range does nothing.
    pub fn click(&mut self, top: usize, sub: Option<usize>) -> Option<MenuAction> {
        if self.closed { return None; }
        let item = self.items.get(top)?;
        if !item.is_selectable() { return None; }
        if let Some(sub) = sub {
            if sub >= item.submenu.len() { return None; }
            self.top = top;
            self.sub = Some(sub);
            return self._choose();
        }
        self.top = top;
        if self._has_submenu(top) {
            self.sub = Some(0);
            return None;
        }
        self.sub = None;
        self._choose()
    }

    /// True once the menu is closed.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The selected top-level item and the selected submenu item (`None`
    /// while no submenu is open).
    pub fn selected(&self) -> (usize, Option<usize>) {
        (self.top, self.sub)
    }

    /// True when the item `index` is enabled and has a submenu.
    fn _has_submenu(&self, index: usize) -> bool {
        self.items.get(index).is_some_and(|i| i.enabled && !i.submenu.is_empty())
    }

    /// The submenu of the selected item (empty when it has none).
    fn _submenu(&self) -> &[MenuItem] {
        self.items.get(self.top).map(|i| i.submenu.as_slice()).unwrap_or(&[])
    }

    /// Opens the submenu of the selected item, its first item selected;
    /// nothing when the item has no submenu or it is already open.
    fn _open_submenu(&mut self) {
        if self.sub.is_some() || !self._has_submenu(self.top) { return; }
        self.sub = Some(0);
    }

    /// Chooses the selected item (of the submenu when it is open): its
    /// action, the menu closed; `None` for a disabled item or no action.
    fn _choose(&mut self) -> Option<MenuAction> {
        let item = match self.sub {
            Some(sub) => self._submenu().get(sub),
            None => self.items.get(self.top),
        };
        let action = item.filter(|i| i.enabled).and_then(|i| i.action)?;
        self.closed = true;
        Some(action)
    }
}

/// Index of the next selectable item of `items` after `from` (before it
/// when `forward` is false), wrapping; `from` when no other item is
/// selectable.
fn _step(items: &[MenuItem], from: usize, forward: bool) -> usize {
    let len = items.len();
    if len == 0 { return from; }
    (1..len)
        .map(|k| if forward { (from + k) % len } else { (from + len - k % len) % len })
        .find(|i| items.get(*i).is_some_and(MenuItem::is_selectable))
        .unwrap_or(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Labels of `items`.
    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    /// Navigation over the menu of a normal window on desktop 0 of 4.
    fn nav() -> MenuNav {
        MenuNav::new(menu_items(false, &Desktops::new(4), 0))
    }

    #[test]
    fn a_normal_window_gets_maximize_and_the_fixed_items() {
        let items = menu_items(false, &Desktops::new(4), 0);
        assert_eq!(labels(&items), ["Maximize", "Minimize", "Move to desktop", "", "Close", "Kill"]);
        assert_eq!(items[0].action, Some(MenuAction::ToggleMaximize));
        assert_eq!(items[1].action, Some(MenuAction::Minimize));
        assert_eq!(items[2].action, None);
        assert!(items[3].is_separator());
        assert_eq!(items[4].action, Some(MenuAction::Close));
        assert_eq!(items[5].action, Some(MenuAction::Kill));
    }

    #[test]
    fn a_maximized_window_gets_restore() {
        let items = menu_items(true, &Desktops::new(4), 0);
        assert_eq!(items[0].label, "Restore");
        assert_eq!(items[0].action, Some(MenuAction::ToggleMaximize));
    }

    #[test]
    fn the_desktop_of_the_window_is_disabled_in_the_submenu() {
        let items = menu_items(false, &Desktops::new(3), 1);
        let sub = &items[2].submenu;
        assert_eq!(labels(sub), ["1", "2", "3"]);
        assert_eq!(sub.iter().map(|i| i.enabled).collect::<Vec<_>>(), [true, false, true]);
        assert_eq!(sub[2].action, Some(MenuAction::MoveToDesktop(2)));
    }

    #[test]
    fn one_desktop_disables_move_to_desktop() {
        let items = menu_items(false, &Desktops::new(1), 0);
        assert!(!items[2].enabled);
        let mut nav = MenuNav::new(items);
        nav.key(MenuKey::Down);
        assert_eq!(nav.key(MenuKey::Down), None);
        assert_eq!(nav.selected(), (4, None), "Down skips the disabled parent and the separator");
    }

    #[test]
    fn the_first_item_is_selected_at_open() {
        assert_eq!(nav().selected(), (0, None));
    }

    #[test]
    fn down_from_move_to_desktop_skips_the_separator_and_lands_on_close() {
        let mut nav = nav();
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        assert_eq!(nav.selected(), (2, None));
        nav.key(MenuKey::Down);
        assert_eq!(nav.selected(), (4, None));
    }

    #[test]
    fn up_from_the_first_item_wraps_to_kill() {
        let mut nav = nav();
        nav.key(MenuKey::Up);
        assert_eq!(nav.selected(), (5, None));
        assert_eq!(nav.items()[5].label, "Kill");
        nav.key(MenuKey::Down);
        assert_eq!(nav.selected(), (0, None), "Down from the last item wraps to the first");
    }

    #[test]
    fn right_down_enter_moves_to_the_next_desktop() {
        let mut nav = nav();
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        assert_eq!(nav.key(MenuKey::Right), None);
        assert_eq!(nav.selected(), (2, Some(0)));
        nav.key(MenuKey::Down);
        assert_eq!(nav.key(MenuKey::Enter), Some(MenuAction::MoveToDesktop(1)));
        assert!(nav.closed());
    }

    #[test]
    fn down_in_the_submenu_skips_the_disabled_desktop() {
        let mut nav = MenuNav::new(menu_items(false, &Desktops::new(4), 1));
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Enter);
        assert_eq!(nav.selected(), (2, Some(0)), "Enter on the parent opens the submenu");
        nav.key(MenuKey::Down);
        assert_eq!(nav.selected(), (2, Some(2)));
        nav.key(MenuKey::Up);
        nav.key(MenuKey::Up);
        assert_eq!(nav.selected(), (2, Some(3)), "Up wraps inside the submenu");
    }

    #[test]
    fn enter_on_a_disabled_desktop_does_nothing() {
        let mut nav = nav();
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Right);
        assert_eq!(nav.key(MenuKey::Enter), None);
        assert!(!nav.closed());
    }

    #[test]
    fn left_closes_the_submenu() {
        let mut nav = nav();
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Down);
        nav.key(MenuKey::Right);
        nav.key(MenuKey::Left);
        assert_eq!(nav.selected(), (2, None));
        assert!(!nav.closed());
    }

    #[test]
    fn right_on_an_item_without_submenu_does_nothing() {
        let mut nav = nav();
        nav.key(MenuKey::Right);
        assert_eq!(nav.selected(), (0, None));
    }

    #[test]
    fn enter_returns_the_action_and_closes() {
        let mut nav = nav();
        nav.key(MenuKey::Down);
        assert_eq!(nav.key(MenuKey::Enter), Some(MenuAction::Minimize));
        assert!(nav.closed());
        assert_eq!(nav.key(MenuKey::Enter), None, "a closed menu runs nothing");
    }

    #[test]
    fn escape_closes_with_no_action() {
        let mut nav = nav();
        assert_eq!(nav.key(MenuKey::Escape), None);
        assert!(nav.closed());
    }

    #[test]
    fn a_click_chooses_an_item_and_opens_a_submenu() {
        let mut nav = nav();
        assert_eq!(nav.click(3, None), None, "a click on the separator does nothing");
        assert!(!nav.closed());
        assert_eq!(nav.click(2, None), None);
        assert_eq!(nav.selected(), (2, Some(0)));
        assert_eq!(nav.click(2, Some(0)), None, "the window's own desktop is disabled");
        assert_eq!(nav.click(2, Some(3)), Some(MenuAction::MoveToDesktop(3)));
        assert!(nav.closed());
        let mut nav = super::tests::nav();
        assert_eq!(nav.click(4, None), Some(MenuAction::Close));
        let mut nav = super::tests::nav();
        assert_eq!(nav.click(9, None), None, "an index out of range does nothing");
        assert_eq!(nav.click(0, Some(1)), None, "Maximize has no submenu");
        assert!(!nav.closed());
    }
}
