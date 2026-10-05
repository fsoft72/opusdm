//! The playback engine: one `playbin3` pipeline plus the bus and position
//! plumbing. It lives on the GTK main thread and reports through a callback.

use crate::error::MediaError;
use crate::event::{Event, PlayState, StreamInfo};
use crate::tags::TagInfo;
use crate::uri::to_uri;
use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use gtk4::gdk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Interval of the `Event::Position` timer while playing.
const POSITION_INTERVAL_MS: u64 = 250;
/// Package that provides the GTK4 video sink.
const GTK4_SINK_PACKAGE: &str = "gstreamer1.0-gtk4";

/// Where the video frames go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoSink {
    /// `gtk4paintablesink`: frames are available as a `gdk::Paintable`.
    #[default]
    Paintable,
    /// Frames and audio are thrown away. Needs no display and no sound card; used
    /// by the tests.
    Discard,
}

/// How an `Engine` is built.
#[derive(Debug, Clone, Copy)]
pub struct EngineOptions {
    /// Decode and play the audio track. Off for a silent video (desktop background).
    pub audio: bool,
    /// Start again at the end of the stream.
    pub looping: bool,
    pub video_sink: VideoSink,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions { audio: true, looping: false, video_sink: VideoSink::Paintable }
    }
}

/// Plays one URI at a time. Dropping it stops the pipeline.
pub struct Engine {
    playbin: gst::Element,
    paintable: Option<gdk::Paintable>,
    uri: Rc<RefCell<String>>,
    looping: Rc<Cell<bool>>,
    timer: Rc<RefCell<Option<glib::SourceId>>>,
    _watch: gst::bus::BusWatchGuard,
}

impl Engine {
    /// Build the pipeline. `on_event` is called on the thread that owns the default
    /// main context (the GTK main thread), from the main loop.
    pub fn new(options: EngineOptions, on_event: impl Fn(Event) + 'static) -> Result<Self, MediaError> {
        gst::init().map_err(|e| MediaError::pipeline(format!("GStreamer init failed: {e}")))?;
        let playbin = gst::ElementFactory::make("playbin3")
            .build()
            .map_err(|_| MediaError::MissingPlugin { name: "playbin3".into(), package: "gstreamer1.0-plugins-base".into() })?;
        let (video_sink, paintable) = _make_video_sink(options.video_sink)?;
        playbin.set_property("video-sink", &video_sink);
        if options.video_sink == VideoSink::Discard {
            let audio_sink = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .map_err(|_| MediaError::MissingPlugin { name: "fakesink".into(), package: "gstreamer1.0-plugins-base".into() })?;
            playbin.set_property("audio-sink", &audio_sink);
        }
        if !options.audio {
            // No audio decoding at all: flags are audio-less.
            playbin.set_property_from_str("flags", "video+soft-volume");
        }
        let bus = playbin.bus().ok_or_else(|| MediaError::pipeline("pipeline has no bus"))?;

        let on_event: Rc<dyn Fn(Event)> = Rc::new(on_event);
        let uri = Rc::new(RefCell::new(String::new()));
        let looping = Rc::new(Cell::new(options.looping));
        let timer = Rc::new(RefCell::new(None));
        let rewinding = Rc::new(Cell::new(false));
        let mut tags = TagInfo::default();
        let watch = {
            let (playbin, on_event, uri, looping, timer) = (playbin.clone(), on_event.clone(), uri.clone(), looping.clone(), timer.clone());
            bus.add_watch_local(move |_, msg| {
                let shared = Shared { on_event: &on_event, timer: &timer, rewinding: &rewinding };
                _handle_message(&playbin, msg, &shared, &uri.borrow(), looping.get(), &mut tags);
                glib::ControlFlow::Continue
            })
            .map_err(|e| MediaError::pipeline(format!("cannot watch the bus: {e}")))?
        };
        Ok(Engine { playbin, paintable, uri, looping, timer, _watch: watch })
    }

    /// The video frames, for a `gtk::Picture`. `None` with `VideoSink::Discard`.
    pub fn paintable(&self) -> Option<gdk::Paintable> {
        self.paintable.clone()
    }

    /// Load `arg` (a path or URL) and preroll it, paused. `Err` only for an
    /// argument that is not a valid path or URL. A file that cannot be opened or
    /// played is reported as `Event::Error` (GStreamer always posts it on the bus
    /// when the state change fails), so there is a single channel for those.
    pub fn open(&self, arg: &str) -> Result<(), MediaError> {
        let uri = to_uri(arg)?;
        self._set_state(gst::State::Null)?;
        self.playbin.set_property("uri", &uri);
        *self.uri.borrow_mut() = uri;
        let _ = self.playbin.set_state(gst::State::Paused);
        Ok(())
    }

    /// Start or resume playback.
    pub fn play(&self) -> Result<(), MediaError> {
        self._set_state(gst::State::Playing)
    }

    /// Pause, keeping the position.
    pub fn pause(&self) -> Result<(), MediaError> {
        self._set_state(gst::State::Paused)
    }

    /// Stop and release the file.
    pub fn stop(&self) -> Result<(), MediaError> {
        self._set_state(gst::State::Null)
    }

    /// Jump to `ns` nanoseconds. `accurate` lands on the exact time (slower),
    /// otherwise on the nearest key frame (fast, for dragging).
    pub fn seek(&self, ns: u64, accurate: bool) -> Result<(), MediaError> {
        let flags = gst::SeekFlags::FLUSH | if accurate { gst::SeekFlags::ACCURATE } else { gst::SeekFlags::KEY_UNIT };
        self.playbin
            .seek_simple(flags, gst::ClockTime::from_nseconds(ns))
            .map_err(|e| MediaError::pipeline(format!("seek failed: {e}")))
    }

