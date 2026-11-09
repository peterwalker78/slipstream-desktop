//! The lit edge's passing light: a soft, wide band that drifts along the bar every so often,
//! brightening its top edge and the line under it a little where it passes, with a faint
//! spectral split at its edges, as light through the edge of a pane of glass has. It comes from
//! where the sun is: from the left in the morning, warm near sunrise; near neutral and gentlest
//! around midday; from the right in the evening, warm again near sunset; and at night a fainter,
//! cooler moonlight that passes more slowly.
//!
//! The band is painted once into a small picture of just those two lines, and moved and stretched
//! over the bar's own picture, which is never repainted for it.

use slipstream_config::sun::Daylight;
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::paint::Painted;

/// One pass along the bar and the rest before the next, in seconds, by day and by night.
const PASS: f64 = 14.0;
const REST: f64 = 8.0;
const NIGHT_PASS: f64 = 19.0;
const NIGHT_REST: f64 = 11.0;
/// How wide the band is, as a share of the bar.
const BAND: f64 = 0.3;
/// How much the band adds to the lines at its middle, as alpha out of 255: about 6% at full
/// strength.
const PEAK: f64 = 18.0;

// Natural light, so fixed colours rather than the colour roles: a low sun's amber, the near
// white of a high one, moonlight's blue, and the two ends of a spectrum split by a glass edge.
const LOW_SUN: u32 = 0xffc27a;
const HIGH_SUN: u32 = 0xfff6ea;
const MOONLIGHT: u32 = 0xbcd2ff;
const SPLIT_RED: u32 = 0xff7a66;
const SPLIT_BLUE: u32 = 0x6fa8ff;

/// The band's picture, its width in logical pixels before it is stretched.
const PICTURE_W: i32 = 240;

#[derive(Default)]
pub struct Light {
    /// The band as painted, and what for: its colour, the way it travels, the bar's height in
    /// pixels and the scale.
    painted: Option<((u32, bool, i32, u64), Painted)>,
}

impl Light {
    /// The band over a bar at `bar` (logical pixels from the screen's corner) at `now` seconds,
    /// lit by `light`, at `alpha`. `None` between passes.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        bar: Rectangle<f64, Logical>,
        scale: f64,
        now: f64,
        light: Daylight,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let along = pass(now, light.night)?;
        let tint = tint(light);
        let height = bar.size.h.round() as i32;
        let key = (tint, light.from_left, height, scale.to_bits());
        if self.painted.as_ref().is_none_or(|(held, _)| *held != key) {
            self.painted = Some((key, paint(tint, light.from_left, height, scale)?));
        }
        let (_, painted) = self.painted.as_ref()?;
        let band = (bar.size.w * BAND).round();
        let travel = bar.size.w + band;
        let left = if light.from_left {
            bar.loc.x - band + travel * along
        } else {
            bar.loc.x + bar.size.w - travel * along
        };
        painted.element_sized(
            renderer,
            Point::from((left, bar.loc.y)),
            Size::from((band as i32, height)),
            alpha * light.strength as f32,
        )
    }
}

/// How far along the bar a pass is at `now` seconds, 0 to 1, or `None` while it rests.
pub fn pass(now: f64, night: bool) -> Option<f64> {
    let (pass, rest) = if night {
        (NIGHT_PASS, NIGHT_REST)
    } else {
        (PASS, REST)
    };
    let t = now.max(0.0) % (pass + rest);
    (t < pass).then_some(t / pass)
}

/// The light's colour, 0xRRGGBB: from a high sun's near white to a low one's amber as it warms,
/// and moonlight's blue at night.
pub fn tint(light: Daylight) -> u32 {
    if light.night {
        return MOONLIGHT;
    }
    // Sixteen steps, so the picture is repainted a handful of times a day at most.
    let t = (light.warmth.clamp(0.0, 1.0) * 16.0).round() / 16.0;
    let mix = |shift: u32| {
        let (a, b) = ((HIGH_SUN >> shift) & 0xff, (LOW_SUN >> shift) & 0xff);
        ((a as f64 + (b as f64 - a as f64) * t).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// The band's picture for a bar `height` logical pixels high: its top edge and the line under it,
/// each a design pixel deep, lit across the middle and split into red and blue at its edges, the
/// red leading the way it travels.
fn paint(tint: u32, rightwards: bool, height: i32, scale: f64) -> Option<Painted> {
    let logical = Size::<i32, Logical>::from((PICTURE_W, height));
    Painted::new(logical, scale, |p| {
        // In the bar's own units: 40 to its height.
        let unit = height as f32 / 40.0;
        p.f *= unit;
        let (w, tall) = (PICTURE_W as f32 / unit, 40.0);
        let at = |rgb: u32, share: f64| (rgb << 8) | (PEAK * share).round() as u32;
        let (trailing, leading) = if rightwards {
            (SPLIT_BLUE, SPLIT_RED)
        } else {
            (SPLIT_RED, SPLIT_BLUE)
        };
        let stops = [
            (0.0, at(trailing, 0.0)),
            (0.14, at(trailing, 0.35)),
            (0.32, at(tint, 0.65)),
            (0.5, at(tint, 1.0)),
            (0.68, at(tint, 0.65)),
            (0.86, at(leading, 0.35)),
            (1.0, at(leading, 0.0)),
        ];
        p.fill_across(0.0, 0.0, w, 1.0, &stops);
        p.fill_across(0.0, tall - 1.0, w, 1.0, &stops);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_drifts_slowly_and_rests_between() {
        assert_eq!(pass(0.0, false), Some(0.0));
        assert_eq!(pass(7.0, false), Some(0.5));
        assert_eq!(pass(PASS + 1.0, false), None, "resting");
        assert_eq!(pass(PASS + REST, false), Some(0.0), "and round again");
        const { assert!(PASS >= 12.0 && PASS <= 16.0 && REST >= 5.0) };
        const { assert!(NIGHT_PASS > PASS, "moonlight passes more slowly") };
    }

    #[test]
    fn the_light_warms_towards_sunrise_and_sunset() {
        let day = |warmth| Daylight {
            from_left: true,
            warmth,
            strength: 1.0,
            night: false,
        };
        assert_eq!(tint(day(0.0)), HIGH_SUN);
        assert_eq!(tint(day(1.0)), LOW_SUN);
        let blue = |rgb: u32| rgb & 0xff;
        assert!(blue(tint(day(0.5))) < blue(HIGH_SUN));
        let night = Daylight {
            night: true,
            ..day(0.0)
        };
        assert_eq!(tint(night), MOONLIGHT);
    }
}
