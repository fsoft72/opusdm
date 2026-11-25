//! Clipboard (`wl_data_device`), primary selection
//! (`zwp_primary_selection_device_manager_v1`) and drag and drop between
//! clients.
//!
//! Transfers are client to client only: the compositor hands the receiving
//! client's pipe to the source client (`send`) and never reads or writes a
//! pipe itself, so a dead or slow client can never block the event loop.
//! The selection focus follows the keyboard focus (`set_selection_focus`
//! from `SeatHandler::focus_changed`, `update_selection_focus` for a focus
//! that goes to nothing). A drag whose source is destroyed mid-drag is
//! cancelled (`cancel_dead_drag`): `leave` to the target, never a `drop`.
//! A drag whose source client is killed cannot be cancelled (Smithay 0.7.0
//! limit): its origin surface is dead too, so the first pointer event after
//! the death (normally the motion `cancel_dead_drag` sends from `refresh`,
//! else a real pointer event) makes `PointerInner::with_grab` discard the
//! grab through `DnDGrab::drop`: a target that had accepted gets `drop`,
//! then `leave` (one that had not gets only `leave`); its `receive` reads
//! end of file at once, as the source is dead. `dropped` logs that case.
//! No hook runs while the killed client's objects are still alive. No wlr-data-control: GDK does not use it.

use smithay::{
    delegate_data_device, delegate_primary_selection,
    input::{pointer::MotionEvent, Seat},
    reexports::wayland_server::{
        protocol::{wl_data_source::WlDataSource, wl_surface::WlSurface},
        Resource,
    },
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState},
            SelectionHandler,
        },
    },
};

use crate::state::{State, LOG_TAG};

/// The client drag in progress, from `ClientDndGrabHandler::started` to
/// `dropped` (Smithay's `DnDGrab` on the pointer owns the transfer; this
/// only names its parts to draw the icon and to notice dead clients).
#[derive(Debug, Clone)]
pub struct Drag {
    /// The source of the data; `None` for a drag inside one client.
    pub source: Option<WlDataSource>,
    /// The icon surface the client gave, drawn at the pointer.
    pub icon: Option<WlSurface>,
    /// Position of the icon relative to the pointer, moved by the icon's
    /// `wl_surface.offset` / attach offsets.
    pub icon_offset: Point<i32, Logical>,
}

impl Drag {
    /// True when the drag had a source and it is gone (destroyed by its
    /// client, or the client died): the drag can no longer give data.
    pub fn is_dead(&self) -> bool {
        self.source.as_ref().is_some_and(|s| !s.alive())
    }
}

impl State {
    /// Gives the clipboard and primary selection focus to the client of
    /// `focused` (the new keyboard focus), or to no client. Called from
    /// `SeatHandler::focus_changed`, where the keyboard is locked: it must
    /// not query the keyboard.
    pub fn set_selection_focus(&mut self, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, &self.seat, client.clone());
        set_primary_focus(dh, &self.seat, client);
    }

    /// Moves the selection focus to the client of the current keyboard
    /// focus. Smithay calls `focus_changed` only when the keyboard goes to a
    /// surface, not when it goes to nothing: the callers that may leave the
    /// keyboard on nothing (`focus_window(None)`) call this after it.
    pub fn update_selection_focus(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "selection focus: the seat has no keyboard");
            return;
        };
        let focus = keyboard.current_focus();
        self.set_selection_focus(focus.as_ref());
    }

    /// Cancels the drag in progress when its source is gone
    /// (`Drag::is_dead`): a pointer motion onto nothing sends `leave` to the
    /// target and deactivates its offer, then the grab ends (no `drop`) and
    /// the surface under the pointer gets the pointer back. Called by
    /// `refresh` and before every pointer event. When the source client was
    /// killed, that first motion makes Smithay discard the grab instead,
    /// with a `drop` to a target that accepted (see the module doc). True
    /// when a dead drag was ended.
    pub fn cancel_dead_drag(&mut self) -> bool {
        if !self.drag.as_ref().is_some_and(Drag::is_dead) { return false; }
        let Some(pointer) = self.seat.get_pointer() else {
            opusdm_ipc::err_log!(LOG_TAG, "drag: its source is gone but the seat has no pointer: not cancelled, forgotten");
            self.drag = None;
            return false;
        };
        opusdm_ipc::debug_log!(LOG_TAG, "drag: its source is gone, ending the drag");
        let time = u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);
        let location = pointer.current_location();
        let event = MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time };
        pointer.motion(self, None, &event);
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        // `dropped` clears it; a grab replaced meanwhile would not call it.
        self.drag = None;
        let under = self.surface_under(location).map(|hit| (hit.surface, hit.location));
        let event = MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time };
        pointer.motion(self, under, &event);
        pointer.frame(self);
        true
    }

    /// The icon surface of the drag in progress and its position in the
    /// space (pointer position plus the icon offset), when it is alive.
    pub fn drag_icon(&self) -> Option<(WlSurface, Point<i32, Logical>)> {
        let drag = self.drag.as_ref()?;
        let icon = drag.icon.as_ref().filter(|i| i.alive())?;
        // The seat always has a pointer (`add_input_devices`); the input paths log its absence.
        let pointer = self.seat.get_pointer()?;
        Some((icon.clone(), pointer.current_location().to_i32_round() + drag.icon_offset))
    }
}

/// A commit of `surface`: when it is the icon of the drag in progress, its
/// buffer offset moves the icon relative to the pointer.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let Some(drag) = state.drag.as_mut() else { return };
    if drag.icon.as_ref() != Some(surface) { return; }
    let delta = with_states(surface, |states| {
        states.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take()
    });
    if let Some(delta) = delta {
        drag.icon_offset += delta;
    }
}

impl SelectionHandler for State {
    /// The compositor never sets a selection of its own.
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    /// The data device state.
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {
    /// A client started a drag (Smithay checked its implicit grab): remember
    /// the source and the icon to draw.
    fn started(&mut self, source: Option<WlDataSource>, icon: Option<WlSurface>, _seat: Seat<Self>) {
        opusdm_ipc::debug_log!(LOG_TAG, "drag started: source {}, icon {}", source.is_some(), icon.is_some());
        self.drag = Some(Drag { source, icon, icon_offset: Point::default() });
    }

    /// The drag ended (drop, release on nothing, or cancel). Called with
    /// the pointer locked: only logs and forgets the drag. A validated drop
    /// whose source is dead (its client was killed, Smithay discarded the
    /// grab) is an error line: the target got `drop` with no data to read.
    fn dropped(&mut self, target: Option<WlSurface>, validated: bool, _seat: Seat<Self>) {
        if validated && self.drag.as_ref().is_some_and(Drag::is_dead) {
            opusdm_ipc::err_log!(
                LOG_TAG,
                "drag: the source client died, Smithay dropped on target {} (its receive reads nothing)",
                target.as_ref().map(|t| t.id().to_string()).unwrap_or_default()
            );
        }
        opusdm_ipc::debug_log!(
            LOG_TAG,
            "drag ended: target {}, validated {validated}",
            target.map(|t| t.id().to_string()).unwrap_or_default()
        );
        self.drag = None;
    }
}

/// The compositor never starts a drag of its own: the defaults are kept.
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

impl PrimarySelectionHandler for State {
    /// The primary selection state.
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_without_source_is_not_dead() {
        let drag = Drag { source: None, icon: None, icon_offset: Point::default() };
        assert!(!drag.is_dead());
    }
}