    /// Volume from 0.0 (silent) to 1.0 (full).
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
    }

    /// Mute or unmute without touching the volume.
    pub fn set_muted(&self, muted: bool) {
        self.playbin.set_property("mute", muted);
    }

    /// Restart at the end of the stream when `on`.
    pub fn set_loop(&self, on: bool) {
        self.looping.set(on);
    }

    /// True while playing, or when a switch to playing is under way.
    pub fn is_playing(&self) -> bool {
        let (_, current, pending) = self.playbin.state(gst::ClockTime::ZERO);
        pending == gst::State::Playing || (pending == gst::State::VoidPending && current == gst::State::Playing)
    }

    /// Current position in nanoseconds, if known.
    pub fn position(&self) -> Option<u64> {
        self.playbin.query_position::<gst::ClockTime>().map(|t| t.nseconds())
    }

    /// Length in nanoseconds, if known.
    pub fn duration(&self) -> Option<u64> {
        self.playbin.query_duration::<gst::ClockTime>().map(|t| t.nseconds())
    }

    /// Change the pipeline state, mapping a refusal to an error.
    fn _set_state(&self, state: gst::State) -> Result<(), MediaError> {
        self.playbin
            .set_state(state)
            .map(|_| ())
            .map_err(|_| MediaError::pipeline(format!("cannot go to {state:?}: the pipeline refused (see the log)")))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(id) = self.timer.borrow_mut().take() { id.remove(); }
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

/// Build the video sink and, for the GTK one, its paintable.
fn _make_video_sink(kind: VideoSink) -> Result<(gst::Element, Option<gdk::Paintable>), MediaError> {
    match kind {
        VideoSink::Discard => {
            let sink = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .map_err(|_| MediaError::MissingPlugin { name: "fakesink".into(), package: "gstreamer1.0-plugins-base".into() })?;
            Ok((sink, None))
        }
        VideoSink::Paintable => {
            let sink = gst::ElementFactory::make("gtk4paintablesink")
                .build()
                .map_err(|_| MediaError::MissingPlugin { name: "gtk4paintablesink".into(), package: GTK4_SINK_PACKAGE.into() })?;
            let paintable = sink.property::<gdk::Paintable>("paintable");
            Ok((sink, Some(paintable)))
        }
    }
}

/// What the bus handler shares with the engine.
struct Shared<'a> {
    on_event: &'a Rc<dyn Fn(Event)>,
    timer: &'a Rc<RefCell<Option<glib::SourceId>>>,
    /// True from the loop rewind at the end of a stream until the seek completes:
    /// the pipeline goes through Paused and back, which the UI must not see.
    rewinding: &'a Rc<Cell<bool>>,
}

/// Translate one bus message into events and side effects (timer, loop).
fn _handle_message(playbin: &gst::Element, msg: &gst::Message, shared: &Shared, uri: &str, looping: bool, tags: &mut TagInfo) {
    use gst::MessageView;
    let (on_event, timer) = (shared.on_event, shared.timer);
    let from_playbin = msg.src().map(|s| s == playbin.upcast_ref::<gst::Object>()).unwrap_or(false);
    match msg.view() {
        MessageView::StateChanged(_) if from_playbin && shared.rewinding.get() => {}
        MessageView::StateChanged(s) if from_playbin => {
            let state = match s.current() {
                gst::State::Playing => PlayState::Playing,
                gst::State::Paused => PlayState::Paused,
                _ => PlayState::Stopped,
            };
            _sync_timer(playbin, on_event, timer, state == PlayState::Playing);
            on_event(Event::State(state));
        }
        MessageView::AsyncDone(_) | MessageView::DurationChanged(_) => {
            if matches!(msg.view(), MessageView::AsyncDone(_)) { shared.rewinding.set(false); }
            if let Some(d) = playbin.query_duration::<gst::ClockTime>() { on_event(Event::Duration(d.nseconds())); }
            if let Some(p) = playbin.query_position::<gst::ClockTime>() { on_event(Event::Position(p.nseconds())); }
        }
        MessageView::StreamCollection(c) => {
            let mut info = StreamInfo::default();
            for stream in c.stream_collection().iter() {
                let kind = stream.stream_type();
                info.video |= kind.contains(gst::StreamType::VIDEO);
                info.audio |= kind.contains(gst::StreamType::AUDIO);
            }
            on_event(Event::Streams(info));
        }
        MessageView::Tag(t) => {
            tags.merge(TagInfo::from_taglist(&t.tags()));
            on_event(Event::Tags(tags.clone()));
        }
        MessageView::Eos(_) => {
            // Rewind in both cases; keep going only when looping.
            shared.rewinding.set(looping);
            let _ = playbin.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO);
            if !looping { let _ = playbin.set_state(gst::State::Paused); }
            on_event(Event::Eos);
        }
        MessageView::Error(e) => {
            let debug = e.debug().map(|d| d.to_string());
            on_event(Event::Error(MediaError::from_gst(&e.error(), debug, uri)));
        }
        _ => {}
    }
}

/// Run the position timer while playing, stop it otherwise.
fn _sync_timer(playbin: &gst::Element, on_event: &Rc<dyn Fn(Event)>, timer: &Rc<RefCell<Option<glib::SourceId>>>, playing: bool) {
    let mut slot = timer.borrow_mut();
    if !playing {
        if let Some(id) = slot.take() { id.remove(); }
        return;
    }
    if slot.is_some() { return; }
    let (playbin, on_event) = (playbin.clone(), on_event.clone());
    *slot = Some(glib::timeout_add_local(Duration::from_millis(POSITION_INTERVAL_MS), move || {
        if let Some(p) = playbin.query_position::<gst::ClockTime>() { on_event(Event::Position(p.nseconds())); }
        glib::ControlFlow::Continue
    }));
}
