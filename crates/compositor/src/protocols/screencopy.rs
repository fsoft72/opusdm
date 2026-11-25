//! wlr-screencopy (`zwlr_screencopy_manager_v1`, version 3): screenshots of
//! the output for `opusdm-shot` (`crates/wayland/src/screencopy.rs`).
//!
//! `capture_output` and `capture_output_region` (the region clipped to the
//! output; an empty one fails) create a frame that announces one shared
//! memory buffer, `Xrgb8888` of the region size with a `width * 4` stride,
//! then `buffer_done` (version 3). There is no `linux_dmabuf` buffer.
//! `overlay_cursor` is ignored: the cursor is never captured.
//!
//! `copy` / `copy_with_damage` with a shm buffer of exactly that format and
//! size queue a `PendingCapture`; any other buffer gets `failed` and an
//! error line instead of the `invalid_buffer` protocol error (the protocol
//! error would kill the whole client, `opusdm-shot`). A second `copy` on
//! one frame is the `already_used` protocol error, as the protocol says.
//! `copy_with_damage` does not wait for new damage: the frame is copied on
//! the next loop iteration and the damage is the whole buffer.
//!
//! The backend services the queue once per loop iteration
//! (`take_pending_captures`, then `complete_capture` with the pixels): the
//! nested backend renders the same element list as the screen into an
//! offscreen buffer and reads it back, the headless backend has no
//! renderer and fills a fixed pixel. Nothing blocks the event loop.
//!
//! Ownership: `Screencopy` lives in `State` (owned by `LoopData`, owned by
//! the backend `run` for the whole run). The queue holds the frame and
//! buffer resources of each pending copy only to answer them; the client
//! owns the objects. A frame leaves the queue when it is answered (`ready`
//! or `failed`), when the client destroys it (`destroyed`), or, when the
//! client died, through `destroyed` too (wayland-server calls it for every
//! object of a dead client); `take_pending_captures` also drops any entry
//! whose frame is no longer alive, with a debug line: not a failure, nobody
//! is left to tell. A live frame whose `wl_buffer` was destroyed before the
//! copy gets `failed` with an error line (its client is still waiting).
//!
//! Tests only (`--headless`): the IPC `debug/screencopy` reports the queue
//! length and can hold the servicing (`Screencopy::hold`), so a test can see
//! an entry wait in the queue and leave it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            backend::{ClientId, GlobalId},
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Clock, Monotonic},
    wayland::shm::{with_buffer_contents, with_buffer_contents_mut, BufferData},
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};

/// Version of the `zwlr_screencopy_manager_v1` global.
pub const SCREENCOPY_VERSION: u32 = 3;

/// Bytes of one `Xrgb8888` pixel.
const BYTES_PER_PIXEL: i32 = 4;

/// A `copy` waiting for the backend: the frame, the client buffer to fill,
/// the output and the region of it (output coordinates).
#[derive(Debug)]
pub struct PendingCapture {
    /// The frame to answer.
    pub frame: ZwlrScreencopyFrameV1,
    /// The client shm buffer, checked at `copy`: `Xrgb8888`, region size,
    /// `width * 4` stride.
    pub buffer: WlBuffer,
    /// The output captured.
    pub output: Output,
    /// The part of the output captured, clipped to it (never empty).
    pub region: Rect,
    /// `copy_with_damage`: a `damage` event comes before `ready`.
    pub with_damage: bool,
}

/// What a frame resource knows: its output and region, or nothing when it
/// already failed at creation; and whether `copy` was used.
#[derive(Debug)]
pub struct FrameData {
    /// Output and clipped region; `None` for a frame that failed at creation.
    target: Option<(Output, Rect)>,
    /// True once `copy` (or `copy_with_damage`) came.
    used: AtomicBool,
}

