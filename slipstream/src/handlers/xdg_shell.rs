use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Space,
        Window, find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{Seat, pointer::Focus},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_output, wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{Slipstream, focus::KeyboardFocus};

impl XdgShellHandler for Slipstream {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // A toplevel exists from the moment a client creates it, and plenty are never shown: Qt
        // makes one and drops it again as a drag starts. Tiling on creation gave each of those a
        // tile and the keyboard for the millisecond it lived, which retiled the workspace twice
        // and left focus somewhere else. It's tiled when it maps instead — `map_if_ready` in the
        // commit handler. Its initial configure waits for its first commit too
        // (`send_initial_configure`), when its minimum size is known, so it can carry the size of
        // the tile it will get and its first buffer is already the right size.
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // One that never mapped was never tiled: dropping it changes nothing on screen.
        self.unmapped
            .retain(|window| window.toplevel() != Some(&surface));
        self.fullscreen_on_map
            .retain(|window| window.toplevel() != Some(&surface));
        if let Some(window) = self.toplevel_window(&surface) {
            self.remove_window(&window);
        }
    }

    // F11, fullscreen video, games. Players and games often ask before they map at all, so a
    // toplevel still waiting for its first buffer is noted and goes fullscreen as it maps.
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        if let Some(window) = self.toplevel_window(&surface) {
            self.set_fullscreen(&window, true);
        } else if let Some(window) = self.unmapped_window(&surface) {
            self.fullscreen_on_map.retain(|waiting| waiting != &window);
            self.fullscreen_on_map.push(window);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.toplevel_window(&surface) {
            self.set_fullscreen(&window, false);
        } else if let Some(window) = self.unmapped_window(&surface) {
            self.fullscreen_on_map.retain(|waiting| waiting != &window);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    // Tiled windows ignore drag-to-move and drag-to-resize from their title bars. Floating
    // windows will bring these back (smallvil's grabs are in git history).
    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    /// A menu asks to take the keyboard and pointer until it's dismissed, so arrows and Esc
    /// reach it and a click outside closes it rather than landing on whatever is behind.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        // Nothing takes the keyboard or the pointer while locked: the menu is dismissed.
        if self.lock.is_some() {
            surface.send_popup_done();
            return;
        }
        let Some(seat) = Seat::<Slipstream>::from_resource(&seat) else {
            return;
        };
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let Ok(mut grab) =
            self.popups
                .grab_popup(KeyboardFocus::Wayland(root), popup, &seat, serial)
        else {
            return;
        };
        // Another grab already holds the seat and this one isn't part of it: the client is out
        // of step, so drop the whole chain rather than leave input captured by a stale menu.
        if let Some(keyboard) = seat.get_keyboard() {
            let theirs = keyboard.has_grab(serial)
                || keyboard.has_grab(grab.previous_serial().unwrap_or(serial));
            if keyboard.is_grabbed() && !theirs {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            tracing::debug!("a popup took the keyboard");
        }
        if let Some(pointer) = seat.get_pointer() {
            let theirs = pointer.has_grab(serial)
                || pointer.has_grab(grab.previous_serial().unwrap_or(grab.serial()));
            if pointer.is_grabbed() && !theirs {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }
}

/// Should be called on `WlSurface::commit`
pub fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    // Handle toplevel commits. X11 windows share the space and have no toplevel.
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    // Handle popup commits.
    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    // NOTE: This should never fail as the initial configure is always
                    // allowed.
                    xdg.send_configure().expect("initial configure failed");
                }
            }
            PopupKind::InputMethod(ref _input_method) => {}
        }
    }
}

impl Slipstream {
    /// The window for a toplevel, on any workspace or minimised into the code rain. A window closed
    /// while minimised has to be found here too, or its stream outlives it.
    fn toplevel_window(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.all_open_windows().into_iter().find(|w| {
            w.toplevel()
                .is_some_and(|t| t.wl_surface() == surface.wl_surface())
        })
    }

    /// The window for a toplevel that hasn't shown anything yet, so a request it makes before
    /// its first buffer isn't simply dropped.
    fn unmapped_window(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.unmapped
            .iter()
            .find(|window| window.toplevel() == Some(surface))
            .cloned()
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
        else {
            return;
        };

        // Kept to the screen its window is on, not the first one.
        let Some(output_geo) = self
            .output_for_window(window)
            .and_then(|output| self.space.output_geometry(&output))
        else {
            return;
        };
        let window_geo = self.space.element_geometry(window).unwrap();

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
