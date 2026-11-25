//! Drawing of the window menu with cairo and pango into memory buffers the
//! nested backend shows above OVERLAY.
//!
//! The border is drawn inside the `MENU_WIDTH` x items-height box of the
//! menu geometry (`menu_size`), so the image covers exactly the rectangles
//! the hit test uses; the first and last items lose `border_width` pixels
//! of their visible height to it.
//!
//! Ownership: the images belong to the `WindowMenu` they were drawn for
//! (`WindowMenu::images`, a cache keyed by the navigation state), so they
//! are dropped with the menu when it closes.

use pangocairo::pango;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

use super::model::{MenuItem, MenuNav};
use super::theme::MenuTheme;
use super::{item_rects, menu_size, WindowMenu};
use crate::draw::{fill_gradient, paint_buffer, set_rgb, RenderCache};

/// Font of the menu labels.
const MENU_FONT: &str = "Sans 10";

/// Text drawn at the right of an item that opens a submenu.
const SUBMENU_ARROW: &str = ">";

/// Thickness of a separator line, in pixels.
const SEPARATOR_LINE: f64 = 1.0;

/// The images of an open window menu: the menu and its open submenu.
#[derive(Debug, Clone)]
pub struct MenuImages {
    /// The top-level menu, `menu_size` of its items.
    pub menu: MemoryRenderBuffer,
    /// The open submenu; `None` while no submenu is open.
    pub submenu: Option<MemoryRenderBuffer>,
}

/// The images last drawn for a menu, keyed by the navigation state and
/// the scale they were drawn for (a failed draw included).
pub type MenuImageCache = RenderCache<(MenuNav, i32), MenuImages>;

impl WindowMenu {
    /// The images of the menu at `scale`: the cached ones while the
    /// navigation state (items and selection) and the scale are the same,
    /// else drawn again. An error is cached too, so a persistent failure
    /// is not drawn again every frame (a new state is tried again); the
    /// menu itself is untouched (it stays usable by keyboard).
    pub fn images(&mut self, theme: &MenuTheme, scale: i32) -> Result<MenuImages, String> {
        let nav = self.nav.clone();
        self.images.get_or_draw((nav.clone(), scale), || {
            Ok(MenuImages { menu: _render_nav_menu(&nav, theme, scale)?, submenu: _render_nav_submenu(&nav, theme, scale)? })
        })
    }
}

/// Draws the top-level menu of `menu` at `scale` (buffer pixels per
/// logical pixel): `menu_size` of its items, the selected item highlighted
/// (it stays highlighted while its submenu is open). Used by the tests;
/// the backend goes through `WindowMenu::images`.
#[cfg(test)]
pub fn render_menu(menu: &WindowMenu, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    _render_nav_menu(&menu.nav, theme, scale)
}

/// Draws the open submenu of `menu` at `scale`; `None` while no submenu
/// is open. Used by the tests; the backend goes through
/// `WindowMenu::images`.
#[cfg(test)]
pub fn render_submenu(menu: &WindowMenu, theme: &MenuTheme, scale: i32) -> Result<Option<MemoryRenderBuffer>, String> {
    _render_nav_submenu(&menu.nav, theme, scale)
}

/// The top-level menu of the navigation state `nav` (see `render_menu`).
fn _render_nav_menu(nav: &MenuNav, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    let (top, _) = nav.selected();
    _render_items(nav.items(), Some(top), theme, scale)
}

/// The open submenu of the navigation state `nav` (see `render_submenu`).
fn _render_nav_submenu(nav: &MenuNav, theme: &MenuTheme, scale: i32) -> Result<Option<MemoryRenderBuffer>, String> {
    let (top, sub) = nav.selected();
    let Some(sub) = sub else { return Ok(None) };
    let parent = nav.items().get(top).ok_or_else(|| format!("window menu: no item {top} for the open submenu"))?;
    _render_items(&parent.submenu, Some(sub), theme, scale).map(Some)
}

/// Draws `items` (the item `selected` highlighted) into an ARGB8888
/// buffer of `menu_size(items)` times `scale`.
fn _render_items(items: &[MenuItem], selected: Option<usize>, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    paint_buffer("window menu", menu_size(items), scale, |cr| _paint(cr, items, selected, theme))
}

