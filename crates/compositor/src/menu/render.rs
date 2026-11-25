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
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Buffer, Rectangle, Transform},
};

use super::model::{MenuItem, MenuNav};
use super::theme::{MenuTheme, Rgb};
use super::{item_rects, menu_size, WindowMenu};

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

/// The images last drawn for a menu and the state they were drawn for.
#[derive(Debug, Clone, Default)]
pub struct MenuImageCache {
    /// The navigation state and the scale of `images`.
    entry: Option<(MenuNav, i32, MenuImages)>,
    /// How many times the images were drawn (read by the tests).
    builds: u32,
}

impl WindowMenu {
    /// The images of the menu at `scale`: the cached ones while the
    /// navigation state (items and selection) and the scale are the same,
    /// else drawn again. On an error the cache is empty and the next call
    /// draws again; the menu itself is untouched (it stays usable by
    /// keyboard).
    pub fn images(&mut self, theme: &MenuTheme, scale: i32) -> Result<MenuImages, String> {
        if let Some((nav, s, images)) = &self.images.entry {
            if *s == scale && *nav == self.nav { return Ok(images.clone()); }
        }
        self.images.entry = None;
        let images = MenuImages { menu: render_menu(self, theme, scale)?, submenu: render_submenu(self, theme, scale)? };
        self.images.entry = Some((self.nav.clone(), scale, images.clone()));
        self.images.builds = self.images.builds.saturating_add(1);
        Ok(images)
    }
}

/// Draws the top-level menu of `menu` at `scale` (buffer pixels per
/// logical pixel): `menu_size` of its items, the selected item highlighted
/// (it stays highlighted while its submenu is open).
pub fn render_menu(menu: &WindowMenu, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    let (top, _) = menu.nav.selected();
    _render_items(menu.nav.items(), Some(top), theme, scale)
}

/// Draws the open submenu of `menu` at `scale`; `None` while no submenu
/// is open.
pub fn render_submenu(menu: &WindowMenu, theme: &MenuTheme, scale: i32) -> Result<Option<MemoryRenderBuffer>, String> {
    let (top, sub) = menu.nav.selected();
    let Some(sub) = sub else { return Ok(None) };
    let parent = menu.nav.items().get(top).ok_or_else(|| format!("window menu: no item {top} for the open submenu"))?;
    _render_items(&parent.submenu, Some(sub), theme, scale).map(Some)
}

/// Draws `items` (the item `selected` highlighted) into an ARGB8888
/// buffer of `menu_size(items)` times `scale`.
fn _render_items(items: &[MenuItem], selected: Option<usize>, theme: &MenuTheme, scale: i32) -> Result<MemoryRenderBuffer, String> {
    if scale < 1 { return Err(format!("window menu: bad scale {scale}")); }
    let (w, h) = menu_size(items);
    if w <= 0 || h <= 0 { return Err(format!("window menu: empty image {w}x{h}")); }
    let (pw, ph) = (w * scale, h * scale);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pw, ph)
        .map_err(|e| format!("window menu: cairo surface {pw}x{ph}: {e}"))?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| format!("window menu: cairo context: {e}"))?;
        cr.scale(f64::from(scale), f64::from(scale));
        _paint(&cr, items, selected, theme).map_err(|e| format!("window menu: cairo drawing: {e}"))?;
    }
    surface.flush();
    let stride = surface.stride();
    if stride != pw * 4 { return Err(format!("window menu: cairo stride {stride} for width {pw}")); }
    let data = surface.data().map_err(|e| format!("window menu: cairo surface data: {e}"))?;
    let opaque: Vec<Rectangle<i32, Buffer>> = vec![Rectangle::from_size((pw, ph).into())];
    Ok(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (pw, ph), scale, Transform::Normal, Some(opaque)))
}

