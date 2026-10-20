//! Copy, paste and cut in whichever app has the keyboard, by sending it the keys it already uses
//! for them: Ctrl+C, Ctrl+V and Ctrl+X, or Ctrl+Shift+C and Ctrl+Shift+V in a terminal, where
//! Ctrl+C interrupts and nothing can be cut, so cutting copies.

use smithay::{
    backend::input::{InputTime, KeyState},
    input::keyboard::{Keycode, Keysym},
};

use crate::{Slipstream, keys::Edit};

/// The letter and whether Shift goes with Ctrl, for `edit` in a terminal or in any other app.
pub fn chord(edit: Edit, terminal: bool) -> (Keysym, bool) {
    match (edit, terminal) {
        (Edit::Copy, false) => (Keysym::c, false),
        (Edit::Paste, false) => (Keysym::v, false),
        (Edit::Cut, false) => (Keysym::x, false),
        (Edit::Copy | Edit::Cut, true) => (Keysym::c, true),
        (Edit::Paste, true) => (Keysym::v, true),
    }
}

impl Slipstream {
    /// Sends the focused app the keys for `edit`. Keys still held, such as the Super of Super+V,
    /// are let go for the app first and the modifiers among them pressed again after, so the app
    /// sees the chord on its own and the keyboard is left as the hand holds it.
    pub fn send_edit(&mut self, edit: Edit) {
        let Some(window) = self.focused_window() else {
            return;
        };
        let terminal =
            crate::state::window_app_id(&window).is_some_and(|id| crate::history::is_terminal(&id));
        let (letter, shift) = chord(edit, terminal);
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let held: Vec<Keycode> = keyboard.pressed_keys().into_iter().collect();
        let modifiers: Vec<Keycode> = held
            .iter()
            .copied()
            .filter(|code| self.is_modifier(*code))
            .collect();
        // A layout without the Latin letter still has the key that was pressed with Super.
        let letter = self
            .keycode_for(letter)
            .or_else(|| held.iter().copied().find(|code| !modifiers.contains(code)));
        let Some(letter) = letter else {
            tracing::warn!(?edit, "no key in the layout to send");
            return;
        };
        tracing::info!(?edit, terminal, "sending the app its keys");
        for code in &held {
            self.send_key(*code, false);
        }
        let mut chord = vec!["Control_L"];
        if shift {
            chord.push("Shift_L");
        }
        for name in &chord {
            self.inject_key(name, true);
        }
        self.send_key(letter, true);
        self.send_key(letter, false);
        for name in chord.iter().rev() {
            self.inject_key(name, false);
        }
        for code in modifiers {
            self.send_key(code, true);
        }
        // Super pressed again on its own is no tap: the hand never let go of it.
        self.keys.super_tap.disarm();
    }

    fn send_key(&mut self, keycode: Keycode, pressed: bool) {
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.key_event(keycode, state, InputTime::now(), true);
    }

    /// Whether `keycode` is a modifier in the active layout.
    fn is_modifier(&mut self, keycode: Keycode) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        keyboard.with_xkb_state(self, |context| {
            let Ok(xkb) = context.xkb().lock() else {
                return false;
            };
            let layout = xkb.active_layout();
            xkb.raw_syms_for_key_in_layout(keycode, layout)
                .iter()
                .any(|sym| sym.is_modifier_key())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_gets_its_own_keys_and_is_never_interrupted() {
        assert_eq!(chord(Edit::Copy, false), (Keysym::c, false));
        assert_eq!(chord(Edit::Paste, false), (Keysym::v, false));
        assert_eq!(chord(Edit::Cut, false), (Keysym::x, false));
        // Ctrl+C would interrupt what's running, and Ctrl+X is a shell's prefix key.
        for edit in [Edit::Copy, Edit::Cut, Edit::Paste] {
            let (_, shift) = chord(edit, true);
            assert!(shift, "{edit:?} in a terminal takes Shift");
        }
        assert_eq!(chord(Edit::Cut, true), (Keysym::c, true));
    }
}
