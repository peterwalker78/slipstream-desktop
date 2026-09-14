//! X11 apps (Steam, and Flatpaks without Wayland access) through XWayland. Their windows tile
//! like any other; menus and tooltips (override-redirect windows) sit where the app puts them.

use std::{os::unix::io::OwnedFd, process::Stdio};

use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
    wayland::{
        compositor::CompositorHandler,
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, XwmId},
    },
};

use crate::{Slipstream, cursor::Cursor, focus::KeyboardFocus, launch, screenshot::Selection};

impl Slipstream {
    /// Starts XWayland. X11 apps can connect once it reports ready, a moment later.
    pub fn start_xwayland(&mut self) {
        let spawned = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            std::iter::empty::<String>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        );
        let (xwayland, client) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                tracing::warn!("couldn't start XWayland, so X11 apps won't open: {err}");
                return;
            }
        };
        let display_handle = self.display_handle.clone();
        let inserted =
            self.loop_handle
                .insert_source(xwayland, move |event, _, state| match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => {
                        // X11 apps draw at 1× and are scaled up with the screen.
                        state.client_compositor_state(&client).set_client_scale(1.0);
                        let mut wm = match X11Wm::start_wm(
                            state.loop_handle.clone(),
                            &display_handle,
                            x11_socket,
                            client.clone(),
                        ) {
                            Ok(wm) => wm,
                            Err(err) => {
                                tracing::warn!("couldn't start the X11 window manager: {err}");
                                return;
                            }
                        };
                        if let Some(image) = Cursor::load().x11_image() {
                            let _ = wm.set_cursor(
                                &image.pixels_rgba,
                                (image.width as u16, image.height as u16).into(),
                                (image.xhot as u16, image.yhot as u16).into(),
                            );
                        }
                        state.xwm = Some(wm);
                        let x_display = format!(":{display_number}");
                        tracing::info!(x_display, "XWayland ready");
                        launch::set_x_display(x_display);
                        if state.session {
                            // DISPLAY isn't in the session's first import: XWayland only picks
                            // its number now. Both places, as `import_environment` does — the
                            // user manager for units, dbus-daemon for what it activates itself.
                            // `launch::spawn` puts DISPLAY in each child's environment, which is
                            // where both commands read it from.
                            let _ = launch::spawn(&[
                                "systemctl".to_string(),
                                "--user".to_string(),
                                "import-environment".to_string(),
                                "DISPLAY".to_string(),
                            ]);
                            let _ = launch::spawn(&[
                                "dbus-update-activation-environment".to_string(),
                                "--systemd".to_string(),
                                "DISPLAY".to_string(),
                            ]);
                        }
                    }
                    XWaylandEvent::Error => {
                        tracing::warn!("XWayland exited while starting, so X11 apps won't open")
                    }
                });
        if let Err(err) = inserted {
            tracing::warn!("couldn't watch XWayland: {}", err.error);
        }
    }
}

impl XWaylandShellHandler for Slipstream {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for Slipstream {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("X11 events only arrive while the window manager runs")
    }

    fn new_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_mapped(true) {
            tracing::warn!("couldn't map an X11 window: {err}");
            return;
        }
        // Games often ask for fullscreen before their window appears.
        let fullscreen = surface.is_fullscreen();
        let window = Window::new_x11_window(surface);
        self.add_window(window.clone());
        if fullscreen {
            self.set_fullscreen(&window, true);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let location = surface.last_configure().loc;
        self.space
            .map_element(Window::new_x11_window(surface), location, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            if surface.is_override_redirect() {
                self.space.unmap_elem(&window);
            } else {
                self.remove_window(&window);
            }
        }
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // A tiled window keeps its tile. Anything else gets the size it asked for.
        let tiled = self
            .workspaces
            .all_windows()
            .iter()
            .any(|window| window.x11_surface() == Some(&surface));
        let mut geometry = surface.last_configure();
        if !tiled {
            if let Some(width) = width {
                geometry.size.w = width as i32;
            }
            if let Some(height) = height {
                geometry.size.h = height as i32;
            }
        }
        let _ = surface.configure(geometry);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Only menus and tooltips place themselves.
        if surface.is_override_redirect() {
            if let Some(window) = self.x11_window(&surface) {
                self.space.map_element(window, geometry.loc, false);
            }
        }
    }

