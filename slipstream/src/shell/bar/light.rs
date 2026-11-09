//! The lit edge's moving light: two soft bands of light drifting slowly along the bar, one after
//! the other at different speeds, so there is nearly always some light crossing it. Each
//! brightens the bar's top edge and the line under it where it passes, washes the bar between
//! them very faintly, and splits into a little red and blue at its edges, as light through the
//! edge of a pane of glass does.
//!
//! Each band is painted once into a small picture and moved and stretched over the bar's own
//! picture, which is never repainted for it. It moves in steps of a thirtieth of a second, so
//! between steps a frame has nothing new and the screen isn't redrawn for it.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::paint::Painted;

/// How often the light moves, a second.
pub const STEPS: f64 = 30.0;

/// One band: how long it takes from coming in at the left to leaving at the right, in seconds,
/// how wide it is as a share of the bar, how bright as a share of `PEAK`, and where it starts.
struct Band {
    cross: f64,
    wide: f64,
    strength: f64,
    start: f64,
}

const BANDS: [Band; 2] = [
    Band {
        cross: 11.0,
        wide: 0.3,
        strength: 1.0,
        start: 0.0,
    },
    Band {
        cross: 14.5,
        wide: 0.22,
        strength: 0.7,
        start: 0.55,
    },
];

/// How much a band adds to the lines at its middle, as alpha out of 255: about 15%.
const PEAK: f64 = 38.0;
/// How much it adds to the bar between them, out of 255: about 2%.
const WASH: f64 = 5.0;
/// The glow either line casts into the bar, a design pixel at a time, as shares of `PEAK`, so
/// the light reads as more than a hairline.
const GLOW: [f64; 5] = [0.45, 0.28, 0.16, 0.08, 0.04];

// Light, so fixed colours rather than the colour roles: neutral white, and the two ends of a
// spectrum split by a glass edge.
const WHITE: u32 = 0xffffff;
const SPLIT_RED: u32 = 0xff7a66;
const SPLIT_BLUE: u32 = 0x6fa8ff;

/// A band's picture, its width in logical pixels before it is stretched.
const PICTURE_W: i32 = 120;

#[derive(Default)]
pub struct Light {
    /// One picture for each band, so each is its own element, and the bar's height in pixels and
    /// the scale they were painted for.
    painted: Option<((i32, u64), [Painted; 2])>,
}

impl Light {
    /// The bands over a bar at `bar` (logical pixels from the screen's corner) at `now` seconds,
    /// at `alpha`.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        bar: Rectangle<f64, Logical>,
        scale: f64,
        now: f64,
        alpha: f32,
    ) -> [Option<MemoryRenderBufferRenderElement<R>>; 2]
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let height = bar.size.h.round() as i32;
        let key = (height, scale.to_bits());
        if self.painted.as_ref().is_none_or(|(held, _)| *held != key) {
            let pictures = [paint(height, scale), paint(height, scale)];
            let [Some(a), Some(b)] = pictures else {
                return [None, None];
            };
            self.painted = Some((key, [a, b]));
        }
        let Some((_, pictures)) = self.painted.as_ref() else {
            return [None, None];
        };
        let now = (now.max(0.0) * STEPS).floor() / STEPS;
        let strengths = strengths(now);
        std::array::from_fn(|n| {
            let band = &BANDS[n];
            let wide = (bar.size.w * band.wide).round();
            let left = bar.loc.x - wide + (bar.size.w + wide) * along(band, now);
            pictures[n].element_sized(
                renderer,
                Point::from((left, bar.loc.y)),
                Size::from((wide as i32, height)),
                alpha * strengths[n] as f32,
            )
        })
    }
}

/// How far along its way `band` is at `now` seconds, 0 (just in at the left) to 1 (just out at
/// the right), and round again with no rest.
fn along(band: &Band, now: f64) -> f64 {
    (now / band.cross + band.start).rem_euclid(1.0)
}

/// Where a band's middle is at `now`, as a share of the bar from its left end.
fn middle(band: &Band, now: f64) -> f64 {
    -band.wide / 2.0 + (1.0 + band.wide) * along(band, now)
}

/// How much of `band` is over the bar at `now`, 0 to 1.
fn shown(band: &Band, now: f64) -> f64 {
    let middle = middle(band, now);
    let (left, right) = (middle - band.wide / 2.0, middle + band.wide / 2.0);
    ((right.min(1.0) - left.max(0.0)) / band.wide).clamp(0.0, 1.0)
}

