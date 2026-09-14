use crate::{Slipstream, state::ClientState};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        seat::WaylandFocus,
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};

use super::xdg_shell;

impl CompositorHandler for Slipstream {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland's connection carries Smithay's own client data rather than ours.
        if let Some(xwayland) = client.get_data::<XWaylandClientData>() {
            return &xwayland.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let window = self
                .space
                .elements()
                .find(|w| w.wl_surface().as_deref() == Some(&root))
                .cloned();
            if let Some(window) = window {
                window.on_commit();
                self.retile_if_min_changed(&window);
                self.retile_if_floating_resized(&window);
            }
        };

        // A toplevel's first commit, or its first after unmapping itself, is answered with the
        // size it will be tiled at.
        self.send_initial_configure(surface);

        xdg_shell::handle_commit(&mut self.popups, &self.space, surface);
        self.layer_commit(surface);

        // A toplevel with its first buffer is a window worth a tile. Before that it may never be
        // shown at all, so it waits in `unmapped`.
        self.map_if_ready(surface);

        // A window's app id arrives here, not when its toplevel is created, so a layout coming
        // back can only recognise a Wayland window once it has committed.
        self.claim_on_commit(surface);

        // An X11 window maps before XWayland gives it a surface; it takes focus once it has one.
        if self
            .pending_focus
            .as_ref()
            .is_some_and(|window| window.wl_surface().is_some())
        {
            let window = self.pending_focus.take().unwrap();
            self.clear_focus();
            self.focus_window(&window);
        }
    }
}

impl BufferHandler for Slipstream {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Slipstream {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
