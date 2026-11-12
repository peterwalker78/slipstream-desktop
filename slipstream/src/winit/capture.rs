//! Nested, the window holds the keyboard, Super included, so Slipstream's keys work as they do
//! in the login session. On a Wayland host it asks the host to pass every key through
//! (keyboard-shortcuts-inhibit) while it holds them. It takes the keyboard when it gains focus
//! or is clicked, and lets go when it loses focus or Ctrl+Alt is pressed and let go on its own,
//! as virtual machines do. Let go of, keys still reach the focused app but run none of
//! Slipstream's shortcuts, so the host's own keys can be used.

use std::mem::ManuallyDrop;

use smithay::reexports::winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::Window,
};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat, wl_surface::WlSurface},
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1},
};

const HELD_TITLE: &str = "Slipstream (Ctrl+Alt lets go of the keyboard)";
const LET_GO_TITLE: &str = "Slipstream (click to use the keyboard here)";

pub struct Capture {
    held: bool,
    chord: LetGoChord,
    /// The host connection is winit's, and its event loop closes it before the closure holding
    /// this is dropped; destroying the queue after that would touch a freed display. The window
    /// lasts the whole run, so this is never dropped and the process exit reclaims it.
    host: Option<ManuallyDrop<HostInhibit>>,
}

impl Capture {
    /// Starts let go of: the window takes the keyboard when the host first focuses it.
    pub fn new(window: &dyn Window) -> Self {
        let host = match HostInhibit::connect(window) {
            Ok(host) => Some(ManuallyDrop::new(host)),
            Err(why) => {
                tracing::info!("the host desktop keeps its own shortcuts: {why}");
                None
            }
        };
        window.set_title(LET_GO_TITLE);
        Self {
            held: false,
            chord: LetGoChord::default(),
            host,
        }
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn take(&mut self, window: &dyn Window) {
        if self.held {
            return;
        }
        self.held = true;
        if let Some(host) = &mut self.host {
            host.inhibit();
        }
        window.set_title(HELD_TITLE);
    }

    pub fn let_go(&mut self, window: &dyn Window) {
        self.chord = LetGoChord::default();
        if !self.held {
            return;
        }
        self.held = false;
        if let Some(host) = &mut self.host {
            host.release();
        }
        window.set_title(LET_GO_TITLE);
    }

    /// A key from the host, by xkb keycode. Ctrl+Alt on its own lets go of the keyboard.
    pub fn key(&mut self, window: &dyn Window, code: u32, pressed: bool) {
        if self.chord.key(code, pressed) && self.held {
            self.let_go(window);
        }
    }

    /// Handles the inhibitor's events, which winit's reads of the shared socket have queued.
    pub fn dispatch(&mut self) {
        if let Some(host) = &mut self.host {
            host.dispatch();
        }
    }
}

/// Spots Ctrl+Alt pressed and let go with no other key in between.
#[derive(Debug, Default)]
struct LetGoChord {
    down: Vec<u32>,
    ctrl: bool,
    alt: bool,
    /// Another key was part of it, or the chord already fired.
    spoilt: bool,
}

// xkb keycodes: the evdev codes plus 8.
const LEFT_CTRL: u32 = 37;
const RIGHT_CTRL: u32 = 105;
const LEFT_ALT: u32 = 64;
const RIGHT_ALT: u32 = 108;

impl LetGoChord {
    /// True on the release that completes the chord.
    fn key(&mut self, code: u32, pressed: bool) -> bool {
        if pressed {
            if !self.down.contains(&code) {
                self.down.push(code);
            }
            match code {
                LEFT_CTRL | RIGHT_CTRL => self.ctrl = true,
                LEFT_ALT | RIGHT_ALT => self.alt = true,
                _ => self.spoilt = true,
            }
            return false;
        }
        self.down.retain(|down| *down != code);
        let done = self.ctrl && self.alt && !self.spoilt;
        if self.down.is_empty() {
            *self = Self::default();
        } else if done {
            self.spoilt = true;
        }
        done
    }
}

/// keyboard-shortcuts-inhibit on the host's connection, for the window's surface.
struct HostInhibit {
    connection: Connection,
    queue: EventQueue<HostEvents>,
    manager: ZwpKeyboardShortcutsInhibitManagerV1,
    seat: WlSeat,
    surface: WlSurface,
    inhibitor: Option<ZwpKeyboardShortcutsInhibitorV1>,
}

impl HostInhibit {
    fn connect(window: &dyn Window) -> Result<Self, String> {
        let display = window.display_handle().map_err(|err| err.to_string())?;
        let RawDisplayHandle::Wayland(display) = display.as_raw() else {
            return Err("only a Wayland host can be asked to pass every key".into());
        };
        let handle = window.window_handle().map_err(|err| err.to_string())?;
        let RawWindowHandle::Wayland(handle) = handle.as_raw() else {
            return Err("the window has no Wayland surface".into());
        };
        // SAFETY: the display is winit's live connection, which lasts as long as the window,
        // and `Capture` never drops this.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, queue) =
            registry_queue_init::<HostEvents>(&connection).map_err(|err| err.to_string())?;
        let qh = queue.handle();
        let manager = globals
            .bind::<ZwpKeyboardShortcutsInhibitManagerV1, _, _>(&qh, 1..=1, ())
            .map_err(|err| format!("no keyboard-shortcuts-inhibit ({err})"))?;
        let seat = globals
            .bind::<WlSeat, _, _>(&qh, 1..=1, ())
            .map_err(|err| format!("no seat ({err})"))?;
        // SAFETY: the pointer is the window's own wl_surface, alive as long as the window.
        let surface =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
                .and_then(|id| WlSurface::from_id(&connection, id))
                .map_err(|err| err.to_string())?;
        Ok(Self {
            connection,
            queue,
            manager,
            seat,
            surface,
            inhibitor: None,
        })
    }

