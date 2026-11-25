//! wlr-layer-shell: panels, docks, the Run window and their popups.
//!
//! Every layer surface goes into the layer map of the single output; the map
//! is arranged on every commit of a layer surface and its non exclusive zone
//! is `State::usable_area()`.

use std::cell::Cell;

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    reexports::{
        wayland_protocols_wlr::layer_shell::v1::server::{
            zwlr_layer_shell_v1::ZwlrLayerShellV1,
            zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
        },
        wayland_server::{
            backend::ClientId,
            delegate_dispatch, delegate_global_dispatch,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
            Client, DataInit, Dispatch, DisplayHandle, Resource,
        },
    },
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                Anchor, KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceCachedState,
                LayerSurfaceData, WlrLayerShellGlobalData, WlrLayerShellHandler, WlrLayerShellState,
                WlrLayerSurfaceUserData,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::geometry::Rect;
use crate::state::{State, LOG_TAG};

/// Mark in the user data of a layer surface: true once it committed a
/// buffer, until it is hidden by a null buffer.
struct LayerShown(Cell<bool>);

impl WlrLayerShellHandler for State {
    /// The layer shell state.
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    /// Puts the new layer surface in the layer map of the output (there is
    /// one output: a requested output is ignored). Its initial configure goes
    /// out on its first commit, after the map is arranged.
    fn new_layer_surface(&mut self, surface: WlrLayerSurface, output: Option<WlOutput>, layer: Layer, namespace: String) {
        let id = surface.wl_surface().id();
        if output.is_some() {
            opusdm_ipc::debug_log!(LOG_TAG, "layer {id}: requested output ignored, one output only");
        }
        opusdm_ipc::debug_log!(LOG_TAG, "new layer surface {id} {namespace:?} on {layer:?}");
        _clear_roleless_anchor(surface.wl_surface(), layer);
        let mut map = layer_map_for_output(&self.output);
        if let Err(err) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            opusdm_ipc::err_log!(LOG_TAG, "layer surface {id} not mapped: {err}");
        }
    }

    /// A popup adopted by a layer surface (`zwlr_layer_surface_v1.get_popup`):
    /// now that its parent is known it is kept inside the output. The popup
    /// manager adds it to the tree of the layer on its first commit.
    fn new_popup(&mut self, parent: WlrLayerSurface, popup: PopupSurface) {
        opusdm_ipc::debug_log!(LOG_TAG, "popup {} adopted by layer {}", popup.wl_surface().id(), parent.wl_surface().id());
        self.unconstrain_popup(&popup);
    }

    /// Removes the layer surface from the layer map (or from the hidden
    /// layers); maximized windows follow the new usable area and the keyboard
    /// goes back to the topmost window when the layer had it.
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let wl = surface.wl_surface().clone();
        let hidden = self.hidden_layers.len();
        self.hidden_layers.retain(|l| l.layer_surface() != &surface);
        if self.hidden_layers.len() != hidden {
            opusdm_ipc::debug_log!(LOG_TAG, "hidden layer surface {} destroyed", wl.id());
        }
        let before = self.usable_area();
        {
            let mut map = layer_map_for_output(&self.output);
            let layer = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            match layer {
                Some(layer) => map.unmap_layer(&layer),
                None => opusdm_ipc::debug_log!(LOG_TAG, "destroyed layer {} was not in the layer map", wl.id()),
            }
        }
        opusdm_ipc::debug_log!(LOG_TAG, "layer surface {} destroyed", wl.id());
        self.usable_area_changed(before);
        _release_keyboard(self, &wl);
    }
}

// `delegate_layer_shell!` without its `zwlr_layer_surface_v1` part: that one
// is the `Dispatch` impl below, which wraps Smithay's.
delegate_dispatch!(State: [ZwlrLayerShellV1: ()] => WlrLayerShellState);
delegate_global_dispatch!(State: [ZwlrLayerShellV1: WlrLayerShellGlobalData] => WlrLayerShellState);

impl Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData> for State {
    /// Every request goes to Smithay unchanged.
    fn request(
        state: &mut State,
        client: &Client,
        resource: &ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        data: &WlrLayerSurfaceUserData,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        <WlrLayerShellState as Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, State>>::request(
            state, client, resource, request, data, dh, data_init,
        );
    }

