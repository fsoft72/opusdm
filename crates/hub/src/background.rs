//! The desktop background: a rotation of colours, gradients, pictures and
//! videos that cross-fades from one entry to the next.
//!
//! One thread-local controller owns the state (the entry list, the order, the
//! timer, the playing video). Every desktop window registers a `gtk::Stack`
//! through [`new_view`]; the stack holds one page per entry and the controller
//! flips them all together. Colour, gradient and picture pages are boxes
//! painted by the per-entry CSS class of the desktop stylesheet; a video page
//! is a `gtk::Picture` showing the paintable of one `opusdm_media::Engine`,
//! shared by every monitor.

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_hub::bg_sequence::Sequence;
use opusdm_hub::fullscreen::{self, Rect};
use opusdm_ipc::{Background, BackgroundEntry, ImageFit};
use opusdm_media::{Engine, EngineOptions, Event, VideoSink};
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Name of the extra page painted when no entry can be shown.
const FALLBACK_PAGE: &str = "fallback";

/// CSS class of the page that paints entry `index`.
pub(crate) fn page_class(index: usize) -> String {
    format!("opusdm-bg-{index}")
}

/// How often the full screen windows are looked for.
const COVER_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// CSS class of the fallback page.
pub(crate) const FALLBACK_CLASS: &str = "opusdm-bg-fallback";

/// One desktop window's stack of pages.
struct View {
    stack: glib::WeakRef<gtk::Stack>,
    /// The monitor the stack covers.
    monitor: Rect,
    /// The picture of each video page, by entry index.
    pictures: Vec<Option<gtk::Picture>>,
}

impl View {
    /// Rebuild every page from `entries`.
    fn populate(&mut self, stack: &gtk::Stack, entries: &[BackgroundEntry], fade_ms: u32) {
        while let Some(child) = stack.first_child() {
            stack.remove(&child);
        }
        stack.set_transition_duration(fade_ms);
        self.pictures = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let (page, picture) = _build_page(entry, i);
                stack.add_named(&page, Some(&i.to_string()));
                picture
            })
            .collect();
        let fallback = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fallback.add_css_class(FALLBACK_CLASS);
        stack.add_named(&fallback, Some(FALLBACK_PAGE));
    }

    /// Show page `name`, with the cross-fade unless `fade` is false.
    fn show(&self, name: &str, fade: bool) {
        let Some(stack) = self.stack.upgrade() else { return };
        let transition = if fade { gtk::StackTransitionType::Crossfade } else { gtk::StackTransitionType::None };
        stack.set_visible_child_full(name, transition);
    }
}

/// The page of entry `index` and, for a video, its picture.
fn _build_page(entry: &BackgroundEntry, index: usize) -> (gtk::Widget, Option<gtk::Picture>) {
    match entry {
        BackgroundEntry::Video { fit, .. } => {
            let picture = gtk::Picture::new();
            picture.set_can_shrink(true);
            picture.set_content_fit(if *fit == ImageFit::Fit { gtk::ContentFit::Contain } else { gtk::ContentFit::Cover });
            picture.add_css_class(&page_class(index));
            (picture.clone().upcast(), Some(picture))
        }
        _ => {
            let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
            page.add_css_class(&page_class(index));
            (page.upcast(), None)
        }
    }
}

/// Everything the rotation needs between two ticks.
struct Controller {
    /// The settings last installed; `None` before the first `install`.
    installed: Option<Background>,
    entries: Vec<BackgroundEntry>,
    fade_ms: u32,
    views: Vec<View>,
    seq: Sequence,
    /// The entry on screen: `Some(index)`, or `None` for the fallback page.
    current: Option<usize>,
    /// Entries that failed at run time (a video that would not play).
    failed: HashSet<usize>,
    timer: Option<glib::SourceId>,
    /// The playing video, if the current entry is one.
    engine: Option<Engine>,
    /// The video that just faded out, kept until the fade is over.
    retiring: Option<Engine>,
    /// Bumped on every install, so a stale callback can tell it is stale.
    generation: u64,
    /// Every monitor is hidden behind a full screen window: the rotation and
    /// the video are paused.
    covered: bool,
    /// When the rotation timer was armed, and for how long.
    armed: Option<(Instant, Duration)>,
    /// Time left to the entry on screen, kept while `covered` stops the timer.
    remaining: Option<Duration>,
    /// The cover check runs on a timer, started with the first view.
    cover_watch: bool,
}

impl Controller {
    fn new() -> Self {
        Controller {
            installed: None,
            entries: Vec::new(),
            fade_ms: 0,
            views: Vec::new(),
            seq: Sequence::new(0, false, 1),
            current: None,
            failed: HashSet::new(),
            timer: None,
            engine: None,
            retiring: None,
            generation: 0,
            covered: false,
            armed: None,
            remaining: None,
            cover_watch: false,
        }
    }
}

