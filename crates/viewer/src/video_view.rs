//! The video area: the engine's paintable (or, for an audio file, its cover
//! and tags) and the shared control bar.

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_media::timefmt::{offset, slider_to_ns};
use opusdm_media::{Engine, EngineOptions, Event, MediaError, PlayState};
use opusdm_media_ui::audio_view::AudioView;
use opusdm_media_ui::controls::{Controls, Handlers};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Volume step of the Up / Down keys, in 0..1.
const VOLUME_STEP: f64 = 0.05;

/// What the video view tells the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoNotice {
    /// The video played to its end.
    Ended,
    /// Playback failed; holds the message.
    Failed(String),
}

/// The engine, created at the first video and reused for the next ones. Shared
/// with the control bar handlers.
type SharedEngine = Rc<RefCell<Option<Rc<Engine>>>>;

/// Page of [`VideoView::frames`] with the video frames.
const FRAMES_PAGE: &str = "video";
/// Page of [`VideoView::frames`] with the cover and tags of an audio file.
const AUDIO_PAGE: &str = "audio";

/// One engine, the frames and the control bar.
pub struct VideoView {
    /// The frames.
    pub stage: gtk::Picture,
    /// The frames, or the audio view while an audio file plays.
    pub frames: gtk::Stack,
    audio: AudioView,
    /// The control bar; hidden in full screen.
    pub bar: gtk::Box,
    controls: Rc<Controls>,
    engine: SharedEngine,
    notify: async_channel::Sender<VideoNotice>,
}

impl VideoView {
    /// Build the widgets; the engine is created later, by the first `open`.
    pub fn new(notify: async_channel::Sender<VideoNotice>) -> Rc<Self> {
        let stage = gtk::Picture::new();
        stage.set_content_fit(gtk::ContentFit::Contain);
        stage.set_hexpand(true);
        stage.set_vexpand(true);
        stage.add_css_class("viewer-stage");
        let engine: SharedEngine = Rc::new(RefCell::new(None));
        let controls = Controls::new(_handlers(&engine), false);
        let audio = AudioView::new("");
        let frames = gtk::Stack::new();
        frames.set_hexpand(true);
        frames.set_vexpand(true);
        frames.add_named(&stage, Some(FRAMES_PAGE));
        frames.add_named(&audio.root, Some(AUDIO_PAGE));
        Rc::new(VideoView { stage, frames, audio, bar: controls.root.clone(), controls, engine, notify })
    }

    /// Open the video `path` and start playing it.
    pub fn open(self: &Rc<Self>, path: &Path) -> Result<(), MediaError> {
        self.frames.set_visible_child_name(FRAMES_PAGE);
        self._play(path)
    }

    /// Open the audio file `path` and start playing it; its cover and tags
    /// show in place of the frames (the file name until the tags arrive).
    pub fn open_audio(self: &Rc<Self>, path: &Path) -> Result<(), MediaError> {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.audio.reset(&name);
        self.frames.set_visible_child_name(AUDIO_PAGE);
        self._play(path)
    }

    /// Load `path` in the engine and play it.
    fn _play(self: &Rc<Self>, path: &Path) -> Result<(), MediaError> {
        let engine = self._engine()?;
        self.controls.set_duration(0);
        self.controls.set_position(0);
        engine.open(&path.to_string_lossy())?;
        engine.play()
    }

    /// Stop playback (a picture is about to be shown).
    pub fn stop(&self) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        if let Err(e) = engine.stop() { opusdm_ipc::err_log!("viewer", "{e}"); }
    }

    /// Pause when playing, play when paused.
    pub fn toggle_play(&self) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        _toggle_play(&engine);
    }

    /// Move the position by `delta_secs`, kept inside the video.
    pub fn skip(&self, delta_secs: i64) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        let target = offset(engine.position().unwrap_or(0), delta_secs, engine.duration().unwrap_or(0));
        if let Err(e) = engine.seek(target, true) { opusdm_ipc::err_log!("viewer", "{e}"); }
    }

    /// Louder (`up`) or quieter by one step.
    pub fn change_volume(&self, up: bool) {
        self.controls.adjust_volume(if up { VOLUME_STEP } else { -VOLUME_STEP });
    }

    /// Flip the mute button.
    pub fn toggle_mute(&self) {
        self.controls.toggle_mute();
    }

    /// The engine, built on first use (a missing GStreamer plugin fails here).
    fn _engine(self: &Rc<Self>) -> Result<Rc<Engine>, MediaError> {
        if let Some(engine) = self.engine.borrow().clone() { return Ok(engine); }
        let (tx, rx) = async_channel::unbounded();
        let engine = Rc::new(Engine::new(EngineOptions::default(), move |e| { let _ = tx.try_send(e); })?);
        self.stage.set_paintable(engine.paintable().as_ref());
        *self.engine.borrow_mut() = Some(engine.clone());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this._on_event(event);
            }
        });
        Ok(engine)
    }

    /// Apply one engine event.
    fn _on_event(&self, event: Event) {
        match event {
            Event::State(s) => self.controls.set_playing(s == PlayState::Playing),
            Event::Duration(d) => self.controls.set_duration(d),
            Event::Position(p) => self.controls.set_position(p),
            Event::Eos => {
                self.controls.set_position(0);
                let _ = self.notify.try_send(VideoNotice::Ended);
            }
            Event::Error(e) => {
                opusdm_ipc::err_log!("viewer", "{e}");
                let _ = self.notify.try_send(VideoNotice::Failed(e.to_string()));
            }
            // Only an audio file shows them; a video's tags fill the hidden view.
            Event::Tags(tags) => self.audio.set_tags(&tags),
            Event::Streams(_) => {}
        }
    }
}

/// Pause when playing, play when paused.
fn _toggle_play(engine: &Engine) {
    let result = if engine.is_playing() { engine.pause() } else { engine.play() };
    if let Err(e) = result { opusdm_ipc::err_log!("viewer", "{e}"); }
}

/// What each control does to the engine, once there is one.
fn _handlers(engine: &SharedEngine) -> Handlers {
    let current = {
        let engine = engine.clone();
        move || engine.borrow().clone()
    };
    let (c_play, c_seek, c_vol, c_mute) = (current.clone(), current.clone(), current.clone(), current);
    Handlers {
        toggle_play: Box::new(move || { if let Some(e) = c_play() { _toggle_play(&e); } }),
        seek: Box::new(move |value, accurate| {
            let Some(engine) = c_seek() else { return };
            let Some(duration) = engine.duration() else { return };
            if let Err(e) = engine.seek(slider_to_ns(value, duration), accurate) { opusdm_ipc::err_log!("viewer", "{e}"); }
        }),
        volume: Box::new(move |v| { if let Some(e) = c_vol() { e.set_volume(v); } }),
        mute: Box::new(move |m| { if let Some(e) = c_mute() { e.set_muted(m); } }),
        looping: None,
    }
}
