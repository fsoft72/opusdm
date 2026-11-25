//! xdg-activation: a client (a launcher, a notification daemon) gets a token
//! and hands it to another client, which asks to activate one of its
//! toplevels with it. A fresh token activates the window (desktop switch,
//! restore, raise, focus: `State::activate_window`); a toplevel not mapped
//! yet keeps the request in `WindowInfo::pending_activation` until it maps.
//!
//! Ownership: `XdgActivationState` (the tokens) lives in `State` for the
//! whole run. A pending activation is a flag of the `WindowInfo` of its
//! window, so it goes away with the window (`toplevel_destroyed` logs it).

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
        },
    },
    wayland::xdg_activation::{
        ActivationTokenData, XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};

use crate::state::{State, LOG_TAG};

/// A token older than this is refused (and pruned when a new one is created).
pub const ACTIVATION_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// True when a token created `age` ago can still activate a window.
fn _is_fresh(age: Duration) -> bool {
    age < ACTIVATION_TOKEN_TIMEOUT
}

impl XdgActivationHandler for State {
    /// The xdg-activation state (the known tokens).
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// Accepts every new token; drops the expired ones first, so tokens
    /// that were never used do not pile up.
    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        self.prune_activation_tokens();
        opusdm_ipc::debug_log!(LOG_TAG, "activation token created for app id {:?}", data.app_id);
        true
    }

    /// Activates the toplevel `surface` when the token is fresh: right away
    /// when it is mapped, when it maps otherwise. The token is removed in
    /// every case; an expired token or a surface that is not a toplevel is
    /// refused with a log line and changes nothing.
    fn request_activation(&mut self, token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        self.xdg_activation_state.remove_token(&token);
        let age = data.timestamp.elapsed();
        if !_is_fresh(age) {
            opusdm_ipc::err_log!(LOG_TAG, "activation of {} refused: token expired ({age:?} old)", surface.id());
            return;
        }
        let Some(info) = self.windows.get_mut(&surface) else {
            opusdm_ipc::err_log!(LOG_TAG, "activation of {} refused: not a toplevel", surface.id());
            return;
        };
        let id = info.id;
        if self.unmapped.contains(&info.window) {
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
}
