//! The applet canvas: the drawing commands the `ctx` recorder of
//! `canvas.js` sends in an `Event::Redraw`, parsed into [`Command`]s and
//! replayed on a cairo context, Canvas 2D style.
//!
//! The applet's `draw(ctx)` runs in JavaScript, in a turn of its own
//! (`opus.ui.redraw` or `opus.ui.show`); the GTK draw function only replays
//! the last command list and never calls JavaScript.
//!
//! Failures never stop a replay: a bad colour keeps the previous style, a
//! missing image or a cairo error skips that command, and [`replay`] returns
//! an `Err` naming the first failure and how many there were. A command with
//! a NaN or infinite number is a no-op, as in browsers (JSON has no NaN: the
//! recorder sends `null`, parsed as NaN).
//!
//! Text uses cairo's "toy" text API: fontconfig picks the family, so an
//! unknown family falls back on a default font instead of failing.

use gtk4::{cairo, gdk};
use gdk::prelude::{TextureExt, TextureExtManual};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Most commands one redraw may send; a longer list is refused.
pub const MAX_COMMANDS: usize = 100_000;

/// Largest image `drawImage` loads, in pixels (width times height).
pub const MAX_IMAGE_PIXELS: i64 = 4096 * 4096;

/// The font of a new canvas, as in browsers.
const DEFAULT_FONT: &str = "10px sans-serif";

/// Every command name, for the "unknown command" error.
const COMMAND_NAMES: [&str; 27] = [
    "save", "restore", "translate", "rotate", "scale", "beginPath", "closePath", "moveTo", "lineTo", "arc", "rect", "fill", "stroke", "fillRect",
    "strokeRect", "clearRect", "fillText", "strokeText", "drawImage", "fillStyle", "strokeStyle", "lineWidth", "lineCap", "globalAlpha", "font",
    "textAlign", "textBaseline",
];

/// One drawing command, as JSON `{ "op": "<name>", ...fields }`. The names
/// and fields follow the Canvas 2D API (`fillRect` has `x`, `y`, `w`, `h`;
/// a style setter has `style` or `value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    Save,
    Restore,
    Translate {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
    },
    Rotate {
        #[serde(with = "num")]
        angle: f64,
    },
    Scale {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
    },
    BeginPath,
    ClosePath,
    MoveTo {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
    },
    LineTo {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
    },
    Arc {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(with = "num")]
        radius: f64,
        #[serde(with = "num")]
        start_angle: f64,
        #[serde(with = "num")]
        end_angle: f64,
        #[serde(default)]
        counterclockwise: bool,
    },
    Rect {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(with = "num")]
        w: f64,
        #[serde(with = "num")]
        h: f64,
    },
    Fill,
    Stroke,
    FillRect {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(with = "num")]
        w: f64,
        #[serde(with = "num")]
        h: f64,
    },
    StrokeRect {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(with = "num")]
        w: f64,
        #[serde(with = "num")]
        h: f64,
    },
    ClearRect {
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(with = "num")]
        w: f64,
        #[serde(with = "num")]
        h: f64,
    },
    FillText {
        text: String,
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(default, with = "opt_num", skip_serializing_if = "Option::is_none")]
        max_width: Option<f64>,
    },
    StrokeText {
        text: String,
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(default, with = "opt_num", skip_serializing_if = "Option::is_none")]
        max_width: Option<f64>,
    },
    /// `path` must be absolute (from `opus.applet.assetPath`); `w` and `h`
    /// scale the image, missing means its natural size.
    DrawImage {
        path: String,
        #[serde(with = "num")]
        x: f64,
        #[serde(with = "num")]
        y: f64,
        #[serde(default, with = "opt_num", skip_serializing_if = "Option::is_none")]
        w: Option<f64>,
        #[serde(default, with = "opt_num", skip_serializing_if = "Option::is_none")]
        h: Option<f64>,
    },
    FillStyle { style: Style },
    StrokeStyle { style: Style },
    LineWidth {
        #[serde(with = "num")]
        value: f64,
    },
    /// `butt`, `round` or `square`.
    LineCap { value: String },
    GlobalAlpha {
        #[serde(with = "num")]
        value: f64,
    },
    /// CSS shorthand subset `"[bold] [italic] <size>px <family>"`.
    Font { value: String },
    /// `left`, `right`, `center`, `start` or `end`.
    TextAlign { value: String },
    /// `top`, `middle`, `alphabetic`, `bottom` (`hanging` is drawn as
    /// `top`, `ideographic` as `bottom`).
    TextBaseline { value: String },
}