/// How bright each band is at `now`. The fainter one gives way as the brighter one overtakes it,
/// so where they meet the light is no brighter than one band. It gives way only as far as the
/// brighter one is over the bar, so nothing jumps when that one goes round again.
fn strengths(now: f64) -> [f64; 2] {
    let [a, b] = &BANDS;
    let apart = (middle(a, now) - middle(b, now)).abs() / ((a.wide + b.wide) / 2.0);
    let near = (1.0 - apart).clamp(0.0, 1.0);
    let give_way = near * near * (3.0 - 2.0 * near) * shown(a, now);
    [a.strength, b.strength * (1.0 - give_way)]
}

/// A band's picture for a bar `height` logical pixels high: its top edge and the line under it,
/// each a design pixel deep with a soft glow into the bar, lit across the middle and split into
/// red and blue at its edges, the red leading, and the bar between them washed very faintly.
fn paint(height: i32, scale: f64) -> Option<Painted> {
    let logical = Size::<i32, Logical>::from((PICTURE_W, height));
    Painted::new(logical, scale, |p| {
        // In the bar's own units: 40 to its height.
        let unit = height as f32 / 40.0;
        p.f *= unit;
        let (w, tall) = (PICTURE_W as f32 / unit, 40.0);
        let stops = |peak: f64| {
            let at = |rgb: u32, share: f64| (rgb << 8) | (peak * share).round() as u32;
            [
                (0.0, at(SPLIT_BLUE, 0.0)),
                (0.14, at(SPLIT_BLUE, 0.35)),
                (0.32, at(WHITE, 0.65)),
                (0.5, at(WHITE, 1.0)),
                (0.68, at(WHITE, 0.65)),
                (0.86, at(SPLIT_RED, 0.35)),
                (1.0, at(SPLIT_RED, 0.0)),
            ]
        };
        p.fill_across(0.0, 1.0, w, tall - 2.0, &stops(WASH));
        p.fill_across(0.0, 0.0, w, 1.0, &stops(PEAK));
        p.fill_across(0.0, tall - 1.0, w, 1.0, &stops(PEAK));
        for (n, share) in GLOW.iter().enumerate() {
            let glow = stops(PEAK * share);
            let n = n as f32 + 1.0;
            p.fill_across(0.0, n, w, 1.0, &glow);
            p.fill_across(0.0, tall - 1.0 - n, w, 1.0, &glow);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_drifts_slowly_and_never_rests() {
        for band in &BANDS {
            assert!((8.0..=15.0).contains(&band.cross), "slow, never fast");
            // Round again the moment it leaves: no wait between crossings.
            let leaving = (1.0 - band.start) * band.cross - 0.001;
            assert!(along(band, leaving) > 0.99);
            assert!(along(band, leaving + 0.002) < 0.01);
        }
        assert_ne!(
            BANDS[0].cross, BANDS[1].cross,
            "two speeds, so they drift apart"
        );
        // Nearly always, a band is well over the bar.
        let moments = 6000;
        let lit = (0..moments)
            .filter(|tenth| {
                let now = *tenth as f64 / 10.0;
                BANDS.iter().any(|band| shown(band, now) > 0.5)
            })
            .count();
        assert!(lit * 10 >= moments * 9, "lit {lit} of {moments}");
    }

    #[test]
    fn the_light_is_calm_where_the_bands_meet() {
        // Peaks of about 14% on the edges, and at most a faint wash over the bar.
        const { assert!(PEAK >= 30.0 && PEAK <= 40.0 && WASH <= 8.0) };
        for tenth in 0..3000 {
            let now = tenth as f64 / 10.0;
            let [a, b] = strengths(now);
            let apart = (middle(&BANDS[0], now) - middle(&BANDS[1], now)).abs();
            if apart < 0.01 && shown(&BANDS[0], now) > 0.99 {
                assert!(b < 0.01, "the fainter band gives way at {now}");
            }
            assert!(a + b <= 1.7);
            // And it gives way smoothly: what shows of it never jumps.
            let [_, next] = strengths(now + 0.1);
            let (seen, next_seen) = (
                b * shown(&BANDS[1], now),
                next * shown(&BANDS[1], now + 0.1),
            );
            assert!((next_seen - seen).abs() < 0.1, "a jump at {now}");
        }
    }
}