/// The screencopy global and the copies waiting for the backend.
#[derive(Debug)]
pub struct Screencopy {
    /// The global, kept for the whole run.
    _global: GlobalId,
    /// Copies queued by `copy`, in order, serviced by the backend.
    pending: Vec<PendingCapture>,
    /// While true, `take_pending_captures` hands nothing to the backend
    /// (set only by the IPC `debug/screencopy`, headless tests).
    hold: bool,
}

impl Screencopy {
    /// Creates the `zwlr_screencopy_manager_v1` global on `display`.
    pub fn new(display: &DisplayHandle) -> Screencopy {
        let global = display.create_global::<State, ZwlrScreencopyManagerV1, ()>(SCREENCOPY_VERSION, ());
        Screencopy { _global: global, pending: Vec::new(), hold: false }
    }

    /// Number of copies waiting for the backend (IPC `debug/screencopy`).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// True while the servicing is held (IPC `debug/screencopy`).
    pub fn held(&self) -> bool {
        self.hold
    }

    /// Holds (`true`) or releases the servicing of the queue (IPC
    /// `debug/screencopy`, headless tests only).
    pub fn set_hold(&mut self, hold: bool) {
        self.hold = hold;
    }
}

impl State {
    /// Takes the copies waiting for the backend (none while held). A copy
    /// whose frame is gone is dropped with a debug line (not a failure); a
    /// live frame whose buffer is gone gets `failed` with an error line.
    pub fn take_pending_captures(&mut self) -> Vec<PendingCapture> {
        if self.screencopy.hold { return Vec::new(); }
        let pending = std::mem::take(&mut self.screencopy.pending);
        pending.into_iter().filter(_still_wanted).collect()
    }

    /// Answers the copy `c` with `pixels` (`Xrgb8888`, top-down, rows of
    /// `region.w * 4` bytes): writes them into the client buffer and sends
    /// `flags(0)`, `damage` (the whole buffer, for `copy_with_damage`) and
    /// `ready` with the current monotonic time. An `Err`, or pixels that
    /// cannot be written, send `failed` with an error line. A frame gone in
    /// the meantime is dropped with a debug line; a buffer gone in the
    /// meantime fails the frame (`_still_wanted`).
    pub fn complete_capture(&mut self, c: PendingCapture, pixels: Result<Vec<u8>, String>) {
        if !_still_wanted(&c) { return; }
        let written = pixels.and_then(|p| _write_buffer(&c.buffer, c.region, &p));
        if let Err(e) = written {
            opusdm_ipc::err_log!(LOG_TAG, "screencopy: frame {} failed: {e}", c.frame.id());
            c.frame.failed();
            return;
        }
        c.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if c.with_damage {
            c.frame.damage(0, 0, c.region.w.unsigned_abs(), c.region.h.unsigned_abs());
        }
        let now = Duration::from(Clock::<Monotonic>::new().now());
        let secs = now.as_secs();
        c.frame.ready((secs >> 32) as u32, (secs & 0xffff_ffff) as u32, now.subsec_nanos());
        opusdm_ipc::debug_log!(LOG_TAG, "screencopy: frame {} ready ({}x{})", c.frame.id(), c.region.w, c.region.h);
    }
}

/// True when the copy `c` can still be answered with pixels. A dead frame
/// (destroyed, or its client died) is dropped with a debug line; a live
/// frame whose buffer was destroyed gets `failed` with an error line, since
/// its client waits for an answer.
fn _still_wanted(c: &PendingCapture) -> bool {
    if !c.frame.is_alive() {
        opusdm_ipc::debug_log!(LOG_TAG, "screencopy: frame {} is gone, dropped", c.frame.id());
        return false;
    }
    if !c.buffer.is_alive() {
        opusdm_ipc::err_log!(LOG_TAG, "screencopy: frame {} failed: its buffer was destroyed before the copy", c.frame.id());
        c.frame.failed();
        return false;
    }
    true
}

