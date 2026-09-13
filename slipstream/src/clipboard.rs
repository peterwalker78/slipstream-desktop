//! Clipboard tools and clipboard managers: the data-control protocols.
//!
//! `ext-data-control-v1`, and `zwlr-data-control-v1` before it, let a client read and set the
//! clipboard and the primary selection at any time, with no window and no keyboard focus. That is
//! what `wl-copy`, `wl-paste` and clipboard managers are built on. Without either protocol they
//! fall back to `wl_data_device`, which only deals with the focused client, so each of them maps
//! a window for an instant to be given focus: it tiles, takes focus from the app in front, and
//! the layout retiles when it closes, on every copy. Both are served, since tools differ in which
//! they speak: `wl-clipboard` prefers ext and falls back to wlr.
//!
//! **Unsandboxed clients only.** A client that can read the clipboard whenever it changes can
//! also watch everything copied, so the globals are filtered as the capture globals are: a client
//! sees them only if, when it connected, its process was proven to be outside any sandbox
//! (`capture::unsandboxed_peer`). A Flatpak, or a client whose process couldn't be looked at,
//! never sees them and keeps the focus-bound `wl_data_device` and primary selection.

use smithay::{
    reexports::wayland_server::{Client, DisplayHandle},
    wayland::selection::{
        ext_data_control, primary_selection::PrimarySelectionState, wlr_data_control,
    },
};

use crate::{Slipstream, capture::Peer, state::ClientState};

/// Whether a connecting client may control the clipboard, from `capture::unsandboxed_peer`'s
/// look at its process: only with proof that it is outside any sandbox.
pub fn client_may_control(peer: Option<&Peer>) -> bool {
    peer.is_some()
}

/// Which clients may see the data-control globals: those `client_may_control` allowed when they
/// connected. A client with no state of ours, such as XWayland, is refused; X11 apps reach the
/// clipboard through the window manager instead.
pub fn may_control(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.may_control_clipboard)
}

/// The globals, made once at startup and offered only to the clients `may_control` allows. Both
/// carry the primary selection as well as the clipboard, on the same seat as `wl_data_device`
/// and the primary selection's own global, so a selection set through any of them reaches the
/// others.
pub struct Globals {
    pub ext: ext_data_control::DataControlState,
    pub wlr: wlr_data_control::DataControlState,
}

pub fn state(display: &DisplayHandle, primary: &PrimarySelectionState) -> Globals {
    Globals {
        ext: ext_data_control::DataControlState::new::<Slipstream, _>(
            display,
            Some(primary),
            may_control,
        ),
        wlr: wlr_data_control::DataControlState::new::<Slipstream, _>(
            display,
            Some(primary),
            may_control,
        ),
    }
}

impl ext_data_control::DataControlHandler for Slipstream {
    fn data_control_state(&mut self) -> &mut ext_data_control::DataControlState {
        &mut self.ext_data_control
    }
}

impl wlr_data_control::DataControlHandler for Slipstream {
    fn data_control_state(&mut self) -> &mut wlr_data_control::DataControlState {
        &mut self.wlr_data_control
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixStream, sync::Arc};

    use smithay::reexports::wayland_server::Display;

    use super::*;
    use crate::capture;

    /// A client's state as the listener makes it, handed to a display, and whether the globals'
    /// filter then lets that client see them.
    fn filter_allows(stream: UnixStream) -> bool {
        let state = ClientState::for_connection(&stream);
        let display = Display::<Slipstream>::new().expect("a display");
        let client = display
            .handle()
            .insert_client(stream, Arc::new(state))
            .expect("the client inserts");
        may_control(&client)
    }

    #[test]
    fn an_unsandboxed_client_controls_the_clipboard_and_still_cannot_capture() {
        // Both ends of a pair are this process: not in a Flatpak, and still running.
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        let state = ClientState::for_connection(&ours);
        assert!(state.may_control_clipboard);
        assert!(!state.may_capture, "the clipboard isn't the screen");
        assert!(filter_allows(ours));
    }

    #[test]
    fn a_client_whose_connecting_process_has_gone_is_refused() {
        for reap in [false, true] {
            let state =
                capture::with_a_client_that_has_gone("clip", reap, ClientState::for_connection);
            assert!(!state.may_control_clipboard, "gone, reaped: {reap}");
            assert!(!state.may_capture);
        }
    }

    #[test]
    fn no_proof_no_control() {
        assert!(!client_may_control(None));
    }

    #[test]
    fn a_client_with_no_state_of_ours_is_refused() {
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        let display = Display::<Slipstream>::new().expect("a display");
        let client = display
            .handle()
            .insert_client(ours, Arc::new(()))
            .expect("the client inserts");
        assert!(!may_control(&client));
    }
}