/// A fill or stroke style: a colour (see [`parse_color`]) or a gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Style {
    Color(String),
    Gradient(Gradient),
}

/// A gradient, as `createLinearGradient` / `createRadialGradient` made it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Gradient {
    Linear {
        #[serde(with = "num")]
        x0: f64,
        #[serde(with = "num")]
        y0: f64,
        #[serde(with = "num")]
        x1: f64,
        #[serde(with = "num")]
        y1: f64,
        #[serde(default)]
        stops: Vec<Stop>,
    },
    Radial {
        #[serde(with = "num")]
        x0: f64,
        #[serde(with = "num")]
        y0: f64,
        #[serde(with = "num")]
        r0: f64,
        #[serde(with = "num")]
        x1: f64,
        #[serde(with = "num")]
        y1: f64,
        #[serde(with = "num")]
        r1: f64,
        #[serde(default)]
        stops: Vec<Stop>,
    },
}

/// One `addColorStop(offset, color)` of a gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    #[serde(with = "num")]
    pub offset: f64,
    pub color: String,
}

/// A number that may arrive as `null` (the recorder's NaN or infinity).
mod num {
    use serde::{Deserialize, Deserializer, Serializer};

    /// A number, or NaN for `null`.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }

    /// The number (`serde_json` writes NaN and infinity as `null`).
    pub fn serialize<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(*value)
    }
}

/// An optional number: a missing field is `None`, `null` is `Some(NaN)`.
mod opt_num {
    use serde::{Deserialize, Deserializer, Serializer};

    /// `Some(number)`, `Some(NaN)` for `null` (missing is `None` by `default`).
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Ok(Some(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN)))
    }

    /// The number, or `null` for `None`.
    pub fn serialize<S: Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => s.serialize_f64(*v),
            None => s.serialize_none(),
        }
    }
}

/// Parse the `commands` of an `Event::Redraw`: an array of command objects,
/// at most [`MAX_COMMANDS`]. An unknown command or a bad field is an error
/// naming the command's position and name.
pub fn parse(value: &Value) -> Result<Vec<Command>, String> {
    let Some(list) = value.as_array() else { return Err(format!("canvas: the commands must be an array, not {value}")) };
    if list.len() > MAX_COMMANDS { return Err(format!("canvas: {} commands in one redraw, at most {MAX_COMMANDS} are allowed", list.len())) }
    let mut commands = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let Some(op) = item.get("op").and_then(Value::as_str) else { return Err(format!("canvas command {index}: no \"op\" name in {item}")) };
        if !COMMAND_NAMES.contains(&op) { return Err(format!("canvas command {index}: unknown command \"{op}\"")) }
        let command = Command::deserialize(item).map_err(|e| format!("canvas command {index} ({op}): {e}"))?;
        commands.push(command);
    }
    Ok(commands)
}

/// A loaded image, ready to paint.
struct Image {
    surface: cairo::ImageSurface,
    width: f64,
    height: f64,
}

/// The images of one canvas, by path. Images not drawn by the last replay
/// are dropped after it; a failed load is not kept.
#[derive(Default)]
pub struct Images {
    loaded: HashMap<String, Image>,
}

impl Images {
    /// How many images are kept.
    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    /// Whether no image is kept.
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }

    /// The image at `path`, loaded on first use.
    fn get(&mut self, path: &str) -> Result<&Image, String> {
        if !self.loaded.contains_key(path) {
            let image = load_image(path)?;
            self.loaded.insert(path.to_string(), image);
        }
        Ok(&self.loaded[path])
    }

    /// Drop every image whose path is not in `used`.
    fn keep_only(&mut self, used: &[&str]) {
        self.loaded.retain(|path, _| used.contains(&path.as_str()));
    }
}

