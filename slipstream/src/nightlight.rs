//! Night light: warmer colours, as the mockup's, which multiplies `#ff8a2a` over the screen at 24%.
//! Done in each screen's gamma ramp rather than drawn, so it covers everything, the pointer and
//! fullscreen games included, and costs nothing per frame. Only on real hardware: nested, the host
//! desktop owns the screen's colours.

/// How much of red, green and blue is left: `1 − 0.24 × (1 − overlay)` for each of the overlay's
/// channels.
pub const WARMTH: [f64; 3] = [
    1.0 - 0.24 * (1.0 - 0xff as f64 / 255.0),
    1.0 - 0.24 * (1.0 - 0x8a as f64 / 255.0),
    1.0 - 0.24 * (1.0 - 0x2a as f64 / 255.0),
];

/// Red, green and blue ramps of `length` steps: straight lines, scaled by `WARMTH` when on.
pub fn ramps(length: usize, on: bool) -> [Vec<u16>; 3] {
    let last = length.saturating_sub(1).max(1) as f64;
    std::array::from_fn(|channel| {
        let top = if on { WARMTH[channel] } else { 1.0 };
        (0..length)
            .map(|step| (step as f64 / last * top * 65535.0).round() as u16)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_the_identity() {
        let [red, green, blue] = ramps(256, false);
        for ramp in [red, green, blue] {
            assert_eq!((ramp[0], ramp[255]), (0, 65535));
            assert!(ramp.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn on_keeps_red_and_takes_some_blue_never_darkening_much() {
        let [red, green, blue] = ramps(256, true);
        assert_eq!(red[255], 65535);
        assert!(green[255] < red[255] && blue[255] < green[255]);
        assert!(
            blue[255] as f64 > 0.75 * 65535.0,
            "a warm tint, not a dim screen"
        );
    }
}
