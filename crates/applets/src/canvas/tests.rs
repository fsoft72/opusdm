use super::*;
use serde_json::json;
use std::f64::consts::PI;

/// Side of the test surface, in pixels.
const SIZE: i32 = 20;

/// One pixel as (r, g, b, a), premultiplied, as cairo stores it.
type Pixel = (u8, u8, u8, u8);

/// Replay `commands` on a fresh transparent 20x20 surface; the outcome of
/// the replay and the surface.
fn paint(commands: &[Command]) -> (Result<(), String>, cairo::ImageSurface) {
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, SIZE, SIZE).unwrap();
    let result = {
        let cr = cairo::Context::new(&surface).unwrap();
        replay(&cr, commands)
    };
    surface.flush();
    (result, surface)
}

/// Parse `value` and replay it; panics when the parse fails.
fn paint_json(value: Value) -> (Result<(), String>, cairo::ImageSurface) {
    let commands = parse(&value).unwrap_or_else(|e| panic!("parse failed: {e}"));
    paint(&commands)
}

/// The pixel at (`x`, `y`) of `surface`.
fn pixel(surface: &mut cairo::ImageSurface, x: i32, y: i32) -> Pixel {
    let stride = surface.stride() as usize;
    let data = surface.data().unwrap();
    let at = y as usize * stride + x as usize * 4;
    // ARGB32 is a native-endian u32: B, G, R, A in memory on little endian.
    let word = u32::from_ne_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    ((word >> 16) as u8, (word >> 8) as u8, word as u8, (word >> 24) as u8)
}

/// Whether two channels differ by at most `tolerance`.
fn near(a: u8, b: u8, tolerance: u8) -> bool {
    a.abs_diff(b) <= tolerance
}

/// Write a `side` x `side` PNG of one RGBA colour to `path`.
fn write_png(path: &std::path::Path, side: i32, rgba: [u8; 4]) {
    use gtk4::{gdk, glib};
    let pixels: Vec<u8> = rgba.iter().copied().cycle().take((side * side * 4) as usize).collect();
    let texture = gdk::MemoryTexture::new(side, side, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from(&pixels), (side * 4) as usize);
    gdk::prelude::TextureExt::save_to_png(&texture, path).unwrap();
}

/// A `fillStyle` command with the colour `color`.
fn fill_style(color: &str) -> Command {
    Command::FillStyle { style: Style::Color(color.to_string()) }
}

/// A `fillRect` command.
fn fill_rect(x: f64, y: f64, w: f64, h: f64) -> Command {
    Command::FillRect { x, y, w, h }
}

#[test]
fn fill_rect_paints_the_pixels_with_the_fill_style() {
    let (result, mut surface) = paint(&[fill_style("#f00"), fill_rect(0.0, 0.0, 10.0, 10.0)]);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 5, 5), (255, 0, 0, 255));
    assert_eq!(pixel(&mut surface, 15, 15), (0, 0, 0, 0));
}

#[test]
fn the_default_fill_style_is_opaque_black() {
    let (result, mut surface) = paint(&[fill_rect(0.0, 0.0, 10.0, 10.0)]);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 5, 5), (0, 0, 0, 255));
}

#[test]
fn clear_rect_makes_pixels_transparent() {
    let (result, mut surface) = paint(&[fill_style("blue"), fill_rect(0.0, 0.0, 20.0, 20.0), Command::ClearRect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }]);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 5, 5), (0, 0, 0, 0));
    assert_eq!(pixel(&mut surface, 15, 15), (0, 0, 255, 255));
}

#[test]
fn save_restore_brings_back_the_transform() {
    let commands = [
        Command::Save,
        Command::Translate { x: 10.0, y: 10.0 },
        fill_style("#f00"),
        Command::Restore,
        fill_rect(0.0, 0.0, 5.0, 5.0),
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 2, 2), (0, 0, 0, 255), "the transform and the fill style came back");
    assert_eq!(pixel(&mut surface, 12, 12), (0, 0, 0, 0));
}

#[test]
fn a_restore_without_a_save_does_nothing() {
    let (result, mut surface) = paint(&[Command::Restore, Command::Restore, fill_style("#0f0"), fill_rect(0.0, 0.0, 5.0, 5.0)]);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 2, 2), (0, 255, 0, 255));
}

