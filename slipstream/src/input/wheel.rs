//! One frame of scrolling, from a mouse's wheel, a touchpad, or a virtual pointer: Super with it
//! zooms, over something of Slipstream's own it moves through the list there, and otherwise it
//! goes to the app under the pointer. Every source goes through `Slipstream::wheel`, so they can't
//! drift apart.

use smithay::{
    backend::input::{Axis, AxisSource, InputTime},
    input::pointer::AxisFrame,
};

use crate::state::Slipstream;

/// A wheel's notch is 120 in v120 units, and this much scroll.
const NOTCH: f64 = 15.0;

/// One frame of scrolling; each pair is horizontal, then vertical.
pub struct Wheel {
    pub source: AxisSource,
    pub time: InputTime,
    /// How far, in scroll, where the device said.
    pub amount: [Option<f64>; 2],
    /// How far in notches, 120 to a notch, where the device counts them.
    pub v120: [Option<f64>; 2],
    /// The axes a finger has stopped scrolling on, where the device says so on its own. A
    /// touchpad says so with no movement instead.
    pub stop: [bool; 2],
}

const AXES: [Axis; 2] = [Axis::Horizontal, Axis::Vertical];

impl Wheel {
    /// How far it scrolled along axis `i`, from the notches where the device gave no amount.
    fn amount(&self, i: usize) -> f64 {
        self.amount[i].unwrap_or_else(|| self.v120[i].unwrap_or(0.0) * NOTCH / 120.0)
    }

    /// A notch's worth of scroll: a touchpad's is twice a wheel's.
    fn notch(&self) -> f64 {
        if self.source == AxisSource::Finger {
            2.0 * NOTCH
        } else {
            NOTCH
        }
    }

    /// What the app under the pointer is sent. Notches beyond what an `i32` holds saturate.
    pub(crate) fn frame(&self) -> AxisFrame {
        let mut frame = AxisFrame::new(self.time).source(self.source);
        for (i, axis) in AXES.into_iter().enumerate() {
            let amount = self.amount(i);
            if amount != 0.0 {
                frame = frame.value(axis, amount);
                if let Some(v120) = self.v120[i] {
                    frame = frame.v120(axis, v120 as i32);
                }
            }
            let lifted = self.source == AxisSource::Finger && self.amount[i] == Some(0.0);
            if self.stop[i] || lifted {
                frame = frame.stop(axis);
            }
        }
        frame
    }
}

impl Slipstream {
    /// A frame of scrolling, from whichever device.
    pub fn wheel(&mut self, wheel: Wheel) {
        self.keys.super_tap.disarm();
        self.keys.layout_tap.disarm();
        self.wake_ui();
        // Super with the wheel zooms the screen, whatever the pointer is over.
        let held = self.seat.get_keyboard().map(|kb| kb.modifier_state());
        if let Some(held) = held
            && crate::keys::zooms_with_wheel(crate::keys::Mods::from_state(&held))
        {
            let now = self.clock.wall();
            self.zoom.wheel(wheel.amount(1) / wheel.notch(), now);
            return;
        }
        // The wheel over an overlay would otherwise scroll the window hidden under it. In the
        // explorer and the notification centre it moves through the list instead.
        if self.pointer_blocked() || self.over_chrome(self.pointer_location()) {
            self.overlay_wheel(wheel.amount(1), wheel.notch());
            return;
        }
        let pointer = self.seat.get_pointer().unwrap();
        pointer.axis(self, wheel.frame());
        pointer.frame(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(source: AxisSource, amount: [Option<f64>; 2], v120: [Option<f64>; 2]) -> Wheel {
        Wheel {
            source,
            time: InputTime::from_millis(7),
            amount,
            v120,
            stop: [false; 2],
        }
    }

    #[test]
    fn a_notch_of_the_wheel_scrolls_as_one() {
        let frame = wheel(AxisSource::Wheel, [None, Some(15.0)], [None, Some(120.0)]).frame();
        assert_eq!(frame.axis, (0.0, 15.0));
        assert_eq!(frame.v120, Some((0, 120)));
        assert_eq!(frame.stop, (false, false));
        // Notches alone are worth their scroll.
        let frame = wheel(AxisSource::Wheel, [None; 2], [Some(-240.0), None]).frame();
        assert_eq!(frame.axis, (-30.0, 0.0));
        assert_eq!(frame.v120, Some((-240, 0)));
    }

    #[test]
    fn a_finger_lifted_stops_the_scroll() {
        let frame = wheel(AxisSource::Finger, [Some(0.0), Some(4.5)], [None; 2]).frame();
        assert_eq!(frame.axis, (0.0, 4.5));
        assert_eq!(frame.stop, (true, false));
        // A wheel never stops that way.
        let frame = wheel(AxisSource::Wheel, [Some(0.0), None], [None; 2]).frame();
        assert_eq!(frame.stop, (false, false));
        // Unless a device says so on its own.
        let mut stopped = wheel(AxisSource::Finger, [None; 2], [None; 2]);
        stopped.stop = [false, true];
        assert_eq!(stopped.frame().stop, (false, true));
    }

    #[test]
    fn notches_beyond_an_i32_saturate() {
        let huge = f64::from(i32::MAX) * 120.0;
        let frame = wheel(AxisSource::Wheel, [None; 2], [Some(-huge), Some(huge)]).frame();
        assert_eq!(frame.v120, Some((i32::MIN, i32::MAX)));
    }

    #[test]
    fn a_touchpad_notch_is_twice_a_wheel_notch() {
        assert_eq!(wheel(AxisSource::Wheel, [None; 2], [None; 2]).notch(), 15.0);
        assert_eq!(
            wheel(AxisSource::Finger, [None; 2], [None; 2]).notch(),
            30.0
        );
    }
}