thread_local! {
    /// GTK is single threaded, so a thread-local holds the one controller.
    static CONTROLLER: RefCell<Controller> = RefCell::new(Controller::new());
}

/// The fade actually used: never longer than half the interval, so a page is
/// always fully shown for a while.
pub(crate) fn effective_fade_ms(fade_ms: u32, interval_secs: u32) -> u32 {
    fade_ms.min(interval_secs.saturating_mul(500))
}

/// A random-looking seed for the shuffle, from the clock.
fn _seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

/// A new desktop window's backdrop. It already shows the entry on screen.
pub(crate) fn new_view(monitor: Rect) -> gtk::Stack {
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    CONTROLLER.with(|c| {
        let c = &mut *c.borrow_mut();
        let mut view = View { stack: stack.downgrade(), monitor, pictures: Vec::new() };
        view.populate(&stack, &c.entries, c.fade_ms);
        if let Some(index) = c.current {
            if let (Some(Some(picture)), Some(engine)) = (view.pictures.get(index), c.engine.as_ref()) {
                picture.set_paintable(engine.paintable().as_ref());
            }
        }
        view.show(&_current_page(c.current), false);
        c.views.retain(|v| v.stack.upgrade().is_some());
        c.views.push(view);
        if !c.cover_watch {
            c.cover_watch = true;
            glib::timeout_add_local(COVER_CHECK_INTERVAL, || {
                CONTROLLER.with(|c| _check_cover(&mut c.borrow_mut()));
                glib::ControlFlow::Continue
            });
        }
    });
    stack
}

/// Adopt `bg`: restyle, rebuild every page and restart the rotation. Does
/// nothing when `bg` is what is already installed, so an unrelated theme
/// change does not restart the wallpaper.
pub(crate) fn install(bg: &Background) {
    CONTROLLER.with(|c| {
        let c = &mut *c.borrow_mut();
        if c.installed.as_ref() == Some(bg) {
            return;
        }
        _stop(c);
        c.installed = Some(bg.clone());
        c.entries = bg.effective_entries();
        c.fade_ms = effective_fade_ms(bg.fade_ms, bg.interval_secs);
        c.seq = Sequence::new(c.entries.len(), bg.shuffle, _seed());
        c.failed.clear();
        c.current = None;
        c.generation += 1;
        crate::desktop_windows::install_desktop_css(&c.entries);
        let (entries, fade_ms) = (c.entries.clone(), c.fade_ms);
        c.views.retain(|v| v.stack.upgrade().is_some());
        for view in &mut c.views {
            if let Some(stack) = view.stack.upgrade() {
                view.populate(&stack, &entries, fade_ms);
            }
        }
        _advance(c, false);
    });
}

/// The name of the page for `current`.
fn _current_page(current: Option<usize>) -> String {
    current.map(|i| i.to_string()).unwrap_or_else(|| FALLBACK_PAGE.to_string())
}

/// Cancel the timer and release the videos.
fn _stop(c: &mut Controller) {
    if let Some(id) = c.timer.take() {
        id.remove();
    }
    c.engine = None;
    c.retiring = None;
    c.armed = None;
    c.remaining = None;
}

/// Show the next usable entry, then arm the timer for the one after. With a
/// single usable entry nothing rotates. `fade` is false only for the very
/// first entry of an install.
fn _advance(c: &mut Controller, fade: bool) {
    if let Some(id) = c.timer.take() {
        id.remove();
    }
    c.armed = None;
    c.remaining = None;
    let mut started = None;
    let picked = loop {
        let next = {
            let (entries, failed) = (&c.entries, &c.failed);
            c.seq.next(|i| entries[i].is_usable() && !failed.contains(&i))
        };
        let Some(idx) = next else { break None };
        if !c.entries[idx].is_video() {
            break Some(idx);
        }
        match _start_video(c, idx) {
            Some(engine) => {
                started = Some(engine);
                break Some(idx);
            }
            None => {
                c.failed.insert(idx);
            }
        }
    };
    if picked.is_none() {
        opusdm_ipc::err_log!("opusdm-hub", "no desktop background entry can be shown; using the default colour");
    }
    if let Some(old) = std::mem::replace(&mut c.engine, started) {
        _retire(c, old);
    }
    c.current = picked;
    let name = _current_page(picked);
    for view in &c.views {
        view.show(&name, fade && c.fade_ms > 0);
    }
    let usable_count = (0..c.entries.len()).filter(|&i| c.entries[i].is_usable() && !c.failed.contains(&i)).count();
    if c.covered {
        if let Some(engine) = &c.engine {
            let _ = engine.pause();
        }
    }
    if usable_count > 1 {
        let interval = c.installed.as_ref().map(|bg| Duration::from_secs(bg.interval_secs.into()));
        match interval {
            Some(interval) if c.covered => c.remaining = Some(interval),
            Some(interval) => _arm_timer(c, interval),
            None => {}
        }
    }
}