/// Load the image file `path` (absolute) into a cairo surface.
fn load_image(path: &str) -> Result<Image, String> {
    if !Path::new(path).is_absolute() { return Err(format!("drawImage: the path must be absolute (use opus.applet.assetPath), not \"{path}\"")) }
    let texture = gdk::Texture::from_filename(path).map_err(|e| format!("drawImage: cannot load {path}: {e}"))?;
    let (width, height) = (texture.width(), texture.height());
    if i64::from(width) * i64::from(height) > MAX_IMAGE_PIXELS {
        return Err(format!("drawImage: {path} is {width}x{height}, larger than {MAX_IMAGE_PIXELS} pixels"));
    }
    let stride = cairo::Format::ARgb32.stride_for_width(width as u32).map_err(|e| format!("drawImage: {path}: {e}"))?;
    let mut data = vec![0u8; stride as usize * height as usize];
    // GDK downloads premultiplied ARGB32 in native order, cairo's own format.
    texture.download(&mut data, stride as usize);
    let surface = cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, width, height, stride).map_err(|e| format!("drawImage: {path}: {e}"))?;
    Ok(Image { surface, width: f64::from(width), height: f64::from(height) })
}

/// Replay `commands` on `cr` with a fresh image cache; see [`replay_with`].
pub fn replay(cr: &cairo::Context, commands: &[Command]) -> Result<(), String> {
    replay_with(cr, commands, &mut Images::default())
}

/// Replay `commands` on `cr`, the images from `images`. The context is left
/// as it was (the replay runs between a cairo save and restore). Every
/// command runs even when some fail; the `Err` names the first failure and
/// how many failed. More than [`MAX_COMMANDS`] commands paint nothing.
pub fn replay_with(cr: &cairo::Context, commands: &[Command], images: &mut Images) -> Result<(), String> {
    if commands.len() > MAX_COMMANDS { return Err(format!("canvas: {} commands in one redraw, at most {MAX_COMMANDS} are allowed", commands.len())) }
    cr.save().map_err(|e| format!("canvas: cairo cannot start the replay: {e}"))?;
    let mut player = Player { cr, images, states: vec![State::default()], used: Vec::new() };
    let mut failures = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        if let Err(e) = player.run(command) {
            failures.push((index, e));
        }
    }
    player.images.keep_only(&player.used);
    let open_saves = player.states.len() - 1;
    cr.new_path();
    // Undo the saves the applet left open, then the replay's own one.
    for _ in 0..open_saves {
        if let Err(e) = cr.restore() { failures.push((commands.len(), format!("cairo restore: {e}"))) }
    }
    if let Err(e) = cr.restore() { failures.push((commands.len(), format!("cairo restore: {e}"))) }
    let Some((index, first)) = failures.first() else { return Ok(()) };
    let name = commands.get(*index).map(command_name).unwrap_or("end of replay");
    match failures.len() {
        1 => Err(format!("canvas command {index} ({name}): {first}")),
        n => Err(format!("{n} of {} canvas commands failed; first: command {index} ({name}): {first}", commands.len())),
    }
}

/// The `op` name of `command`.
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Save => "save",
        Command::Restore => "restore",
        Command::Translate { .. } => "translate",
        Command::Rotate { .. } => "rotate",
        Command::Scale { .. } => "scale",
        Command::BeginPath => "beginPath",
        Command::ClosePath => "closePath",
        Command::MoveTo { .. } => "moveTo",
        Command::LineTo { .. } => "lineTo",
        Command::Arc { .. } => "arc",
        Command::Rect { .. } => "rect",
        Command::Fill => "fill",
        Command::Stroke => "stroke",
        Command::FillRect { .. } => "fillRect",
        Command::StrokeRect { .. } => "strokeRect",
        Command::ClearRect { .. } => "clearRect",
        Command::FillText { .. } => "fillText",
        Command::StrokeText { .. } => "strokeText",
        Command::DrawImage { .. } => "drawImage",
        Command::FillStyle { .. } => "fillStyle",
        Command::StrokeStyle { .. } => "strokeStyle",
        Command::LineWidth { .. } => "lineWidth",
        Command::LineCap { .. } => "lineCap",
        Command::GlobalAlpha { .. } => "globalAlpha",
        Command::Font { .. } => "font",
        Command::TextAlign { .. } => "textAlign",
        Command::TextBaseline { .. } => "textBaseline",
    }
}

