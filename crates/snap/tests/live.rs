//! Live check of the snapper against a real nested X server and Openbox,
//! driven with XTEST pointer events. Ignored by default: run it through
//! `scripts/snap-live-test.sh`, which starts Xephyr and Openbox, or point
//! `DISPLAY` at a session that has no other window the test could disturb.

use opusdm_snap::x11::Display;
use opusdm_snap::zones::{zone_at, zone_rect, Rect, Zone};
use opusdm_snap::Watcher;
use opusdm_x11::Session;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt, CreateWindowAux, EventMask, Window, WindowClass};
use x11rb::protocol::xtest;
use x11rb::wrapper::ConnectionExt as _;

/// XTEST event types.
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// Client size of the test window.
const CLIENT: (u16, u16) = (500, 300);

/// Pixels of difference allowed between a requested and a real rectangle.
const TOLERANCE: i32 = 3;

/// Time given to the window manager and the watcher to settle after a step.
const SETTLE: Duration = Duration::from_millis(700);

/// Time the pointer rests at each end of a drag, so the 16 ms watcher sees it.
const HOLD: Duration = Duration::from_millis(250);

/// Steps of one pointer movement, and the pause after each.
const DRAG_STEPS: i32 = 25;
const STEP_PAUSE: Duration = Duration::from_millis(15);

/// What a test needs: a connection for the test client and for XTEST.
struct Rig {
    session: Session,
    display: Display,
}

impl Rig {
    fn new() -> Rig {
        let rig = Rig { session: opusdm_x11::connect().expect("an X server is needed (set DISPLAY)"), display: Display::connect().expect("snap display") };
        rig.wait_for(|| rig.root_has("_NET_SUPPORTING_WM_CHECK") && rig.root_has("_NET_WORKAREA"), "the window manager to be up");
        rig
    }

    /// Whether the root window carries the property `name`.
    fn root_has(&self, name: &str) -> bool {
        let conn = &self.session.conn;
        let atom = conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom;
        let reply = conn.get_property(false, self.session.root, atom, AtomEnum::ANY, 0, 1).unwrap().reply().unwrap();
        reply.type_ != x11rb::NONE
    }