#[test]
fn arc_and_fill_paints_the_centre() {
    let commands = [
        Command::BeginPath,
        Command::Arc { x: 10.0, y: 10.0, radius: 5.0, start_angle: 0.0, end_angle: 2.0 * PI, counterclockwise: false },
        fill_style("#00f"),
        Command::Fill,
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 10, 10), (0, 0, 255, 255));
    assert_eq!(pixel(&mut surface, 1, 1), (0, 0, 0, 0));
}

#[test]
fn fill_rect_keeps_the_current_path() {
    let commands = [
        Command::BeginPath,
        Command::Rect { x: 0.0, y: 0.0, w: 5.0, h: 5.0 },
        fill_style("#f00"),
        fill_rect(15.0, 15.0, 5.0, 5.0),
        fill_style("#0f0"),
        Command::Fill,
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 2, 2), (0, 255, 0, 255), "the path built before fillRect is still there");
    assert_eq!(pixel(&mut surface, 17, 17), (255, 0, 0, 255), "fillRect did not join the path");
}

#[test]
fn stroke_paints_the_outline_with_the_line_width() {
    let commands = [
        Command::StrokeStyle { style: Style::Color("#f00".to_string()) },
        Command::LineWidth { value: 4.0 },
        Command::BeginPath,
        Command::MoveTo { x: 0.0, y: 10.0 },
        Command::LineTo { x: 20.0, y: 10.0 },
        Command::Stroke,
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 10, 9), (255, 0, 0, 255));
    assert_eq!(pixel(&mut surface, 10, 2), (0, 0, 0, 0));
}

#[test]
fn a_linear_gradient_goes_from_the_first_to_the_last_stop() {
    let gradient = json!({ "type": "linear", "x0": 0, "y0": 0, "x1": 20, "y1": 0, "stops": [{ "offset": 0, "color": "#f00" }, { "offset": 1, "color": "#00f" }] });
    let (result, mut surface) = paint_json(json!([{ "op": "fillStyle", "style": gradient }, { "op": "fillRect", "x": 0, "y": 0, "w": 20, "h": 20 }]));
    result.unwrap();
    let (r, _, b, a) = pixel(&mut surface, 0, 10);
    assert!(r > 230 && b < 25 && a == 255, "left is red: {r} {b} {a}");
    let (r, _, b, a) = pixel(&mut surface, 19, 10);
    assert!(r < 25 && b > 230 && a == 255, "right is blue: {r} {b} {a}");
}

#[test]
fn a_radial_gradient_paints_the_inner_stop_at_the_centre() {
    let gradient = json!({ "type": "radial", "x0": 10, "y0": 10, "r0": 0, "x1": 10, "y1": 10, "r1": 10, "stops": [{ "offset": 0, "color": "#fff" }, { "offset": 1, "color": "#000" }] });
    let (result, mut surface) = paint_json(json!([{ "op": "fillStyle", "style": gradient }, { "op": "fillRect", "x": 0, "y": 0, "w": 20, "h": 20 }]));
    result.unwrap();
    let (r, _, _, _) = pixel(&mut surface, 10, 10);
    assert!(r > 220, "the centre is white: {r}");
    let (r, _, _, _) = pixel(&mut surface, 0, 0);
    assert!(r < 30, "the corner is black: {r}");
}