/// The part of `region` (output coordinates, `None`: all of it) inside an
/// output of `size`; `None` when it is empty.
pub fn clip_region(size: (i32, i32), region: Option<Rect>) -> Option<Rect> {
    let Some(r) = region else {
        return (size.0 > 0 && size.1 > 0).then_some(Rect { x: 0, y: 0, w: size.0, h: size.1 });
    };
    let x0 = r.x.max(0);
    let y0 = r.y.max(0);
    let x1 = r.x.saturating_add(r.w).min(size.0);
    let y1 = r.y.saturating_add(r.h).min(size.1);
    if x1 <= x0 || y1 <= y0 { return None; }
    Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// The `region` of a top-down `Xrgb8888` picture of `size` (rows of
/// `size.0 * 4` bytes), as rows of `region.w * 4` bytes; an error when the
/// region is not inside the picture or the picture is short.
pub fn crop_xrgb(full: &[u8], size: (i32, i32), region: Rect) -> Result<Vec<u8>, String> {
    let inside = region.x >= 0 && region.y >= 0 && region.w > 0 && region.h > 0
        && region.x.saturating_add(region.w) <= size.0
        && region.y.saturating_add(region.h) <= size.1;
    if !inside { return Err(format!("region {region:?} is not inside the {}x{} picture", size.0, size.1)); }
    let stride = _usize(size.0)? * 4;
    if full.len() < stride * _usize(size.1)? { return Err(format!("picture of {} bytes for {}x{}", full.len(), size.0, size.1)); }
    let (x, y, w, h) = (_usize(region.x)?, _usize(region.y)?, _usize(region.w)?, _usize(region.h)?);
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = row * stride + x * 4;
        let Some(src) = full.get(start..start + w * 4) else { return Err(format!("row {row} out of the picture")) };
        out.extend_from_slice(src);
    }
    Ok(out)
}

/// `Xrgb8888` (B, G, R, X in memory) of RGBA bytes; the X byte is 0xff.
pub fn rgba_to_xrgb(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for [r, g, b, _] in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[*b, *g, *r, 0xff]);
    }
    out
}

/// `n` as a `usize`; an error for a negative value.
fn _usize(n: i32) -> Result<usize, String> {
    usize::try_from(n).map_err(|_| format!("negative size {n}"))
}

/// Checks that `buffer` is a shm buffer of `Xrgb8888`, `w` x `h`, stride `w * 4`.
fn _check_buffer(buffer: &WlBuffer, w: i32, h: i32) -> Result<(), String> {
    let data = with_buffer_contents(buffer, |_, _, data| data).map_err(|e| format!("not a usable shm buffer ({e:?})"))?;
    _check_buffer_data(&data, w, h)
}

/// Checks the shm `data` of a buffer against `Xrgb8888`, `w` x `h`, stride `w * 4`.
fn _check_buffer_data(data: &BufferData, w: i32, h: i32) -> Result<(), String> {
    let stride = w.saturating_mul(BYTES_PER_PIXEL);
    if data.format != wl_shm::Format::Xrgb8888 || data.width != w || data.height != h || data.stride != stride {
        return Err(format!(
            "buffer {:?} {}x{} stride {}, expected Xrgb8888 {w}x{h} stride {stride}",
            data.format, data.width, data.height, data.stride
        ));
    }
    Ok(())
}