    /// Smithay's destroy (which calls `layer_destroyed`), then the size check
    /// Smithay keeps on the `wl_surface` is satisfied for the commits of the
    /// now role-less surface (see [`_set_roleless_anchor`]).
    fn destroyed(state: &mut State, client: ClientId, resource: &ZwlrLayerSurfaceV1, data: &WlrLayerSurfaceUserData) {
        let surface = state
            .layer_shell_state
            .layer_surfaces()
            .find(|layer| layer.shell_surface() == resource)
            .map(|layer| layer.wl_surface().clone());
        <WlrLayerShellState as Dispatch<ZwlrLayerSurfaceV1, WlrLayerSurfaceUserData, State>>::destroyed(
            state, client, resource, data,
        );
        let Some(surface) = surface else {
            opusdm_ipc::debug_log!(LOG_TAG, "destroyed layer surface {} was not known", resource.id());
            return;
        };
        _set_roleless_anchor(&surface);
    }
}

/// Smithay 0.7 checks the layer size on every commit of the `wl_surface`
/// (a pre-commit hook that is never removed) and, when the layer surface is
/// destroyed, resets its state to size 0 with no anchor. GTK hides a layer
/// window by destroying the `zwlr_layer_surface_v1` and then committing a
/// null buffer on the same `wl_surface`: the check fails and the client is
/// killed with `invalid_size` (opusdm-notify died this way). Anchoring the
/// pending state to every edge makes the check pass; nothing reads it while
/// the surface has no layer role, and [`_clear_roleless_anchor`] drops it
/// when the surface gets a layer role again.
fn _set_roleless_anchor(surface: &WlSurface) {
    with_states(surface, |states| {
        states.cached_state.get::<LayerSurfaceCachedState>().pending().anchor = Anchor::all();
    });
}

/// A new layer surface on `surface` starts from the default state (as on a
/// fresh `wl_surface`) with its `layer`, not from the anchors left by
/// [`_set_roleless_anchor`]. Called on `get_layer_surface`, before any
/// request of the client on the new layer surface.
fn _clear_roleless_anchor(surface: &WlSurface, layer: Layer) {
    with_states(surface, |states| {
        let mut cached = states.cached_state.get::<LayerSurfaceCachedState>();
        *cached.pending() = LayerSurfaceCachedState { layer, ..Default::default() };
    });
}

/// True when `layer` asks for the keyboard exclusively on a layer where that
/// is honoured (TOP and OVERLAY).
fn _wants_exclusive_keyboard(layer: &LayerSurface) -> bool {
    let exclusive = layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive;
    exclusive && matches!(layer.layer(), Layer::Top | Layer::Overlay)
}

/// True once the initial configure of the layer surface `surface` was sent;
/// `None` (logged) when the surface has no layer surface data.
fn _initial_configure_sent(surface: &WlSurface) -> Option<bool> {
    let sent = with_states(surface, |states| {
        let data = states.data_map.get::<LayerSurfaceData>()?;
        data.lock().ok().map(|attrs| attrs.initial_configure_sent)
    });
    if sent.is_none() {
        opusdm_ipc::err_log!(LOG_TAG, "layer {}: no layer surface data", surface.id());
    }
    sent
}

/// Takes the exclusive keyboard away from the layer `surface` when it has it
/// and moves the keyboard to the topmost window when `surface` (or nothing,
/// or a dead surface) has it.
fn _release_keyboard(state: &mut State, surface: &WlSurface) {
    if state.exclusive_layer.as_ref() == Some(surface) {
        opusdm_ipc::debug_log!(LOG_TAG, "exclusive keyboard layer {} released", surface.id());
        state.exclusive_layer = None;
    }
    if state.focus_is_gone_or(surface) {
        state.focus_topmost();
    }
}

/// Ends the popup grab (an open menu) of another surface before the
/// exclusive layer `layer` takes the keyboard: the popups of the grab root
/// get `popup_done` and the keyboard and pointer grabs are unset. A grab
/// rooted at `layer` itself (its own menu) is left alone; no grab: nothing.
fn _end_popup_grabs_for(state: &mut State, layer: &WlSurface) {
    let mut roots: Vec<WlSurface> = Vec::new();
    if let Some(root) = state.seat.get_keyboard().and_then(|k| k.grab_start_data().and_then(|d| d.focus.clone())) {
        roots.push(root);
    }
    if let Some((root, _)) = state.seat.get_pointer().and_then(|p| p.grab_start_data().and_then(|d| d.focus.clone())) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots.iter().filter(|r| *r != layer) {
        opusdm_ipc::debug_log!(LOG_TAG, "exclusive layer {} ends the popup grab rooted at {}", layer.id(), root.id());
        state.dismiss_popups_and_grabs(root);
    }
}

