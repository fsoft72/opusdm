//! Nested backend: the compositor in a winit window of the host session,
//! rendered with GLES, driven by the host pointer and keyboard.
//!
//! Ownership: the winit event source (input, resize, focus, close) and the
//! frame timer (`Frame`: renderer, damage tracker, cursor) are owned by the
//! calloop event loop of `run` for the whole run. A host window close or a
//! signal sets `State::quit`; the next frame tick stops the loop, `run`
//! returns, and dropping `LoopData` disconnects every client.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        egl::EGLDevice,
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputEvent, KeyState, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{
            damage::OutputDamageTracker,
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                AsRenderElements, Kind,
            },
            gles::GlesRenderer,
            ImportDma, ImportMemWl,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend, WinitInput},
    },
    delegate_dmabuf,
    desktop::{layer_map_for_output, utils::send_frames_surface_tree, LayerSurface, Window},
    input::pointer::{AxisFrame, CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    output::Mode,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopSignal,
        },
        winit::{dpi::PhysicalSize, window::Window as WinitWindow},
    },
    utils::{IsAlive, Logical, Physical, Point, Scale, Size, Transform},
    wayland::{
        compositor::with_states,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shell::wlr_layer::Layer,
    },
};

use super::{announce, housekeeping, new_output, setup_common, stop_on_quit, LoopData, REFRESH_MHZ};
use crate::args::Args;
use crate::draw::DRAW_SCALE;
use crate::geometry::Rect;
use crate::menu::{render::MenuImages, MenuLayout};
use crate::state::{State, LOG_TAG};
use crate::switcher::render::{switcher_rect, switcher_size};

/// Name of the nested output.
const OUTPUT_NAME: &str = "NESTED-1";

/// Title of the host window.
const WINDOW_TITLE: &str = "OpusDM compositor (nested)";

/// Period of the frame timer.
const TICK: Duration = Duration::from_millis(16);

/// Color of the output where nothing is drawn (RGBA).
const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

/// The same kind of frame error is logged at most once in this period.
const ERROR_LOG_PERIOD: Duration = Duration::from_secs(1);

/// The GL framebuffer of the window is upside down: the damage tracker
/// renders with this transform (as smallvil and anvil do), the output
/// advertised to the clients stays `Normal`.
const RENDER_TRANSFORM: Transform = Transform::Flipped180;

smithay::backend::renderer::element::render_elements! {
    /// What the nested backend draws: client surfaces, the window menu and
    /// the window switcher.
    NestedElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Menu=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Limits how often one kind of error is logged.
#[derive(Debug, Default)]
struct ErrorThrottle {
    /// Last time each kind was logged.
    last: HashMap<&'static str, Instant>,
}

impl ErrorThrottle {
    /// True when an error of `kind` at `now` must be logged: the first one,
    /// or the first one at least `ERROR_LOG_PERIOD` after the last logged.
    fn should_log(&mut self, kind: &'static str, now: Instant) -> bool {
        let due = self.last.get(kind).is_none_or(|t| now.saturating_duration_since(*t) >= ERROR_LOG_PERIOD);
        if due {
            self.last.insert(kind, now);
        }
        due
    }

    /// Logs `message` with `err_log!` unless `kind` was logged less than
    /// `ERROR_LOG_PERIOD` ago.
    fn log(&mut self, kind: &'static str, message: &str) {
        if !self.should_log(kind, Instant::now()) { return; }
        opusdm_ipc::err_log!(LOG_TAG, "{kind}: {message}");
    }
}

/// Cursor shown by the host window itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostCursor {
    /// A named cursor of the host theme.
    Named(CursorIcon),
    /// No host cursor (hidden, or a client surface is drawn instead).
    Hidden,
}

/// What the frame timer owns for the whole run.
struct Frame {
    /// The winit window with its GLES renderer.
    backend: WinitGraphicsBackend<GlesRenderer>,
    /// Damage tracker of the window framebuffer, rebuilt when the size changes.
    damage: OutputDamageTracker,
    /// Size `damage` was built for.
    damage_size: Size<i32, Physical>,
    /// Cursor last applied to the host window.
    host_cursor: Option<HostCursor>,
    /// Rate limit of the frame error logs.
    errors: ErrorThrottle,
    /// True once a frame was submitted: before the first swap the window
    /// surface has no buffer age (Mesa answers EGL BAD_SURFACE).
    presented: bool,
}