    fn inhibit(&mut self) {
        if self.inhibitor.is_none() {
            let qh = self.queue.handle();
            self.inhibitor =
                Some(
                    self.manager
                        .inhibit_shortcuts(&self.surface, &self.seat, &qh, ()),
                );
            let _ = self.connection.flush();
        }
    }

    fn release(&mut self) {
        if let Some(inhibitor) = self.inhibitor.take() {
            inhibitor.destroy();
            let _ = self.connection.flush();
        }
    }

    fn dispatch(&mut self) {
        if let Err(err) = self.queue.dispatch_pending(&mut HostEvents) {
            tracing::warn!("keyboard-shortcuts-inhibit: {err}");
        }
    }
}

struct HostEvents;

impl Dispatch<WlRegistry, GlobalListContents> for HostEvents {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for HostEvents {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => {
                tracing::debug!("the host passes every key to the window");
            }
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => {
                tracing::debug!("the host keeps its own shortcuts for now");
            }
            _ => {}
        }
    }
}

delegate_noop!(HostEvents: ignore WlSeat);
delegate_noop!(HostEvents: ZwpKeyboardShortcutsInhibitManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 36;

    fn chord(keys: &[(u32, bool)]) -> Vec<bool> {
        let mut chord = LetGoChord::default();
        keys.iter()
            .map(|(code, pressed)| chord.key(*code, *pressed))
            .collect()
    }

    #[test]
    fn ctrl_alt_on_its_own_lets_go_of_the_keyboard() {
        let fired = chord(&[
            (LEFT_CTRL, true),
            (LEFT_ALT, true),
            (LEFT_ALT, false),
            (LEFT_CTRL, false),
        ]);
        assert_eq!(fired, [false, false, true, false]);
        let fired = chord(&[
            (RIGHT_ALT, true),
            (RIGHT_CTRL, true),
            (RIGHT_CTRL, false),
            (RIGHT_ALT, false),
        ]);
        assert_eq!(fired, [false, false, true, false]);
    }

    #[test]
    fn ctrl_alt_with_another_key_keeps_the_keyboard() {
        let fired = chord(&[
            (LEFT_CTRL, true),
            (LEFT_ALT, true),
            (T, true),
            (T, false),
            (LEFT_ALT, false),
            (LEFT_CTRL, false),
        ]);
        assert!(!fired.contains(&true));
        // Ctrl or Alt alone is never the chord.
        let fired = chord(&[(LEFT_CTRL, true), (LEFT_CTRL, false)]);
        assert!(!fired.contains(&true));
        let fired = chord(&[(LEFT_ALT, true), (LEFT_ALT, false)]);
        assert!(!fired.contains(&true));
    }

    #[test]
    fn a_spoilt_chord_does_not_spoil_the_next() {
        let fired = chord(&[
            (LEFT_CTRL, true),
            (T, true),
            (LEFT_ALT, true),
            (T, false),
            (LEFT_ALT, false),
            (LEFT_CTRL, false),
            (LEFT_CTRL, true),
            (LEFT_ALT, true),
            (LEFT_CTRL, false),
        ]);
        assert_eq!(fired.iter().filter(|fired| **fired).count(), 1);
        assert_eq!(fired.last(), Some(&true));
    }
}
