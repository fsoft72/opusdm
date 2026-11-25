//! xdg-activation: a client (a launcher, a notification daemon) gets a token
//! and hands it to another client, which asks to activate one of its
//! toplevels with it. A fresh token activates the window (desktop switch,
//! restore, raise, focus: `State::activate_window`); a toplevel not mapped
//! yet keeps the request in `WindowInfo::pending_activation` until it maps.
//!
//! Strict policy (owner decision 2026-11-25): a token is honoured only when,
//! at its creation, it carried an input serial no older than the last input
//! its client received (`State::last_input`: key press, keyboard enter,
//! pointer button press) and that client held the keyboard or the pointer.
//! The verdict is taken in `token_created` and kept in the token's user
//! data. A refused request on a mapped window marks it `urgent` (no focus
//! change, no desktop switch); on a window not mapped yet it is only a
//! debug line, since a new window gets the focus when it maps anyway.
//!
//! Ownership: `XdgActivationState` (the tokens, with their verdicts) lives
//! in `State` for the whole run. A pending activation and the `urgent` flag
//! are fields of the `WindowInfo` of their window, so they go away with the
//! window (`toplevel_destroyed` logs a pending one). `State::last_input` is
//! owned by `State` for the whole run; the entry of a client that
//! disconnected is dropped by the next `refresh` (`prune_last_input`), so a
//! dead client only leaves a stale entry until then, which no token can use
//! (its client is gone).

use std::time::Duration;

