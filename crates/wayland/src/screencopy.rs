//! Screenshots on Wayland through `wlr-screencopy` (labwc, sway and other
//! wlroots compositors): every output is copied into shared memory and
//! returned as opaque RGBA with its place in the compositor's layout.
//!
//! Each call opens a short-lived connection of its own: screenshots are
//! rare, and the copy must not wait behind the window watcher.

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// One output's picture and where the output sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputImage {
    /// Output name (`eDP-1`, `HEADLESS-1`...).
    pub name: String,
    /// Position of the output in the compositor's layout.
    pub x: i32,
    pub y: i32,
    /// Size of the picture in pixels.
    pub width: u32,
    pub height: u32,
    /// Opaque RGBA, `width * 4` bytes a row.
    pub rgba: Vec<u8>,
}

/// An output as the registry and `wl_output` describe it.
#[derive(Default)]
struct OutputInfo {
    proxy: Option<wl_output::WlOutput>,
    name: String,
    x: i32,
    y: i32,
}

/// What the frame of the copy in progress reported.
#[derive(Default)]
struct FrameState {
    /// `wl_shm` format, width, height and stride of the buffer to provide.
    buffer: Option<(WEnum<wl_shm::Format>, u32, u32, u32)>,
    /// All buffer types were announced (version 3), or version < 3.
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

/// The dispatch state of a capture.
#[derive(Default)]
struct State {
    outputs: Vec<OutputInfo>,
    frame: FrameState,
}

/// Copy every output. Fails when the compositor offers no screencopy, or a
/// copy fails or uses a pixel format other than 32 bit RGB.
pub fn capture_outputs() -> Result<Vec<OutputImage>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot reach the compositor: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("cannot read the compositor globals: {e}"))?;
    let qh = queue.handle();
    let manager = globals
        .bind::<ZwlrScreencopyManagerV1, _, _>(&qh, 1..=3, ())
        .map_err(|_| "the compositor offers no screen copy (wlr-screencopy)".to_string())?;
    let shm = globals.bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ()).map_err(|e| format!("no wl_shm: {e}"))?;
    let mut state = State::default();
    globals.contents().with_list(|list| {
        for global in list.iter().filter(|g| g.interface == wl_output::WlOutput::interface().name) {
            let index = state.outputs.len();
            let proxy = globals.registry().bind::<wl_output::WlOutput, _, _>(global.name, global.version.min(4), &qh, index);
            state.outputs.push(OutputInfo { proxy: Some(proxy), ..OutputInfo::default() });
        }
    });
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    let mut images = Vec::new();
    for index in 0..state.outputs.len() {
        let Some(output) = state.outputs[index].proxy.clone() else { continue };
        images.push(copy_output(&manager, &shm, &output, &mut queue, &mut state, index)?);
    }
    manager.destroy();
    Ok(images)
}

/// Copy one output (index `index` of `state.outputs`).
fn copy_output(
    manager: &ZwlrScreencopyManagerV1,
    shm: &wl_shm::WlShm,
    output: &wl_output::WlOutput,
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    index: usize,
) -> Result<OutputImage, String> {
    let qh = queue.handle();
    state.frame = FrameState::default();
    let frame = manager.capture_output(0, output, &qh, ());
    while !(state.frame.buffer_done || state.frame.failed) {
        queue.blocking_dispatch(state).map_err(|e| e.to_string())?;
    }
    let name = state.outputs[index].name.clone();
    let Some((format, width, height, stride)) = state.frame.buffer.take() else {
        return Err(format!("the compositor did not offer a memory buffer for output {name}"));
    };
    let size = stride as usize * height as usize;
    let file = memfd(size)?;
    let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
    let format_value = match format {
        WEnum::Value(f) => f,
        WEnum::Unknown(v) => return Err(format!("unknown pixel format {v:#x} on output {name}")),
    };
    let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format_value, &qh, ());
    frame.copy(&buffer);
    while !(state.frame.ready || state.frame.failed) {
        queue.blocking_dispatch(state).map_err(|e| e.to_string())?;
    }
    let failed = state.frame.failed;
    frame.destroy();
    buffer.destroy();
    pool.destroy();
    if failed { return Err(format!("the compositor could not copy output {name}")); }
    let mut data = Vec::with_capacity(size);
    let mut reader = File::from(file);
    reader.read_to_end(&mut data).map_err(|e| format!("cannot read the copied pixels: {e}"))?;
    let rgba = to_rgba(format_value, &data, width, height, stride, state.frame.y_invert)
        .ok_or_else(|| format!("unsupported pixel format {format_value:?} on output {name}"))?;
    let OutputInfo { x, y, .. } = state.outputs[index];
    Ok(OutputImage { name, x, y, width, height, rgba })
}