/// A fill or stroke source.
#[derive(Clone)]
enum Paint {
    Color(f64, f64, f64, f64),
    Pattern(cairo::Pattern),
}

/// A parsed `font`.
#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub bold: bool,
    pub italic: bool,
    pub size: f64,
    pub family: String,
}

/// Horizontal text anchor.
#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
    Center,
}

/// Vertical text anchor.
#[derive(Clone, Copy)]
enum Baseline {
    Top,
    Middle,
    Alphabetic,
    Bottom,
}

/// The canvas state that `save` / `restore` keep and cairo does not (cairo
/// keeps the transform and the clip).
#[derive(Clone)]
struct State {
    fill: Paint,
    stroke: Paint,
    line_width: f64,
    line_cap: cairo::LineCap,
    alpha: f64,
    font: Font,
    align: Align,
    baseline: Baseline,
}

impl Default for State {
    /// The state of a new canvas: black, 1 px lines, opaque, 10px sans-serif.
    fn default() -> State {
        State {
            fill: Paint::Color(0.0, 0.0, 0.0, 1.0),
            stroke: Paint::Color(0.0, 0.0, 0.0, 1.0),
            line_width: 1.0,
            line_cap: cairo::LineCap::Butt,
            alpha: 1.0,
            font: parse_font(DEFAULT_FONT).expect("the default font parses"),
            align: Align::Left,
            baseline: Baseline::Alphabetic,
        }
    }
}

/// One replay in progress.
struct Player<'a> {
    cr: &'a cairo::Context,
    images: &'a mut Images,
    /// The state stack; the last one is current, never empty.
    states: Vec<State>,
    /// The image paths drawn so far (the cache keeps only these).
    used: Vec<&'a str>,
}

/// Whether every number is finite.
fn finite(numbers: &[f64]) -> bool {
    numbers.iter().all(|n| n.is_finite())
}

impl<'a> Player<'a> {
    /// The current state.
    fn state(&mut self) -> &mut State {
        self.states.last_mut().expect("the state stack is never empty")
    }