    /// Create, map and wait for a framed, active window; return its id.
    fn client_window(&self, x: i16, y: i16) -> Window {
        let conn = &self.session.conn;
        let window = conn.generate_id().unwrap();
        conn.create_window(
            0,
            window,
            self.session.root,
            x,
            y,
            CLIENT.0,
            CLIENT.1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::EXPOSURE),
        )
        .unwrap();
        conn.change_property8(xproto::PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, b"snap live test").unwrap();
        conn.map_window(window).unwrap();
        conn.flush().unwrap();
        self.wait_for(|| self.is_active(window) && self.frame(window).is_some_and(|f| f.w > i32::from(CLIENT.0)), "the window to be framed and active");
        window
    }

    /// Poll `condition` for up to five seconds.
    fn wait_for(&self, condition: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Create and map a dock of `height` pixels along the bottom edge that
    /// reserves its band with `_NET_WM_STRUT_PARTIAL`.
    fn bottom_dock(&self, screen: Rect, height: u16) -> Window {
        let conn = &self.session.conn;
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom;
        let window = conn.generate_id().unwrap();
        let (width, y) = (screen.w as u16, (screen.y + screen.h) as i16 - height as i16);
        conn.create_window(0, window, self.session.root, screen.x as i16, y, width, height, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.change_property32(xproto::PropMode::REPLACE, window, atom("_NET_WM_WINDOW_TYPE"), AtomEnum::ATOM, &[atom("_NET_WM_WINDOW_TYPE_DOCK")]).unwrap();
        let strut = [0, 0, 0, u32::from(height), 0, 0, 0, 0, 0, 0, screen.x as u32, (screen.x + screen.w - 1) as u32];
        conn.change_property32(xproto::PropMode::REPLACE, window, atom("_NET_WM_STRUT_PARTIAL"), AtomEnum::CARDINAL, &strut).unwrap();
        conn.map_window(window).unwrap();
        conn.flush().unwrap();
        window
    }

    fn is_active(&self, window: Window) -> bool {
        let conn = &self.session.conn;
        let atom = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").unwrap().reply().unwrap().atom;
        let reply = conn.get_property(false, self.session.root, atom, AtomEnum::WINDOW, 0, 1).unwrap().reply().unwrap();
        reply.value32().and_then(|mut v| v.next()) == Some(window)
    }

    /// The frame rectangle of `window`.
    fn frame(&self, window: Window) -> Option<Rect> {
        self.display.frame_rect(window).unwrap()
    }

    /// The usable area under `pointer`.
    fn area(&self, pointer: (i32, i32)) -> Rect {
        self.display.environment().unwrap().area(pointer)
    }

    fn move_pointer(&self, to: (i32, i32)) {
        let conn = &self.session.conn;
        xtest::fake_input(conn, MOTION_NOTIFY, 0, 0, self.session.root, to.0 as i16, to.1 as i16, 0).unwrap();
        conn.flush().unwrap();
    }

    fn button(&self, down: bool) {
        let conn = &self.session.conn;
        xtest::fake_input(conn, if down { BUTTON_PRESS } else { BUTTON_RELEASE }, 1, 0, self.session.root, 0, 0, 0).unwrap();
        conn.flush().unwrap();
    }

    /// Press the left button at `from`, move to `to` in steps, rest, release.
    /// `during` runs while the button is held at `to`.
    fn drag(&self, from: (i32, i32), to: (i32, i32), during: impl FnOnce()) {
        self.move_pointer(from);
        std::thread::sleep(STEP_PAUSE * 4);
        self.button(true);
        std::thread::sleep(HOLD);
        for step in 1..=DRAG_STEPS {
            self.move_pointer((from.0 + (to.0 - from.0) * step / DRAG_STEPS, from.1 + (to.1 - from.1) * step / DRAG_STEPS));
            std::thread::sleep(STEP_PAUSE);
        }
        std::thread::sleep(HOLD);
        during();
        self.button(false);
        std::thread::sleep(SETTLE);
    }

    /// A point on the title bar of `window`.
    fn title_point(&self, window: Window) -> (i32, i32) {
        let frame = self.frame(window).expect("the window is mapped");
        (frame.x + frame.w / 2, frame.y + 8)
    }

    /// Drag the title bar of `window` to `to`.
    fn drag_title(&self, window: Window, to: (i32, i32), during: impl FnOnce()) {
        self.drag(self.title_point(window), to, during);
    }

    fn close(&self, window: Window) {
        self.session.conn.destroy_window(window).unwrap();
        self.session.conn.flush().unwrap();
    }
}

/// Whether two rectangles are the same within [`TOLERANCE`].
fn near(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= TOLERANCE && (a.y - b.y).abs() <= TOLERANCE && (a.w - b.w).abs() <= TOLERANCE && (a.h - b.h).abs() <= TOLERANCE
}

/// The 32 bit override-redirect windows under the root with their rectangles:
/// where the preview of the snapper lives while a zone is armed.
fn preview_windows(rig: &Rig) -> Vec<Rect> {
    let conn = &rig.session.conn;
    let tree = conn.query_tree(rig.session.root).unwrap().reply().unwrap();
    let mut found = Vec::new();
    for child in tree.children {
        let (Ok(attrs), Ok(geometry)) = (conn.get_window_attributes(child).unwrap().reply(), conn.get_geometry(child).unwrap().reply()) else { continue };
        if attrs.override_redirect && attrs.map_state == xproto::MapState::VIEWABLE && geometry.depth == 32 {
            found.push(Rect::new(i32::from(geometry.x), i32::from(geometry.y), i32::from(geometry.width), i32::from(geometry.height)));
        }
    }
    found
}

#[test]
#[ignore = "needs an X server with Openbox; run scripts/snap-live-test.sh"]
fn snapping_and_restoring_windows_on_a_real_session() {
    let _watcher = Watcher::start(true).expect("the snap watcher starts");
    std::thread::sleep(Duration::from_millis(500));
    let rig = Rig::new();
    let compositor = rig.display.compositor_running().unwrap();
    println!("compositor running: {compositor}");

    // Every zone: drag the title bar to a point that arms it, release there.
    let screen = rig.area((10, 10));
    let probes: [((i32, i32), Zone); 7] = [
        ((screen.x + screen.w / 2, screen.y + 2), Zone::Maximize),
        ((screen.x + 2, screen.y + 2), Zone::TopLeft),
        ((screen.x + screen.w - 3, screen.y + 2), Zone::TopRight),
        ((screen.x + 2, screen.y + screen.h - 3), Zone::BottomLeft),
        ((screen.x + screen.w - 3, screen.y + screen.h - 3), Zone::BottomRight),
        ((screen.x + 2, screen.y + screen.h / 2), Zone::Left),
        ((screen.x + screen.w - 3, screen.y + screen.h / 2), Zone::Right),
    ];
    for (point, zone) in probes {
        assert_eq!(zone_at(point, screen), Some(zone), "the probe point arms {zone:?}");
        let window = rig.client_window(300, 200);
        let expected = zone_rect(zone, screen);
        rig.drag_title(window, point, || {
            if compositor {
                let previews = preview_windows(&rig);
                assert!(previews.iter().any(|r| near(*r, expected)), "the preview of {zone:?} shows at {expected:?}, found {previews:?}");
            }
        });
        let frame = rig.frame(window).unwrap();
        assert!(near(frame, expected), "{zone:?}: the window is at {frame:?}, expected {expected:?}");
        if compositor {
            assert!(preview_windows(&rig).is_empty(), "the preview is gone after the release");
        }
        rig.close(window);
        std::thread::sleep(Duration::from_millis(300));
    }

    // The bottom edge between the corners has no zone: the window just moves.
    let window = rig.client_window(300, 200);
    let before = rig.frame(window).unwrap();
    let middle_bottom = (screen.x + screen.w / 2, screen.y + screen.h - 3);
    assert_eq!(zone_at(middle_bottom, screen), None);
    rig.drag_title(window, middle_bottom, || {});
    let after = rig.frame(window).unwrap();
    assert_eq!((after.w, after.h), (before.w, before.h), "a drop outside every zone keeps the size");
    assert!(after.y > before.y, "the window followed the pointer down");
    rig.close(window);

    // Snap, then drag away: the pre-snap size comes back under the pointer.
    let window = rig.client_window(300, 200);
    let original = rig.frame(window).unwrap();
    let left = (screen.x + 2, screen.y + screen.h / 2);
    rig.drag_title(window, left, || {});
    let snapped = rig.frame(window).unwrap();
    assert!(near(snapped, zone_rect(Zone::Left, screen)), "snapped left: {snapped:?}");
    let away = (screen.x + screen.w / 2, screen.y + screen.h / 2);
    rig.drag_title(window, away, || {});
    let restored = rig.frame(window).unwrap();
    assert!(near(Rect::new(0, 0, restored.w, restored.h), Rect::new(0, 0, original.w, original.h)), "restored size {restored:?}, original {original:?}");
    assert!(restored.contains(away), "the pointer is still inside the restored window {restored:?}");
    rig.close(window);

    // A second snap after a restore remembers the FIRST size.
    let window = rig.client_window(300, 200);
    let first = rig.frame(window).unwrap();
    rig.drag_title(window, left, || {});
    rig.drag_title(window, away, || {});
    rig.drag_title(window, left, || {});
    rig.drag_title(window, away, || {});
    let again = rig.frame(window).unwrap();
    assert!((again.w - first.w).abs() <= TOLERANCE && (again.h - first.h).abs() <= TOLERANCE, "size after two cycles {again:?}, first {first:?}");
    rig.close(window);

    // A dock that reserves a band with a strut shrinks the zones of its monitor.
    let dock = rig.bottom_dock(screen, 40);
    rig.wait_for(|| rig.area((10, 10)).h == screen.h - 40, "the strut of the dock to count");
    let area = rig.area((10, 10));
    assert_eq!((area.x, area.y, area.w), (screen.x, screen.y, screen.w), "only the bottom band is reserved: {area:?}");
    let window = rig.client_window(300, 200);
    rig.drag_title(window, (area.x + area.w / 2, area.y + 2), || {});
    let frame = rig.frame(window).unwrap();
    assert!(near(frame, area), "maximized above the dock: {frame:?}, area {area:?}");
    rig.close(window);
    rig.close(dock);
}

#[test]
#[ignore = "needs an X server; run scripts/snap-live-test.sh"]
fn an_idle_display_sleeps_until_a_button_event() {
    let rig = Rig::new();
    // Nothing happens: the wait runs to its timeout instead of returning early.
    let started = Instant::now();
    rig.display.wait_for_button(Duration::from_millis(400)).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(350), "woke after {:?} with no event", started.elapsed());

    // A button press wakes it at once, long before the timeout.
    let waiter = std::thread::spawn(|| {
        let display = Display::connect().unwrap();
        let started = Instant::now();
        display.wait_for_button(Duration::from_secs(5)).unwrap();
        started.elapsed()
    });
    std::thread::sleep(Duration::from_millis(500));
    rig.button(true);
    rig.button(false);
    let slept = waiter.join().unwrap();
    assert!(slept < Duration::from_secs(2), "the press did not wake the display, slept {slept:?}");
}

