//! wlr-layer-shell: panels, docks, the Run window and their popups.
//!
//! Every layer surface goes into the layer map of the single output; the map
//! is arranged on every commit of a layer surface and its non exclusive zone
//! is `State::usable_area()`.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    delegate_layer_shell,
    desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType},
    reexports::wayland_server::{
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        Resource,
    },
    wayland::{
        compositor::with_states,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::state::{State, LOG_TAG};

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

    /// Removes the layer surface from the layer map; maximized windows follow
    /// the new usable area and the keyboard goes back to the topmost window
    /// when the layer had it.
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let wl = surface.wl_surface().clone();
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

delegate_layer_shell!(State);

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

/// Commit of a layer surface: arranges the layer map, sends the initial
/// configure on the first commit, follows a change of the usable area and
/// gives the keyboard to (or takes it from) an exclusive keyboard layer when
/// it maps (buffer attached) or unmaps (null buffer). Not a layer surface:
/// nothing to do.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    let before = state.usable_area();
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
    state.usable_area_changed(before);
    let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
    let exclusive = has_buffer && _wants_exclusive_keyboard(&layer);
    let holds = state.exclusive_layer.as_ref() == Some(surface);
    if exclusive && !holds {
        opusdm_ipc::debug_log!(LOG_TAG, "exclusive keyboard layer {} mapped, it takes the keyboard", surface.id());
        state.exclusive_layer = Some(surface.clone());
        state.focus_layer(surface);
    } else if !exclusive && holds {
        _release_keyboard(state, surface);
    }
}