    /// Run one command. A command with a NaN or infinite number does
    /// nothing; the cairo status is checked after the others.
    fn run(&mut self, command: &'a Command) -> Result<(), String> {
        let cr = self.cr;
        match command {
            Command::Save => {
                cr.save().map_err(|e| e.to_string())?;
                let top = self.state().clone();
                self.states.push(top);
            }
            Command::Restore => {
                // A restore with no save is ignored, as in browsers.
                if self.states.len() < 2 { return Ok(()) }
                cr.restore().map_err(|e| e.to_string())?;
                self.states.pop();
            }
            Command::Translate { x, y } => {
                if !finite(&[*x, *y]) { return Ok(()) }
                cr.translate(*x, *y);
            }
            Command::Rotate { angle } => {
                if !finite(&[*angle]) { return Ok(()) }
                cr.rotate(*angle);
            }
            Command::Scale { x, y } => {
                if !finite(&[*x, *y]) { return Ok(()) }
                // A zero factor would put cairo in an error state for the rest of the replay.
                if *x == 0.0 || *y == 0.0 { return Err(format!("scale({x}, {y}): a zero factor is not supported")) }
                cr.scale(*x, *y);
            }
            Command::BeginPath => cr.new_path(),
            Command::ClosePath => cr.close_path(),
            Command::MoveTo { x, y } => {
                if !finite(&[*x, *y]) { return Ok(()) }
                cr.move_to(*x, *y);
            }
            Command::LineTo { x, y } => {
                if !finite(&[*x, *y]) { return Ok(()) }
                cr.line_to(*x, *y);
            }
            Command::Arc { x, y, radius, start_angle, end_angle, counterclockwise } => {
                if !finite(&[*x, *y, *radius, *start_angle, *end_angle]) { return Ok(()) }
                if *radius < 0.0 { return Err(format!("arc: the radius {radius} is negative")) }
                if *counterclockwise {
                    cr.arc_negative(*x, *y, *radius, *start_angle, *end_angle);
                } else {
                    cr.arc(*x, *y, *radius, *start_angle, *end_angle);
                }
            }
            Command::Rect { x, y, w, h } => {
                if !finite(&[*x, *y, *w, *h]) { return Ok(()) }
                cr.rectangle(*x, *y, *w, *h);
            }
            Command::Fill => {
                let state = self.state().clone();
                with_paint(cr, &state.fill, state.alpha, |cr| cr.fill_preserve()).map_err(cairo_text)?;
            }
            Command::Stroke => {
                let state = self.state().clone();
                self.stroke_current(&state).map_err(cairo_text)?;
            }
            Command::FillRect { x, y, w, h } => {
                if !finite(&[*x, *y, *w, *h]) { return Ok(()) }
                let state = self.state().clone();
                apart_from_path(cr, |cr| {
                    cr.rectangle(*x, *y, *w, *h);
                    with_paint(cr, &state.fill, state.alpha, |cr| cr.fill())
                })
                .map_err(cairo_text)?;
            }
            Command::StrokeRect { x, y, w, h } => {
                if !finite(&[*x, *y, *w, *h]) { return Ok(()) }
                let state = self.state().clone();
                apart_from_path(cr, |cr| {
                    cr.rectangle(*x, *y, *w, *h);
                    self.stroke_current(&state)
                })
                .map_err(cairo_text)?;
            }
            Command::ClearRect { x, y, w, h } => {
                if !finite(&[*x, *y, *w, *h]) { return Ok(()) }
                apart_from_path(cr, |cr| {
                    cr.save()?;
                    cr.set_operator(cairo::Operator::Clear);
                    cr.rectangle(*x, *y, *w, *h);
                    let filled = cr.fill();
                    cr.restore()?;
                    filled
                })
                .map_err(cairo_text)?;
            }
            Command::FillText { text, x, y, max_width } => {
                if !finite(&[*x, *y]) || !max_width.is_none_or(f64::is_finite) { return Ok(()) }
                let state = self.state().clone();
                self.text(&state, text, *x, *y, *max_width, false)?;
            }
            Command::StrokeText { text, x, y, max_width } => {
                if !finite(&[*x, *y]) || !max_width.is_none_or(f64::is_finite) { return Ok(()) }
                let state = self.state().clone();
                self.text(&state, text, *x, *y, *max_width, true)?;
            }
            Command::DrawImage { path, x, y, w, h } => {
                if !finite(&[*x, *y]) || !w.is_none_or(f64::is_finite) || !h.is_none_or(f64::is_finite) { return Ok(()) }
                self.used.push(path.as_str());
                let alpha = self.state().alpha;
                let image = self.images.get(path)?;
                let (w, h) = (w.unwrap_or(image.width), h.unwrap_or(image.height));
                if w == 0.0 || h == 0.0 { return Ok(()) }
                cr.save().map_err(|e| e.to_string())?;
                cr.translate(*x, *y);
                cr.scale(w / image.width, h / image.height);
                let painted = cr.set_source_surface(&image.surface, 0.0, 0.0).and_then(|_| cr.paint_with_alpha(alpha));
                cr.restore().map_err(|e| e.to_string())?;
                painted.map_err(|e| format!("drawImage {path}: {e}"))?;
            }
            Command::FillStyle { style } => {
                let paint = paint_of(style)?;
                self.state().fill = paint;
            }
            Command::StrokeStyle { style } => {
                let paint = paint_of(style)?;
                self.state().stroke = paint;
            }
            Command::LineWidth { value } => {
                // Zero, negative and non-finite widths are ignored, as in browsers.
                if !value.is_finite() || *value <= 0.0 { return Ok(()) }
                self.state().line_width = *value;
            }
            Command::LineCap { value } => {
                let cap = match value.as_str() {
                    "butt" => cairo::LineCap::Butt,
                    "round" => cairo::LineCap::Round,
                    "square" => cairo::LineCap::Square,
                    other => return Err(format!("lineCap must be butt, round or square, not \"{other}\"")),
                };
                self.state().line_cap = cap;
            }
            Command::GlobalAlpha { value } => {
                // Out of range and non-finite values are ignored, as in browsers.
                if !(0.0..=1.0).contains(value) { return Ok(()) }
                self.state().alpha = *value;
            }
            Command::Font { value } => {
                let Some(font) = parse_font(value) else { return Err(format!("font \"{value}\" is not \"[bold] [italic] <size>px <family>\"")) };
                self.state().font = font;
            }
            Command::TextAlign { value } => {
                let align = match value.as_str() {
                    "left" | "start" => Align::Left,
                    "right" | "end" => Align::Right,
                    "center" => Align::Center,
                    other => return Err(format!("textAlign must be left, right, center, start or end, not \"{other}\"")),
                };
                self.state().align = align;
            }
            Command::TextBaseline { value } => {
                let baseline = match value.as_str() {
                    "top" | "hanging" => Baseline::Top,
                    "middle" => Baseline::Middle,
                    "alphabetic" => Baseline::Alphabetic,
                    "bottom" | "ideographic" => Baseline::Bottom,
                    other => return Err(format!("textBaseline must be top, middle, alphabetic or bottom, not \"{other}\"")),
                };
                self.state().baseline = baseline;
            }
        }
        cr.status().map_err(|e| format!("cairo: {e}"))
    }

