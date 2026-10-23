//! Slipstream's three colours, chosen together: the focus ring anywhere round the colour wheel,
//! and bullet time's and the accent from it by colour theory.
//!
//! The wheel is OKLCH's, where hues the same distance apart look the same distance apart.
//! Slipstream's own ice cyan and amber already sit there as a split complement, 210° apart.
//!
//! A screen can't show every hue equally light and vivid: a red as light as ice cyan is only
//! salmon, and a deep blue can't be light at all. So each colour on the wheel is the most vivid
//! its hue can be within a band of lightness that still reads on Slipstream's dark surfaces.

/// The band of lightness the wheel's colours keep to, and the lightness preferred within it when
/// several are as vivid.
const DARKEST: f64 = 0.65;
const LIGHTEST: f64 = 0.85;
const LIGHTNESS: f64 = 0.80;
/// How vivid the wheel's colours get, at most.
const MOST_CHROMA: f64 = 0.20;
/// Below this a colour is grey enough that its hue says nothing, so its partners are as vivid as
/// the wheel's own.
const GREY: f64 = 0.04;

/// Three colours that go together, each `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub focus: &'static str,
    pub bullet: &'static str,
    pub accent: &'static str,
}

/// The sets offered. The first is Slipstream's own, which the settings start with; the rest are
/// split complements, triads and squares on the wheel, each with its three colours kept apart so
/// that the focus ring and bullet time never look alike.
pub const THEMES: [Theme; 10] = [
    // Ice cyan, with amber both for the slow world and for what's on.
    theme("Slipstream", "#42d3ff", "#ffb547", "#ffb547"),
    // Split complement of 223°: 73° and 13°.
    theme("Lagoon", "#46d3ff", "#f7b14d", "#fea4ad"),
    // Triad of 223°: 343° and 103°.
    theme("Neon", "#46d3ff", "#ff9bd9", "#d2c44c"),
    // Split complement of 50°: 260° and 200°.
    theme("Ember", "#feaa7a", "#9ec2fe", "#12dbe3"),
    // Split complement of 300°: 150° and 90°.
    theme("Orchid", "#ccb0ff", "#79da90", "#e4bc46"),
    // Rectangle on 25°: 205° and 85°.
    theme("Coral", "#fea69e", "#04daeb", "#eab946"),
    // Split complement of 95°: 305° and 245°.
    theme("Sunflower", "#ddbf47", "#d3adfe", "#87c8ff"),
    // Split complement of 355°: 205° and 145°.
    theme("Rose", "#ff9fc5", "#04daeb", "#84d987"),
    // Triad of 15°: 135° and 255°.
    theme("Ruby", "#ffa4aa", "#99d576", "#97c4fe"),
    // Square on 250°: 70° and 340°.
    theme("Glacier", "#90c6fe", "#fab050", "#fd9cdd"),
];

const fn theme(
    name: &'static str,
    focus: &'static str,
    bullet: &'static str,
    accent: &'static str,
) -> Theme {
    Theme {
        name,
        focus,
        bullet,
        accent,
    }
}

/// The colour at `hue` degrees round the wheel, each channel 0–1.
pub fn on_wheel(hue: f64) -> [f32; 3] {
    vivid(hue, MOST_CHROMA)
}

/// Where `rgb` sits on the wheel, in degrees.
pub fn hue(rgb: [f32; 3]) -> f64 {
    oklch(rgb).2
}

/// Bullet time's colour and the accent for a focus ring of `rgb`: the two either side of its
/// opposite, 30° away from it, no more vivid than it is. Bullet time takes the one 210° on,
/// which from ice cyan is Slipstream's amber.
pub fn partners(rgb: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (_, chroma, hue) = oklch(rgb);
    let chroma = if chroma < GREY { MOST_CHROMA } else { chroma };
    (vivid(hue + 210.0, chroma), vivid(hue + 150.0, chroma))
}

