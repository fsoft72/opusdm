//! The control bar: play / pause, seek slider, times, volume, mute and loop.

use crate::icons::{Icon, IconView};
use opusdm_media::timefmt::{format_time, ns_to_slider, SLIDER_MAX};
use gtk4 as gtk;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Width of the volume slider in pixels.
const VOLUME_WIDTH: i32 = 90;
/// Volume slider range, in percent.
const VOLUME_MAX: f64 = 100.0;

/// What the window does when the user acts on a control.
pub struct Handlers {
    pub toggle_play: Box<dyn Fn()>,
    /// Slider value (0..=`SLIDER_MAX`) and whether the seek must be accurate
    /// (button released) or may use the nearest key frame (still dragging).
    pub seek: Box<dyn Fn(f64, bool)>,
    /// Volume from 0.0 to 1.0.
    pub volume: Box<dyn Fn(f64)>,
    pub mute: Box<dyn Fn(bool)>,
    pub looping: Box<dyn Fn(bool)>,
}

/// The bar widget and the bits of state it needs.
pub struct Controls {
    pub root: gtk::Box,
    play: gtk::Button,
    play_icon: IconView,
    scale: gtk::Scale,
    time: gtk::Label,
    mute: gtk::ToggleButton,
    volume: gtk::Scale,
    dragging: Rc<Cell<bool>>,
    duration: Cell<u64>,
}

impl Controls {
    /// Build the bar. `looping` is the initial state of the Loop button.
    pub fn new(handlers: Handlers, looping: bool) -> Rc<Self> {
        let h = Rc::new(handlers);
        let play_icon = IconView::new(Icon::Play);
        let play = gtk::Button::new();
        play.set_child(Some(&play_icon.area));
        play.set_tooltip_text(Some("Play (Space)"));
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, SLIDER_MAX, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        let time = gtk::Label::new(Some("00:00 / 00:00"));
        time.add_css_class("player-time");
        let mute_icon = IconView::new(Icon::Volume);
        let mute = gtk::ToggleButton::new();
        mute.set_child(Some(&mute_icon.area));
        mute.set_tooltip_text(Some("Mute (M)"));
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, VOLUME_MAX, 1.0);
        volume.set_draw_value(false);
        volume.set_value(VOLUME_MAX);
        volume.set_size_request(VOLUME_WIDTH, -1);
        let loop_btn = gtk::ToggleButton::new();
        loop_btn.set_child(Some(&IconView::new(Icon::Loop).area));
        loop_btn.set_tooltip_text(Some("Loop"));
        loop_btn.set_active(looping);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.set_margin_start(8);
        root.set_margin_end(8);
        root.set_margin_top(6);
        root.set_margin_bottom(6);
        for w in [play.upcast_ref::<gtk::Widget>(), scale.upcast_ref(), time.upcast_ref(), mute.upcast_ref(), volume.upcast_ref(), loop_btn.upcast_ref()] {
            root.append(w);
            // Keys go to the window (Space, arrows...), never to a focused button.
            w.set_focusable(false);
        }

        let dragging = Rc::new(Cell::new(false));
        let this = Rc::new(Controls { root, play, play_icon, scale, time, mute, volume, dragging, duration: Cell::new(0) });

        let hh = h.clone();
        this.play.connect_clicked(move |_| (hh.toggle_play)());
        // `change-value` only fires for the user, never for `set_value` from code.
        let (hh, dragging) = (h.clone(), this.dragging.clone());
        this.scale.connect_change_value(move |_, _, value| {
            (hh.seek)(value, !dragging.get());
            gtk::glib::Propagation::Proceed
        });
        // Press and release on the slider, watched without claiming the gesture.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let dragging = this.dragging.clone();
        click.connect_pressed(move |_, _, _, _| dragging.set(true));
        let (hh, dragging, scale) = (h.clone(), this.dragging.clone(), this.scale.clone());
        click.connect_released(move |_, _, _, _| {
            dragging.set(false);
            (hh.seek)(scale.value(), true);
        });
        this.scale.add_controller(click);
        let hh = h.clone();
        this.mute.connect_toggled(move |b| {
            mute_icon.set(if b.is_active() { Icon::Muted } else { Icon::Volume });
            (hh.mute)(b.is_active());
        });
        let hh = h.clone();
        this.volume.connect_value_changed(move |s| (hh.volume)(s.value() / VOLUME_MAX));
        let hh = h;
        loop_btn.connect_toggled(move |b| (hh.looping)(b.is_active()));
        this
    }

    /// Label the play button for the current state.
    pub fn set_playing(&self, playing: bool) {
        self.play_icon.set(if playing { Icon::Pause } else { Icon::Play });
        self.play.set_tooltip_text(Some(if playing { "Pause (Space)" } else { "Play (Space)" }));
    }

    /// Remember the length (nanoseconds), used to map the slider.
    pub fn set_duration(&self, duration: u64) {
        self.duration.set(duration);
    }

    /// Show `position`; the slider is left alone while the user drags it.
    pub fn set_position(&self, position: u64) {
        let duration = self.duration.get();
        self.time.set_text(&format!("{} / {}", format_time(position), format_time(duration)));
        if self.dragging.get() { return; }
        self.scale.set_value(ns_to_slider(position, duration));
    }

    /// Flip the Mute button (which applies it through the handler).
    pub fn toggle_mute(&self) {
        self.mute.set_active(!self.mute.is_active());
    }

    /// Move the volume slider by `delta` (0..1 scale); the handler applies it.
    pub fn adjust_volume(&self, delta: f64) {
        self.volume.set_value((self.volume.value() + delta * VOLUME_MAX).clamp(0.0, VOLUME_MAX));
    }
}