    // Tiled windows aren't dragged or resized by their title bars.
    /// An X11 app's own title bar or edge, on a floating window: it moves or resizes with the
    /// pointer, as a Wayland app's does.
    fn resize_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32, edges: ResizeEdge) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as Edge;
        let Some((window, location)) = self.x11_floating_press(&surface) else {
            return;
        };
        let edges = match edges {
            ResizeEdge::Top => Edge::Top,
            ResizeEdge::Bottom => Edge::Bottom,
            ResizeEdge::Left => Edge::Left,
            ResizeEdge::TopLeft => Edge::TopLeft,
            ResizeEdge::BottomLeft => Edge::BottomLeft,
            ResizeEdge::Right => Edge::Right,
            ResizeEdge::TopRight => Edge::TopRight,
            ResizeEdge::BottomRight => Edge::BottomRight,
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        self.start_floating_resize(window, edges, location, button, serial);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, button: u32) {
        let Some((window, location)) = self.x11_floating_press(&surface) else {
            return;
        };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        self.start_floating_move(window, location, button, serial);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.set_fullscreen(&window, true);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.set_fullscreen(&window, false);
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the focused X11 app may read the clipboard.
        matches!(
            self.seat.get_keyboard().and_then(|keyboard| keyboard.current_focus()),
            Some(KeyboardFocus::X11(surface)) if surface.xwm_id() == Some(xwm)
        )
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        // A screenshot the compositor put on the clipboard is written out here, not asked of a
        // client.
        let ours = match selection {
            SelectionTarget::Clipboard => current_data_device_selection_userdata(&self.seat)
                .and_then(|selection| match &*selection {
                    Selection::X11 => None,
                    other => Some(other.clone()),
                }),
            SelectionTarget::Primary => None,
        };
        match ours {
            Some(Selection::Image(png)) => {
                if mime_type == crate::screenshot::PNG {
                    crate::screenshot::send_image(png, fd);
                }
                return;
            }
            Some(Selection::Text(text)) => {
                if crate::history::TEXT_TYPES.contains(&mime_type.as_str()) {
                    crate::screenshot::send_bytes(move || text.as_bytes().to_vec(), fd);
                }
                return;
            }
            _ => {}
        }
        let failed = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd)
                    .map_err(|err| format!("{err:?}"))
            }
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd)
                .map_err(|err| format!("{err:?}")),
        };
        if let Err(err) = failed {
            tracing::warn!(err, "couldn't pass the clipboard to an X11 app");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(
                    &self.display_handle,
                    &self.seat,
                    mime_types.clone(),
                    Selection::X11,
                );
                self.loop_handle
                    .insert_idle(move |state| state.clipboard_changed(mime_types, true));
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, Selection::X11)
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            // Only a selection an X11 app made: a screenshot on the clipboard stays.
            SelectionTarget::Clipboard => {
                let theirs = current_data_device_selection_userdata(&self.seat)
                    .is_some_and(|selection| matches!(*selection, Selection::X11));
                if theirs {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                let theirs = current_primary_selection_userdata(&self.seat)
                    .is_some_and(|selection| matches!(*selection, Selection::X11));
                if theirs {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::warn!("XWayland's window manager disconnected");
        self.xwm = None;
    }
}

impl Slipstream {
    /// For an X11 app's move or resize: its floating window and where the pointer is, while a
    /// button is held and nothing else has the pointer.
    fn x11_floating_press(
        &self,
        surface: &X11Surface,
    ) -> Option<(
        smithay::desktop::Window,
        smithay::utils::Point<f64, smithay::utils::Logical>,
    )> {
        let window = self.x11_window(surface)?;
        if !self.workspaces.is_floating(&window) || self.lock.is_some() {
            return None;
        }
        let pointer = self.seat.get_pointer()?;
        if pointer.is_grabbed() {
            // Its click grab is the only one expected; a drag of ours is already under way.
            if self.drag.is_some() {
                return None;
            }
        }
        Some((window, pointer.current_location()))
    }
}