/// Runs the compositor in a window of the host session until the window is
/// closed, SIGTERM or SIGINT.
pub fn run(args: &Args) -> Result<(), String> {
    let attributes = WinitWindow::default_attributes()
        .with_inner_size(PhysicalSize::new(args.size.0, args.size.1))
        .with_title(WINDOW_TITLE)
        .with_visible(true);
    let (mut backend, winit_loop) = winit::init_from_attributes::<GlesRenderer>(attributes)
        .map_err(|e| format!("nested backend: cannot open the host window with GLES: {e}"))?;
    let size = backend.window_size();
    let common = setup_common(new_output(OUTPUT_NAME, "Nested", (size.w, size.h)))?;
    let mut event_loop = common.event_loop;
    let mut state = common.state;
    state.shm_state.update_formats(backend.renderer().shm_formats());
    let _dmabuf_global = _create_dmabuf_global(&mut backend, &mut state);
    let mut pressed_buttons: HashSet<u32> = HashSet::new();
    event_loop
        .handle()
        .insert_source(winit_loop, move |event, _, data| _on_winit_event(event, data, &mut pressed_buttons))
        .map_err(|e| format!("winit source: {e}"))?;
    let mut frame = Frame {
        damage: OutputDamageTracker::new(size, f64::from(DRAW_SCALE), RENDER_TRANSFORM),
        damage_size: size,
        backend,
        host_cursor: None,
        errors: ErrorThrottle::default(),
        presented: false,
    };
    let signal = event_loop.get_signal();
    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, data| _tick(&mut frame, data, &signal))
        .map_err(|e| format!("frame timer: {e}"))?;
    announce(&mut state, args, &common.socket_name)?;
    let mut data = LoopData { state, display: common.display };
    event_loop.run(None, &mut data, |_| {}).map_err(|e| format!("event loop: {e}"))?;
    opusdm_ipc::debug_log!(LOG_TAG, "nested loop ended, disconnecting the clients");
    Ok(())
}

/// Creates the `linux-dmabuf` global (version 4, default feedback on the
/// render node of the EGL display) with the renderer's dmabuf formats, so
/// GL clients (GTK4) can hand over GPU buffers. A software renderer
/// (llvmpipe) or an EGL display without a known render node gets no
/// global: Mesa llvmpipe fails to map client dmabufs ("dmabuf import failed
/// to mmap") and then crashes the compositor while sampling them; the
/// clients use shared memory instead. Logged once.
fn _create_dmabuf_global(backend: &mut WinitGraphicsBackend<GlesRenderer>, state: &mut State) -> Option<DmabufGlobal> {
    let device = match _hardware_render_device(backend) {
        Ok(device) => device,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "linux-dmabuf not offered ({e}): clients fall back to shared memory");
            return None;
        }
    };
    let formats = backend.renderer().dmabuf_formats();
    let feedback = match DmabufFeedbackBuilder::new(device, formats).build() {
        Ok(feedback) => feedback,
        Err(e) => {
            opusdm_ipc::err_log!(LOG_TAG, "linux-dmabuf not offered (feedback: {e}): clients fall back to shared memory");
            return None;
        }
    };
    opusdm_ipc::debug_log!(LOG_TAG, "linux-dmabuf v4, main device {device}");
    let display = state.display_handle.clone();
    Some(state.dmabuf_state.create_global_with_default_feedback::<State>(&display, &feedback))
}

