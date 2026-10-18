//! The look a theme gives the hub: the paint of the bar, the picture of the
//! start menu button and the font of the start menu once opened. The look is
//! kept per process; every bar (one per monitor, rebuilt on hotplug) reads it
//! when built, and [`set_hub_look`] restyles the bars that are alive.

use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ipc::look::{HubBar, HubBarKind, HubLook};
use std::cell::RefCell;

/// CSS class of the start menu popover, the target of the menu font.
pub const MENU_CLASS: &str = "opusdm-appmenu";

/// Pixel size of the start menu button picture.
pub const MENU_ICON_SIZE: i32 = 18;

// Embedded at compile-time so the binary has zero runtime file dependencies.
const BALL_PNG: &[u8] = include_bytes!("../../../assets/ball.png");

/// The stylesheet of the look, over the panel stylesheet (APPLICATION + 1).
const PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1;

thread_local! {
    /// The look now in force.
    static CURRENT: RefCell<HubLook> = RefCell::new(HubLook::default());
    /// The stylesheet carrying the look, added to the display on first use.
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    /// The start menu button pictures of the bars that are alive.
    static MENU_IMAGES: RefCell<Vec<glib::WeakRef<gtk::Image>>> = const { RefCell::new(Vec::new()) };
}

/// The CSS rule that paints the bar. Colours are checked again here, the CSS
/// is built from theme values.
pub fn bar_css(bar: &HubBar) -> String {
    let bar = bar.clone().sanitized();
    match bar.kind {
        HubBarKind::Solid => format!(".opusdm-panel {{ background: {}; }}\n", bar.color),
        HubBarKind::Gradient => {
            format!(".opusdm-panel {{ background: linear-gradient({}deg, {}, {}); }}\n", bar.angle, bar.start, bar.end)
        }
        HubBarKind::Image => {
            let picture = opusdm_ui::surface_rule(".opusdm-panel", &bar.image);
            format!(".opusdm-panel {{ background-color: {}; }}\n{picture}\n", opusdm_ipc::desktop::DEFAULT_SOLID)
        }
    }
}

/// The whole stylesheet of `look`: empty when the theme sets nothing, which
/// leaves the built-in panel look.
pub fn look_css(look: &HubLook) -> String {
    let mut css = String::new();
    if let Some(bar) = &look.bar {
        css.push_str(&bar_css(bar));
    }
    if let Some(font) = &look.menu_font {
        css.push_str(&opusdm_ui::font_rule(&format!(".{MENU_CLASS}, .{MENU_CLASS} *"), font));
    }
    css
}

/// The built-in start menu picture.
fn ball_texture() -> gdk::Texture {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(BALL_PNG)).expect("valid ball.png asset")
}

/// The start menu picture of `look`: its icon file, or the built-in ball when
/// the theme sets none or the file cannot be loaded (logged).
fn menu_texture(look: &HubLook) -> gdk::Texture {
    let Some(path) = &look.menu_icon else { return ball_texture() };
    match gdk::Texture::from_filename(path) {
        Ok(texture) => texture,
        Err(e) => {
            opusdm_ipc::err_log!("opusdm-panel", "cannot load the start menu icon {}: {e}", path.display());
            ball_texture()
        }
    }
}

/// A new start menu button picture showing the look now in force. It is kept
/// in step with later [`set_hub_look`] calls until it is dropped.
pub fn new_menu_image() -> gtk::Image {
    let image = CURRENT.with(|c| gtk::Image::from_paintable(Some(&menu_texture(&c.borrow()))));
    image.set_pixel_size(MENU_ICON_SIZE);
    MENU_IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        images.retain(|w| w.upgrade().is_some());
        images.push(image.downgrade());
    });
    image
}

/// Adopt `look`: restyle the bars and the start menu, swap the start menu
/// picture of the bars that are alive. Does nothing for an unchanged look.
pub fn set_hub_look(look: &HubLook) {
    if CURRENT.with(|c| *c.borrow() == *look) && PROVIDER.with(|p| p.borrow().is_some()) {
        return;
    }
    let icon_changed = CURRENT.with(|c| c.borrow().menu_icon != look.menu_icon);
    CURRENT.with(|c| *c.borrow_mut() = look.clone());
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_none() {
            let Some(display) = gdk::Display::default() else {
                opusdm_ipc::err_log!("opusdm-panel", "no display: the hub look is not applied");
                return;
            };
            let provider = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(&display, &provider, PRIORITY);
            *p = Some(provider);
        }
        if let Some(provider) = p.as_ref() {
            provider.load_from_data(&look_css(look));
        }
    });
    if !icon_changed {
        return;
    }
    let texture = menu_texture(look);
    MENU_IMAGES.with(|images| {
        for image in images.borrow().iter().filter_map(|w| w.upgrade()) {
            image.set_paintable(Some(&texture));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::look::{FontSpec, SurfaceImage};

    #[test]
    fn a_look_that_sets_nothing_has_no_css() {
        assert_eq!(look_css(&HubLook::default()), "");
    }

    #[test]
    fn solid_and_gradient_bars_become_a_background() {
        let solid = HubBar { color: "#102030".into(), ..HubBar::default() };
        assert!(bar_css(&solid).contains("background: #102030;"));
        let gradient = HubBar { kind: HubBarKind::Gradient, start: "#000000".into(), end: "#ffffff".into(), angle: 90, ..HubBar::default() };
        assert!(bar_css(&gradient).contains("linear-gradient(90deg, #000000, #ffffff)"));
    }

    #[test]
    fn a_bad_colour_never_reaches_the_stylesheet() {
        let bar = HubBar { color: "red; } * { display: none".into(), ..HubBar::default() };
        assert!(!bar_css(&bar).contains("display"));
    }

    #[test]
    fn a_picture_bar_without_a_picture_keeps_the_default_colour() {
        let bar = HubBar { kind: HubBarKind::Image, image: SurfaceImage::default(), ..HubBar::default() };
        let css = bar_css(&bar);
        assert!(css.contains("background-color: #2e3436;") && !css.contains("url("));
    }

    #[test]
    fn the_menu_font_targets_the_menu_popover_only() {
        let look = HubLook { menu_font: Some(FontSpec { family: "Cantarell".into(), size: 13 }), ..HubLook::default() };
        let css = look_css(&look);
        assert!(css.contains(".opusdm-appmenu *") && css.contains("font-size: 13pt") && css.contains("\"Cantarell\""));
        assert!(!css.contains(".opusdm-panel"));
    }
}