/// Start the rotation timer: when it fires after `delay`, move on to the
/// next entry.
fn _arm_timer(c: &mut Controller, delay: Duration) {
    let generation = c.generation;
    c.armed = Some((Instant::now(), delay));
    c.timer = Some(glib::timeout_add_local_once(delay, move || {
        CONTROLLER.with(|c| {
            let c = &mut *c.borrow_mut();
            c.timer = None;
            if c.generation == generation {
                _advance(c, true);
            }
        });
    }));
}

/// Look for full screen windows. When they hide every monitor the video
/// pauses and the rotation timer stops, keeping the time the entry has left;
/// when the desktop shows again both go on from where they were.
fn _check_cover(c: &mut Controller) {
    let monitors: Vec<Rect> = c.views.iter().filter(|v| v.stack.upgrade().is_some()).map(|v| v.monitor).collect();
    let covered = match fullscreen::fullscreen_windows() {
        Ok(windows) => fullscreen::covers_all(&monitors, &windows),
        Err(e) => {
            opusdm_ipc::debug_log!("opusdm-hub", "cannot look for full screen windows: {e}");
            false
        }
    };
    if covered == c.covered {
        return;
    }
    c.covered = covered;
    opusdm_ipc::debug_log!("opusdm-hub", "desktop background {}", if covered { "paused: hidden by full screen windows" } else { "resumed" });
    if covered {
        if let Some(id) = c.timer.take() {
            id.remove();
        }
        if let Some((since, delay)) = c.armed.take() {
            c.remaining = Some(delay.saturating_sub(since.elapsed()));
        }
        if let Some(engine) = &c.engine {
            let _ = engine.pause();
        }
        return;
    }
    if let Some(engine) = &c.engine {
        let _ = engine.play();
    }
    if let Some(left) = c.remaining.take() {
        _arm_timer(c, left);
    }
}

/// Keep `old` playing until the fade that hides it is over, then drop it.
fn _retire(c: &mut Controller, old: Engine) {
    c.retiring = Some(old);
    let generation = c.generation;
    glib::timeout_add_local_once(Duration::from_millis(c.fade_ms.into()), move || {
        CONTROLLER.with(|c| {
            let c = &mut *c.borrow_mut();
            if c.generation == generation {
                c.retiring = None;
            }
        });
    });
}

/// Open and play the video of entry `idx` on a fresh engine and hand its
/// frames to every view. `None` when the video cannot be started.
fn _start_video(c: &Controller, idx: usize) -> Option<Engine> {
    let BackgroundEntry::Video { path, .. } = &c.entries[idx] else { return None };
    let generation = c.generation;
    let options = EngineOptions { audio: false, looping: true, video_sink: VideoSink::Paintable };
    let engine = Engine::new(options, move |event| {
        if let Event::Error(e) = event {
            opusdm_ipc::err_log!("opusdm-hub", "desktop background video failed: {e}");
            _video_failed(generation, idx);
        }
    })
    .inspect_err(|e| opusdm_ipc::err_log!("opusdm-hub", "cannot start the desktop background video: {e}"))
    .ok()?;
    engine
        .open(&path.to_string_lossy())
        .and_then(|_| engine.play())
        .inspect_err(|e| opusdm_ipc::err_log!("opusdm-hub", "cannot play {}: {e}", path.display()))
        .ok()?;
    let paintable = engine.paintable();
    for view in &c.views {
        if let Some(Some(picture)) = view.pictures.get(idx) {
            picture.set_paintable(paintable.as_ref());
        }
    }
    Some(engine)
}

/// A video reported an error while playing: skip it from now on, and move on
/// at once when it is the entry on screen.
fn _video_failed(generation: u64, idx: usize) {
    CONTROLLER.with(|c| {
        let c = &mut *c.borrow_mut();
        if c.generation != generation {
            return;
        }
        c.failed.insert(idx);
        if c.current == Some(idx) {
            _advance(c, true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fade_never_exceeds_half_the_interval() {
        assert_eq!(effective_fade_ms(1000, 300), 1000);
        assert_eq!(effective_fade_ms(10_000, 5), 2500);
        assert_eq!(effective_fade_ms(0, 5), 0);
    }

    #[test]
    fn page_classes_are_per_entry() {
        assert_eq!(page_class(0), "opusdm-bg-0");
        assert_eq!(page_class(12), "opusdm-bg-12");
    }
}