/// Writes `pixels` (rows of `region.w * 4` bytes) into the shm `buffer`,
/// checked again (the pool may have been resized since `copy`).
fn _write_buffer(buffer: &WlBuffer, region: Rect, pixels: &[u8]) -> Result<(), String> {
    let row = _usize(region.w)? * 4;
    let rows = _usize(region.h)?;
    if pixels.len() != row * rows { return Err(format!("{} bytes of pixels for {}x{}", pixels.len(), region.w, region.h)); }
    let written = with_buffer_contents_mut(buffer, |ptr, len, data| -> Result<(), String> {
        _check_buffer_data(&data, region.w, region.h)?;
        let offset = _usize(data.offset)?;
        let stride = _usize(data.stride)?;
        let end = offset.checked_add(stride * rows).ok_or("buffer end overflows")?;
        if end > len { return Err(format!("buffer ends at {end}, past its pool of {len} bytes")); }
        // SAFETY: Smithay hands a pointer to the mapped pool of `len` bytes,
        // valid and not aliased by Rust for the duration of this closure
        // (a SIGBUS from a shrunk file is caught by Smithay).
        let pool = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        for (i, src) in pixels.chunks_exact(row).enumerate() {
            let start = offset + i * stride;
            let Some(dst) = pool.get_mut(start..start + row) else { return Err(format!("row {i} out of the pool")) };
            dst.copy_from_slice(src);
        }
        Ok(())
    });
    written.map_err(|e| format!("cannot map the buffer ({e:?})"))?
}

/// Creates the frame `frame` of a capture of `output` (`region`: a part of
/// it, output coordinates) and announces its buffer, or sends `failed`
/// (logged) when the output is unknown or the region empty.
fn _new_frame(
    state: &State,
    data_init: &mut DataInit<'_, State>,
    frame: New<ZwlrScreencopyFrameV1>,
    overlay_cursor: i32,
    output: &WlOutput,
    region: Option<Rect>,
) {
    let target = Output::from_resource(output).and_then(|o| {
        let geo = state.space.output_geometry(&o)?;
        clip_region((geo.size.w, geo.size.h), region).map(|r| (o, r))
    });
    let frame = data_init.init(frame, FrameData { target: target.clone(), used: AtomicBool::new(false) });
    if overlay_cursor != 0 {
        opusdm_ipc::debug_log!(LOG_TAG, "screencopy: overlay_cursor ignored on frame {}, the cursor is not captured", frame.id());
    }
    let Some((_, r)) = target else {
        opusdm_ipc::err_log!(LOG_TAG, "screencopy: frame {} failed: unknown output or empty region {region:?}", frame.id());
        frame.failed();
        return;
    };
    frame.buffer(wl_shm::Format::Xrgb8888, r.w.unsigned_abs(), r.h.unsigned_abs(), (r.w * BYTES_PER_PIXEL).unsigned_abs());
    if frame.version() >= 3 {
        frame.buffer_done();
    }
    opusdm_ipc::debug_log!(LOG_TAG, "screencopy: frame {} of {r:?}", frame.id());
}