    /// Stroke the current path (kept) with the stroke style of `state`.
    fn stroke_current(&self, state: &State) -> Result<(), cairo::Error> {
        let cr = self.cr;
        cr.set_line_width(state.line_width);
        cr.set_line_cap(state.line_cap);
        with_paint(cr, &state.stroke, state.alpha, |cr| cr.stroke_preserve())
    }

    /// Fill or stroke `text` at (`x`, `y`) with the font and anchors of
    /// `state`, squeezed horizontally to `max_width` when wider. The
    /// current path is kept.
    fn text(&self, state: &State, text: &str, x: f64, y: f64, max_width: Option<f64>, stroke: bool) -> Result<(), String> {
        let cr = self.cr;
        if max_width.is_some_and(|w| w <= 0.0) { return Ok(()) }
        let font = &state.font;
        let slant = if font.italic { cairo::FontSlant::Italic } else { cairo::FontSlant::Normal };
        let weight = if font.bold { cairo::FontWeight::Bold } else { cairo::FontWeight::Normal };
        apart_from_path(cr, |cr| {
            cr.save()?;
            cr.select_font_face(&font.family, slant, weight);
            cr.set_font_size(font.size);
            let drawn = (|| {
                let width = cr.text_extents(text)?.x_advance();
                let extents = cr.font_extents()?;
                let dx = match state.align {
                    Align::Left => 0.0,
                    Align::Right => -width,
                    Align::Center => -width / 2.0,
                };
                let dy = match state.baseline {
                    Baseline::Top => extents.ascent(),
                    Baseline::Middle => (extents.ascent() - extents.descent()) / 2.0,
                    Baseline::Alphabetic => 0.0,
                    Baseline::Bottom => -extents.descent(),
                };
                cr.translate(x, y);
                if let Some(max) = max_width.filter(|max| width > *max) {
                    cr.scale(max / width, 1.0);
                }
                cr.move_to(dx, dy);
                cr.text_path(text);
                if stroke {
                    self.stroke_current(state)
                } else {
                    with_paint(cr, &state.fill, state.alpha, |cr| cr.fill())
                }
            })();
            cr.restore()?;
            drawn
        })
        .map_err(|e| format!("text: {e}"))
    }
}

