//! Input methods: typing Chinese, Japanese, Korean and other scripts through IBus or fcitx5, and
//! on-screen keyboards.
//!
//! Apps say where text goes through `zwp_text_input_v3`, and an input method
//! (`zwp_input_method_v2`) turns keys into text for them, with a candidate pop-up by the caret.
//! Smithay routes between the two and follows keyboard focus on its own. On-screen keyboards and
//! typing tools use `zwp_virtual_keyboard_v1` to send keys.
//!
//! **Unsandboxed clients only**, for the two that aren't apps asking for text. An input method
//! sees every key typed into every app, and a virtual keyboard can type into any of them, so
//! both globals are filtered as the clipboard's are: a client sees them only if, when it
//! connected, its process was proven to be outside any sandbox.

use smithay::{
    desktop::{PopupKind, PopupManager},
    reexports::wayland_server::{Client, DisplayHandle, protocol::wl_surface::WlSurface},
    utils::{Logical, Rectangle},
    wayland::{
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface},
        seat::WaylandFocus,
        text_input::TextInputManagerState,
        virtual_keyboard::VirtualKeyboardManagerState,
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
