//! Input methods: typing Chinese, Japanese, Korean and other scripts through IBus or fcitx5, and
//! on-screen keyboards.
//!
//! Apps say where text goes through `zwp_text_input_v3`, and an input method
//! (`zwp_input_method_v2`) turns keys into text for them, with a candidate pop-up by the caret.
//! Smithay routes between the two and follows keyboard focus on its own. On-screen keyboards and
//! typing tools use `zwp_virtual_keyboard_v1` to send keys, which go to the focused app (through
//! the input method, when one is typing) and never run Slipstream's own bindings.
//!
//! **Unsandboxed clients only**, for the two that aren't apps asking for text. An input method
//! sees every key typed into every app, and a virtual keyboard can type into any of them, so
//! both globals are filtered as the clipboard's are: a client sees them only if, when it
//! connected, its process was proven to be outside any sandbox.

use std::sync::Arc;

use smithay::{
    backend::input::{Event, InputEvent, KeyboardKeyEvent},
    desktop::{PopupKind, PopupManager},
    input::keyboard::{FilterResult, KeyboardHandle, Layout},
    reexports::wayland_server::{Client, DisplayHandle, protocol::wl_surface::WlSurface},
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        input_method::{
            InputMethodHandler, InputMethodManagerState, InputMethodSeat, PopupSurface,
        },
        seat::WaylandFocus,
        text_input::{TextInputActivation, TextInputManagerState},
        virtual_keyboard::{
            VirtualKeyboardBackend, VirtualKeyboardHandler, VirtualKeyboardManagerState,
            VirtualKeyboardSpecialEvent,
        },
    },
};

use crate::{Slipstream, capture::Peer, state::ClientState};

/// Whether a connecting client may be an input method or a virtual keyboard, from
/// `capture::unsandboxed_peer`'s look at its process: only with proof that it is outside any
/// sandbox.
pub fn client_may_type(peer: Option<&Peer>) -> bool {
    peer.is_some()
}

fn may_type(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.may_type)
}

/// The three globals, made once at startup. Nothing needs to be kept: Smithay holds the state.
pub fn init(display: &DisplayHandle) {
    TextInputManagerState::new::<Slipstream>(display);
    InputMethodManagerState::new::<Slipstream, _>(display, may_type);
    VirtualKeyboardManagerState::new::<Slipstream, _>(display, may_type);
}

/// The candidate pop-up goes with the window it's typing into, and is drawn with that window's
/// other pop-ups.
impl InputMethodHandler for Slipstream {
    fn new_popup(&mut self, surface: PopupSurface) {
        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            tracing::warn!("couldn't track an input method's pop-up: {err}");
        }
    }

    fn popup_repositioned(&mut self, _surface: PopupSurface) {}

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        self.space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(parent))
            .map(|window| window.geometry())
            .unwrap_or_default()
    }
}

impl TextInputActivation for Slipstream {}

/// A virtual keyboard's keymap standing in for the seat's own while its keys are read, with the
/// seat's keymap and active layout to put back.
pub struct StandIn {
    keymap: Arc<str>,
    own: String,
    layout: Layout,
}

/// A virtual keyboard's keycodes mean something only under its own keymap, so the seat takes that
/// keymap on for them, and a real key puts the seat's own back first (`own_keymap`).
impl VirtualKeyboardHandler for Slipstream {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        match event {
            InputEvent::Keyboard { event } => {
                let device = event.device();
                self.stand_in(&keyboard, device.keymap());
                let (keycode, state) = (event.key_code(), event.state());
                let (serial, time) = (SERIAL_COUNTER.next_serial(), event.time());
                // An input method passing on a key it didn't use, through a virtual keyboard of
                // its own, would get that key straight back from its grab, for ever.
                let grab_client = self.seat.input_method().keyboard_grab_client();
                if grab_client.is_some() && grab_client == device.client() {
                    keyboard.input_forward_bypassing_grab(self, keycode, state, serial, time, true);
                } else {
                    keyboard.input_from_source(
                        device.source(),
                        self,
                        keycode,
                        state,
                        serial,
                        time,
                        |_, _, _| FilterResult::Forward::<()>,
                    );
                }
            }
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
                device,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            }) => {
                self.stand_in(&keyboard, device.keymap());
                keyboard.with_xkb_state(self, |mut context| {
                    context.set_modifier_mask(mods_depressed, mods_latched, mods_locked, group)
                });
            }
            _ => {}
        }
    }
}

impl Slipstream {
    /// Reads keys with a virtual keyboard's `keymap`, or the seat's own when it sent none.
    fn stand_in(&mut self, keyboard: &KeyboardHandle<Self>, keymap: Option<Arc<str>>) {
        let Some(keymap) = keymap else {
            self.own_keymap();
            return;
        };
        if let Some(stand_in) = &self.keys.stand_in
            && Arc::ptr_eq(&stand_in.keymap, &keymap)
        {
            return;
        }
        self.own_keymap();
        let (own, layout) = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            (xkb.keymap_as_string(), xkb.active_layout())
        });
        // An input method handing back keys usually sends the seat's own keymap.
        if *keymap == *own {
            return;
        }
        match keyboard.set_keymap_from_string(self, keymap.to_string()) {
            Ok(()) => {
                self.keys.stand_in = Some(StandIn {
                    keymap,
                    own,
                    layout,
                })
            }
            Err(err) => tracing::warn!(?err, "couldn't use a virtual keyboard's keymap"),
        }
    }

    /// Puts the seat's own keymap and layout back, if a virtual keyboard's stands in for it.
    pub fn own_keymap(&mut self) {
        let Some(stand_in) = self.keys.stand_in.take() else {
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if keyboard.set_keymap_from_string(self, stand_in.own).is_ok() {
            keyboard.with_xkb_state(self, |mut context| context.set_layout(stand_in.layout));
        }
    }
}