/// The text of a cairo error.
fn cairo_text(error: cairo::Error) -> String {
    format!("cairo: {error}")
}

/// Run `draw` on an empty path, then put back the path built before (the
/// rectangle and text commands do not touch the canvas path).
fn apart_from_path<E: From<cairo::Error>>(cr: &cairo::Context, draw: impl FnOnce(&cairo::Context) -> Result<(), E>) -> Result<(), E> {
    let saved = cr.copy_path()?;
    cr.new_path();
    let drawn = draw(cr);
    cr.new_path();
    cr.append_path(&saved);
    drawn
}

/// Set `paint` as the source and run `draw`, with `alpha` applied: folded
/// into a colour, or through a group for a gradient.
fn with_paint(cr: &cairo::Context, paint: &Paint, alpha: f64, draw: impl FnOnce(&cairo::Context) -> Result<(), cairo::Error>) -> Result<(), cairo::Error> {
    match paint {
        Paint::Color(r, g, b, a) => {
            cr.set_source_rgba(*r, *g, *b, a * alpha);
            draw(cr)
        }
        Paint::Pattern(pattern) if alpha >= 1.0 => {
            cr.set_source(pattern)?;
            draw(cr)
        }
        Paint::Pattern(pattern) => {
            cr.push_group();
            let drawn = cr.set_source(pattern).and_then(|_| draw(cr));
            // Always pop, even after a failure, so the group stack stays balanced.
            cr.pop_group_to_source()?;
            drawn?;
            cr.paint_with_alpha(alpha)
        }
    }
}

/// The cairo source of `style`; a bad colour (also in a gradient stop) is
/// an error, and then the previous style is kept.
fn paint_of(style: &Style) -> Result<Paint, String> {
    match style {
        Style::Color(text) => {
            let Some((r, g, b, a)) = parse_color(text) else { return Err(format!("\"{text}\" is not a colour")) };
            Ok(Paint::Color(r, g, b, a))
        }
        Style::Gradient(Gradient::Linear { x0, y0, x1, y1, stops }) => {
            if !finite(&[*x0, *y0, *x1, *y1]) { return Err("a linear gradient has a non-finite coordinate".to_string()) }
            let gradient = cairo::LinearGradient::new(*x0, *y0, *x1, *y1);
            add_stops(&gradient, stops)?;
            Ok(Paint::Pattern(cairo::Pattern::clone(&gradient)))
        }
        Style::Gradient(Gradient::Radial { x0, y0, r0, x1, y1, r1, stops }) => {
            if !finite(&[*x0, *y0, *r0, *x1, *y1, *r1]) { return Err("a radial gradient has a non-finite coordinate".to_string()) }
            if *r0 < 0.0 || *r1 < 0.0 { return Err("a radial gradient has a negative radius".to_string()) }
            let gradient = cairo::RadialGradient::new(*x0, *y0, *r0, *x1, *y1, *r1);
            add_stops(&gradient, stops)?;
            Ok(Paint::Pattern(cairo::Pattern::clone(&gradient)))
        }
    }
}

/// Add the colour `stops` to `gradient`; a bad offset or colour is an error.
fn add_stops(gradient: &cairo::Gradient, stops: &[Stop]) -> Result<(), String> {
    for stop in stops {
        if !(0.0..=1.0).contains(&stop.offset) { return Err(format!("a gradient stop offset must be from 0 to 1, not {}", stop.offset)) }
        let Some((r, g, b, a)) = parse_color(&stop.color) else { return Err(format!("gradient stop: \"{}\" is not a colour", stop.color)) };
        gradient.add_color_stop_rgba(stop.offset, r, g, b, a);
    }
    Ok(())
}

