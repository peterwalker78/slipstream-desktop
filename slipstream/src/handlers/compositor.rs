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
        let mut hid = false;
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
                hid = root == *surface && self.unmap_if_hidden(surface, &window);
                if !hid {
                    self.retile_if_min_changed(&window);
                    self.retile_if_floating_resized(&window);
                }
            } else if let Some(window) = self.hidden_window(&root) {
                // A window on a workspace no screen is showing, or in the rain, keeps its size up
                // to date too: bullet time draws it, and a window whose size reads as nothing is
                // drawn unscaled, far bigger than its frame.
                window.on_commit();
                hid = root == *surface && self.unmap_if_hidden(surface, &window);
            }
        }

        // A toplevel's first commit, or its first after unmapping itself, is answered with the
        // size it will be tiled at. The commit that unmaps it isn't one: the toplevel commits again
        // when it wants to come back.
        if !hid {
            self.send_initial_configure(surface);
        }

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

impl Slipstream {
    /// The window whose own surface is `root` among those the space doesn't hold: on a workspace
    /// no screen is showing, or minimised into the rain.
    fn hidden_window(&self, root: &WlSurface) -> Option<smithay::desktop::Window> {
        let is_root =
            |window: &smithay::desktop::Window| window.wl_surface().as_deref() == Some(root);
        self.workspaces
            .all_windows()
            .into_iter()
            .find(is_root)
            .or_else(|| {
                self.rain
                    .streams
                    .iter()
                    .map(|stream| stream.window.clone())
                    .find(is_root)
            })
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
