use crate::{Slipstream, state::ClientState};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    reexports::wayland_server::{
        Client, Resource,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, add_blocker,
            add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
        },
        drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState},
        seat::WaylandFocus,
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};

use super::xdg_shell;

impl CompositorHandler for Slipstream {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland's connection carries Smithay's own client data rather than ours.
        if let Some(xwayland) = client.get_data::<XWaylandClientData>() {
            return &xwayland.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            // A client using explicit sync hands over a buffer the GPU may still be drawing:
            // hold the commit back until its acquire point is signalled, so nothing reads the
            // buffer half-drawn. The release point is signalled by the renderer once it lets go.
            let acquire = with_states(surface, |states| {
                states
                    .cached_state
                    .get::<DrmSyncobjCachedState>()
                    .pending()
                    .acquire_point
                    .clone()
            });
            let Some(acquire) = acquire else {
                return;
            };
            let Some(client) = surface.client() else {
                return;
            };
            match acquire.generate_blocker() {
                Ok((blocker, source)) => {
                    let inserted = state.loop_handle.insert_source(source, move |_, _, state| {
                        let dh = state.display_handle.clone();
                        state
                            .client_compositor_state(&client)
                            .blocker_cleared(state, &dh);
                        Ok(())
                    });
                    match inserted {
                        Ok(_) => add_blocker(surface, blocker),
                        Err(err) => tracing::warn!("couldn't wait on an acquire point: {err}"),
                    }
                }
                Err(err) => tracing::warn!("couldn't wait on an acquire point: {err}"),
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.something_changed();
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
        }

        // A toplevel's first commit, or its first after unmapping itself, is answered with the
        // size it will be tiled at.
        self.send_initial_configure(surface);

        xdg_shell::handle_commit(&mut self.popups, surface);
        self.layer_commit(surface);

        // A toplevel with its first buffer is a window worth a tile. Before that it may never be
        // shown at all, so it waits in `unmapped`.
        self.map_if_ready(surface);

        // A window's app id arrives here, not when its toplevel is created, so a layout coming
        // back can only recognise a Wayland window once it has committed.
        self.claim_on_commit(surface);

        // An X11 window maps before XWayland gives it a surface; it takes focus once it has one.
        if let Some(window) = self
            .pending_focus
            .take_if(|window| window.wl_surface().is_some())
        {
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
        &self.protocols.shm_state
    }
}

impl DrmSyncobjHandler for Slipstream {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.protocols.syncobj.as_mut()
    }
}