/// `rgb` as `#rrggbb`.
pub fn hex(rgb: [f32; 3]) -> String {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Lightness, chroma and hue in degrees.
fn oklch(rgb: [f32; 3]) -> (f64, f64, f64) {
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

/// The most vivid colour of `hue`, up to `chroma`, within the wheel's band of lightness, at the
/// preferred lightness where several are as vivid.
fn vivid(hue: f64, chroma: f64) -> [f32; 3] {
    let steps = ((LIGHTEST - DARKEST) / 0.005).round() as usize;
    let (lightness, chroma) = (0..=steps)
        .map(|step| {
            let lightness = DARKEST + step as f64 * 0.005;
            (lightness, most_chroma(lightness, hue, chroma))
        })
        .max_by(|(one, c1), (other, c2)| {
            let rounded = |c: f64| (c * 1000.0).round();
            rounded(*c1).total_cmp(&rounded(*c2)).then(
                (other - LIGHTNESS)
                    .abs()
                    .total_cmp(&(one - LIGHTNESS).abs()),
            )
        })
        .expect("a band of lightness");
    linear_rgb(lightness, chroma, hue).map(|c| encoded(c.clamp(0.0, 1.0)) as f32)
}

/// How vivid `hue` can be at `lightness`, up to `chroma`, and still be shown on a screen.
fn most_chroma(lightness: f64, hue: f64, chroma: f64) -> f64 {
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
fn linear_rgb(lightness: f64, chroma: f64, hue: f64) -> [f64; 3] {
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
        slipstream_config::rgb(hex).expect("a colour")
    }

    /// How far apart two hues are, either way round.
    fn apart(one: f64, other: f64) -> f64 {
        let d = (one - other).rem_euclid(360.0);
        d.min(360.0 - d)
    }

    #[test]
    fn the_first_theme_is_what_the_settings_start_with() {
        let first = THEMES[0];
        assert_eq!(first.focus, slipstream_config::SELECTED_TILE);
        assert_eq!(first.bullet, slipstream_config::BULLET_TIME);
        assert_eq!(first.accent, slipstream_config::ACCENT);
    }

    #[test]
    fn slipstreams_amber_is_its_cyans_split_complement() {
        let (bullet, _) = partners(rgb(slipstream_config::SELECTED_TILE));
        let amber = rgb(slipstream_config::BULLET_TIME);
        assert!(apart(hue(bullet), hue(amber)) < 3.0, "{}", hex(bullet));
        assert!(
            (oklch(bullet).0 - oklch(amber).0).abs() < 0.05,
            "{}",
            hex(bullet)
        );
    }

    #[test]
    fn partners_sit_either_side_of_the_opposite() {
        for step in 0..36 {
            let focus = on_wheel(step as f64 * 10.0);
            let (bullet, accent) = partners(focus);
            assert!((apart(hue(focus), hue(bullet)) - 150.0).abs() < 2.0);
            assert!((apart(hue(focus), hue(accent)) - 150.0).abs() < 2.0);
            assert!((apart(hue(bullet), hue(accent)) - 60.0).abs() < 2.0);
        }
    }

    #[test]
    fn a_grey_ring_still_gets_colours() {
        let (bullet, accent) = partners(rgb("#e8eef7"));
        assert!(oklch(bullet).1 > GREY);
        assert!(oklch(accent).1 > GREY);
    }

    #[test]
    fn every_theme_keeps_its_three_colours_apart() {
        for theme in THEMES.iter().skip(1) {
            let [focus, bullet, accent] = [theme.focus, theme.bullet, theme.accent].map(rgb);
            assert!(
                apart(hue(focus), hue(bullet)) >= 55.0,
                "{}: the focus ring and bullet time look alike",
                theme.name
            );
            assert!(apart(hue(focus), hue(accent)) >= 55.0, "{}", theme.name);
            assert!(apart(hue(bullet), hue(accent)) >= 55.0, "{}", theme.name);
        }
    }

    #[test]
    fn every_theme_colour_reads_on_the_dark() {
        for theme in THEMES {
            for hex in [theme.focus, theme.bullet, theme.accent] {
                let (lightness, ..) = oklch(rgb(hex));
                assert!(
                    (DARKEST - 0.01..=LIGHTEST + 0.01).contains(&lightness),
                    "{}: {hex}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn the_wheel_keeps_to_its_band_of_lightness() {
        for degree in (0..360).step_by(5) {
            let (lightness, ..) = oklch(on_wheel(degree as f64));
            assert!(
                (DARKEST - 0.01..=LIGHTEST + 0.01).contains(&lightness),
                "{degree}°"
            );
        }
    }

    #[test]
    fn the_wheel_has_a_real_red_and_a_deep_blue() {
        let [r, g, b] = on_wheel(29.0);
        assert!(r > 0.95 && g < 0.45 && b < 0.4, "{}", hex([r, g, b]));
        let [r, g, b] = on_wheel(265.0);
        assert!(b > 0.95 && r < 0.5 && g < 0.6, "{}", hex([r, g, b]));
    }

    #[test]
    fn a_colour_reads_back_where_it_was_put() {
        for step in 0..12 {
            let at = step as f64 * 30.0;
            assert!(apart(hue(on_wheel(at)), at) < 1.0, "{at}");
        }
        assert_eq!(hex(rgb("#42d3ff")), "#42d3ff");
    }
}
