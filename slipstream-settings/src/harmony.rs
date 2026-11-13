//! Slipstream's three colours, chosen together: the focus ring anywhere round the colour wheel,
//! bullet time's from across the wheel, and the accent a deep shade of the focus ring's own hue,
//! so that what's on reads as part of the same family rather than as a third bright colour.
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
/// The accent's lightness: deep enough to sit under bright text and beside the bright ring
/// without competing with it.
const DEEP: f64 = 0.44;
/// How vivid the accent is, as a share of the focus ring's vividness.
const DEEP_SHARE: f64 = 0.63;
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

/// The sets offered. The first is Slipstream's own, which the settings start with. Each follows
/// the rule `partners` keeps: bullet time's colour 210° on from the focus ring and as bright, and
/// the accent a deep shade of the focus ring's own hue.
pub const THEMES: [Theme; 10] = [
    // Ice cyan, amber for the slow world, and deep teal for what's on.
    theme("Slipstream", "#42d3ff", "#ffb547", "#005a75"),
    theme("Lagoon", "#00efc8", "#fe7c75", "#006150"),
    theme("Neon", "#81d943", "#fe6ec7", "#306104"),
    theme("Ember", "#feaa7a", "#87b3fd", "#73462b"),
    theme("Orchid", "#ccb0ff", "#87d297", "#594974"),
    theme("Coral", "#fea69e", "#76c9f8", "#73443f"),
    theme("Sunflower", "#ddbf47", "#cc9fff", "#625105"),
    theme("Rose", "#ff9fc5", "#40d4e3", "#734055"),
    theme("Ruby", "#ffa4aa", "#68ccf2", "#734246"),
    theme("Glacier", "#90c6fe", "#ccc074", "#375573"),
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

/// Bullet time's colour and the accent for a focus ring of `rgb`. Bullet time takes the hue
/// 210° on, as vivid, which from ice cyan is Slipstream's amber; the accent is the ring's own hue,
/// deep and calmer, which from ice cyan is Slipstream's deep teal.
pub fn partners(rgb: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (_, chroma, hue) = oklch(rgb);
    let chroma = if chroma < GREY { MOST_CHROMA } else { chroma };
    (vivid(hue + 210.0, chroma), deep(hue, chroma * DEEP_SHARE))
}

/// `hue` at the accent's deep lightness, as vivid as it can be up to `chroma`.
fn deep(hue: f64, chroma: f64) -> [f32; 3] {
    let chroma = most_chroma(DEEP, hue, chroma);
    linear_rgb(DEEP, chroma, hue).map(|c| encoded(c.clamp(0.0, 1.0)) as f32)
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
    fn bullet_time_sits_across_the_wheel_and_the_accent_is_the_rings_own_hue_deep() {
        for step in 0..36 {
            let focus = on_wheel(step as f64 * 10.0);
            let (bullet, accent) = partners(focus);
            assert!((apart(hue(focus), hue(bullet)) - 150.0).abs() < 2.0);
            assert!(apart(hue(focus), hue(accent)) < 6.0, "{}", hex(accent));
            assert!((oklch(accent).0 - DEEP).abs() < 0.02, "{}", hex(accent));
            assert!(oklch(focus).0 - oklch(accent).0 > 0.15, "{}", hex(accent));
        }
    }

    #[test]
    fn slipstreams_deep_teal_is_its_cyans_accent() {
        let (_, accent) = partners(rgb(slipstream_config::SELECTED_TILE));
        let teal = rgb(slipstream_config::ACCENT);
        assert!(apart(hue(accent), hue(teal)) < 6.0, "{}", hex(accent));
        assert!(
            (oklch(accent).0 - oklch(teal).0).abs() < 0.03,
            "{}",
            hex(accent)
        );
    }

    #[test]
    fn a_grey_ring_still_gets_colours() {
        let (bullet, accent) = partners(rgb("#e8eef7"));
        assert!(oklch(bullet).1 > GREY);
        assert!(oklch(accent).1 > GREY);
    }

    #[test]
    fn every_theme_follows_the_rule() {
        for theme in THEMES {
            let [focus, bullet, accent] = [theme.focus, theme.bullet, theme.accent].map(rgb);
            assert!(
                apart(hue(focus), hue(bullet)) >= 120.0,
                "{}: the focus ring and bullet time look alike",
                theme.name
            );
            assert!(apart(hue(focus), hue(accent)) < 8.0, "{}", theme.name);
            for bright in [focus, bullet] {
                let (lightness, ..) = oklch(bright);
                assert!(
                    (DARKEST - 0.01..=LIGHTEST + 0.01).contains(&lightness),
                    "{}",
                    theme.name
                );
            }
            assert!((oklch(accent).0 - DEEP).abs() < 0.03, "{}", theme.name);
        }
    }

    #[test]
    fn no_two_themes_share_a_focus_ring() {
        for (i, one) in THEMES.iter().enumerate() {
            for other in &THEMES[i + 1..] {
                assert!(
                    apart(hue(rgb(one.focus)), hue(rgb(other.focus))) >= 9.0,
                    "{} and {}",
                    one.name,
                    other.name
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
