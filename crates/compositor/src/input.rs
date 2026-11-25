//! Pointer and keyboard input: hit test, click focus, Alt + drag grabs,
//! global hotkeys matched before the focused client.
//!
//! Backend independent: the backend (nested, task 7) turns its events into
//! calls of `pointer_motion`, `pointer_button`, `pointer_axis` and
//! `keyboard_key`, in output (= space) coordinates.

use smithay::{
    backend::input::{ButtonState, KeyState},
    desktop::{layer_map_for_output, space::SpaceElement, LayerSurface, Window, WindowSurfaceType},
    input::{
        keyboard::{xkb, FilterResult, Keycode, Keysym, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
};

use crate::decor::geometry::{frame_rect, frame_zone, FrameZone, Insets, TitleButton};
use crate::geometry::{Edges, Rect};
use crate::grabs::edges_for_point;
use crate::hotkeys::{combo_matches, combo_text, mods_of, Binding};
use crate::menu::model::MenuKey;
use crate::switcher::SwitcherKey;
use opusdm_ipc::bindings::{KeyCombo, Mods};
use crate::state::{State, LOG_TAG};
use crate::wm::WindowId;

/// Linux input code of the left mouse button (`BTN_LEFT`).
pub const BTN_LEFT: u32 = 0x110;

/// Linux input code of the right mouse button (`BTN_RIGHT`).
pub const BTN_RIGHT: u32 = 0x111;

/// Linux input code of the middle mouse button (`BTN_MIDDLE`).
pub const BTN_MIDDLE: u32 = 0x112;

/// Longest time between two left presses on the same title bar that make a
/// double click, in milliseconds (Openbox `doubleClickTime`).
pub const DOUBLE_CLICK_MS: u32 = 500;

/// What the pointer does on the server side frames (title bar buttons,
/// double click, cursor). Only window ids: a window that goes is simply
/// not found any more.
#[derive(Debug, Clone, Default)]
pub struct FramePointer {
    /// The title bar button under the pointer and its window (drawn hovered).
    pub hovered: Option<(WindowId, TitleButton)>,
    /// The title bar button the left button was pressed on: released on
    /// the same button it runs (drawn pressed while the pointer is on it).
    pub pressed: Option<(WindowId, TitleButton)>,
    /// The window and the time (ms) of the last left press on a title,
    /// for the double click.
    pub last_title_press: Option<(WindowId, u32)>,
    /// True while the cursor is the one the compositor set for a frame.
    pub cursor_set: bool,
}

/// What is under a point: a surface, or the frame of a decorated window.
#[derive(Debug, Clone)]
pub enum Under {
    /// A surface that gets the pointer.
    Surface(Hit),
    /// A part of the frame of a decorated window (never `FrameZone::Client`).
    Frame(Window, FrameZone),
}

/// The cursor shown over the frame `zone`: the resize arrow of a border
/// (its corner on a corner), the default arrow elsewhere.
pub fn frame_cursor(zone: FrameZone) -> CursorIcon {
    let FrameZone::Border(edges) = zone else { return CursorIcon::Default };
    match edges {
        Edges { top: true, left: true, .. } => CursorIcon::NwResize,
        Edges { top: true, right: true, .. } => CursorIcon::NeResize,
        Edges { bottom: true, left: true, .. } => CursorIcon::SwResize,
        Edges { bottom: true, right: true, .. } => CursorIcon::SeResize,
        Edges { top: true, .. } => CursorIcon::NResize,
        Edges { bottom: true, .. } => CursorIcon::SResize,
        Edges { left: true, .. } => CursorIcon::WResize,
        Edges { right: true, .. } => CursorIcon::EResize,
        _ => CursorIcon::Default,
    }
}

/// True when the left presses at `first` and `then` (ms, the clock may
/// wrap) are at most `DOUBLE_CLICK_MS` apart.
pub fn is_double_click(first: u32, then: u32) -> bool {
    then.wrapping_sub(first) <= DOUBLE_CLICK_MS
}

/// Where the key filter sends an intercepted key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRoute {
    /// Nothing runs: the release or repeat of a swallowed key, or a key
    /// the open window menu does not use.
    Swallowed,
    /// A binding matched by the key (its combo text for the logs).
    Binding(Binding, String),
    /// A key for the open window menu.
    Menu(MenuKey),
    /// A key for the open window switcher.
    Switcher(SwitcherKey),
}

/// The window menu key of `sym`: arrows (keypad too), Return / KP_Enter
/// and Escape; `None` for any other key.
pub fn menu_key_of(sym: Keysym) -> Option<MenuKey> {
    let key = match sym {
        Keysym::Up | Keysym::KP_Up => MenuKey::Up,
        Keysym::Down | Keysym::KP_Down => MenuKey::Down,
        Keysym::Left | Keysym::KP_Left => MenuKey::Left,
        Keysym::Right | Keysym::KP_Right => MenuKey::Right,
        Keysym::Return | Keysym::KP_Enter => MenuKey::Enter,
        Keysym::Escape => MenuKey::Escape,
        _ => return None,
    };
    Some(key)
}

/// One step of the pointer hit test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitSlot {
    /// Layer surfaces on the OVERLAY layer.
    Overlay,
    /// The topmost window when it is fullscreen (shown above TOP).
    Fullscreen,
    /// Layer surfaces on the TOP layer.
    Top,
    /// The windows, topmost first.
    Windows,
    /// Layer surfaces on the BOTTOM layer.
    Bottom,
    /// Layer surfaces on the BACKGROUND layer (the desktop).
    Background,
}