#[test]
#[ignore = "needs an X server; run scripts/snap-live-test.sh"]
fn the_preview_is_clipped_to_rounded_corners_and_painted_with_a_border() {
    use opusdm_snap::overlay::{argb, rounded_spans, Overlay, ACCENT, BORDER, CORNER_RADIUS, FILL_ALPHA, LINE_ALPHA};
    use x11rb::protocol::shape::{self, SK};

    let rig = Rig::new();
    // No compositor is needed to look at the clip and the pixels.
    let mut overlay = Overlay::create(&rig.display, false).unwrap().expect("a 32 bit visual");
    let rect = Rect::new(100, 80, 300, 200);
    overlay.show(rect, 0);
    overlay.tick(&rig.display, 10_000).unwrap();
    rig.session.conn.flush().unwrap();
    let window = overlay.window();
    let conn = &rig.session.conn;

    // The bounding region is the rounded rectangle.
    let region = shape::get_rectangles(conn, window, SK::BOUNDING).unwrap().reply().unwrap();
    let mut got: Vec<(i16, i16, u16, u16)> = region.rectangles.iter().map(|r| (r.x, r.y, r.width, r.height)).collect();
    let mut want: Vec<(i16, i16, u16, u16)> = rounded_spans(rect.w, rect.h, CORNER_RADIUS).iter().map(|r| (r.x as i16, r.y as i16, r.w as u16, r.h as u16)).collect();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want, "the bounding region of the preview");

    // The border colour on the edge, the translucent fill inside.
    let image = conn.get_image(xproto::ImageFormat::Z_PIXMAP, window, 0, 0, rect.w as u16, rect.h as u16, !0).unwrap().reply().unwrap();
    let pixel = |x: i32, y: i32| {
        let at = ((y * rect.w + x) * 4) as usize;
        u32::from_le_bytes([image.data[at], image.data[at + 1], image.data[at + 2], image.data[at + 3]])
    };
    assert_eq!(pixel(rect.w / 2, 0), argb(ACCENT, LINE_ALPHA), "top edge");
    assert_eq!(pixel(rect.w / 2, rect.h / 2), argb(ACCENT, FILL_ALPHA), "inside");
    assert_eq!(pixel(rect.w / 2, BORDER - 1), argb(ACCENT, LINE_ALPHA), "last border row");
    assert_eq!(pixel(rect.w / 2, BORDER), argb(ACCENT, FILL_ALPHA), "first fill row");
}