/// Paints `items` on `cr` in logical pixels: the border, the items
/// background gradient over the whole inner box, then each item (the
/// active gradient behind `selected`, its label, a `>` on a submenu
/// parent, a line for a separator), each clipped to its visible box.
fn _paint(cr: &cairo::Context, items: &[MenuItem], selected: Option<usize>, theme: &MenuTheme) -> Result<(), cairo::Error> {
    let (w, h) = menu_size(items);
    let (w, h) = (f64::from(w), f64::from(h));
    let bw = f64::from(theme.border_width).min(w / 2.0).min(h / 2.0);
    set_rgb(cr, theme.border);
    cr.rectangle(0.0, 0.0, w, h);
    cr.fill()?;
    let (x0, x1, top, bottom) = (bw, w - bw, bw, h - bw);
    if x1 <= x0 || bottom <= top { return Ok(()); }
    fill_gradient(cr, theme.item_bg, x0, top, x1 - x0, bottom - top)?;
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string(MENU_FONT)));
    layout.set_ellipsize(pango::EllipsizeMode::End);
    for (i, (item, rect)) in items.iter().zip(item_rects((0, 0), items)).enumerate() {
        let y0 = f64::from(rect.y).max(top);
        let y1 = f64::from(rect.y + rect.h).min(bottom);
        if y1 <= y0 { continue; }
        cr.save()?;
        cr.rectangle(x0, y0, x1 - x0, y1 - y0);
        cr.clip();
        let drawn = _paint_item(cr, &layout, item, selected == Some(i), theme, (x0, y0, x1 - x0, y1 - y0));
        cr.restore()?;
        drawn?;
    }
    Ok(())
}