use smithay::{
    reexports::{
        wayland_protocols::xdg::activation::v1::server::{
            xdg_activation_token_v1::XdgActivationTokenV1,
            xdg_activation_v1::{self, XdgActivationV1},
        },
        wayland_server::{
            delegate_dispatch, delegate_global_dispatch, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch,
            DisplayHandle, Resource,
            backend::ClientId,
        },
    },
    utils::Serial,
    wayland::{
        compositor,
        shell::{wlr_layer::LAYER_SURFACE_ROLE, xdg::XDG_POPUP_ROLE},
        xdg_activation::{
            ActivationTokenData, XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::state::{State, LOG_TAG};

/// A token older than this is refused (and pruned when a new one is created).
pub const ACTIVATION_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// True when a token created `age` ago can still activate a window.
fn _is_fresh(age: Duration) -> bool {
    age < ACTIVATION_TOKEN_TIMEOUT
}

/// The strict policy's verdict on a token, taken when it was created
/// (`token_created`): why it is refused, `None` when it is honoured. Kept
/// in the token's user data until `request_activation` reads it.
struct TokenVerdict(Option<String>);

/// Why the strict policy refuses a token created with the input serial
/// `serial` by a client whose last input serial is `last` and that held
/// the keyboard or the pointer (`holds_input`) at that moment; `None` when
/// the token is honoured.
fn _refusal(serial: Option<Serial>, last: Option<Serial>, holds_input: bool) -> Option<&'static str> {
    let Some(serial) = serial else { return Some("the token carries no input serial") };
    let Some(last) = last else { return Some("its client never received input") };
    if !holds_input { return Some("its client held neither the keyboard nor the pointer") }
    if serial < last { return Some("its serial is older than the last input of its client") }
    None
}

/// True when a surface with the role `role` is one that is never a window
/// but that clients ask to activate anyway: a layer surface or an xdg popup
/// (GTK requests an activation for every surface it shows). Such a request
/// is a debug trace; any other surface that is not in the window table is
/// an error.
fn _is_known_non_window(role: Option<&str>) -> bool {
    matches!(role, Some(LAYER_SURFACE_ROLE | XDG_POPUP_ROLE))
}

impl XdgActivationHandler for State {
    /// The xdg-activation state (the known tokens).
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Accepts every new token, with the strict policy's verdict on it
    /// (`TokenVerdict`, from the creating client's input and focus right
    /// now) in its user data; drops the expired tokens first, so tokens
    /// that were never used do not pile up.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        self.prune_activation_tokens();
        let holds_input = data.client_id.as_ref().is_some_and(|client| self._holds_input(client));
        let last = data.client_id.as_ref().and_then(|client| self.last_input.get(client).copied());
        let refusal = _refusal(data.serial.as_ref().map(|(serial, _)| *serial), last, holds_input);
        data.user_data.insert_if_missing_threadsafe(|| TokenVerdict(refusal.map(str::to_string)));
        let verdict = refusal.unwrap_or("honoured");
        opusdm_ipc::debug_log!(LOG_TAG, "activation token created for app id {:?}: {verdict}", data.app_id);
        true
    }

    /// Activates the toplevel `surface` when the token is fresh and the
    /// strict policy honoured it (`TokenVerdict`): right away when it is
    /// mapped, when it maps otherwise. The token is removed in every case;
    /// an expired token or a surface that is not a toplevel is refused with
    /// a log line and changes nothing (a debug line for a layer surface or
    /// a popup, which GTK asks for as a matter of course). A token the
    /// policy refused marks a mapped window `urgent` (unless it is already
    /// the active window) and leaves an unmapped one alone (it gets the
    /// focus when it maps), with a debug line.
    fn request_activation(&mut self, token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        self.xdg_activation_state.remove_token(&token);
        let age = data.timestamp.elapsed();
        if !_is_fresh(age) {
            opusdm_ipc::err_log!(LOG_TAG, "activation of {} refused: token expired ({age:?} old)", surface.id());
            return;
        }
        let Some(info) = self.windows.get_mut(&surface) else {
            let role = compositor::get_role(&surface);
            if _is_known_non_window(role) {
                // Expected: GTK asks for every surface it shows (panel, popups, Run window).
                opusdm_ipc::debug_log!(LOG_TAG, "activation of {} ignored: a {} is not a window", surface.id(), role.unwrap_or_default());
            } else {
                opusdm_ipc::err_log!(LOG_TAG, "activation of {} refused: not a toplevel (role {role:?})", surface.id());
            }
            return;
        };
        let id = info.id;
        let refusal = match data.user_data.get::<TokenVerdict>() {
            Some(TokenVerdict(refusal)) => refusal.clone(),
            None => {
                opusdm_ipc::err_log!(LOG_TAG, "activation of window {}: the token has no policy verdict, refused", id.0);
                Some("no policy verdict".to_string())
            }
        };
        let unmapped = self.unmapped.contains(&info.window);
        if let Some(reason) = refusal {
            if unmapped {
                opusdm_ipc::debug_log!(LOG_TAG, "activation of window {} refused ({reason}): not mapped yet, it gets the focus when it maps", id.0);
                return;
            }
            if self.active_window == Some(id) {
                opusdm_ipc::debug_log!(LOG_TAG, "activation of window {} refused ({reason}): it is already active", id.0);
                return;
            }
            info.urgent = true;
            opusdm_ipc::debug_log!(LOG_TAG, "activation of window {} refused ({reason}): marked urgent", id.0);
            return;
        }
        if unmapped {
            info.pending_activation = true;
            opusdm_ipc::debug_log!(LOG_TAG, "activation of window {} pending until it maps", id.0);
            return;
        }
        if let Err(err) = self.activate_window(id) {
            opusdm_ipc::err_log!(LOG_TAG, "activation of window {}: {err}", id.0);
        }
    }
}

impl State {
    /// Records `serial` as the last input sent to the client of `surface`
    /// (a key press, a keyboard enter, a pointer button press), for the
    /// strict xdg-activation policy. The event sent last wins, not the
    /// highest serial: a click on an unfocused window takes its press
    /// serial first, then the keyboard enter of the click focus a newer
    /// one, and the press is sent after the enter (GTK puts the press
    /// serial in its token). A surface with no client (dead) is a debug
    /// line.
    pub fn note_client_input(&mut self, surface: &WlSurface, serial: Serial) {
        let Some(client) = surface.client() else {
            opusdm_ipc::debug_log!(LOG_TAG, "input serial for {}: the surface has no client, not recorded", surface.id());
            return;
        };
        self.last_input.insert(client.id(), serial);
    }

    /// Records the keyboard enter just sent to `surface` (its serial is the
    /// keyboard's `last_enter`) as input of its client. Called from
    /// `SeatHandler::focus_changed`, while Smithay holds the keyboard's
    /// focus lock: only `last_enter` (its own lock) is read here.
    pub fn note_keyboard_enter(&mut self, surface: &WlSurface) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            opusdm_ipc::err_log!(LOG_TAG, "keyboard enter: the seat has no keyboard, enter serial not recorded");
            return;
        };
        let Some(serial) = keyboard.last_enter() else {
            opusdm_ipc::debug_log!(LOG_TAG, "keyboard enter of {}: no enter serial, not recorded", surface.id());
            return;
        };
        self.note_client_input(surface, serial);
    }

    /// Drops the `last_input` entries of the clients that disconnected.
    pub fn prune_last_input(&mut self) {
        let handle = self.display_handle.backend_handle();
        let before = self.last_input.len();
        self.last_input.retain(|client, _| handle.get_client_data(client.clone()).is_ok());
        let pruned = before - self.last_input.len();
        if pruned > 0 {
            opusdm_ipc::debug_log!(LOG_TAG, "{pruned} input serial(s) of disconnected clients pruned");
        }
    }

    /// True when the client `client` has the keyboard focus or the pointer
    /// focus right now.
    fn _holds_input(&self, client: &ClientId) -> bool {
        let owns = |surface: Option<WlSurface>| surface.and_then(|s| s.client()).is_some_and(|c| &c.id() == client);
        let keyboard = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let pointer = self.seat.get_pointer().and_then(|p| p.current_focus());
        owns(keyboard) || owns(pointer)
    }

    /// Removes the activation tokens older than `ACTIVATION_TOKEN_TIMEOUT`.
    pub fn prune_activation_tokens(&mut self) {
        let before = self.xdg_activation_state.tokens().count();
        self.xdg_activation_state.retain_tokens(|_, data| _is_fresh(data.timestamp.elapsed()));
        let pruned = before.saturating_sub(self.xdg_activation_state.tokens().count());
        if pruned > 0 {
            opusdm_ipc::debug_log!(LOG_TAG, "{pruned} expired activation token(s) pruned");
        }
    }
}