/// Order of the pointer hit test, the reverse of the render order.
pub const HIT_ORDER: [HitSlot; 6] =
    [HitSlot::Overlay, HitSlot::Fullscreen, HitSlot::Top, HitSlot::Windows, HitSlot::Bottom, HitSlot::Background];

/// Returns the first hit of `probe` over the slots of `HIT_ORDER`.
pub fn first_hit<T>(mut probe: impl FnMut(HitSlot) -> Option<T>) -> Option<T> {
    HIT_ORDER.iter().find_map(|slot| probe(*slot))
}

/// What a hit surface belongs to.
#[derive(Debug, Clone)]
pub enum HitOwner {
    /// A toplevel window (the surface may be one of its popups or subsurfaces).
    Window(Window),
    /// A layer surface (the surface may be one of its popups or subsurfaces).
    Layer(LayerSurface),
}

/// The surface under a point.
#[derive(Debug, Clone)]
pub struct Hit {
    /// The surface that gets the pointer.
    pub surface: WlSurface,
    /// Origin of `surface` in the space.
    pub location: Point<f64, Logical>,
    /// The window or layer surface it belongs to.
    pub owner: HitOwner,
}

impl State {
    /// Surface under `pos` (space coordinates), searched in `HIT_ORDER`;
    /// popups are hit before their parent. `None` on the frame of a
    /// decorated window (no client gets the pointer there).
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        match self.under(pos)? {
            Under::Surface(hit) => Some(hit),
            Under::Frame(..) => None,
        }
    }

    /// What is under `pos` (space coordinates), searched in `HIT_ORDER`:
    /// a surface (popups before their parent) or, among the windows, the
    /// frame of a decorated window, which covers what is below it.
    pub fn under(&self, pos: Point<f64, Logical>) -> Option<Under> {
        first_hit(|slot| match slot {
            HitSlot::Overlay => self._layer_hit(Layer::Overlay, pos).map(Under::Surface),
            HitSlot::Fullscreen => self._fullscreen_hit(pos).map(Under::Surface),
            HitSlot::Top => self._layer_hit(Layer::Top, pos).map(Under::Surface),
            HitSlot::Windows => self._windows_hit(pos),
            HitSlot::Bottom => self._layer_hit(Layer::Bottom, pos).map(Under::Surface),
            HitSlot::Background => self._layer_hit(Layer::Background, pos).map(Under::Surface),
        })
    }

    /// The pointer moved to `position` (space coordinates): the surface under
    /// it gets the pointer focus (an active grab decides instead).
    ///
    /// A motion that ends a grab is sent once more. Smithay 0.7's popup grab
    /// (`PopupPointerGrab::motion`), once its menus are gone, only unsets
    /// itself and drops that motion: the pointer kept its old position and
    /// the focus restored by `unset_grab` entered with those stale
    /// coordinates, so the next click went where the menu had been (the
    /// first pager click after a tasklist menu was lost). The second motion
    /// corrects position and focus in the same frame. Our move / resize
    /// grabs also end on a motion (their window died or was unmapped), but
    /// they forward the motion first: there the second one repeats one
    /// `wl_pointer.motion` with the same coordinates in the same frame
    /// (harmless). A drag whose source died since the last `refresh` is
    /// cancelled first (`cancel_dead_drag`), so the motion never reaches it.
    pub fn pointer_motion(&mut self, position: Point<f64, Logical>, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer motion: the seat has no pointer");
            return;
        };
        self.cancel_dead_drag();
        let was_grabbed = pointer.is_grabbed();
        let found = self.under(position);
        let frame = match &found {
            Some(Under::Frame(window, zone)) if !was_grabbed => Some((window.clone(), *zone)),
            _ => None,
        };
        let on_surface = matches!(found, Some(Under::Surface(_)));
        let under = match found {
            Some(Under::Surface(hit)) => Some((hit.surface, hit.location)),
            _ => None,
        };
        self._frame_motion(frame, on_surface);
        let event = MotionEvent { location: position, serial: SERIAL_COUNTER.next_serial(), time: time_msec };
        pointer.motion(self, under, &event);
        if was_grabbed && !pointer.is_grabbed() {
            opusdm_ipc::debug_log!(LOG_TAG, "pointer grab ended by a motion: motion sent again");
            let under = self.surface_under(position).map(|hit| (hit.surface, hit.location));
            let event = MotionEvent { location: position, serial: SERIAL_COUNTER.next_serial(), time: time_msec };
            pointer.motion(self, under, &event);
        }
        pointer.frame(self);
    }

    /// A pointer button changed. While the window menu is open a press goes
    /// to the menu (`window_menu_click`) and neither it nor its release
    /// reach a client. A press with no active grab on the frame of a
    /// decorated window goes to `_frame_press` (Openbox mousebinds: title
    /// bar buttons, move, double click, window menu, lower, border resize).
    /// Any other press with no active grab focuses what is
    /// under the pointer (`_click_focus`); with Alt held on a window, the left
    /// button starts a `MoveGrab` and the right button a `ResizeGrab` on the
    /// edges of the quadrant the pointer is in: the grab consumes the press
    /// and its release, the client never sees them. A dead drag is
    /// cancelled first (`cancel_dead_drag`): its release drops nothing.
    pub fn pointer_button(&mut self, button: u32, pressed: bool, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer button: the seat has no pointer");
            return;
        };
        self.cancel_dead_drag();
        if pressed && self.menu.is_some() {
            self.swallowed_buttons.insert(button);
            self.window_menu_click(pointer.current_location());
            return;
        }
        if !pressed && self.swallowed_buttons.remove(&button) {
            opusdm_ipc::debug_log!(LOG_TAG, "release of button {button:#x} swallowed: its press went to the window menu or a frame");
            self._frame_release(button, pointer.current_location());
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
        if pressed && !pointer.is_grabbed() {
            let location = pointer.current_location();
            match self.under(location) {
                Some(Under::Frame(window, zone)) => {
                    if self._frame_press(window, zone, button, location, serial, time_msec) {
                        self.swallowed_buttons.insert(button);
                        return;
                    }
                }
                found => {
                    let hit = match found {
                        Some(Under::Surface(hit)) => Some(hit),
                        _ => None,
                    };
                    self._click_focus(hit.as_ref());
                    if let Some(Hit { surface, location: origin, owner: HitOwner::Window(window) }) = hit {
                        self._alt_grab(window, Some((surface, origin)), button, location, serial);
                    }
                }
            }
        }
        // With a grab just started this press goes to the grab, which drops it.
        self._send_button(button, state, serial, time_msec);
    }

    /// Sends the button event to the pointer (its grab, or the surface
    /// with the pointer focus) and ends the pointer frame.
    fn _send_button(&mut self, button: u32, state: ButtonState, serial: Serial, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer button: the seat has no pointer");
            return;
        };
        pointer.button(self, &ButtonEvent { serial, time: time_msec, button, state });
        pointer.frame(self);
    }

    /// The pointer moved, with no grab, onto the frame part `frame` (or off
    /// every frame: `None`; `on_surface` when it is on a client surface):
    /// the hovered title bar button follows it, and the cursor is the one
    /// of the frame part (`frame_cursor`); leaving the frames sets the
    /// default cursor once (a client surface sets its own on enter).
    fn _frame_motion(&mut self, frame: Option<(Window, FrameZone)>, on_surface: bool) {
        let Some((window, zone)) = frame else {
            self.frame_pointer.hovered = None;
            if self.frame_pointer.cursor_set {
                self.frame_pointer.cursor_set = false;
                if !on_surface {
                    self.cursor_status = CursorImageStatus::default_named();
                }
            }
            return;
        };
        let id = self._toplevel_id(&window);
        self.frame_pointer.hovered = match (id, zone) {
            (Some(id), FrameZone::Button(button)) => Some((id, button)),
            _ => None,
        };
        self.cursor_status = CursorImageStatus::Named(frame_cursor(zone));
        self.frame_pointer.cursor_set = true;
    }

    /// A press of `button` with no grab on the `zone` of the frame of
    /// `window` (Openbox mousebinds). Middle on the title or a border
    /// lowers the window. Left and right raise and focus it first; with
    /// Alt held they start the Alt move / resize grab as on the client.
    /// Left on a button arms it (`_frame_release` runs it), on the title
    /// starts a `MoveGrab` (a second press within `DOUBLE_CLICK_MS` toggles
    /// maximize instead), on a border a `ResizeGrab` from its edges. Right
    /// on the title or a border opens the window menu at the pointer.
    /// Returns true when the press (and its release) is swallowed, false
    /// when it goes on to the pointer (a grab just started takes it).
    fn _frame_press(
        &mut self,
        window: Window,
        zone: FrameZone,
        button: u32,
        location: Point<f64, Logical>,
        serial: Serial,
        time_msec: u32,
    ) -> bool {
        let Some(id) = self._toplevel_id(&window) else {
            opusdm_ipc::err_log!(LOG_TAG, "frame press: the window is not in the table, press dropped");
            return true;
        };
        if button == BTN_MIDDLE {
            if matches!(zone, FrameZone::Title | FrameZone::Border(_)) {
                if let Err(e) = self.lower_window(id) {
                    opusdm_ipc::err_log!(LOG_TAG, "frame middle press: {e}");
                }
            }
            return true;
        }
        if button != BTN_LEFT && button != BTN_RIGHT {
            opusdm_ipc::debug_log!(LOG_TAG, "frame press of button {button:#x} on window {}: no binding", id.0);
            return true;
        }
        self.focus_window(Some(window.clone()));
        if self._alt_held() {
            self._alt_grab(window, None, button, location, serial);
            return false;
        }
        let start_data = GrabStartData { focus: None, button, location };
        match (button, zone) {
            (BTN_RIGHT, FrameZone::Title | FrameZone::Border(_)) => {
                let at = location.to_i32_floor();
                self.open_window_menu_at((at.x, at.y));
                true
            }
            (BTN_LEFT, FrameZone::Button(title_button)) => {
                self.frame_pointer.pressed = Some((id, title_button));
                opusdm_ipc::debug_log!(LOG_TAG, "title button {title_button:?} of window {} pressed", id.0);
                true
            }
            (BTN_LEFT, FrameZone::Title) => {
                let last = self.frame_pointer.last_title_press.take();
                if last.is_some_and(|(last_id, at)| last_id == id && is_double_click(at, time_msec)) {
                    opusdm_ipc::debug_log!(LOG_TAG, "double click on the title of window {}: toggle maximize", id.0);
                    if let Err(e) = self.toggle_maximize(id) {
                        opusdm_ipc::err_log!(LOG_TAG, "title double click: {e}");
                    }
                    return true;
                }
                self.frame_pointer.last_title_press = Some((id, time_msec));
                self.start_move_grab(window, start_data, serial);
                false
            }
            (BTN_LEFT, FrameZone::Border(edges)) => {
                self.start_resize_grab(window, edges, start_data, serial);
                false
            }
            _ => true,
        }
    }

    /// The release of a swallowed `button` at `location`: when it is the
    /// left button and a title bar button was pressed, that button runs if
    /// the pointer is still on it (Close = `close_window`, Iconify =
    /// `set_minimized`, Maximize = `toggle_maximize`); off it nothing runs.
    fn _frame_release(&mut self, button: u32, location: Point<f64, Logical>) {
        if button != BTN_LEFT { return; }
        let Some((id, title_button)) = self.frame_pointer.pressed.take() else { return };
        let still_on = match self.under(location) {
            Some(Under::Frame(window, FrameZone::Button(b))) => b == title_button && self._toplevel_id(&window) == Some(id),
            _ => false,
        };
        if !still_on {
            opusdm_ipc::debug_log!(LOG_TAG, "title button {title_button:?} of window {} released off it: nothing runs", id.0);
            return;
        }
        let result = match title_button {
            TitleButton::Close => self.close_window(id),
            TitleButton::Iconify => self.set_minimized(id, true),
            TitleButton::Maximize => self.toggle_maximize(id),
        };
        match result {
            Ok(()) => opusdm_ipc::debug_log!(LOG_TAG, "title button {title_button:?} of window {} run", id.0),
            Err(e) => opusdm_ipc::err_log!(LOG_TAG, "title button {title_button:?} of window {}: {e}", id.0),
        }
    }

    /// The id of the toplevel `window` in the table; `None` when it is not there.
    fn _toplevel_id(&self, window: &Window) -> Option<WindowId> {
        let wl = window.toplevel()?.wl_surface();
        self.windows.get(wl).map(|info| info.id)
    }

    /// A scroll frame: sent to the surface with the pointer focus; while
    /// the window menu is open it is dropped (no client scrolls under it).
    pub fn pointer_axis(&mut self, frame: AxisFrame) {
        if self.menu.is_some() {
            opusdm_ipc::debug_log!(LOG_TAG, "scroll dropped: the window menu is open");
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer axis: the seat has no pointer");
            return;
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    /// A key changed. A press that matches a binding (`HotkeyTable::lookup`,
    /// on the layout independent keysym) is swallowed and its keycode
    /// recorded; the release (and a repeat press) of a recorded keycode is
    /// swallowed too, even when the modifiers were released first. While
    /// the window menu is open every non-modifier press goes to the menu
    /// (only the quit binding still matches) and is recorded the same way.
    /// While the window switcher is open every non-modifier press goes to
    /// the switcher the same way (`_switcher_filter`), and after every key
    /// event the switcher checks its hold modifiers (released: the selected
    /// window is activated). Every other key, modifiers included, goes to
    /// the surface with the keyboard focus as before. The binding, the menu
    /// key or the switcher key runs after the keyboard is updated.
    pub fn keyboard_key(&mut self, keycode: Keycode, pressed: bool, time_msec: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "keyboard key: the seat has no keyboard");
            return;
        };
        let state = if pressed { KeyState::Pressed } else { KeyState::Released };
        let serial = SERIAL_COUNTER.next_serial();
        let routed = keyboard.input::<KeyRoute, _>(self, keycode, state, serial, time_msec, |s, mods, handle| {
            s._hotkey_filter(keycode, pressed, mods, &handle)
        });
        match routed {
            Some(KeyRoute::Binding(binding, combo)) => self.run_binding(binding, &combo),
            Some(KeyRoute::Menu(key)) => self.window_menu_key(key),
            Some(KeyRoute::Switcher(key)) => self.switcher_key(key),
            Some(KeyRoute::Swallowed) | None => {}
        }
        if self.switcher.is_some() {
            self.check_switcher_mods();
        }
    }

    /// The hotkey filter of `keyboard_key`: `Intercept(Binding)` for a press
    /// that matches a binding (its keycode recorded), `Intercept(Swallowed)`
    /// for a key event of a recorded keycode (its release forgets it), the
    /// window switcher's filter while the switcher is open, the window
    /// menu's filter while the menu is open, `Forward` otherwise.
    fn _hotkey_filter(
        &mut self,
        keycode: Keycode,
        pressed: bool,
        mods: &ModifiersState,
        handle: &KeysymHandle<'_>,
    ) -> FilterResult<KeyRoute> {
        if !pressed {
            if !self.swallowed_keys.remove(&keycode) { return FilterResult::Forward; }
            opusdm_ipc::debug_log!(LOG_TAG, "hotkey release of key {} swallowed", keycode.raw());
            return FilterResult::Intercept(KeyRoute::Swallowed);
        }
        // A switcher or a menu whose window went since the last refresh must not eat this key.
        self.check_switcher();
        if self.switcher.is_some() { return self._switcher_filter(keycode, mods, handle); }
        self.check_window_menu();
        if self.menu.is_some() { return self._menu_filter(keycode, mods, handle); }
        if self.swallowed_keys.contains(&keycode) {
            opusdm_ipc::debug_log!(LOG_TAG, "hotkey repeat press of key {} swallowed", keycode.raw());
            return FilterResult::Intercept(KeyRoute::Swallowed);
        }
        let mods = mods_of(mods);
        let Some((binding, name)) = self._lookup_key(mods, handle) else { return FilterResult::Forward };
        self.swallowed_keys.insert(keycode);
        FilterResult::Intercept(KeyRoute::Binding(binding, combo_text(mods, &name)))
    }

    /// The filter of a key press while the window menu is open: a modifier
    /// goes to the focused client as usual (its release does too, so the
    /// client never sees a modifier stuck); the quit binding still runs and
    /// the window menu binding runs too (it closes the menu); a hub hotkey
    /// is swallowed with an `err_log!`; a window manager built-in does not
    /// run and, as any other press (repeats included), goes to the menu. Every press taken has its keycode recorded so its
    /// release is swallowed.
    fn _menu_filter(&mut self, keycode: Keycode, mods: &ModifiersState, handle: &KeysymHandle<'_>) -> FilterResult<KeyRoute> {
        if handle.raw_syms().iter().any(|sym| sym.is_modifier_key()) { return FilterResult::Forward; }
        self.swallowed_keys.insert(keycode);
        let mods = mods_of(mods);
        match self._lookup_key(mods, handle) {
            Some((binding @ (Binding::Quit | Binding::WindowMenu), name)) => {
                return FilterResult::Intercept(KeyRoute::Binding(binding, combo_text(mods, &name)));
            }
            Some((Binding::Hub(function), name)) => {
                opusdm_ipc::err_log!(LOG_TAG, "hotkey {}: swallowed, the window menu is open ({function} not run)", combo_text(mods, &name));
                return FilterResult::Intercept(KeyRoute::Swallowed);
            }
            // Not run while the menu is open; the key goes to the menu as before the built-ins
            // were matched (Alt+Escape still closes the menu).
            Some((Binding::BuiltIn(action), name)) => {
                opusdm_ipc::debug_log!(LOG_TAG, "hotkey {}: {action:?} not run, the key goes to the window menu", combo_text(mods, &name));
            }
            None => {}
        }
        let sym = handle.modified_sym();
        match menu_key_of(sym) {
            Some(key) => FilterResult::Intercept(KeyRoute::Menu(key)),
            None => {
                opusdm_ipc::debug_log!(LOG_TAG, "key {} ignored by the window menu", xkb::keysym_get_name(sym));
                FilterResult::Intercept(KeyRoute::Swallowed)
            }
        }
    }

    /// The filter of a key press while the window switcher is open: a
    /// modifier goes to the focused client as usual (so its release does
    /// too, and the switcher sees the hold modifiers go); the key of the
    /// opening combo moves the selection (with Shift toggled, the other
    /// way); Escape cancels; the quit binding still runs; a hub hotkey is
    /// swallowed with an `err_log!`; any other key, the other built-ins
    /// included, is swallowed with a `debug_log!`. Every press taken has its keycode recorded so
    /// its release is swallowed.
    fn _switcher_filter(&mut self, keycode: Keycode, mods: &ModifiersState, handle: &KeysymHandle<'_>) -> FilterResult<KeyRoute> {
        if handle.raw_syms().iter().any(|sym| sym.is_modifier_key()) { return FilterResult::Forward; }
        self.swallowed_keys.insert(keycode);
        let Some(switcher) = &self.switcher else {
            opusdm_ipc::err_log!(LOG_TAG, "window switcher filter: the switcher closed, key swallowed");
            return FilterResult::Intercept(KeyRoute::Swallowed);
        };
        let combo_mods = mods_of(mods);
        match self._lookup_key(combo_mods, handle) {
            // The quit binding still runs, as with the window menu open.
            Some((Binding::Quit, name)) => {
                return FilterResult::Intercept(KeyRoute::Binding(Binding::Quit, combo_text(combo_mods, &name)));
            }
            Some((Binding::Hub(function), name)) => {
                opusdm_ipc::err_log!(LOG_TAG, "hotkey {}: swallowed, the window switcher is open ({function} not run)", combo_text(combo_mods, &name));
                return FilterResult::Intercept(KeyRoute::Swallowed);
            }
            _ => {}
        }
        // Only the key name is compared: the modifiers decide the direction.
        let probe = KeyCombo { mods: Mods::default(), name: switcher.key.clone() };
        let raw = handle.raw_latin_sym_or_raw_current_sym();
        let modified = handle.modified_sym();
        let is_key = raw.into_iter().chain([modified]).any(|sym| combo_matches(&probe, Mods::default(), &xkb::keysym_get_name(sym)));
        if is_key { return FilterResult::Intercept(KeyRoute::Switcher(SwitcherKey::Step(switcher.steps_forward(mods.shift)))); }
        if modified == Keysym::Escape { return FilterResult::Intercept(KeyRoute::Switcher(SwitcherKey::Cancel)); }
        opusdm_ipc::debug_log!(LOG_TAG, "window switcher: key {} swallowed", xkb::keysym_get_name(modified));
        FilterResult::Intercept(KeyRoute::Swallowed)
    }

    /// The binding of the key `handle` with `mods` and the keysym name that
    /// matched: the layout independent base keysym
    /// (`raw_latin_sym_or_raw_current_sym`, `Ctrl+Shift+1`) first, then,
    /// when it differs, the keysym the keymap state produces
    /// (`modified_sym`, `Ctrl+Shift+exclam`, as the lister's recorder and
    /// GDK write a shifted key; keypad keys with Num Lock alike).
    fn _lookup_key(&self, mods: Mods, handle: &KeysymHandle<'_>) -> Option<(Binding, String)> {
        let raw = handle.raw_latin_sym_or_raw_current_sym();
        if let Some(sym) = raw {
            let name = xkb::keysym_get_name(sym);
            if let Some(binding) = self.hotkeys.lookup(mods, &name) { return Some((binding, name)); }
        }
        let modified = handle.modified_sym();
        if Some(modified) == raw || modified == Keysym::NoSymbol { return None; }
        let name = xkb::keysym_get_name(modified);
        self.hotkeys.lookup(mods, &name).map(|binding| (binding, name))
    }

    /// Focus on a press: a window is raised and gets the keyboard; a layer
    /// surface that takes the keyboard (OnDemand, or Exclusive) gets it unless
    /// another exclusive keyboard layer is mapped; a layer surface with no
    /// keyboard interactivity and an empty spot leave the focus as it is.
    fn _click_focus(&mut self, hit: Option<&Hit>) {
        let Some(hit) = hit else {
            opusdm_ipc::debug_log!(LOG_TAG, "click on no surface, focus unchanged");
            return;
        };
        match &hit.owner {
            HitOwner::Window(window) => self.focus_window(Some(window.clone())),
            HitOwner::Layer(layer) => self._click_layer(layer),
        }
    }

    /// Click focus on the layer surface `layer` (see `_click_focus`).
    fn _click_layer(&mut self, layer: &LayerSurface) {
        let wl = layer.wl_surface().clone();
        if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::None {
            opusdm_ipc::debug_log!(LOG_TAG, "click on layer {} without keyboard interactivity", wl.id());
            return;
        }
        if let Some(exclusive) = self.exclusive_layer.as_ref().filter(|l| *l != &wl) {
            opusdm_ipc::debug_log!(LOG_TAG, "click on layer {}: exclusive layer {} keeps the keyboard", wl.id(), exclusive.id());
            return;
        }
        self.focus_layer(&wl);
    }

    /// Starts the Alt + button grab on `window` (`focus`: the surface with
    /// the pointer at the press, none on the frame), when Alt is held and
    /// `button` is the left (move) or right (resize) one.
    fn _alt_grab(
        &mut self,
        window: Window,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        button: u32,
        location: Point<f64, Logical>,
        serial: Serial,
    ) {
        if button != BTN_LEFT && button != BTN_RIGHT { return; }
        if !self._alt_held() { return; }
        let start_data = GrabStartData { focus, button, location };
        if button == BTN_LEFT {
            self.start_move_grab(window, start_data, serial);
            return;
        }
        let Some(loc) = self.space.element_location(&window) else {
            opusdm_ipc::err_log!(LOG_TAG, "Alt resize refused: the window is not mapped");
            return;
        };
        let size = window.geometry().size;
        let rel = location.to_i32_round() - loc;
        let edges = edges_for_point(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h }, (rel.x, rel.y));
        self.start_resize_grab(window, edges, start_data, serial);
    }

    /// True when Alt is held, from the modifiers of the seat keyboard.
    fn _alt_held(&self) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "Alt check: the seat has no keyboard");
            return false;
        };
        keyboard.modifier_state().alt
    }

    /// Hit on the layer surfaces of `layer` (topmost first), popups included.
    fn _layer_hit(&self, layer: Layer, pos: Point<f64, Logical>) -> Option<Hit> {
        let output_loc = self.space.output_geometry(&self.output)?.loc.to_f64();
        let map = layer_map_for_output(&self.output);
        let local = pos - output_loc;
        let surface = map.layer_under(layer, local)?;
        let geo = map.layer_geometry(surface)?;
        // `layer_geometry` is the bounding box; the surface origin is its location minus the bbox offset.
        let origin = output_loc + (geo.loc - surface.bbox().loc).to_f64();
        let (wl, loc) = surface.surface_under(pos - origin, WindowSurfaceType::ALL)?;
        Some(Hit { surface: wl, location: origin + loc.to_f64(), owner: HitOwner::Layer(surface.clone()) })
    }

    /// Hit on the topmost window when it is fullscreen.
    fn _fullscreen_hit(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        let top = self.space.elements().next_back()?;
        let wl = top.toplevel()?.wl_surface();
        if !self.windows.get(wl).is_some_and(|i| i.fullscreen) { return None; }
        let loc = self.space.element_location(top)?;
        _window_hit(top, loc - top.geometry().loc, pos)
    }

    /// Hit on the windows, topmost first: for each window, its surfaces
    /// (as Smithay's `Space::element_under`: in its bounding box and its
    /// input region), then its frame when it is decorated.
    fn _windows_hit(&self, pos: Point<f64, Logical>) -> Option<Under> {
        for window in self.space.elements().rev() {
            let Some(loc) = self.space.element_location(window) else { continue };
            let render_loc = loc - window.geometry().loc;
            let in_bbox = self.space.element_bbox(window).is_some_and(|bbox| bbox.to_f64().contains(pos));
            if in_bbox && window.is_in_input_region(&(pos - render_loc.to_f64())) {
                return _window_hit(window, render_loc, pos).map(Under::Surface);
            }
            if let Some(zone) = self._frame_zone_at(window, loc, pos) {
                return Some(Under::Frame(window.clone(), zone));
            }
        }
        None
    }

    /// The part of the frame of `window` (client at `loc`) under `pos`;
    /// `None` when it has no frame, `pos` is outside it or in its client area.
    fn _frame_zone_at(&self, window: &Window, loc: Point<i32, Logical>, pos: Point<f64, Logical>) -> Option<FrameZone> {
        let insets = self.insets_of(window.toplevel()?.wl_surface());
        if insets == Insets::default() { return None; }
        let size = window.geometry().size;
        let frame = frame_rect(Rect { x: loc.x, y: loc.y, w: size.w, h: size.h }, insets);
        let at = pos.to_i32_floor();
        let inside = at.x >= frame.x && at.y >= frame.y && at.x < frame.x.saturating_add(frame.w) && at.y < frame.y.saturating_add(frame.h);
        if !inside { return None; }
        match frame_zone(frame, (at.x, at.y), insets, &self.title_layout) {
            FrameZone::Client => None,
            zone => Some(zone),
        }
    }
}

