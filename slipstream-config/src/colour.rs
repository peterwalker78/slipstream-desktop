//! Colour maths shared by the desktop and Settings: OKLCH, where equal steps look equal, and
//! WCAG's contrast ratio, with the rule that keeps a colour of the user's legible on a dark
//! surface.
//!
//! Colours are sRGB, each channel 0–1.

/// The contrast WCAG asks of marks that carry meaning but aren't text (success criterion 1.4.11,
/// non-text contrast).
pub const NON_TEXT: f64 = 3.0;

/// WCAG 2's contrast ratio between two colours, from 1 (the same) to 21 (black on white).
pub fn contrast(one: [f32; 3], other: [f32; 3]) -> f64 {
    let luminance = |rgb: [f32; 3]| {
        let [r, g, b] = rgb.map(|c| linear(c as f64));
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// `rgb` made light enough to stand `ratio` to 1 against a darker `background`: the same OKLCH
/// hue and chroma, with the lightness raised as little as reaches the ratio. A colour that
/// already reaches it comes back as it was, and none comes back darker. Where the screen can't
/// show the chroma at the new lightness, it is the most the screen can show of that hue. A
/// background too light for any colour to reach the ratio gets the lightest.
pub fn legible(rgb: [f32; 3], background: [f32; 3], ratio: f64) -> [f32; 3] {
    if contrast(rgb, background) >= ratio {
        return rgb;
    }
    let (lightness, chroma, hue) = oklch(rgb);
    let at = |lightness: f64| {
        let chroma = most_chroma(lightness, hue, chroma);
        linear_rgb(lightness, chroma, hue).map(|c| encoded(c.clamp(0.0, 1.0)) as f32)
    };
    // Each colour drawn is whole bytes, so a lightness counts only once its bytes reach the ratio.
    let reaches = |lightness: f64| {
        let bytes = at(lightness).map(|c| (c * 255.0).round() / 255.0);
        contrast(bytes, background) >= ratio
    };
    let (mut low, mut high) = (lightness, 1.0);
    if !reaches(high) {
        return at(high);
    }
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if reaches(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    at(high)
}

/// An sRGB channel as linear light.
pub fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light as an sRGB channel.
pub fn encoded(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Lightness, chroma and hue in degrees.
pub fn oklch(rgb: [f32; 3]) -> (f64, f64, f64) {
    let [r, g, b] = rgb.map(|c| linear(c as f64));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    (
        lightness,
        a.hypot(b),
        b.atan2(a).to_degrees().rem_euclid(360.0),
    )
}

/// How vivid `hue` can be at `lightness`, up to `chroma`, and still be shown on a screen.
pub fn most_chroma(lightness: f64, hue: f64, chroma: f64) -> f64 {
    let shown = |chroma| {
        linear_rgb(lightness, chroma, hue)
            .iter()
            .all(|c| (-1e-4..=1.0 + 1e-4).contains(c))
    };
    if shown(chroma) {
        return chroma;
    }
    let (mut low, mut high) = (0.0, chroma);
    for _ in 0..18 {
        let middle = (low + high) / 2.0;
        if shown(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// The OKLCH colour as linear red, green and blue, which may fall outside 0–1.
pub fn linear_rgb(lightness: f64, chroma: f64, hue: f64) -> [f64; 3] {
    let (sin, cos) = hue.to_radians().sin_cos();
    let (a, b) = (chroma * cos, chroma * sin);
    let l = (lightness + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (lightness - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (lightness - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: &str) -> [f32; 3] {
        crate::rgb(hex).unwrap()
    }

    /// Whole bytes, as the colour is drawn.
    fn bytes(rgb: [f32; 3]) -> [f32; 3] {
        rgb.map(|c| (c * 255.0).round() / 255.0)
    }

    #[test]
    fn contrast_runs_from_one_to_twenty_one() {
        assert!((contrast(rgb("#000000"), rgb("#ffffff")) - 21.0).abs() < 1e-9);
        assert!((contrast(rgb("#42d3ff"), rgb("#42d3ff")) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_deep_colour_is_lifted_to_three_to_one_and_keeps_its_hue() {
        let (teal, backing) = (rgb("#005a75"), rgb("#0b0e13"));
        assert!(contrast(teal, backing) < NON_TEXT);
        let lifted = legible(teal, backing, NON_TEXT);
        assert!(contrast(bytes(lifted), backing) >= NON_TEXT);
        assert!(
            contrast(bytes(lifted), backing) < NON_TEXT + 0.1,
            "only just"
        );
        let (before, after) = (oklch(teal), oklch(lifted));
        assert!(after.0 > before.0, "lighter, never darker");
        assert!((after.2 - before.2).abs() < 1.0, "same hue");
        assert!((after.1 - before.1).abs() < 0.005, "same chroma");
    }

    #[test]
    fn a_colour_that_already_reaches_the_ratio_is_left_alone() {
        let backing = rgb("#0b0e13");
        for colour in ["#42d3ff", "#ffb547", "#ffffff"] {
            assert_eq!(legible(rgb(colour), backing, NON_TEXT), rgb(colour));
        }
    }

    #[test]
    fn every_deep_colour_of_the_wheel_reaches_the_ratio() {
        let backing = rgb("#0b0e13");
        for degree in (0..360).step_by(15) {
            let deep =
                linear_rgb(0.44, 0.08, degree as f64).map(|c| encoded(c.clamp(0.0, 1.0)) as f32);
            let lifted = legible(deep, backing, NON_TEXT);
            assert!(contrast(bytes(lifted), backing) >= NON_TEXT, "{degree}");
        }
    }
}
