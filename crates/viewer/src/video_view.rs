//! The video area: the engine's paintable and a bar with play / pause, a seek
//! slider and the time.

use opusdm_media::timefmt::{format_time, ns_to_slider, offset, slider_to_ns, SLIDER_MAX};
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_media::{Engine, EngineOptions, Event, MediaError, PlayState};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// What the video view tells the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoNotice {
    /// The video played to its end.
    Ended,
    /// Playback failed; holds the message.
    Failed(String),
}

/// One engine, created at the first video and reused for the next ones.
pub struct VideoView {
    /// The frames.
    pub stage: gtk::Picture,
    /// Play / pause, slider and time; hidden in full screen.
    pub bar: gtk::Box,
    play: gtk::Button,
    scale: gtk::Scale,
    time: gtk::Label,
    engine: RefCell<Option<Rc<Engine>>>,
    duration: Cell<u64>,
    dragging: Rc<Cell<bool>>,
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
        let play = gtk::Button::with_label("Play");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, SLIDER_MAX, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        let time = gtk::Label::new(Some("00:00 / 00:00"));
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.set_margin_start(8);
        bar.set_margin_end(8);
        bar.set_margin_top(6);
        bar.set_margin_bottom(6);
        for w in [play.upcast_ref::<gtk::Widget>(), scale.upcast_ref(), time.upcast_ref()] {
            bar.append(w);
            // Keys go to the window, never to a focused button.
            w.set_focusable(false);
        }
        let this = Rc::new(VideoView { stage, bar, play, scale, time, engine: RefCell::new(None), duration: Cell::new(0), dragging: Rc::new(Cell::new(false)), notify });
        this._wire_controls();
        this
    }

    /// Play button and slider act on the engine, if there is one.
    fn _wire_controls(self: &Rc<Self>) {
        let this = self.clone();
        self.play.connect_clicked(move |_| this.toggle_play());
        // `change-value` only fires for the user, never for `set_value` from code.
        let this = self.clone();
        self.scale.connect_change_value(move |_, _, value| {
            this._seek_to_slider(value, !this.dragging.get());
            glib::Propagation::Proceed
        });
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let dragging = self.dragging.clone();
        click.connect_pressed(move |_, _, _, _| dragging.set(true));
        let this = self.clone();
        click.connect_released(move |_, _, _, _| {
            this.dragging.set(false);
            this._seek_to_slider(this.scale.value(), true);
        });
        self.scale.add_controller(click);
    }

    /// Open `path` and start playing it.
    pub fn open(self: &Rc<Self>, path: &Path) -> Result<(), MediaError> {
        let engine = self._engine()?;
        self.duration.set(0);
        self._show_position(0);
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
        let result = if engine.is_playing() { engine.pause() } else { engine.play() };
        if let Err(e) = result { opusdm_ipc::err_log!("viewer", "{e}"); }
    }

    /// Move the position by `delta_secs`, kept inside the video.
    pub fn skip(&self, delta_secs: i64) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        let target = offset(engine.position().unwrap_or(0), delta_secs, engine.duration().unwrap_or(0));
        if let Err(e) = engine.seek(target, true) { opusdm_ipc::err_log!("viewer", "{e}"); }
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
            Event::State(s) => self.play.set_label(if s == PlayState::Playing { "Pause" } else { "Play" }),
            Event::Duration(d) => self.duration.set(d),
            Event::Position(p) => self._show_position(p),
            Event::Eos => {
                self._show_position(0);
                let _ = self.notify.try_send(VideoNotice::Ended);
            }
            Event::Error(e) => {
                opusdm_ipc::err_log!("viewer", "{e}");
                let _ = self.notify.try_send(VideoNotice::Failed(e.to_string()));
            }
            Event::Tags(_) | Event::Streams(_) => {}
        }
    }

    /// Time text, and the slider unless the user holds it.
    fn _show_position(&self, position: u64) {
        let duration = self.duration.get();
        self.time.set_text(&format!("{} / {}", format_time(position), format_time(duration)));
        if self.dragging.get() { return; }
        self.scale.set_value(ns_to_slider(position, duration));
    }

    /// Seek where the slider points; `accurate` is false while it is dragged.
    fn _seek_to_slider(&self, value: f64, accurate: bool) {
        let Some(engine) = self.engine.borrow().clone() else { return };
        let Some(duration) = engine.duration() else { return };
        if let Err(e) = engine.seek(slider_to_ns(value, duration), accurate) { opusdm_ipc::err_log!("viewer", "{e}"); }
    }
}