/// A shared memory file of `size` bytes.
fn memfd(size: usize) -> Result<OwnedFd, String> {
    // SAFETY: a plain syscall taking a NUL terminated name and flags; the
    // returned descriptor is owned by nothing else.
    let fd = unsafe { libc::memfd_create(c"opusdm-screencopy".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 { return Err(format!("memfd_create: {}", std::io::Error::last_os_error())); }
    // SAFETY: `fd` was just created and is valid.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let file = File::from(fd);
    file.set_len(size as u64).map_err(|e| format!("cannot size the pixel buffer: {e}"))?;
    Ok(file.into())
}

/// Opaque RGBA of a 32 bit `wl_shm` picture, flipped when `y_invert`.
/// `None` for any other pixel format or a buffer shorter than announced.
pub fn to_rgba(format: wl_shm::Format, data: &[u8], width: u32, height: u32, stride: u32, y_invert: bool) -> Option<Vec<u8>> {
    use wl_shm::Format;
    // Little endian byte order of each format's 32 bit pixel: where R, G, B are.
    let (r, g, b) = match format {
        Format::Argb8888 | Format::Xrgb8888 => (2, 1, 0),
        Format::Abgr8888 | Format::Xbgr8888 => (0, 1, 2),
        _ => return None,
    };
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    if stride < w * 4 || data.len() < stride * h { return None; }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src = if y_invert { h - 1 - row } else { row };
        for px in data[src * stride..src * stride + w * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[r], px[g], px[b], 255]);
        }
    }
    Some(out)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    /// Outputs added during a capture are not copied.
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    /// Name and position of output `index`.
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, index: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let Some(info) = state.outputs.get_mut(*index) else { return };
        match event {
            wl_output::Event::Name { name } => info.name = name,
            wl_output::Event::Geometry { x, y, .. } => (info.x, info.y) = (x, y),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    /// Collect what the frame says about the buffer and the copy.
    fn event(state: &mut Self, frame: &ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwlr_screencopy_frame_v1::Event;
        match event {
            Event::Buffer { format, width, height, stride } => {
                state.frame.buffer = Some((format, width, height, stride));
                // Before version 3 there is no buffer_done: one buffer event is all.
                if frame.version() < 3 { state.frame.buffer_done = true; }
            }
            Event::BufferDone => state.frame.buffer_done = true,
            Event::Flags { flags } => {
                state.frame.y_invert = matches!(flags, WEnum::Value(f) if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            Event::Ready { .. } => state.frame.ready = true,
            Event::Failed => state.frame.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    /// The manager has no events.
    fn event(_: &mut Self, _: &ZwlrScreencopyManagerV1, _: <ZwlrScreencopyManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    /// The supported formats are announced by the frame instead.
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    /// The pool has no events.
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    /// `release` does not matter: the buffer is destroyed after one copy.
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrgb_becomes_opaque_rgba() {
        // One row of two pixels, padded to a stride of 12 bytes.
        let data = [10, 20, 30, 0, 1, 2, 3, 0, 9, 9, 9, 9];
        let rgba = to_rgba(wl_shm::Format::Xrgb8888, &data, 2, 1, 12, false).unwrap();
        assert_eq!(rgba, [30, 20, 10, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn xbgr_keeps_its_order_and_y_invert_flips() {
        let data = [1, 2, 3, 0, 4, 5, 6, 0];
        let rgba = to_rgba(wl_shm::Format::Xbgr8888, &data, 1, 2, 4, true).unwrap();
        assert_eq!(rgba, [4, 5, 6, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn other_formats_and_short_buffers_are_refused() {
        assert!(to_rgba(wl_shm::Format::Rgb565, &[0; 8], 2, 1, 4, false).is_none());
        assert!(to_rgba(wl_shm::Format::Xrgb8888, &[0; 7], 2, 1, 8, false).is_none());
    }
}
