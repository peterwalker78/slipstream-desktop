//! Holding Caps Lock down keeps the desktop awake: the UI stops fading to the wallpaper until it's
//! held again. A tap is still Caps Lock.
//!
//! xkb changes its state the moment it sees a key, before any filter can look at it, so Caps Lock's
//! press is kept back from it until the press turns out to be a tap or a hold. Let go before
//! `HOLD` and the press is sent on, late, followed by the release, and Caps Lock toggles as ever.
//! Held past `HOLD`, it toggles Awake instead, and neither the press nor the release is ever seen
//! by xkb or an app. Any other key pressed while Caps Lock waits settles it as a tap on the spot,
//! with Caps Lock sent first, so a capital rolled straight into still comes out capital and
//! Caps Lock chords (a screen reader's) still work.
//!
//! Awake holds off the fade only, never the lock: it's easily left on, and a desk left awake
//! mustn't be left unlocked too.

use std::time::Duration;

use smithay::{backend::input::InputTime, input::keyboard::Keycode};

/// How long Caps Lock is held before it counts as a hold. A brisk tap is well under half of it.
pub const HOLD: Duration = Duration::from_millis(500);

/// What to do with a key, given what Caps Lock is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do with Caps Lock: the key goes on as usual.
    Pass,
    /// Caps Lock went down: keep it back, and wait `HOLD` to see if it's held.
    Wait(u64),
    /// Caps Lock was only tapped: send its press on first, then this key as usual.
    Replay(Keycode, InputTime),
    /// Caps Lock let go after a hold: it goes nowhere.
    Swallow,
}

struct Held {
    keycode: Keycode,
    time: InputTime,
    /// Which press this is, so a wait left over from an earlier tap can't count for this one.
    press: u64,
    long: bool,
}

#[derive(Default)]
pub struct CapsKey {
    held: Option<Held>,
    presses: u64,
}

impl CapsKey {
    /// A key pressed or let go. `caps` says whether it's Caps Lock; `may_hold` whether a hold may
    /// start now (not at the lock screen, where it only types).
    pub fn key(
        &mut self,
        keycode: Keycode,
        pressed: bool,
        time: InputTime,
        caps: bool,
        may_hold: bool,
    ) -> Step {
        let Some(held) = &self.held else {
            if pressed && caps && may_hold {
                self.presses += 1;
                self.held = Some(Held {
                    keycode,
                    time,
                    press: self.presses,
                    long: false,
                });
                return Step::Wait(self.presses);
            }
            return Step::Pass;
        };
        if keycode == held.keycode {
            if pressed {
                // A repeat of a key still down; it's already accounted for.
                return Step::Swallow;
            }
            let held = self.held.take().unwrap();
            return if held.long {
                Step::Swallow
            } else {
                Step::Replay(held.keycode, held.time)
            };
        }
        if pressed && !held.long {
            let held = self.held.take().unwrap();
            return Step::Replay(held.keycode, held.time);
        }
        Step::Pass
    }

    /// The wait for `press` is over. True if Caps Lock is still down, so it's a hold.
    pub fn elapsed(&mut self, press: u64) -> bool {
        match &mut self.held {
            Some(held) if held.press == press && !held.long => {
                held.long = true;
                true
            }
            _ => false,
        }
    }

    /// A Caps Lock press still being kept back, and not yet a hold, handed back to be sent on.
    /// The lock taking the keyboard settles it as a tap: a key down at the lock is the lock's.
    pub fn settle(&mut self) -> Option<(Keycode, InputTime)> {
        if self.held.as_ref().is_some_and(|held| !held.long) {
            let held = self.held.take().unwrap();
            return Some((held.keycode, held.time));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: u32 = 66;
    const Q: u32 = 24;

    fn at(ms: u32) -> InputTime {
        InputTime::from_millis(ms)
    }

    fn key(caps: &mut CapsKey, code: u32, pressed: bool, ms: u32) -> Step {
        caps.key(Keycode::new(code), pressed, at(ms), code == CAPS, true)
    }

    #[test]
    fn a_tap_is_caps_lock_sent_late() {
        let mut caps = CapsKey::default();
        assert_eq!(key(&mut caps, CAPS, true, 0), Step::Wait(1));
        assert_eq!(
            key(&mut caps, CAPS, false, 120),
            Step::Replay(Keycode::new(CAPS), at(0))
        );
        // The wait running out afterwards changes nothing.
        assert!(!caps.elapsed(1));
    }

    #[test]
    fn a_hold_is_awake_and_its_release_goes_nowhere() {
        let mut caps = CapsKey::default();
        let Step::Wait(press) = key(&mut caps, CAPS, true, 0) else {
            panic!("Caps Lock wasn't kept back");
        };
        assert!(caps.elapsed(press));
        assert!(!caps.elapsed(press), "one hold toggles once");
        assert_eq!(key(&mut caps, CAPS, false, 900), Step::Swallow);
        // The next press is a fresh one.
        assert_eq!(key(&mut caps, CAPS, true, 2000), Step::Wait(2));
    }

    #[test]
    fn a_key_rolled_into_settles_it_as_a_tap_first() {
        let mut caps = CapsKey::default();
        key(&mut caps, CAPS, true, 0);
        assert_eq!(
            key(&mut caps, Q, true, 60),
            Step::Replay(Keycode::new(CAPS), at(0))
        );
        assert!(!caps.elapsed(1), "already a tap");
        // Caps Lock's own release then goes on as any other key's would.
        assert_eq!(key(&mut caps, CAPS, false, 90), Step::Pass);
        assert_eq!(key(&mut caps, Q, false, 110), Step::Pass);
    }

    #[test]
    fn keys_during_a_hold_pass_as_usual() {
        let mut caps = CapsKey::default();
        key(&mut caps, CAPS, true, 0);
        caps.elapsed(1);
        assert_eq!(key(&mut caps, Q, true, 700), Step::Pass);
        assert_eq!(key(&mut caps, Q, false, 750), Step::Pass);
        assert_eq!(key(&mut caps, CAPS, false, 800), Step::Swallow);
    }

    #[test]
    fn a_wait_left_over_from_a_tap_cant_make_the_next_press_a_hold() {
        let mut caps = CapsKey::default();
        key(&mut caps, CAPS, true, 0);
        key(&mut caps, CAPS, false, 100);
        assert_eq!(key(&mut caps, CAPS, true, 300), Step::Wait(2));
        // The first press's wait ends at 500 ms, only 200 ms into the second.
        assert!(!caps.elapsed(1));
        assert_eq!(
            key(&mut caps, CAPS, false, 400),
            Step::Replay(Keycode::new(CAPS), at(300))
        );
    }

    #[test]
    fn at_the_lock_screen_caps_lock_is_only_caps_lock() {
        let mut caps = CapsKey::default();
        assert_eq!(
            caps.key(Keycode::new(CAPS), true, at(0), true, false),
            Step::Pass
        );
        assert_eq!(key(&mut caps, CAPS, false, 900), Step::Pass);
    }

    #[test]
    fn locking_mid_press_hands_the_press_back() {
        let mut caps = CapsKey::default();
        key(&mut caps, CAPS, true, 0);
        assert_eq!(caps.settle(), Some((Keycode::new(CAPS), at(0))));
        assert_eq!(caps.settle(), None);
        let mut held = CapsKey::default();
        key(&mut held, CAPS, true, 0);
        held.elapsed(1);
        assert_eq!(held.settle(), None, "a hold is already decided");
    }
}