/// True when the layer surface `layer` committed a buffer and was not
/// hidden since.
fn _is_shown(layer: &LayerSurface) -> bool {
    layer.user_data().get::<LayerShown>().is_some_and(|shown| shown.0.get())
}

/// Records whether the layer surface `layer` shows a buffer.
fn _set_shown(layer: &LayerSurface, shown: bool) {
    layer.user_data().insert_if_missing(|| LayerShown(Cell::new(false)));
    match layer.user_data().get::<LayerShown>() {
        Some(mark) => mark.0.set(shown),
        None => opusdm_ipc::err_log!(LOG_TAG, "layer {}: shown mark not stored", layer.wl_surface().id()),
    }
}

/// A commit of a hidden layer surface (the client's new initial commit):
/// puts it back in the layer map, where the rest of `handle_commit` sends
/// it a fresh initial configure. Not a hidden layer: nothing to do.
fn _map_hidden_again(state: &mut State, surface: &WlSurface) {
    let Some(index) = state.hidden_layers.iter().position(|l| l.wl_surface() == surface) else { return };
    let layer = state.hidden_layers.remove(index);
    opusdm_ipc::debug_log!(LOG_TAG, "hidden layer {} committed again, back in the layer map", surface.id());
    if let Err(err) = layer_map_for_output(&state.output).map_layer(&layer) {
        opusdm_ipc::err_log!(LOG_TAG, "hidden layer {} not mapped again: {err}", surface.id());
    }
}

/// Hides the layer surface `layer`, which committed a null buffer with its
/// role kept, as an xdg toplevel is unmapped: out of the layer map (its
/// exclusive zone goes), parked in `hidden_layers`, its initial configure
/// marked as not sent (the protocol asks the client for a new initial
/// commit, answered with a configure), its keyboard given back. `before`
/// is the usable area before the commit.
fn _hide_layer(state: &mut State, layer: &LayerSurface, before: Rect) {
    let surface = layer.wl_surface().clone();
    layer_map_for_output(&state.output).unmap_layer(layer);
    let reset = with_states(&surface, |states| {
        let data = states.data_map.get::<LayerSurfaceData>()?;
        let mut attrs = data.lock().ok()?;
        attrs.initial_configure_sent = false;
        Some(())
    });
    if reset.is_none() {
        opusdm_ipc::err_log!(LOG_TAG, "hidden layer {}: no layer surface data, initial configure not reset", surface.id());
    }
    _set_shown(layer, false);
    state.hidden_layers.push(layer.clone());
    opusdm_ipc::debug_log!(LOG_TAG, "layer {} hidden by a null buffer, out of the layer map", surface.id());
    state.usable_area_changed(before);
    _release_keyboard(state, &surface);
}

/// Commit of a layer surface: maps a hidden layer again, arranges the layer
/// map, sends the initial configure on the first commit, hides a shown
/// layer that commits a null buffer, follows a change of the usable area
/// and gives the keyboard to (or takes it from) an exclusive keyboard layer
/// when it maps (buffer attached) or unmaps (null buffer). Not a layer
/// surface: nothing to do.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let before = state.usable_area();
    _map_hidden_again(state, surface);
    let layer = {
        let mut map = layer_map_for_output(&state.output);
        let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned() else { return };
        map.arrange();
        layer
    };
    let Some(initial_sent) = _initial_configure_sent(surface) else { return };
    if !initial_sent {
        layer.layer_surface().send_configure();
    }
    let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
    if !has_buffer && _is_shown(&layer) {
        _hide_layer(state, &layer, before);
        return;
    }
    if has_buffer {
        _set_shown(&layer, true);
    }
    state.usable_area_changed(before);
    let exclusive = has_buffer && _wants_exclusive_keyboard(&layer);
    let holds = state.exclusive_layer.as_ref() == Some(surface);
    if exclusive && !holds {
        opusdm_ipc::debug_log!(LOG_TAG, "exclusive keyboard layer {} mapped, it takes the keyboard", surface.id());
        _end_popup_grabs_for(state, surface);
        state.exclusive_layer = Some(surface.clone());
        state.focus_layer(surface);
    } else if !exclusive && holds {
        _release_keyboard(state, surface);
    }
}