/// The `dev_t` of the render node (or else the DRM device) of the EGL
/// display of the window; an error for a software device.
fn _hardware_render_device(backend: &mut WinitGraphicsBackend<GlesRenderer>) -> Result<u64, String> {
    let device = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .map_err(|e| format!("EGL device: {e}"))?;
    if device.is_software() { return Err("the EGL device is a software renderer".into()); }
    let path = device
        .render_device_path()
        .or_else(|_| device.drm_device_path())
        .map_err(|e| format!("device path: {e}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(meta.rdev())
}

impl DmabufHandler for State {
    /// The dmabuf state.
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// Queues the dmabuf: the renderer lives in the frame timer, which
    /// imports it and answers the notifier on its next frame.
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        self.pending_dmabufs.push((dmabuf, notifier));
    }
}

delegate_dmabuf!(State);

/// Handles one winit event: input goes to the seat, a resize changes the
/// output mode, losing the focus releases what is held, a close quits.
fn _on_winit_event(event: WinitEvent, data: &mut LoopData, pressed_buttons: &mut HashSet<u32>) {
    let state = &mut data.state;
    match event {
        WinitEvent::Resized { size, .. } => _resize_output(state, size),
        WinitEvent::Focus(true) => opusdm_ipc::debug_log!(LOG_TAG, "host window focused"),
        WinitEvent::Focus(false) => _release_held(state, pressed_buttons),
        WinitEvent::Input(input) => _on_input(state, input, pressed_buttons),
        WinitEvent::CloseRequested => {
            opusdm_ipc::debug_log!(LOG_TAG, "host window closed, quitting");
            state.quit.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        WinitEvent::Redraw => {}
    }
}

/// The host window is now `size`: the output takes that mode, the layer
/// map is arranged again and maximized / fullscreen windows follow.
fn _resize_output(state: &mut State, size: Size<i32, Physical>) {
    if size.w <= 0 || size.h <= 0 {
        opusdm_ipc::debug_log!(LOG_TAG, "host window resized to {size:?}, ignored");
        return;
    }
    let before = state.usable_area();
    let mode = Mode { size, refresh: REFRESH_MHZ };
    state.output.change_current_state(Some(mode), None, None, None);
    state.output.set_preferred(mode);
    layer_map_for_output(&state.output).arrange();
    opusdm_ipc::debug_log!(LOG_TAG, "output {OUTPUT_NAME} resized to {}x{}", size.w, size.h);
    state.output_area_changed(before);
}

/// The host window lost the focus: the host will not send the releases of
/// what is held now, so every pressed button and key is released here (an
/// Alt + drag grab ends, a client does not autorepeat forever).
fn _release_held(state: &mut State, pressed_buttons: &mut HashSet<u32>) {
    let time = _now_msec(state);
    for button in pressed_buttons.drain() {
        opusdm_ipc::debug_log!(LOG_TAG, "host focus lost: releasing button {button:#x}");
        state.pointer_button(button, false, time);
    }
    let Some(keyboard) = state.seat.get_keyboard() else {
        opusdm_ipc::err_log!(LOG_TAG, "host focus lost: the seat has no keyboard, keys not released");
        return;
    };
    for key in keyboard.pressed_keys() {
        opusdm_ipc::debug_log!(LOG_TAG, "host focus lost: releasing key {}", key.raw());
        state.keyboard_key(key, false, time);
    }
}

/// Milliseconds since the compositor started, for synthesized events.
fn _now_msec(state: &State) -> u32 {
    u32::try_from(state.start_time.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Maps one winit input event to the backend independent input functions.
fn _on_input(state: &mut State, input: InputEvent<WinitInput>, pressed_buttons: &mut HashSet<u32>) {
    match input {
        InputEvent::Keyboard { event } => {
            state.keyboard_key(event.key_code(), event.state() == KeyState::Pressed, event.time_msec());
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let area = state.output_area();
            let local = event.position_transformed((area.w, area.h).into());
            let position = Point::<f64, Logical>::from((f64::from(area.x), f64::from(area.y))) + local;
            state.pointer_motion(position, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            let button = event.button_code();
            let pressed = event.state() == ButtonState::Pressed;
            if pressed {
                pressed_buttons.insert(button);
            } else {
                pressed_buttons.remove(&button);
            }
            state.pointer_button(button, pressed, event.time_msec());
        }
        InputEvent::PointerAxis { event } => state.pointer_axis(_axis_frame(&event)),
        InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } => {}
        other => opusdm_ipc::debug_log!(LOG_TAG, "winit input ignored: {other:?}"),
    }
}

/// Builds the scroll frame of a winit wheel event (as anvil).
fn _axis_frame(event: &impl PointerAxisEvent<WinitInput>) -> AxisFrame {
    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let discrete = event.amount_v120(axis);
        let amount = event.amount(axis).unwrap_or_else(|| discrete.unwrap_or(0.0) * 15.0 / 120.0);
        if amount == 0.0 { continue; }
        frame = frame.relative_direction(axis, event.relative_direction(axis)).value(axis, amount);
        if let Some(discrete) = discrete {
            frame = frame.v120(axis, discrete as i32);
        }
    }
    if event.source() == AxisSource::Finger {
        for axis in [Axis::Horizontal, Axis::Vertical] {
            if event.amount(axis) == Some(0.0) {
                frame = frame.stop(axis);
            }
        }
    }
    frame
}

/// One frame tick: stops the loop on a quit request, else answers the
/// pending dmabuf imports, renders, sends the frame callbacks and does the
/// shared housekeeping.
fn _tick(frame: &mut Frame, data: &mut LoopData, signal: &LoopSignal) -> TimeoutAction {
    if stop_on_quit(data, signal) { return TimeoutAction::Drop; }
    _import_dmabufs(frame, &mut data.state);
    _render(frame, &mut data.state);
    data.state.send_frames();
    _send_cursor_frame(&data.state);
    housekeeping(data);
    TimeoutAction::ToDuration(TICK)
}

/// Imports the dmabufs queued by `dmabuf_imported` and answers each notifier.
fn _import_dmabufs(frame: &mut Frame, state: &mut State) {
    for (dmabuf, notifier) in std::mem::take(&mut state.pending_dmabufs) {
        if let Err(e) = frame.backend.renderer().import_dmabuf(&dmabuf, None) {
            opusdm_ipc::err_log!(LOG_TAG, "dmabuf import failed: {e}");
            notifier.failed();
            continue;
        }
        if let Err(e) = notifier.successful::<State>() {
            opusdm_ipc::err_log!(LOG_TAG, "dmabuf import: the client buffer is gone: {e}");
        }
    }
}

/// Renders one frame into the host window: binds, renders the elements in
/// stacking order through the damage tracker and submits the damage. A
/// failure is logged (rate limited) and the frame is skipped.
fn _render(frame: &mut Frame, state: &mut State) {
    let host_cursor = _update_cursor(state);
    if frame.host_cursor != Some(host_cursor) {
        _apply_host_cursor(frame.backend.window(), host_cursor);
        frame.host_cursor = Some(host_cursor);
    }
    let size = frame.backend.window_size();
    if size != frame.damage_size {
        frame.damage = OutputDamageTracker::new(size, f64::from(DRAW_SCALE), RENDER_TRANSFORM);
        frame.damage_size = size;
    }
    let Frame { backend, damage, errors, presented, .. } = frame;
    let menu = _menu_images(state, errors);
    let switcher = _switcher_image(state, errors);
    // The buffer age can only be asked while the window surface is current.
    // Smithay's `bind` does not make it current (only rendering does), and
    // an import or a frame with no damage may leave the context on no
    // surface: then Mesa answers EGL BAD_SURFACE, logged by Smithay as an
    // ERROR. In that case the age is unknown, 0 (full redraw), without
    // asking.
    if let Err(e) = backend.bind() {
        return errors.log("frame bind", &e.to_string());
    }
    let current = backend.egl_surface().is_current();
    let age = if *presented && current { backend.buffer_age().unwrap_or(0) } else { 0 };
    let rendered = {
        let (renderer, mut framebuffer) = match backend.bind() {
            Ok(bound) => bound,
            Err(e) => return errors.log("frame bind", &e.to_string()),
        };
        let elements = _elements(renderer, state, menu.as_ref(), switcher.as_ref(), errors);
        match damage.render_output(renderer, &mut framebuffer, age, &elements, CLEAR_COLOR) {
            Ok(result) => result.damage.cloned(),
            Err(e) => return errors.log("frame render_output", &format!("{e:?}")),
        }
    };
    let Some(rects) = rendered else { return };
    match backend.submit(Some(&rects)) {
        Ok(()) => *presented = true,
        Err(e) => errors.log("frame submit", &e.to_string()),
    }
}

/// Resets a cursor surface that died to the default cursor and returns
/// what the host window must show.
fn _update_cursor(state: &mut State) -> HostCursor {
    if let CursorImageStatus::Surface(surface) = &state.cursor_status {
        if !surface.alive() {
            opusdm_ipc::debug_log!(LOG_TAG, "cursor surface is dead, default cursor");
            state.cursor_status = CursorImageStatus::default_named();
        }
    }
    match &state.cursor_status {
        CursorImageStatus::Named(icon) => HostCursor::Named(*icon),
        CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => HostCursor::Hidden,
    }
}

/// Shows `cursor` on the host window.
fn _apply_host_cursor(window: &WinitWindow, cursor: HostCursor) {
    match cursor {
        HostCursor::Named(icon) => {
            window.set_cursor(icon);
            window.set_cursor_visible(true);
        }
        HostCursor::Hidden => window.set_cursor_visible(false),
    }
}

/// The images and the layout of the open window menu, drawn again only
/// when its state changed (`WindowMenu::images`); `None` when no menu is
/// open or drawing failed (logged, rate limited: the menu stays open and
/// usable by keyboard, it is only not shown).
fn _menu_images(state: &mut State, errors: &mut ErrorThrottle) -> Option<(MenuImages, MenuLayout)> {
    let theme = state.menu_theme;
    let menu = state.menu.as_mut()?;
    match menu.images(&theme, DRAW_SCALE) {
        Ok(images) => Some((images, menu.layout())),
        Err(e) => {
            errors.log("window menu render", &e);
            None
        }
    }
}

/// The image of the open window switcher and its box on the output,
/// drawn again only when its rows, selection or scale changed
/// (`WindowSwitcher::images`); `None` when it is closed or drawing failed
/// (logged, rate limited: the switcher stays usable by keyboard, it is only
/// not shown).
fn _switcher_image(state: &mut State, errors: &mut ErrorThrottle) -> Option<(MemoryRenderBuffer, Rect)> {
    let theme = state.menu_theme;
    let rows = state.switcher_rows()?;
    let rect = switcher_rect(switcher_size(rows.len(), &theme), state.output_area());
    let switcher = state.switcher.as_mut()?;
    match switcher.image(rows, &theme, DRAW_SCALE) {
        Ok(image) => Some((image, rect)),
        Err(e) => {
            errors.log("window switcher render", &e);
            None
        }
    }
}

/// Render elements, front to back (the reverse of the drawing order
/// BACKGROUND, BOTTOM, windows bottom to top, TOP, the topmost window when
/// it is fullscreen, OVERLAY, the window menu, its submenu, the window
/// switcher, cursor). Popups come with their parent, just above it; the
/// order matches the hit test of `input.rs` (the open menu takes every
/// click before it; the switcher takes no click).
fn _elements(
    renderer: &mut GlesRenderer,
    state: &State,
    menu: Option<&(MenuImages, MenuLayout)>,
    switcher: Option<&(MemoryRenderBuffer, Rect)>,
    errors: &mut ErrorThrottle,
) -> Vec<NestedElement> {
    let output_loc = state.output_area();
    let origin = Point::<i32, Logical>::from((output_loc.x, output_loc.y));
    let mut elements = _cursor_elements(renderer, state, origin);
    if let Some((image, rect)) = switcher {
        elements.extend(_image_element(renderer, image, *rect, origin, "window switcher texture", errors));
    }
    if let Some((images, layout)) = menu {
        elements.extend(_menu_elements(renderer, images, layout, origin, errors));
    }
    let map = layer_map_for_output(&state.output);
    let layers = |layer: Layer| -> Vec<LayerSurface> { map.layers_on(layer).rev().cloned().collect() };
    let fullscreen = _fullscreen_top(state);
    for layer in layers(Layer::Overlay) {
        elements.extend(_layer_elements(renderer, &map, &layer));
    }
    if let Some(window) = &fullscreen {
        elements.extend(_window_elements(renderer, state, window, origin));
    }
    for layer in layers(Layer::Top) {
        elements.extend(_layer_elements(renderer, &map, &layer));
    }
    for window in state.space.elements().rev().filter(|w| Some(*w) != fullscreen.as_ref()) {
        elements.extend(_window_elements(renderer, state, window, origin));
    }
    for layer in layers(Layer::Bottom).into_iter().chain(layers(Layer::Background)) {
        elements.extend(_layer_elements(renderer, &map, &layer));
    }
    elements
}

/// Elements of the window menu, front to back: the open submenu, then the
/// menu, each at its box of `layout`. A texture upload failure is logged
/// (rate limited) and that image is left out of the frame.
fn _menu_elements(
    renderer: &mut GlesRenderer,
    images: &MenuImages,
    layout: &MenuLayout,
    origin: Point<i32, Logical>,
    errors: &mut ErrorThrottle,
) -> Vec<NestedElement> {
    let submenu = images.submenu.as_ref().zip(layout.submenu.as_ref().map(|(_, rect, _)| *rect));
    submenu
        .into_iter()
        .chain([(&images.menu, layout.menu)])
        .filter_map(|(buffer, rect)| _image_element(renderer, buffer, rect, origin, "window menu texture", errors))
        .collect()
}

/// The element of the compositor drawn image `buffer` at the box `rect`
/// (output coordinates, placed at `DRAW_SCALE`, the scale it was drawn
/// at); `None` when the texture upload fails (logged as `kind`, rate
/// limited: the image is left out of the frame).
fn _image_element(
    renderer: &mut GlesRenderer,
    buffer: &MemoryRenderBuffer,
    rect: Rect,
    origin: Point<i32, Logical>,
    kind: &'static str,
    errors: &mut ErrorThrottle,
) -> Option<NestedElement> {
    let loc = (Point::<i32, Logical>::from((rect.x, rect.y)) - origin).to_f64().to_physical(f64::from(DRAW_SCALE));
    match MemoryRenderBufferRenderElement::from_buffer(renderer, loc, buffer, None, None, None, Kind::Unspecified) {
        Ok(element) => Some(NestedElement::Menu(element)),
        Err(e) => {
            errors.log(kind, &e.to_string());
            None
        }
    }
}

/// The topmost window when it is fullscreen (drawn above TOP, as hit).
fn _fullscreen_top(state: &State) -> Option<Window> {
    let top = state.space.elements().next_back()?;
    let wl = top.toplevel()?.wl_surface();
    state.windows.get(wl).is_some_and(|i| i.fullscreen).then(|| top.clone())
}

/// Elements of `window` with its popups, at its place in the space.
fn _window_elements(
    renderer: &mut GlesRenderer,
    state: &State,
    window: &Window,
    origin: Point<i32, Logical>,
) -> Vec<NestedElement> {
    // Every element of the space has a location: no per-frame log for the impossible case.
    let Some(loc) = state.space.element_location(window) else { return Vec::new() };
    let render_loc = loc - window.geometry().loc - origin;
    AsRenderElements::<GlesRenderer>::render_elements(window, renderer, render_loc.to_physical(1), Scale::from(1.0), 1.0)
}

/// Elements of the layer surface `layer` with its popups; the surface
/// origin is its layer map geometry minus the bbox offset (as the hit test).
fn _layer_elements(
    renderer: &mut GlesRenderer,
    map: &smithay::desktop::LayerMap,
    layer: &LayerSurface,
) -> Vec<NestedElement> {
    // `layer` comes from this map, so it has a geometry: no per-frame log.
    let Some(geo) = map.layer_geometry(layer) else { return Vec::new() };
    let loc = geo.loc - layer.bbox().loc;
    AsRenderElements::<GlesRenderer>::render_elements(layer, renderer, loc.to_physical(1), Scale::from(1.0), 1.0)
}

/// Elements of the client cursor surface at the pointer, minus its hotspot;
/// none for a named or hidden cursor (the host draws a named one).
fn _cursor_elements(
    renderer: &mut GlesRenderer,
    state: &State,
    origin: Point<i32, Logical>,
) -> Vec<NestedElement> {
    let CursorImageStatus::Surface(surface) = &state.cursor_status else { return Vec::new() };
    // The seat always has a pointer (`add_input_devices`); the input paths log its absence.
    let Some(pointer) = state.seat.get_pointer() else { return Vec::new() };
    let hotspot = with_states(surface, |states| {
        states.data_map.get::<CursorImageSurfaceData>().and_then(|d| d.lock().ok().map(|a| a.hotspot))
    })
    .unwrap_or_default();
    let loc = pointer.current_location().to_i32_round() - hotspot - origin;
    render_elements_from_surface_tree(renderer, surface, loc.to_physical(1), Scale::from(1.0), 1.0, Kind::Cursor)
}

/// Sends the frame callbacks of a client cursor surface (animated cursors).
fn _send_cursor_frame(state: &State) {
    let CursorImageStatus::Surface(surface) = &state.cursor_status else { return };
    let output = &state.output;
    send_frames_surface_tree(surface, output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_error_of_a_kind_is_logged() {
        let mut t = ErrorThrottle::default();
        assert!(t.should_log("bind", Instant::now()));
    }

    #[test]
    fn the_same_error_is_logged_at_most_once_per_period() {
        let mut t = ErrorThrottle::default();
        let start = Instant::now();
        assert!(t.should_log("bind", start));
        assert!(!t.should_log("bind", start + Duration::from_millis(500)));
        assert!(t.should_log("bind", start + ERROR_LOG_PERIOD));
        assert!(!t.should_log("bind", start + ERROR_LOG_PERIOD + Duration::from_millis(10)));
    }

    #[test]
    fn each_kind_of_error_has_its_own_period() {
        let mut t = ErrorThrottle::default();
        let start = Instant::now();
        assert!(t.should_log("bind", start));
        assert!(t.should_log("submit", start));
        assert!(!t.should_log("bind", start));
    }
}
