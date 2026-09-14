mod compositor;
mod xdg_shell;

use std::os::unix::io::OwnedFd;

use crate::{Slipstream, focus::KeyboardFocus};

//
// Wl Seat
//

use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::keyboard::LedState;
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Serial;
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::output::OutputHandler;
use smithay::input::pointer::PointerHandle;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

impl SeatHandler for Slipstream {
    type KeyboardFocus = KeyboardFocus;
    // The pointer aims at the same type as the keyboard, which is what Smithay's popup grab
    // needs: it moves the pointer's focus to whatever the keyboard is focused on.
    type PointerFocus = KeyboardFocus;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Slipstream> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocus>) {
        self.concentration.end();
        let dh = &self.display_handle;
        let client = focused
            .and_then(|focus| focus.wl_surface())
            .and_then(|surface| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
        // The keyboard is still being changed over here, so the grabs are looked at once it has.
        let focused = focused.cloned();
        self.loop_handle
            .insert_idle(move |state| state.grabs_after_focus_change(focused.as_ref()));
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        self.update_leds(led_state);
    }
}

/// Games locking or confining the pointer: whether a constraint holds is `takeback.rs`'s call.
impl PointerConstraintsHandler for Slipstream {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.update_pointer_constraint();
    }
}

/// Virtual machines and remote desktops asking for every key, granted to the focused window.
impl KeyboardShortcutsInhibitHandler for Slipstream {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, _inhibitor: KeyboardShortcutsInhibitor) {
        self.update_shortcuts_inhibitor();
    }
}

/// Apps playing video ask for the screen to stay awake; while any does, the UI doesn't fade.
impl IdleInhibitHandler for Slipstream {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle.inhibitors.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle
            .inhibitors
            .retain(|inhibitor| *inhibitor != surface);
    }
}

//
// Clipboard, middle-click paste and drag-and-drop
//

/// Selections from Wayland apps are offered to X11 apps too.
impl SelectionHandler for Slipstream {
    type SelectionUserData = crate::screenshot::Selection;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        // A copy an app made is read for the clipboard history once the selection is in place.
        if ty == SelectionTarget::Clipboard {
            if let Some(types) = source.as_ref().map(|source| source.mime_types()) {
                self.loop_handle
                    .insert_idle(move |state| state.clipboard_changed(types, false));
            }
        }
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|source| source.mime_types())) {
                tracing::warn!(?err, ?ty, "couldn't offer the selection to X11 apps");
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &crate::screenshot::Selection,
    ) {
        match user_data {
            crate::screenshot::Selection::Image(png) => {
                if mime_type == crate::screenshot::PNG {
                    crate::screenshot::send_image(png.clone(), fd);
                }
                return;
            }
            crate::screenshot::Selection::Text(text) => {
                if crate::history::TEXT_TYPES.contains(&mime_type.as_str()) {
                    let text = text.clone();
                    crate::screenshot::send_bytes(move || text.as_bytes().to_vec(), fd);
                }
                return;
            }
            crate::screenshot::Selection::X11 => {}
        }
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd) {
                tracing::warn!(?err, "couldn't pass an X11 app's selection on");
            }
        }
    }
}

impl DataDeviceHandler for Slipstream {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl PrimarySelectionHandler for Slipstream {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl DndGrabHandler for Slipstream {}
impl WaylandDndGrabHandler for Slipstream {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        if self.lock.is_some() {
            source.cancel();
            return;
        }
        match type_ {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let start_data = ptr.grab_start_data().unwrap();

                // create a dnd grab to start the operation
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                // slipstream lacks touch handling
                source.cancel();
            }
        }
    }
}

//
// Decorations
//

/// Tiled windows get no title bars: clients are asked to leave decorations to Slipstream, which
/// draws the focus ring. (Without this, Qt apps draw their own fallback title bars.) GTK apps
/// ignore the request and keep their header bars.
impl XdgDecorationHandler for Slipstream {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.new_decoration(toplevel.clone());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ServerSide);
    }
}

//
// Activation
//

/// Apps bringing a window forward: a link clicked in one app opening in a browser already
/// running, say. Whether it happens is `Slipstream::activation_requested`'s call.
impl XdgActivationHandler for Slipstream {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // A token for a click or key press means a window may be on its way because you asked.
        if data.serial.is_some() {
            self.concentration.asked(std::time::Instant::now());
        }
        // Tokens nobody used are dropped once they're too old to be honoured anyway.
        self.xdg_activation_state
            .retain_tokens(|_, data| data.timestamp.elapsed().as_secs() < 120);
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        self.xdg_activation_state.remove_token(&token);
        self.activation_requested(&data, &surface);
    }
}

//
// Wl Output & Xdg Output
//

impl OutputHandler for Slipstream {}

/// Needed by the cursor-shape protocol, which lets apps name a cursor from the theme rather than
/// drawing their own.
impl smithay::input::tablet::TabletSeatHandler for Slipstream {
    type ToolFocus = KeyboardFocus;
}

/// Portal dialogs (a file chooser for a sandboxed app) name the window they belong to.
impl smithay::wayland::xdg_foreign::XdgForeignHandler for Slipstream {
    fn xdg_foreign_state(&mut self) -> &mut smithay::wayland::xdg_foreign::XdgForeignState {
        &mut self.xdg_foreign_state
    }
}

impl FractionalScaleHandler for Slipstream {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // The scale of the screen its window is on, once it has one; the focused screen, where a
        // new window opens, before that. `retile` keeps it up to date as windows move.
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        let window = self
            .space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(&root))
            .cloned();
        let scale = window
            .and_then(|window| self.output_for_window(&window))
            .or_else(|| self.screens.focused_output())
            .or_else(|| self.space.outputs().next().cloned())
            .map(|output| output.current_scale().fractional_scale())
            .unwrap_or(1.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}

smithay::delegate_dispatch2!(Slipstream);
