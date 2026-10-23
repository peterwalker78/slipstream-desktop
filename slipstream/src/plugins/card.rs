//! The colours of a gauge, on the plugin chip and in the plugin panel: how much of some allowance
//! is used, warming as it runs out, and faded when the numbers are old.

use crate::panel::HOT;

const CALM: u32 = 0xcfd6e2ff;

/// A gauge's colour at `percent` used: `attention`, the secondary colour, from three quarters,
/// and red-orange once nearly all of it has gone.
pub fn ink(percent: u8, attention: u32) -> u32 {
    match percent {
        90.. => HOT,
        75.. => attention,
        _ => CALM,
    }
}

/// `rgba` faded, for numbers that are old.
pub fn dim(rgba: u32) -> u32 {
    let alpha = (rgba & 0xff) * 45 / 100;
    (rgba & !0xff) | alpha
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_warm_as_the_allowance_runs_out() {
        assert_eq!(ink(74, crate::panel::AMBER), CALM);
        assert_eq!(ink(75, crate::panel::AMBER), crate::panel::AMBER);
        assert_eq!(ink(90, crate::panel::AMBER), HOT);
        assert_eq!(dim(0xffb547ff), 0xffb54772);
    }
}