/// The named colours [`parse_color`] knows, as `#rrggbb`.
const NAMED_COLORS: [(&str, &str); 16] = [
    ("black", "#000000"),
    ("white", "#ffffff"),
    ("red", "#ff0000"),
    ("green", "#008000"),
    ("lime", "#00ff00"),
    ("blue", "#0000ff"),
    ("yellow", "#ffff00"),
    ("cyan", "#00ffff"),
    ("magenta", "#ff00ff"),
    ("gray", "#808080"),
    ("grey", "#808080"),
    ("silver", "#c0c0c0"),
    ("orange", "#ffa500"),
    ("purple", "#800080"),
    ("navy", "#000080"),
    ("maroon", "#800000"),
];

/// A CSS colour as (r, g, b, a) from 0 to 1: `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa`, `rgb(r, g, b)`, `rgba(r, g, b, a)` (channels 0-255 and alpha
/// 0-1 clamped, as browsers do), `transparent` and a few names (black,
/// white, red, green, lime, blue, yellow, cyan, magenta, gray/grey, silver,
/// orange, purple, navy, maroon). Case and surrounding spaces are ignored.
pub fn parse_color(text: &str) -> Option<(f64, f64, f64, f64)> {
    let text = text.trim().to_ascii_lowercase();
    if text == "transparent" { return Some((0.0, 0.0, 0.0, 0.0)) }
    if let Some((_, hex)) = NAMED_COLORS.iter().find(|(name, _)| *name == text) {
        return parse_hex(&hex[1..]);
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(args) = text.strip_prefix("rgba(").and_then(|rest| rest.strip_suffix(')')) {
        return parse_rgb_args(args, true);
    }
    if let Some(args) = text.strip_prefix("rgb(").and_then(|rest| rest.strip_suffix(')')) {
        return parse_rgb_args(args, false);
    }
    None
}

/// `rgb`, `rgba`, `rrggbb` or `rrggbbaa` hex digits.
fn parse_hex(hex: &str) -> Option<(f64, f64, f64, f64)> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None }
    let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|d| f64::from(d * 17) / 255.0);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|d| f64::from(d) / 255.0);
    match hex.len() {
        3 => Some((digit(0)?, digit(1)?, digit(2)?, 1.0)),
        4 => Some((digit(0)?, digit(1)?, digit(2)?, digit(3)?)),
        6 => Some((pair(0)?, pair(2)?, pair(4)?, 1.0)),
        8 => Some((pair(0)?, pair(2)?, pair(4)?, pair(6)?)),
        _ => None,
    }
}

/// The arguments of `rgb(...)` (three) or `rgba(...)` (four).
fn parse_rgb_args(args: &str, with_alpha: bool) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = args.split(',').map(|part| part.trim().parse::<f64>().ok().filter(|n| n.is_finite())).collect::<Option<_>>()?;
    let channel = |n: f64| n.clamp(0.0, 255.0) / 255.0;
    match (parts.as_slice(), with_alpha) {
        ([r, g, b], false) => Some((channel(*r), channel(*g), channel(*b), 1.0)),
        ([r, g, b, a], true) => Some((channel(*r), channel(*g), channel(*b), a.clamp(0.0, 1.0))),
        _ => None,
    }
}

/// Parse the `font` subset `"[bold] [italic] <size>px <family>"` (also
/// `normal`, `oblique`, and a family list, of which the first is used; a
/// missing family is `sans-serif`). `None` when it does not fit.
pub fn parse_font(text: &str) -> Option<Font> {
    let mut font = Font { bold: false, italic: false, size: 0.0, family: String::new() };
    let mut rest = text.trim();
    loop {
        let (word, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        match word.to_ascii_lowercase().as_str() {
            "bold" => font.bold = true,
            "italic" | "oblique" => font.italic = true,
            "normal" => {}
            _ => break,
        }
        rest = tail.trim_start();
    }
    let (size, family) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let size: f64 = size.strip_suffix("px")?.parse().ok()?;
    if !size.is_finite() || size <= 0.0 { return None }
    font.size = size;
    let first = family.split(',').next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'').trim();
    font.family = if first.is_empty() { "sans-serif".to_string() } else { first.to_string() };
    Some(font)
}

#[cfg(test)]
mod tests;