/// Hit on `window` drawn at `render_loc`, popups included.
fn _window_hit(window: &Window, render_loc: Point<i32, Logical>, pos: Point<f64, Logical>) -> Option<Hit> {
    let (surface, loc) = window.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL)?;
    Some(Hit { surface, location: (loc + render_loc).to_f64(), owner: HitOwner::Window(window.clone()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probe that hits every slot in `present`, returning the slot itself.
    fn probe(present: &[HitSlot]) -> impl FnMut(HitSlot) -> Option<HitSlot> + '_ {
        move |slot| present.contains(&slot).then_some(slot)
    }

    #[test]
    fn the_overlay_is_hit_before_everything() {
        let all = HIT_ORDER;
        assert_eq!(first_hit(probe(&all)), Some(HitSlot::Overlay));
    }

    #[test]
    fn a_fullscreen_window_is_hit_before_the_top_layer() {
        assert_eq!(first_hit(probe(&[HitSlot::Top, HitSlot::Fullscreen, HitSlot::Windows])), Some(HitSlot::Fullscreen));
    }

    #[test]
    fn the_top_layer_is_hit_before_the_windows() {
        assert_eq!(first_hit(probe(&[HitSlot::Windows, HitSlot::Top])), Some(HitSlot::Top));
    }

    #[test]
    fn the_windows_are_hit_before_bottom_and_background() {
        assert_eq!(first_hit(probe(&[HitSlot::Background, HitSlot::Bottom, HitSlot::Windows])), Some(HitSlot::Windows));
        assert_eq!(first_hit(probe(&[HitSlot::Background, HitSlot::Bottom])), Some(HitSlot::Bottom));
        assert_eq!(first_hit(probe(&[HitSlot::Background])), Some(HitSlot::Background));
    }

    #[test]
    fn nothing_under_the_pointer_is_no_hit() {
        assert_eq!(first_hit(probe(&[])), None);
    }

    #[test]
    fn two_presses_at_most_500_ms_apart_are_a_double_click() {
        assert!(is_double_click(1000, 1500));
        assert!(!is_double_click(1000, 1501));
        assert!(is_double_click(u32::MAX - 100, 200), "the clock wraps");
    }

    #[test]
    fn a_border_shows_its_resize_cursor_and_the_title_the_arrow() {
        let edges = |top, bottom, left, right| FrameZone::Border(Edges { top, bottom, left, right });
        assert_eq!(frame_cursor(edges(false, false, false, true)), CursorIcon::EResize);
        assert_eq!(frame_cursor(edges(true, false, true, false)), CursorIcon::NwResize);
        assert_eq!(frame_cursor(edges(false, true, false, true)), CursorIcon::SeResize);
        assert_eq!(frame_cursor(edges(false, true, false, false)), CursorIcon::SResize);
        assert_eq!(frame_cursor(FrameZone::Title), CursorIcon::Default);
        assert_eq!(frame_cursor(FrameZone::Button(TitleButton::Close)), CursorIcon::Default);
    }
}
