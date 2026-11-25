//! Pointer and keyboard input: hit test, click focus, Alt + drag grabs,
//! global hotkeys matched before the focused client.
//!
//! Backend independent: the backend (nested, task 7) turns its events into
//! calls of `pointer_motion`, `pointer_button`, `pointer_axis` and
//! `keyboard_key`, in output (= space) coordinates.

use smithay::{
    backend::input::{ButtonState, KeyState},
    desktop::{layer_map_for_output, LayerSurface, Window, WindowSurfaceType},
    input::{
        keyboard::{xkb, FilterResult, Keycode, Keysym, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
};

use crate::geometry::Rect;
use crate::grabs::edges_for_point;
use crate::hotkeys::{combo_text, mods_of, Binding};
use opusdm_ipc::bindings::Mods;
use crate::state::{State, LOG_TAG};

/// Linux input code of the left mouse button (`BTN_LEFT`).
pub const BTN_LEFT: u32 = 0x110;

/// Linux input code of the right mouse button (`BTN_RIGHT`).
pub const BTN_RIGHT: u32 = 0x111;

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
    /// popups are hit before their parent.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        first_hit(|slot| match slot {
            HitSlot::Overlay => self._layer_hit(Layer::Overlay, pos),
            HitSlot::Fullscreen => self._fullscreen_hit(pos),
            HitSlot::Top => self._layer_hit(Layer::Top, pos),
            HitSlot::Windows => self._windows_hit(pos),
            HitSlot::Bottom => self._layer_hit(Layer::Bottom, pos),
            HitSlot::Background => self._layer_hit(Layer::Background, pos),
        })
    }

    /// The pointer moved to `position` (space coordinates): the surface under
    /// it gets the pointer focus (an active grab decides instead).
    pub fn pointer_motion(&mut self, position: Point<f64, Logical>, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer motion: the seat has no pointer");
            return;
        };
        let under = self.surface_under(position).map(|hit| (hit.surface, hit.location));
        let event = MotionEvent { location: position, serial: SERIAL_COUNTER.next_serial(), time: time_msec };
        pointer.motion(self, under, &event);
        pointer.frame(self);
    }

    /// A pointer button changed. A press with no active grab focuses what is
    /// under the pointer (`_click_focus`); with Alt held on a window, the left
    /// button starts a `MoveGrab` and the right button a `ResizeGrab` on the
    /// edges of the quadrant the pointer is in: the grab consumes the press
    /// and its release, the client never sees them.
    pub fn pointer_button(&mut self, button: u32, pressed: bool, time_msec: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "pointer button: the seat has no pointer");
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
        if pressed && !pointer.is_grabbed() {
            let location = pointer.current_location();
            let hit = self.surface_under(location);
            self._click_focus(hit.as_ref());
            if let Some(hit) = hit {
                self._alt_grab(hit, button, location, serial);
            }
        }
        // With a grab just started this press goes to the grab, which drops it.
        pointer.button(self, &ButtonEvent { serial, time: time_msec, button, state });
        pointer.frame(self);
    }

    /// A scroll frame: sent to the surface with the pointer focus.
    pub fn pointer_axis(&mut self, frame: AxisFrame) {
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
    /// swallowed too, even when the modifiers were released first. Every
    /// other key, modifiers included, goes to the surface with the keyboard
    /// focus as before. The binding runs after the keyboard is updated.
    pub fn keyboard_key(&mut self, keycode: Keycode, pressed: bool, time_msec: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "keyboard key: the seat has no keyboard");
            return;
        };
        let state = if pressed { KeyState::Pressed } else { KeyState::Released };
        let serial = SERIAL_COUNTER.next_serial();
        let matched = keyboard.input::<Option<(Binding, String)>, _>(self, keycode, state, serial, time_msec, |s, mods, handle| {
            s._hotkey_filter(keycode, pressed, mods, &handle)
        });
        if let Some(Some((binding, combo))) = matched {
            self.run_binding(binding, &combo);
        }
    }

    /// The hotkey filter of `keyboard_key`: `Intercept(Some(..))` for a press
    /// that matches a binding (its keycode recorded), `Intercept(None)` for
    /// a key event of a recorded keycode (its release forgets it), `Forward`
    /// otherwise.
    fn _hotkey_filter(
        &mut self,
        keycode: Keycode,
        pressed: bool,
        mods: &ModifiersState,
        handle: &KeysymHandle<'_>,
    ) -> FilterResult<Option<(Binding, String)>> {
        if !pressed {
            if !self.swallowed_keys.remove(&keycode) { return FilterResult::Forward; }
            opusdm_ipc::debug_log!(LOG_TAG, "hotkey release of key {} swallowed", keycode.raw());
            return FilterResult::Intercept(None);
        }
        if self.swallowed_keys.contains(&keycode) {
            opusdm_ipc::debug_log!(LOG_TAG, "hotkey repeat press of key {} swallowed", keycode.raw());
            return FilterResult::Intercept(None);
        }
        let mods = mods_of(mods);
        let Some((binding, name)) = self._lookup_key(mods, handle) else { return FilterResult::Forward };
        self.swallowed_keys.insert(keycode);
        FilterResult::Intercept(Some((binding, combo_text(mods, &name))))
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

    /// Starts the Alt + button grab on the window of `hit`, when Alt is held
    /// and `button` is the left (move) or right (resize) one.
    fn _alt_grab(&mut self, hit: Hit, button: u32, location: Point<f64, Logical>, serial: Serial) {
        if button != BTN_LEFT && button != BTN_RIGHT { return; }
        let HitOwner::Window(window) = hit.owner else { return };
        if !self._alt_held() { return; }
        let start_data = GrabStartData { focus: Some((hit.surface, hit.location)), button, location };
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

    /// Hit on the windows, topmost first.
    fn _windows_hit(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        let (window, render_loc) = self.space.element_under(pos)?;
        _window_hit(window, render_loc, pos)
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
}