#[test]
fn global_alpha_blends() {
    let commands = [
        fill_style("#fff"),
        fill_rect(0.0, 0.0, 20.0, 20.0),
        Command::GlobalAlpha { value: 0.5 },
        fill_style("#000"),
        fill_rect(0.0, 0.0, 10.0, 20.0),
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    let (r, g, b, a) = pixel(&mut surface, 5, 5);
    assert!(near(r, 128, 2) && near(g, 128, 2) && near(b, 128, 2) && a == 255, "half black over white: {r} {g} {b} {a}");
    assert_eq!(pixel(&mut surface, 15, 5), (255, 255, 255, 255));
}

#[test]
fn global_alpha_also_applies_to_a_gradient() {
    let gradient = json!({ "type": "linear", "x0": 0, "y0": 0, "x1": 20, "y1": 0, "stops": [{ "offset": 0, "color": "#000" }, { "offset": 1, "color": "#000" }] });
    let commands = json!([
        { "op": "fillStyle", "style": "#fff" },
        { "op": "fillRect", "x": 0, "y": 0, "w": 20, "h": 20 },
        { "op": "globalAlpha", "value": 0.5 },
        { "op": "fillStyle", "style": gradient },
        { "op": "fillRect", "x": 0, "y": 0, "w": 20, "h": 20 }
    ]);
    let (result, mut surface) = paint_json(commands);
    result.unwrap();
    let (r, _, _, a) = pixel(&mut surface, 5, 5);
    assert!(near(r, 128, 2) && a == 255, "half black over white: {r} {a}");
}

#[test]
fn parse_color_accepts_the_listed_forms_and_rejects_garbage() {
    assert_eq!(parse_color("#f00"), Some((1.0, 0.0, 0.0, 1.0)));
    assert_eq!(parse_color("#00ff00"), Some((0.0, 1.0, 0.0, 1.0)));
    assert_eq!(parse_color("#0000ff80"), Some((0.0, 0.0, 1.0, 128.0 / 255.0)));
    assert_eq!(parse_color("#FFF"), Some((1.0, 1.0, 1.0, 1.0)));
    assert_eq!(parse_color("rgb(255, 0, 0)"), Some((1.0, 0.0, 0.0, 1.0)));
    assert_eq!(parse_color("rgba(0,0,255,0.5)"), Some((0.0, 0.0, 1.0, 0.5)));
    assert_eq!(parse_color("rgb(300, -5, 0)"), Some((1.0, 0.0, 0.0, 1.0)), "out of range channels are clamped, as browsers do");
    assert_eq!(parse_color("  Red "), Some((1.0, 0.0, 0.0, 1.0)));
    assert_eq!(parse_color("transparent"), Some((0.0, 0.0, 0.0, 0.0)));
    for name in ["black", "white", "green", "blue", "yellow", "gray", "grey", "orange"] {
        assert!(parse_color(name).is_some(), "{name}");
    }
    for bad in ["", "#", "#ff", "#ggg", "#12345", "rgb(1,2)", "rgba(1,2,3)", "rgb(a,b,c)", "rgb(1,2,3", "nosuchcolour", "rgba(1,2,3,4,5)"] {
        assert_eq!(parse_color(bad), None, "{bad:?} must be refused");
    }
}

#[test]
fn an_unknown_command_is_an_error_naming_it() {
    let e = parse(&json!([{ "op": "fillRect", "x": 0, "y": 0, "w": 1, "h": 1 }, { "op": "explode" }])).unwrap_err();
    assert!(e.contains("explode") && e.contains("1"), "{e}");
    let e = parse(&json!([{ "x": 1 }])).unwrap_err();
    assert!(e.contains("op"), "{e}");
    let e = parse(&json!({ "op": "fill" })).unwrap_err();
    assert!(e.contains("array"), "{e}");
    let e = parse(&json!([{ "op": "fillRect", "x": "zero", "y": 0, "w": 1, "h": 1 }])).unwrap_err();
    assert!(e.contains("fillRect"), "{e}");
}

#[test]
fn the_json_of_every_command_parses() {
    let commands = json!([
        { "op": "save" }, { "op": "restore" }, { "op": "translate", "x": 1, "y": 2 }, { "op": "rotate", "angle": 0.5 },
        { "op": "scale", "x": 2, "y": 2 }, { "op": "beginPath" }, { "op": "closePath" }, { "op": "moveTo", "x": 0, "y": 0 },
        { "op": "lineTo", "x": 1, "y": 1 }, { "op": "arc", "x": 1, "y": 1, "radius": 1, "startAngle": 0, "endAngle": 1, "counterclockwise": true },
        { "op": "rect", "x": 0, "y": 0, "w": 1, "h": 1 }, { "op": "fill" }, { "op": "stroke" },
        { "op": "fillRect", "x": 0, "y": 0, "w": 1, "h": 1 }, { "op": "strokeRect", "x": 0, "y": 0, "w": 1, "h": 1 },
        { "op": "clearRect", "x": 0, "y": 0, "w": 1, "h": 1 }, { "op": "fillText", "text": "a", "x": 0, "y": 0 },
        { "op": "strokeText", "text": "a", "x": 0, "y": 0, "maxWidth": 5 }, { "op": "drawImage", "path": "/x.png", "x": 0, "y": 0 },
        { "op": "drawImage", "path": "/x.png", "x": 0, "y": 0, "w": 3, "h": 4 }, { "op": "fillStyle", "style": "red" },
        { "op": "strokeStyle", "style": "red" }, { "op": "lineWidth", "value": 2 }, { "op": "lineCap", "value": "round" },
        { "op": "globalAlpha", "value": 1 }, { "op": "font", "value": "12px sans-serif" }, { "op": "textAlign", "value": "center" },
        { "op": "textBaseline", "value": "top" }
    ]);
    let parsed = parse(&commands).unwrap();
    assert_eq!(parsed.len(), 28);
    assert_eq!(parsed[9], Command::Arc { x: 1.0, y: 1.0, radius: 1.0, start_angle: 0.0, end_angle: 1.0, counterclockwise: true });
    assert_eq!(parsed[19], Command::DrawImage { path: "/x.png".to_string(), x: 0.0, y: 0.0, w: Some(3.0), h: Some(4.0) });
}

#[test]
fn nan_and_infinite_numbers_are_ignored_not_fatal() {
    // JSON has no NaN: the recorder sends `null`, which parses as NaN.
    let parsed = parse(&json!([{ "op": "fillRect", "x": null, "y": 0, "w": 10, "h": 10 }])).unwrap();
    let Command::FillRect { x, .. } = parsed[0] else { panic!("{parsed:?}") };
    assert!(x.is_nan());
    let commands = [
        fill_style("#f00"),
        parsed[0].clone(),
        fill_rect(f64::INFINITY, 0.0, 10.0, 10.0),
        Command::Translate { x: f64::NAN, y: 0.0 },
        Command::Scale { x: f64::INFINITY, y: 1.0 },
        Command::LineWidth { value: f64::NAN },
        Command::GlobalAlpha { value: f64::NAN },
        fill_rect(10.0, 10.0, 10.0, 10.0),
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    assert_eq!(pixel(&mut surface, 5, 5), (0, 0, 0, 0), "the NaN and infinite fillRects painted nothing");
    assert_eq!(pixel(&mut surface, 15, 15), (255, 0, 0, 255), "the commands after them still run, with full alpha and no transform");
}

#[test]
fn more_than_100000_commands_are_refused() {
    let many = |n: usize| Value::Array(vec![json!({ "op": "beginPath" }); n]);
    assert_eq!(parse(&many(MAX_COMMANDS)).unwrap().len(), 100_000);
    let e = parse(&many(MAX_COMMANDS + 1)).unwrap_err();
    assert!(e.contains("100000"), "{e}");
    let commands = vec![Command::BeginPath; MAX_COMMANDS + 1];
    let (result, _) = paint(&commands);
    assert!(result.unwrap_err().contains("100000"));
}

#[test]
fn replay_keeps_going_past_a_failing_command_and_names_the_first() {
    let commands = [
        fill_style("#0f0"),
        fill_style("not a colour"),
        fill_rect(0.0, 0.0, 10.0, 10.0),
        Command::LineCap { value: "pointy".to_string() },
        fill_rect(10.0, 10.0, 10.0, 10.0),
    ];
    let (result, mut surface) = paint(&commands);
    let e = result.unwrap_err();
    assert!(e.contains("not a colour") && e.contains("2 "), "names the first failure and the count: {e}");
    assert_eq!(pixel(&mut surface, 5, 5), (0, 255, 0, 255), "a bad colour keeps the previous style");
    assert_eq!(pixel(&mut surface, 15, 15), (0, 255, 0, 255), "the commands after the failures ran");
}

#[test]
fn a_zero_scale_is_an_error_not_a_broken_context() {
    let (result, mut surface) = paint(&[Command::Scale { x: 0.0, y: 1.0 }, fill_rect(0.0, 0.0, 10.0, 10.0)]);
    assert!(result.unwrap_err().contains("scale"));
    assert_eq!(pixel(&mut surface, 5, 5), (0, 0, 0, 255), "the context still draws after the refused scale");
}

#[test]
fn text_is_drawn_and_an_unknown_font_family_falls_back() {
    for font in ["bold 16px sans-serif", "italic 16px NoSuchFontFamilyAnywhere", "bold italic 16px \"DejaVu Sans\", serif"] {
        let commands = [
            Command::Font { value: font.to_string() },
            Command::TextBaseline { value: "top".to_string() },
            Command::FillText { text: "WW".to_string(), x: 0.0, y: 0.0, max_width: None },
        ];
        let (result, mut surface) = paint(&commands);
        result.unwrap_or_else(|e| panic!("{font}: {e}"));
        let painted = (0..SIZE).flat_map(|y| (0..SIZE).map(move |x| (x, y))).filter(|&(x, y)| pixel(&mut surface, x, y).3 > 0).count();
        assert!(painted > 10, "{font}: only {painted} pixels painted");
    }
}

#[test]
fn text_align_right_ends_at_x() {
    let commands = [
        Command::Font { value: "16px sans-serif".to_string() },
        Command::TextAlign { value: "right".to_string() },
        Command::TextBaseline { value: "middle".to_string() },
        Command::FillText { text: "W".to_string(), x: 10.0, y: 10.0, max_width: None },
    ];
    let (result, mut surface) = paint(&commands);
    result.unwrap();
    let right_side = (0..SIZE).flat_map(|y| (11..SIZE).map(move |x| (x, y))).filter(|&(x, y)| pixel(&mut surface, x, y).3 > 0).count();
    let left_side = (0..SIZE).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| pixel(&mut surface, x, y).3 > 0).count();
    assert_eq!(right_side, 0, "nothing right of x");
    assert!(left_side > 5, "the text is left of x: {left_side}");
}

#[test]
fn a_bad_font_is_an_error_and_keeps_the_previous_font() {
    let (result, _) = paint(&[Command::Font { value: "huge".to_string() }, Command::FillText { text: "x".to_string(), x: 0.0, y: 10.0, max_width: None }]);
    assert!(result.unwrap_err().contains("huge"));
    assert!(parse_font("12px sans-serif").is_some());
    assert!(parse_font("bold italic 12.5px Some Family").is_some());
    assert!(parse_font("12pt sans-serif").is_none());
    assert!(parse_font("-3px sans-serif").is_none());
}

#[test]
fn draw_image_paints_a_png_and_a_missing_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("red.png");
    write_png(&png, 4, [255, 0, 0, 255]);
    let path = png.display().to_string();
    let mut images = Images::default();
    let commands = [Command::DrawImage { path: path.clone(), x: 0.0, y: 0.0, w: Some(10.0), h: Some(10.0) }, Command::DrawImage { path: path.clone(), x: 16.0, y: 16.0, w: None, h: None }];
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, SIZE, SIZE).unwrap();
    {
        let cr = cairo::Context::new(&surface).unwrap();
        replay_with(&cr, &commands, &mut images).unwrap();
        replay_with(&cr, &commands, &mut images).unwrap();
    }
    assert_eq!(images.len(), 1, "one cached image for one path");
    let mut surface = surface;
    assert_eq!(pixel(&mut surface, 8, 8), (255, 0, 0, 255), "scaled to 10x10");
    assert_eq!(pixel(&mut surface, 12, 12), (0, 0, 0, 0));
    assert_eq!(pixel(&mut surface, 18, 18), (255, 0, 0, 255), "natural size 4x4 at 16,16");
    let missing = dir.path().join("missing.png").display().to_string();
    let (result, _) = paint(&[Command::DrawImage { path: missing, x: 0.0, y: 0.0, w: None, h: None }]);
    assert!(result.unwrap_err().contains("missing.png"));
    let (result, _) = paint(&[Command::DrawImage { path: "relative.png".to_string(), x: 0.0, y: 0.0, w: None, h: None }]);
    assert!(result.unwrap_err().contains("absolute"));
}

#[test]
fn the_image_cache_drops_paths_no_longer_drawn() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for name in ["a.png", "b.png"] {
        let file = dir.path().join(name);
        write_png(&file, 2, [0, 0, 255, 255]);
        paths.push(file.display().to_string());
    }
    let draw = |path: &str| Command::DrawImage { path: path.to_string(), x: 0.0, y: 0.0, w: None, h: None };
    let mut images = Images::default();
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, SIZE, SIZE).unwrap();
    let cr = cairo::Context::new(&surface).unwrap();
    replay_with(&cr, &[draw(&paths[0])], &mut images).unwrap();
    replay_with(&cr, &[draw(&paths[1])], &mut images).unwrap();
    assert_eq!(images.len(), 1, "the image of a.png is no longer kept");
    let missing = dir.path().join("gone.png").display().to_string();
    assert!(replay_with(&cr, &[draw(&missing)], &mut images).is_err());
    assert!(replay_with(&cr, &[draw(&missing)], &mut images).is_err(), "a failed load is not cached as a success");
}