/// Queues the `copy` of `frame` into `buffer`, or sends `failed` (logged)
/// for a frame that failed at creation or a buffer of the wrong kind; a
/// second `copy` is the `already_used` protocol error.
fn _copy(state: &mut State, frame: &ZwlrScreencopyFrameV1, data: &FrameData, buffer: WlBuffer, with_damage: bool) {
    if data.used.swap(true, Ordering::Relaxed) {
        opusdm_ipc::err_log!(LOG_TAG, "screencopy: second copy on frame {}, protocol error", frame.id());
        frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "copy was already requested on this frame");
        return;
    }
    let Some((output, region)) = data.target.clone() else {
        opusdm_ipc::err_log!(LOG_TAG, "screencopy: copy on frame {} that already failed", frame.id());
        frame.failed();
        return;
    };
    if let Err(e) = _check_buffer(&buffer, region.w, region.h) {
        opusdm_ipc::err_log!(LOG_TAG, "screencopy: frame {} failed: {e}", frame.id());
        frame.failed();
        return;
    }
    state.screencopy.pending.push(PendingCapture { frame: frame.clone(), buffer, output, region, with_damage });
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for State {
    /// A client bound the manager.
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        opusdm_ipc::debug_log!(LOG_TAG, "screencopy: manager {} bound (version {})", manager.id(), manager.version());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    /// `capture_output` and `capture_output_region` create a frame.
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        match request {
            Request::CaptureOutput { frame, overlay_cursor, output } => {
                _new_frame(state, data_init, frame, overlay_cursor, &output, None);
            }
            Request::CaptureOutputRegion { frame, overlay_cursor, output, x, y, width, height } => {
                let region = Rect { x, y, w: width, h: height };
                _new_frame(state, data_init, frame, overlay_cursor, &output, Some(region));
            }
            Request::Destroy => {}
            other => opusdm_ipc::err_log!(LOG_TAG, "screencopy: unknown manager request {other:?} ignored"),
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for State {
    /// `copy` and `copy_with_damage` queue the copy (see `_copy`).
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_frame_v1::Request;
        match request {
            Request::Copy { buffer } => _copy(state, resource, data, buffer, false),
            Request::CopyWithDamage { buffer } => _copy(state, resource, data, buffer, true),
            Request::Destroy => {}
            other => opusdm_ipc::err_log!(LOG_TAG, "screencopy: unknown frame request {other:?} ignored"),
        }
    }

    /// The frame is gone (destroyed, or its client died): a copy of it
    /// still queued leaves the queue (not a failure).
    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrScreencopyFrameV1, _data: &FrameData) {
        let before = state.screencopy.pending.len();
        state.screencopy.pending.retain(|c| &c.frame != resource);
        if state.screencopy.pending.len() != before {
            opusdm_ipc::debug_log!(LOG_TAG, "screencopy: frame {} destroyed before its copy, dropped", resource.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_region_is_the_whole_output() {
        assert_eq!(clip_region((1920, 1080), None), Some(Rect { x: 0, y: 0, w: 1920, h: 1080 }));
    }

    #[test]
    fn a_region_is_clipped_to_the_output() {
        let r = Rect { x: 1900, y: -10, w: 100, h: 50 };
        assert_eq!(clip_region((1920, 1080), Some(r)), Some(Rect { x: 1900, y: 0, w: 20, h: 40 }));
        assert_eq!(clip_region((1920, 1080), Some(Rect { x: 10, y: 10, w: 100, h: 50 })), Some(Rect { x: 10, y: 10, w: 100, h: 50 }));
    }

    #[test]
    fn an_empty_or_outside_region_is_none() {
        assert_eq!(clip_region((1920, 1080), Some(Rect { x: 2000, y: 0, w: 10, h: 10 })), None);
        assert_eq!(clip_region((1920, 1080), Some(Rect { x: 0, y: 0, w: 0, h: 10 })), None);
        assert_eq!(clip_region((1920, 1080), Some(Rect { x: 0, y: 0, w: -5, h: 10 })), None);
        assert_eq!(clip_region((1920, 1080), Some(Rect { x: i32::MAX, y: 0, w: i32::MAX, h: 10 })), None);
    }

    #[test]
    fn crop_takes_the_rows_of_the_region() {
        // 3x2 picture, pixel value = index.
        let full: Vec<u8> = (0..6u8).flat_map(|i| [i; 4]).collect();
        let out = crop_xrgb(&full, (3, 2), Rect { x: 1, y: 1, w: 2, h: 1 }).unwrap();
        assert_eq!(out, [4, 4, 4, 4, 5, 5, 5, 5]);
    }

    #[test]
    fn crop_refuses_a_region_outside_or_a_short_picture() {
        let full = vec![0u8; 3 * 2 * 4];
        assert!(crop_xrgb(&full, (3, 2), Rect { x: 2, y: 0, w: 2, h: 1 }).is_err());
        assert!(crop_xrgb(&full[..20], (3, 2), Rect { x: 0, y: 0, w: 1, h: 1 }).is_err());
    }

    #[test]
    fn rgba_becomes_bgrx() {
        assert_eq!(rgba_to_xrgb(&[1, 2, 3, 4, 5, 6, 7, 8]), [3, 2, 1, 0xff, 7, 6, 5, 0xff]);
    }
}
