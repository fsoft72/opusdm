//! The player window: video or audio view, control bar, full screen and keys.

use crate::args::Args;
use crate::audio_view::AudioView;
use crate::controls::{Controls, Handlers};
use crate::timefmt::offset;
use crate::view::{choose, ViewKind};
use gtk4 as gtk;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_media::{Engine, EngineOptions, Event, MediaError, PlayState};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Seconds a Left / Right key moves the position.
const SKIP_SECS: i64 = 5;
/// Volume step of the Up / Down keys, in 0..1.
const VOLUME_STEP: f64 = 0.05;
/// Milliseconds the controls stay visible after the pointer moves in full screen.
const CONTROLS_HIDE_MS: u64 = 2000;
const DEFAULT_WIDTH: i32 = 900;
const DEFAULT_HEIGHT: i32 = 560;

/// Set when a playback error was shown: the process then exits with 1.
pub static FAILED: AtomicBool = AtomicBool::new(false);

/// Open a window for `args.target` and start playing.
pub fn open(app: &gtk::Application, args: &Args) {
    let name = std::path::Path::new(&args.target)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| args.target.clone());
    let window = gtk::ApplicationWindow::builder().application(app).title(&name).default_width(DEFAULT_WIDTH).default_height(DEFAULT_HEIGHT).build();

    let (tx, rx) = async_channel::unbounded();
    let options = EngineOptions { looping: args.looping, ..Default::default() };
    let engine = match Engine::new(options, move |e| { let _ = tx.try_send(e); }) {
        Ok(e) => Rc::new(e),
        Err(e) => return fatal(&window, &e),
    };

    let picture = gtk::Picture::new();
    picture.set_paintable(engine.paintable().as_ref());
    picture.set_content_fit(gtk::ContentFit::Contain);
    let audio = Rc::new(AudioView::new(&name));
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&picture, Some("video"));
    stack.add_named(&audio.root, Some("audio"));
    stack.add_css_class("player-stage");

    let controls = Controls::new(_handlers(&engine), args.looping);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&stack);
    vbox.append(&controls.root);
    window.set_child(Some(&vbox));
    _wire_keys(&window, &engine, &controls);
    _wire_fullscreen(&window, &stack, &controls);

    let (w, c, a, s) = (window.clone(), controls.clone(), audio.clone(), stack.clone());
    glib::spawn_future_local(async move {
        while let Ok(event) = rx.recv().await {
            _on_event(&w, &c, &a, &s, &name, event);
        }
    });

    window.present();
    if let Err(e) = engine.open(&args.target).and_then(|_| engine.play()) { fatal(&window, &e); }
}

/// What each control does to the engine.
fn _handlers(engine: &Rc<Engine>) -> Handlers {
    let (e_play, e_seek, e_vol, e_mute, e_loop) = (engine.clone(), engine.clone(), engine.clone(), engine.clone(), engine.clone());
    Handlers {
        toggle_play: Box::new(move || _toggle_play(&e_play)),
        seek: Box::new(move |value, accurate| {
            let Some(duration) = e_seek.duration() else { return };
            let ns = crate::timefmt::slider_to_ns(value, duration);
            if let Err(e) = e_seek.seek(ns, accurate) { opusdm_ipc::err_log!("player", "{e}"); }
        }),
        volume: Box::new(move |v| e_vol.set_volume(v)),
        mute: Box::new(move |m| e_mute.set_muted(m)),
        looping: Box::new(move |on| e_loop.set_loop(on)),
    }
}

/// Pause when playing, play when paused.
fn _toggle_play(engine: &Engine) {
    let playing = engine.is_playing();
    let result = if playing { engine.pause() } else { engine.play() };
    if let Err(e) = result { opusdm_ipc::err_log!("player", "{e}"); }
}

/// Space, arrows, F, M and Esc.
fn _wire_keys(window: &gtk::ApplicationWindow, engine: &Rc<Engine>, controls: &Rc<Controls>) {
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (w, engine, controls) = (window.clone(), engine.clone(), controls.clone());
    key.connect_key_pressed(move |_, k, _, _| {
        let skip = |delta: i64| {
            let duration = engine.duration().unwrap_or(0);
            let target = offset(engine.position().unwrap_or(0), delta, duration);
            let _ = engine.seek(target, true);
        };
        match k {
            gdk::Key::space => _toggle_play(&engine),
            gdk::Key::Left => skip(-SKIP_SECS),
            gdk::Key::Right => skip(SKIP_SECS),
            gdk::Key::Up | gdk::Key::Down => {
                controls.adjust_volume(if k == gdk::Key::Up { VOLUME_STEP } else { -VOLUME_STEP });
            }
            gdk::Key::f | gdk::Key::F => w.set_fullscreened(!w.is_fullscreen()),
            gdk::Key::m | gdk::Key::M => controls.toggle_mute(),
            gdk::Key::Escape if w.is_fullscreen() => w.set_fullscreened(false),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(key);
}

/// Double click toggles full screen; in full screen the bar hides until the
/// pointer moves.
fn _wire_fullscreen(window: &gtk::ApplicationWindow, stack: &gtk::Stack, controls: &Rc<Controls>) {
    let click = gtk::GestureClick::new();
    let w = window.clone();
    click.connect_pressed(move |_, n, _, _| {
        if n == 2 { w.set_fullscreened(!w.is_fullscreen()); }
    });
    stack.add_controller(click);

    let c = controls.clone();
    window.connect_fullscreened_notify(move |w| c.root.set_visible(!w.is_fullscreen()));
    let motion = gtk::EventControllerMotion::new();
    let (w, c) = (window.clone(), controls.clone());
    let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    motion.connect_motion(move |_, _, _| {
        if !w.is_fullscreen() { return; }
        c.root.set_visible(true);
        if let Some(id) = pending.take() { id.remove(); }
        let (w, c, p) = (w.clone(), c.clone(), pending.clone());
        let id = glib::timeout_add_local_once(Duration::from_millis(CONTROLS_HIDE_MS), move || {
            p.set(None);
            if w.is_fullscreen() { c.root.set_visible(false); }
        });
        pending.set(Some(id));
    });
    window.add_controller(motion);
}

/// Apply one engine event to the widgets.
fn _on_event(window: &gtk::ApplicationWindow, controls: &Controls, audio: &AudioView, stack: &gtk::Stack, name: &str, event: Event) {
    match event {
        Event::State(s) => controls.set_playing(s == PlayState::Playing),
        Event::Duration(d) => controls.set_duration(d),
        Event::Position(p) => controls.set_position(p),
        Event::Eos => controls.set_position(0),
        Event::Streams(info) => stack.set_visible_child_name(match choose(Some(info)) { ViewKind::Video => "video", ViewKind::Audio => "audio" }),
        Event::Tags(tags) => {
            audio.set_tags(&tags);
            if let Some(title) = &tags.title { window.set_title(Some(&format!("{} - {name}", title))); }
        }
        Event::Error(e) => fatal(window, &e),
    }
}

/// Log `error`, show it in a requester and make the process exit with 1. Only the
/// first error is shown: GStreamer often follows it with a second, more technical one.
fn fatal(window: &gtk::ApplicationWindow, error: &MediaError) {
    opusdm_ipc::err_log!("player", "{error}");
    if FAILED.swap(true, Ordering::SeqCst) { return; }
    let w = window.clone();
    opusdm_ui::requester(window, "OpusDM Player", &error.to_string(), None, &[("Close", ())], move |_| w.close());
    window.present();
}