// `delegate_xdg_activation!` without its `xdg_activation_v1` part: that one
// is the `Dispatch` impl below, which logs an unknown token (Smithay ignores
// it silently) and then hands the request to Smithay.
delegate_dispatch!(State: [XdgActivationTokenV1: ActivationTokenData] => XdgActivationState);
delegate_global_dispatch!(State: [XdgActivationV1: ()] => XdgActivationState);

impl Dispatch<XdgActivationV1, ()> for State {
    /// Logs an `activate` with an unknown (never created, already used or
    /// pruned) token, then lets Smithay handle every request.
    fn request(
        state: &mut State,
        client: &Client,
        resource: &XdgActivationV1,
        request: xdg_activation_v1::Request,
        data: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let xdg_activation_v1::Request::Activate { token, surface } = &request {
            let known = state.xdg_activation_state.data_for_token(&XdgActivationToken::from(token.clone())).is_some();
            if !known {
                opusdm_ipc::err_log!(LOG_TAG, "activation of {} refused: unknown or expired token", surface.id());
            }
        }
        <XdgActivationState as Dispatch<XdgActivationV1, (), State>>::request(state, client, resource, request, data, dh, data_init);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_fresh_until_the_timeout() {
        assert!(_is_fresh(Duration::ZERO));
        assert!(_is_fresh(ACTIVATION_TOKEN_TIMEOUT - Duration::from_millis(1)));
        assert!(!_is_fresh(ACTIVATION_TOKEN_TIMEOUT));
        assert!(!_is_fresh(Duration::from_secs(60)));
    }

    #[test]
    fn the_strict_policy_wants_a_fresh_serial_from_a_client_holding_input() {
        let (s5, s7) = (Serial::from(5), Serial::from(7));
        assert_eq!(_refusal(Some(s7), Some(s7), true), None, "the serial of the last input");
        assert_eq!(_refusal(Some(s7), Some(s5), true), None, "a newer serial (a key release after the press)");
        assert!(_refusal(None, Some(s5), true).is_some(), "no serial");
        assert!(_refusal(Some(s7), None, true).is_some(), "a client that never had input");
        assert!(_refusal(Some(s7), Some(s7), false).is_some(), "a client holding neither keyboard nor pointer");
        assert!(_refusal(Some(s5), Some(s7), true).is_some(), "a stale serial");
    }

    #[test]
    fn layer_surfaces_and_popups_are_known_non_windows() {
        assert!(_is_known_non_window(Some("zwlr_layer_surface_v1")));
        assert!(_is_known_non_window(Some("xdg_popup")));
        assert!(!_is_known_non_window(Some("xdg_toplevel")), "a toplevel not in the table is unexpected");
        assert!(!_is_known_non_window(Some("wl_subsurface")));
        assert!(!_is_known_non_window(None), "a surface with no role is unexpected");
    }
}
