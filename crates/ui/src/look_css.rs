//! CSS for a theme's fonts and background pictures. Pure string building, no
//! GTK, so every rule is unit-testable. Values coming from a theme (font
//! names, image paths) are escaped here and never spliced in raw.

use opusdm_ipc::desktop::ImageFit;
use opusdm_ipc::look::{FontSpec, SurfaceImage};
use opusdm_ipc::Settings;
use std::path::Path;

/// CSS class carried by every requester / dialog window, so a theme picture
/// can target them (and only them).
pub const REQUESTER_CLASS: &str = "opus-requester";

/// Opacity of the file rows when the Lister has a background picture, so the
/// picture shows through without hiding the text.
pub const ROW_ALPHA_WITH_PICTURE: f32 = 0.6;

/// Largest font size a theme may request, in points.
const MAX_FONT_SIZE: u32 = 72;

/// Quote `s` as a CSS string: backslash and double quote are escaped and
/// control characters (newlines included) are dropped.
fn css_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A CSS `url()` for an absolute file path, percent-encoding every byte that
/// is not alphanumeric or one of `/ - _ . ~`.
fn file_url(path: &Path) -> String {
    let mut out = String::from("url(\"file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out.push_str("\")");
    out
}

/// The `background-*` declarations that realise `fit`.
fn fit_declarations(fit: ImageFit) -> &'static str {
    match fit {
        ImageFit::Fill => "background-repeat: no-repeat; background-position: center; background-size: cover;",
        ImageFit::Fit => "background-repeat: no-repeat; background-position: center; background-size: contain;",
        ImageFit::Center => "background-repeat: no-repeat; background-position: center; background-size: auto;",
        ImageFit::Tile => "background-repeat: repeat; background-position: top left; background-size: auto;",
    }
}

/// A rule setting the font family and size of `selector`, or an empty string
/// when `font` sets neither.
pub fn font_rule(selector: &str, font: &FontSpec) -> String {
    let mut decls = String::new();
    let family = font.family.trim();
    if !family.is_empty() {
        decls.push_str(&format!("font-family: {}; ", css_quote(family)));
    }
    if font.size > 0 {
        decls.push_str(&format!("font-size: {}pt; ", font.size.min(MAX_FONT_SIZE)));
    }
    if decls.is_empty() {
        return String::new();
    }
    format!("{selector} {{ {decls}}} ")
}

/// A rule painting `surface`'s picture on `selector`, or an empty string when
/// there is no picture, its path is not absolute, or the file is missing (the
/// last case is logged).
pub fn surface_rule(selector: &str, surface: &SurfaceImage) -> String {
    let Some(path) = surface.image_path.as_ref() else { return String::new() };
    if !path.is_absolute() {
        return String::new();
    }
    if !path.is_file() {
        opusdm_ipc::err_log!("ui", "background picture not found: {}", path.display());
        return String::new();
    }
    format!("{selector} {{ background-image: {}; {} }} ", file_url(path), fit_declarations(surface.fit))
}

/// The theme look shared by every OpusDM window: the UI font and the
/// requester picture. The Lister adds its list font and list picture on top
/// (see `opusdm-lister`'s `theme.rs`).
pub fn ui_look_css(settings: &Settings) -> String {
    let mut css = font_rule("window", &settings.fonts.ui);
    let picture = surface_rule(&format!("window.{REQUESTER_CLASS}"), &settings.surfaces.requester);
    if !picture.is_empty() {
        css.push_str(&picture);
        css.push_str(&format!(
            "window.{REQUESTER_CLASS} box, window.{REQUESTER_CLASS} grid {{ background-color: transparent; }} "
        ));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusdm_ipc::desktop::ImageFit;

    fn existing_image() -> std::path::PathBuf {
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        f.into_temp_path().keep().unwrap()
    }

    #[test]
    fn quoting_escapes_quotes_and_backslashes_and_drops_control_characters() {
        assert_eq!(css_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(css_quote("a\nb;}"), "\"ab;}\"");
    }

    #[test]
    fn a_hostile_family_name_stays_inside_its_string() {
        let css = font_rule("window", &FontSpec { family: "x\"; } * { color: red".into(), size: 0 });
        assert_eq!(css, "window { font-family: \"x\\\"; } * { color: red\"; } ");
    }

    #[test]
    fn empty_font_emits_nothing_and_size_is_clamped() {
        assert_eq!(font_rule("window", &FontSpec::default()), "");
        assert_eq!(font_rule("window", &FontSpec { family: String::new(), size: 9999 }), "window { font-size: 72pt; } ");
        assert_eq!(font_rule("x", &FontSpec { family: " Fira ".into(), size: 11 }), "x { font-family: \"Fira\"; font-size: 11pt; } ");
    }

    #[test]
    fn file_url_percent_encodes_everything_unsafe() {
        assert_eq!(file_url(Path::new("/a b/c\"d).png")), "url(\"file:///a%20b/c%22d%29.png\")");
    }

    #[test]
    fn every_fit_maps_to_its_declarations() {
        let img = existing_image();
        let rule = |fit| surface_rule("s", &SurfaceImage { image_path: Some(img.clone()), fit });
        assert!(rule(ImageFit::Fill).contains("background-size: cover;"));
        assert!(rule(ImageFit::Fit).contains("background-size: contain;"));
        let center = rule(ImageFit::Center);
        assert!(center.contains("no-repeat") && center.contains("background-position: center;"));
        assert!(rule(ImageFit::Tile).contains("background-repeat: repeat;"));
    }

    #[test]
    fn missing_relative_or_absent_pictures_emit_nothing() {
        let none = SurfaceImage::default();
        assert_eq!(surface_rule("s", &none), "");
        let relative = SurfaceImage { image_path: Some("rel.png".into()), fit: ImageFit::Fill };
        assert_eq!(surface_rule("s", &relative), "");
        let missing = SurfaceImage { image_path: Some("/no/such/file.png".into()), fit: ImageFit::Fill };
        assert_eq!(surface_rule("s", &missing), "");
    }

    #[test]
    fn requester_picture_makes_inner_boxes_transparent() {
        let mut settings = Settings::default();
        settings.surfaces.requester = SurfaceImage { image_path: Some(existing_image()), fit: ImageFit::Center };
        let css = ui_look_css(&settings);
        assert!(css.contains("window.opus-requester {"));
        assert!(css.contains("window.opus-requester box"));
        assert!(css.contains("background-color: transparent;"));
    }

    #[test]
    fn default_settings_emit_no_look_css() {
        assert_eq!(ui_look_css(&Settings::default()), "");
    }
}
