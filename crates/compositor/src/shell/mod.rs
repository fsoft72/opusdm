//! Surface protocols: `wl_compositor` (commit handling), `wl_shm` and the shells.

pub mod xdg;

use std::sync::OnceLock;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler, CompositorState},
        shm::{ShmHandler, ShmState},
    },
};

use crate::state::{ClientState, State, LOG_TAG};

/// Returns the root of the subsurface tree `surface` belongs to.
fn _root_surface(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl CompositorHandler for State {
    /// The compositor state.
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    /// The per-client compositor state; a client without `ClientState` (never
    /// expected: every client is inserted with one) gets a shared fallback.
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        static FALLBACK: OnceLock<CompositorClientState> = OnceLock::new();
        if let Some(data) = client.get_data::<ClientState>() {
            return &data.compositor_state;
        }
        opusdm_ipc::err_log!(LOG_TAG, "client {:?} has no ClientState", client.id());
        FALLBACK.get_or_init(CompositorClientState::default)
    }

    /// Imports the buffer, unmaps a toplevel that committed a null buffer,
    /// updates the window of the surface tree, sends the initial configure of a popup, sends the initial configure or
    /// maps a toplevel, and refreshes the space.
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if xdg::unmap_if_bufferless(self, surface) {
            self.space.refresh();
            return;
        }
        if !is_sync_subsurface(surface) {
            let root = _root_surface(surface);
            let window = self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root));
            if let Some(window) = window {
                window.on_commit();
            }
        }
        xdg::handle_commit(self, surface);
        xdg::handle_popup_commit(self, surface);
        self.space.refresh();
    }
}

impl BufferHandler for State {
    /// Nothing to release: shm buffers are not cached.
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    /// The shm state.
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