/// Paints `items` on `cr` in logical pixels: the border, the items
/// background gradient over the whole inner box, then each item (the
/// active gradient behind `selected`, its label, a `>` on a submenu
/// parent, a line for a separator), each clipped to its visible box.
fn _paint(cr: &cairo::Context, items: &[MenuItem], selected: Option<usize>, theme: &MenuTheme) -> Result<(), cairo::Error> {
    let (w, h) = menu_size(items);
    let (w, h) = (f64::from(w), f64::from(h));
    let bw = f64::from(theme.border_width).min(w / 2.0).min(h / 2.0);
    _set_rgb(cr, theme.border);
    cr.rectangle(0.0, 0.0, w, h);
    cr.fill()?;
    let (x0, x1, top, bottom) = (bw, w - bw, bw, h - bw);
    if x1 <= x0 || bottom <= top { return Ok(()); }
    _fill_gradient(cr, theme.item_bg, x0, top, x1 - x0, bottom - top)?;
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
        _set_rgb(cr, theme.disabled_text);
        cr.rectangle(x + pad, (y + h / 2.0).floor(), (w - 2.0 * pad).max(0.0), SEPARATOR_LINE);
        return cr.fill();
    }
    if active {
        _fill_gradient(cr, theme.active_bg, x, y, w, h)?;
    }
    let colour = match (item.enabled, active) {
        (false, _) => theme.disabled_text,
        (true, true) => theme.active_text,
        (true, false) => theme.item_text,
    };
    _set_rgb(cr, colour);
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

/// Fills `(x, y, w, h)` with the vertical gradient `colours.0` (top) to
/// `colours.1` (bottom).
fn _fill_gradient(cr: &cairo::Context, colours: (Rgb, Rgb), x: f64, y: f64, w: f64, h: f64) -> Result<(), cairo::Error> {
    let gradient = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    for (offset, rgb) in [(0.0, colours.0), (1.0, colours.1)] {
        let (r, g, b) = _components(rgb);
        gradient.add_color_stop_rgb(offset, r, g, b);
    }
    cr.set_source(&gradient)?;
    cr.rectangle(x, y, w, h);
    cr.fill()
}

/// Sets the source of `cr` to the solid colour `rgb`.
fn _set_rgb(cr: &cairo::Context, rgb: Rgb) {
    let (r, g, b) = _components(rgb);
    cr.set_source_rgb(r, g, b);
}

/// The components of `rgb` in `0.0..=1.0`.
fn _components(rgb: Rgb) -> (f64, f64, f64) {
    (f64::from(rgb.0) / 255.0, f64::from(rgb.1) / 255.0, f64::from(rgb.2) / 255.0)
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
        let mut out = Vec::new();
        buffer
            .render()
            .draw(|mem| -> Result<_, ()> {
                out = mem.to_vec();
                Ok(Vec::new())
            })
            .unwrap();
        out
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
        assert_eq!(menu.images.builds, 1, "same state: cached");
        menu.nav.key(MenuKey::Down);
        let images = menu.images(&theme, 1).expect("drawn again");
        assert_eq!(menu.images.builds, 2, "the selection moved");
        assert!(images.submenu.is_none());
        menu.nav.key(MenuKey::Down);
        menu.nav.key(MenuKey::Right);
        let images = menu.images(&theme, 1).expect("drawn with the submenu");
        assert_eq!(menu.images.builds, 3, "the submenu opened");
        assert!(images.submenu.is_some());
        menu.images(&theme, 2).expect("drawn at scale 2");
        assert_eq!(menu.images.builds, 4, "the scale changed");
    }

    #[test]
    fn a_bad_scale_is_an_error_and_nothing_is_cached() {
        let mut menu = menu();
        assert!(render_menu(&menu, &MenuTheme::default(), 0).is_err());
        assert!(menu.images(&MenuTheme::default(), 0).is_err());
        assert_eq!(menu.images.builds, 0);
        assert!(menu.images.entry.is_none());
    }
}