/// Paints one item inside the visible box `(x, y, w, h)`.
fn _paint_item(
    cr: &cairo::Context,
    layout: &pango::Layout,
    item: &MenuItem,
    active: bool,
    theme: &MenuTheme,
    (x, y, w, h): (f64, f64, f64, f64),
) -> Result<(), cairo::Error> {
    let pad = f64::from(theme.padding);
    if item.is_separator() {
        set_rgb(cr, theme.disabled_text);
        cr.rectangle(x + pad, (y + h / 2.0).floor(), (w - 2.0 * pad).max(0.0), SEPARATOR_LINE);
        return cr.fill();
    }
    if active {
        fill_gradient(cr, theme.active_bg, x, y, w, h)?;
    }
    let colour = match (item.enabled, active) {
        (false, _) => theme.disabled_text,
        (true, true) => theme.active_text,
        (true, false) => theme.item_text,
    };
    set_rgb(cr, colour);
    let mut label_width = w - 2.0 * pad;
    if !item.submenu.is_empty() {
        layout.set_width(-1);
        layout.set_text(SUBMENU_ARROW);
        let (aw, ah) = layout.pixel_size();
        let aw = f64::from(aw);
        cr.move_to(x + w - pad - aw, y + (h - f64::from(ah)) / 2.0);
        pangocairo::functions::show_layout(cr, layout);
        label_width -= aw + pad;
    }
    layout.set_width((label_width.max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_text(&item.label);
    let (_, th) = layout.pixel_size();
    cr.move_to(x + pad, y + (h - f64::from(th)) / 2.0);
    pangocairo::functions::show_layout(cr, layout);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::menu::model::{menu_items, MenuKey, MenuNav};
    use crate::menu::{MENU_ITEM_HEIGHT, MENU_SEPARATOR_HEIGHT, MENU_WIDTH};
    use crate::wm::desktops::Desktops;
    use crate::wm::WindowId;

    /// An open menu of a normal window on desktop 0 of 4.
    fn menu() -> WindowMenu {
        let nav = MenuNav::new(menu_items(false, &Desktops::new(4), 0));
        WindowMenu {
            window: WindowId(1),
            nav,
            origin: (0, 0),
            output: Rect { x: 0, y: 0, w: 1920, h: 1080 },
            images: MenuImageCache::default(),
        }
    }

    /// The bytes of `buffer`.
    fn bytes(buffer: &mut MemoryRenderBuffer) -> Vec<u8> {
        crate::draw::buffer_bytes(buffer)
    }

    /// The ARGB8888 pixel `(x, y)` of `data`, an image `width` pixels wide,
    /// as `(r, g, b)`.
    fn pixel(data: &[u8], width: i32, x: i32, y: i32) -> (u8, u8, u8) {
        let i = ((y * width + x) * 4) as usize;
        (data[i + 2], data[i + 1], data[i])
    }

    #[test]
    fn render_menu_returns_a_buffer_of_menu_width_x_items_height() {
        let menu = menu();
        let height = 5 * MENU_ITEM_HEIGHT + MENU_SEPARATOR_HEIGHT;
        assert_eq!(menu_size(menu.nav.items()), (MENU_WIDTH, height));
        let mut buffer = render_menu(&menu, &MenuTheme::default(), 1).expect("rendered");
        assert_eq!(bytes(&mut buffer).len(), (MENU_WIDTH * height * 4) as usize);
        let mut buffer = render_menu(&menu, &MenuTheme::default(), 2).expect("rendered at scale 2");
        assert_eq!(bytes(&mut buffer).len(), (MENU_WIDTH * 2 * height * 2 * 4) as usize);
    }

    #[test]
    fn the_border_is_inside_and_the_selected_item_is_highlighted() {
        let menu = menu();
        let theme = MenuTheme::default();
        let mut buffer = render_menu(&menu, &theme, 1).expect("rendered");
        let data = bytes(&mut buffer);
        assert_eq!(pixel(&data, MENU_WIDTH, 0, 0), (0x18, 0x18, 0x18), "border at the corner");
        assert_eq!(pixel(&data, MENU_WIDTH, MENU_WIDTH - 1, 30), (0x18, 0x18, 0x18), "border on the right edge");
        let (r, g, b) = pixel(&data, MENU_WIDTH, MENU_WIDTH - 10, 6);
        assert!(b > r && b > 0x80, "the selected first item has the blue active gradient, got {:?}", (r, g, b));
        let (r, g, b) = pixel(&data, MENU_WIDTH, MENU_WIDTH - 10, MENU_ITEM_HEIGHT + 2);
        assert!(r == g && g == b && r < 0x40, "the second item has the dark items gradient, got {:?}", (r, g, b));
    }

    #[test]
    fn the_submenu_is_drawn_only_while_it_is_open() {
        let mut menu = menu();
        let theme = MenuTheme::default();
        assert!(render_submenu(&menu, &theme, 1).expect("no error").is_none());
        menu.nav.key(MenuKey::Down);
        menu.nav.key(MenuKey::Down);
        menu.nav.key(MenuKey::Right);
        let mut sub = render_submenu(&menu, &theme, 1).expect("no error").expect("submenu open");
        assert_eq!(bytes(&mut sub).len(), (MENU_WIDTH * 4 * MENU_ITEM_HEIGHT * 4) as usize, "4 desktops");
    }

    #[test]
    fn the_images_are_drawn_again_only_when_the_menu_state_changes() {
        let mut menu = menu();
        let theme = MenuTheme::default();
        menu.images(&theme, 1).expect("drawn");
        menu.images(&theme, 1).expect("cached");
        assert_eq!(menu.images.builds(), 1, "same state: cached");
        menu.nav.key(MenuKey::Down);
        let images = menu.images(&theme, 1).expect("drawn again");
        assert_eq!(menu.images.builds(), 2, "the selection moved");
        assert!(images.submenu.is_none());
        menu.nav.key(MenuKey::Down);
        menu.nav.key(MenuKey::Right);
        let images = menu.images(&theme, 1).expect("drawn with the submenu");
        assert_eq!(menu.images.builds(), 3, "the submenu opened");
        assert!(images.submenu.is_some());
        menu.images(&theme, 2).expect("drawn at scale 2");
        assert_eq!(menu.images.builds(), 4, "the scale changed");
    }

    #[test]
    fn a_render_error_is_not_drawn_again_for_the_same_state() {
        let mut menu = menu();
        assert!(render_menu(&menu, &MenuTheme::default(), 0).is_err());
        assert!(menu.images(&MenuTheme::default(), 0).is_err());
        assert!(menu.images(&MenuTheme::default(), 0).is_err(), "the cached failure");
        assert_eq!(menu.images.builds(), 1, "a failed draw is tried once per state");
        menu.images(&MenuTheme::default(), 1).expect("a new state is drawn");
        assert_eq!(menu.images.builds(), 2);
    }
}
