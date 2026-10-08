//! The docked window's slot in the bar: a stream laid on its side.
//!
//! A minimised window's stream falls down the edge of the screen and says how hard its app is
//! working. A docked window has no stream there, so the bar carries one for it: the same light,
//! running along a short strip instead of down a card, with the app's name in it. Streaks
//! (`streaks.rs`) or code rain (`glmatrix.rs`), whichever the streams are drawn in, each redrawn
//! to suit a strip a few rows high: more of it, faster and greener, the busier the app.

use slipstream_config::Effects;
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Size},
};

use crate::{
    glmatrix::{Band, Glyphs, Look},
    paint::{self, Painter},
    streaks::Streaks,
    usage::Meter,
};

/// The slot's height in logical pixels, and how far below the top of the bar it sits: a little
/// smaller than the pills beside it, since it moves and they don't.
pub const HEIGHT: i32 = 18;
pub const TOP: i32 = 7;
/// How round its corners are, in logical pixels.
const RADIUS: f32 = 5.0;
/// The dark card the light runs over: the streams' own.
const CARD: u32 = 0x0b0d12ff;
/// Its edge, when the keyboard is elsewhere, and how strong it is in the ring's colour when the
/// keyboard is in the pane.
const EDGE: u32 = 0xffffff1c;
const EDGE_HELD: u32 = 0xa6;
/// How far inside the edge the light stops, in logical pixels.
const INSET: f64 = 1.0;
/// How much of each light the slot shows, of 255. It sits among the bar's small text, where a
/// row of glyphs at full brightness would shout; threads a pixel fine carry far less light, and
/// are shown nearly whole.
const GAIN_RAIN: u32 = 150;
const GAIN_STREAKS: u32 = 230;
/// How much of the strip's height a glyph of code rain takes.
const GLYPH: f32 = 0.8;
/// The load is shown in this many steps, so a reading that holds still repaints nothing.
const STEPS: f32 = 40.0;
/// How far behind the front of light running into the strip its glow reaches, and how far the
/// dark that follows the light out takes to fall, in logical pixels.
const FRONT: f32 = 14.0;
/// How bright that front is, of 255, and how far ahead of itself it glows, as a share of the
/// reach behind it.
const FRONT_GLOW: f32 = 120.0;
const AHEAD: f32 = 0.3;
/// The edge's colour when the landing's flash is on it.
const EDGE_FLASH: u32 = 0xffffffe6;

/// What the painted strip shows, beyond the light itself.
#[derive(Clone, Copy, PartialEq)]
struct Shown {
    device: (i32, i32),
    scale: f64,
    keyboard: bool,
    ring: [f32; 3],
    effects: Effects,
    /// The stretch the light is in, in the strip's own pixels, and the flash on the edge in
    /// sixteenths.
    lit: (u16, u16),
    flash: u8,
}

/// The light in the strip: the streams' streaks, or the code rain when that is what the streams
/// are drawn in.
enum Light {
    Streaks(Streaks),
    Rain(Band),
}

pub struct Slot {
    name: String,
    /// How hard the docked app is working, 0 to 1.
    meter: Meter,
    light: Option<Light>,
    stepped_to: f64,
    seed: u64,
    look_load: Option<u16>,
    painted: Option<(Shown, paint::Painted)>,
}

impl Slot {
    /// A slot for an app called `name` whose process is `pid`, from `now` on the animation clock.
    pub fn new(name: String, pid: Option<u32>, now: f64) -> Self {
        Self {
            name,
            meter: Meter::new(pid),
            light: None,
            stepped_to: now,
            seed: now.to_bits() | 1,
            look_load: None,
            painted: None,
        }
    }

    /// The strip `width` logical pixels wide with its corner at `at`, lit for the app's load now
    /// (or `demand`, when a load is being shown for every app), in the light the streams are
    /// drawn in. Its edge is in `ring`'s colour while the `keyboard` is in the docked pane.
    /// The light is shown between `lit`'s two shares of the strip's length, and the edge is
    /// lit by `flash` of the landing's white.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        at: Point<f64, Logical>,
        width: i32,
        scale: f64,
        now: f64,
        alpha: f32,
        keyboard: bool,
        ring: [f32; 3],
        reduced_motion: bool,
        demand: Option<f32>,
        (effects, glyphs): (Effects, Option<&mut Glyphs>),
        (lit, flash): ((f32, f32), f32),
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.meter.refresh();
        let load = demand.unwrap_or(self.meter.load).clamp(0.0, 1.0);
        let step = (load * STEPS).round() as u16;
        let logical = Size::<i32, Logical>::from((width.max(1), HEIGHT));
        let device = (
            (logical.w as f64 * scale).round().max(1.0) as i32,
            (logical.h as f64 * scale).round().max(1.0) as i32,
        );
        let inset = (INSET * scale).floor().max(1.0) as usize;
        let (iw, ih) = (
            (device.0 as usize).saturating_sub(2 * inset).max(1),
            (device.1 as usize).saturating_sub(2 * inset).max(1),
        );
        let along = |share: f32| (share.clamp(0.0, 1.0) * iw as f32).round() as u16;
        let shown = Shown {
            device,
            scale,
            keyboard,
            ring,
            effects,
            lit: (along(lit.0), along(lit.1)),
            flash: (flash.clamp(0.0, 1.0) * 16.0).round() as u8,
        };
        let stale = self
            .painted
            .as_ref()
            .is_none_or(|(painted, _)| *painted != shown);
        // The light starts afresh in the other kind when the setting changes, or at a new size.
        let fits = match (&self.light, effects) {
            (Some(Light::Streaks(_)), Effects::Slipstream) => true,
            (Some(Light::Rain(_)), Effects::Matrix) => self
                .painted
                .as_ref()
                .is_some_and(|(painted, _)| painted.device == device),
            _ => false,
        };
        if !fits {
            self.light = None;
            self.look_load = None;
        }
        let seconds = now - self.stepped_to;
        self.stepped_to = now;
        let (name, seed, look_load) = (&self.name, self.seed, &mut self.look_load);
        let light = self.light.get_or_insert_with(|| match effects {
            Effects::Slipstream => Light::Streaks(Streaks::along(name, load, iw, ih, scale, seed)),
            Effects::Matrix => {
                let (wide, tall) = Band::size_along(iw as f32, ih as f32 * GLYPH);
                Light::Rain(Band::new(
                    &name.to_uppercase(),
                    Look::for_demand(load),
                    wide,
                    tall,
                    seed,
                ))
            }
        });
        let moved = *look_load != Some(step);
        let changed = match light {
            Light::Streaks(streaks) => {
                // A card on its side: as wide as the strip is high.
                streaks.resize(ih, iw, scale);
                let recoloured = streaks.set_name_colour(ring);
                let changed = if reduced_motion {
                    let changed = moved || !streaks.is_still();
                    if changed {
                        streaks.hold(load);
                    }
                    changed
                } else {
                    let was_still = streaks.is_still();
                    if moved {
                        streaks.set_load(load);
                    }
                    streaks.step(seconds) || was_still
                };
                changed || recoloured
            }
            Light::Rain(band) => {
                let recoloured = band.set_name_colour(ring);
                let changed = if reduced_motion {
                    let changed = moved || !band.is_still();
                    if changed {
                        band.hold(load);
                    }
                    changed
                } else {
                    let was_still = band.is_still();
                    if moved {
                        band.set_look(Look::for_demand(load));
                    }
                    band.step(seconds) || was_still
                };
                changed || recoloured
            }
        };
        *look_load = Some(step);
        if changed || stale {
            // The strip's light, upright: `iw` across and `ih` down.
            let mut lit = vec![0u8; iw * ih * 4];
            let gain = match light {
                Light::Streaks(streaks) => {
                    streaks.draw_along(&mut lit, iw, ih);
                    GAIN_STREAKS
                }
                Light::Rain(band) => {
                    if let Some(glyphs) = glyphs {
                        band.draw_along(&mut lit, iw, ih, glyphs);
                    }
                    GAIN_RAIN
                }
            };
            let reach = (FRONT * scale as f32).max(1.0);
            keep_between(&mut lit, iw, shown.lit, reach);
            let edge = edge_colour(keyboard, ring, shown.flash as f32 / 16.0);
            let pixmap = paint_strip(&lit, gain, device, inset, (iw, ih), scale, edge)?;
            self.painted = Some((
                shown,
                paint::Painted {
                    buffer: paint::buffer(&pixmap),
                    logical,
                    device,
                    scale,
                },
            ));
        }
        self.painted.as_ref()?.1.element(renderer, at, alpha)
    }
}

/// Leaves the strip's light, `w` pixels long, only between `from` and `to` along it. Where the
/// light is still running in, its front glows for `reach` pixels behind it; where it is running
/// out, the dark follows it over the same distance.
fn keep_between(light: &mut [u8], w: usize, (from, to): (u16, u16), reach: f32) {
    let (from, to) = (from as f32, to as f32);
    let whole = from <= 0.0 && to >= w as f32;
    if whole || w == 0 {
        return;
    }
    for row in light.chunks_exact_mut(w * 4) {
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let at = x as f32 + 0.5;
            if to <= from {
                pixel.fill(0);
                continue;
            }
            if at > to {
                // Just ahead of the front its glow falls off, so it has no hard edge.
                let ahead = (1.0 - (at - to) / (reach * AHEAD)).clamp(0.0, 1.0);
                let glow = if to < w as f32 {
                    ahead * ahead * FRONT_GLOW
                } else {
                    0.0
                };
                pixel.fill(glow as u8);
                continue;
            }
            let kept = if from > 0.0 {
                ((at - from) / reach).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let glow = if to < w as f32 {
                let behind = (to - at) / reach;
                (1.0 - behind).clamp(0.0, 1.0).powi(2) * FRONT_GLOW
            } else {
                0.0
            };
            for channel in &mut pixel[..3] {
                *channel = (*channel as f32 * kept + glow).min(255.0) as u8;
            }
            pixel[3] = (pixel[3] as f32 * kept).max(glow) as u8;
        }
    }
}

/// The edge's colour: the ring's while the `keyboard` is in the pane, a faint white otherwise,
/// and either way `flash` of the way to the white of a pane landing.
fn edge_colour(keyboard: bool, ring: [f32; 3], flash: f32) -> u32 {
    let rest = if keyboard {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        (channel(ring[0]) << 24) | (channel(ring[1]) << 16) | (channel(ring[2]) << 8) | EDGE_HELD
    } else {
        EDGE
    };
    let flash = flash.clamp(0.0, 1.0);
    let mix = |shift: u32| {
        let (a, b) = ((rest >> shift) & 0xff, (EDGE_FLASH >> shift) & 0xff);
        ((a as f32 + (b as f32 - a as f32) * flash).round() as u32) << shift
    };
    mix(24) | mix(16) | mix(8) | mix(0)
}

/// The strip's card `device` screen pixels big with `light`, `inner.0`×`inner.1`, added inside
/// its edge at `gain` of 255, softened along the corners, with its edge in `edge`.
fn paint_strip(
    light: &[u8],
    gain: u32,
    device: (i32, i32),
    inset: usize,
    inner: (usize, usize),
    scale: f64,
    edge: u32,
) -> Option<resvg::tiny_skia::Pixmap> {
    let (w, h) = (device.0.max(1) as u32, device.1.max(1) as u32);
    let radius = RADIUS * scale as f32;
    let (iw, ih) = inner;
    let mut p = Painter::new(w, h, 1.0)?;
    p.fill(0.0, 0.0, w as f32, h as f32, radius, CARD);
    // The shape inside the edge that the light may fall on.
    let mut shape = Painter::new(iw as u32, ih as u32, 1.0)?;
    shape.fill(
        0.0,
        0.0,
        iw as f32,
        ih as f32,
        (radius - inset as f32).max(0.0),
        0xffffffff,
    );
    let cover = shape.pixmap.data();
    let data = p.pixmap.data_mut();
    for (i, light) in light.chunks_exact(4).enumerate() {
        let cover = cover[i * 4 + 3] as u32 * gain / 255;
        if cover == 0 || light[..3] == [0, 0, 0] {
            continue;
        }
        let at = ((i / iw + inset) * w as usize + i % iw + inset) * 4;
        for (channel, &light) in data[at..at + 3].iter_mut().zip(&light[..3]) {
            let added = *channel as u32 + (light as u32 * cover + 127) / 255;
            *channel = added.min(255) as u8;
        }
    }
    let line = (scale.round() as f32).max(1.0);
    p.border(0.0, 0.0, w as f32, h as f32, radius, line, edge);
    Some(p.pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_is_shown_quieter_than_a_streams_and_only_inside_the_edge() {
        let mut light = vec![0u8; 6 * 4 * 4];
        // Every pixel fully lit in red.
        for pixel in light.chunks_exact_mut(4) {
            pixel[0] = 255;
        }
        let strip = paint_strip(&light, GAIN_RAIN, (8, 6), 1, (6, 4), 1.0, EDGE).unwrap();
        let dark = paint_strip(&[0u8; 96], GAIN_RAIN, (8, 6), 1, (6, 4), 1.0, EDGE).unwrap();
        let red = |p: &resvg::tiny_skia::Pixmap, x, y| p.pixel(x, y).unwrap().red();
        let middle = red(&strip, 4, 3);
        assert!(middle > red(&dark, 4, 3) + 100 && middle < 200, "{middle}");
        // The outermost row is the edge's, and takes none of it.
        assert_eq!(red(&strip, 4, 0), red(&dark, 4, 0));
    }

    #[test]
    fn the_edge_is_in_the_rings_colour_only_while_the_keyboard_is_in_the_pane() {
        let dark = [0u8; 4 * 6 * 4];
        let ring = [0.2, 0.8, 1.0];
        let paint = |keyboard: bool| {
            let edge = edge_colour(keyboard, ring, 0.0);
            paint_strip(&dark, GAIN_RAIN, (8, 6), 1, (6, 4), 1.0, edge).unwrap()
        };
        let (quiet, held) = (paint(false), paint(true));
        let edge = |p: &resvg::tiny_skia::Pixmap| p.pixel(4, 0).unwrap();
        assert_ne!(edge(&quiet), edge(&held));
        assert!(edge(&held).blue() > edge(&held).red());
        // Inside the edge nothing differs.
        assert_eq!(quiet.pixel(4, 3), held.pixel(4, 3));
    }

    /// A strip `w` long and two rows high, every pixel lit green.
    fn lit_strip(w: usize) -> Vec<u8> {
        let mut light = vec![0u8; w * 2 * 4];
        for pixel in light.chunks_exact_mut(4) {
            pixel[1] = 200;
            pixel[3] = 200;
        }
        light
    }

    #[test]
    fn light_running_in_stops_at_its_front_and_glows_there() {
        let mut light = lit_strip(40);
        keep_between(&mut light, 40, (0, 20), 6.0);
        let green = |x: usize| light[x * 4 + 1];
        let red = |x: usize| light[x * 4];
        // Untouched well behind the front, white-hot at it, and nothing past it.
        assert_eq!((red(2), green(2)), (0, 200));
        assert!(red(19) > 80 && green(19) > 200, "{} {}", red(19), green(19));
        // A little of the glow just ahead of it too, so it has no hard edge.
        assert!(red(20) > 0 && red(20) < red(19));
        assert_eq!(&light[25 * 4..26 * 4], &[0, 0, 0, 0]);
        // The second row is the same.
        assert_eq!(light[(40 + 19) * 4], red(19));
    }

    #[test]
    fn light_running_out_leaves_the_dark_behind_it() {
        let mut light = lit_strip(40);
        keep_between(&mut light, 40, (20, 40), 6.0);
        let green = |x: usize| light[x * 4 + 1];
        assert_eq!(green(5), 0);
        assert!(green(22) > 0 && green(22) < 200, "{}", green(22));
        assert_eq!(green(35), 200);
        // No front glows on light that is leaving.
        assert_eq!(light[35 * 4], 0);
    }

    #[test]
    fn a_whole_strip_and_an_empty_one() {
        let mut whole = lit_strip(40);
        keep_between(&mut whole, 40, (0, 40), 6.0);
        assert_eq!(whole, lit_strip(40));
        let mut none = lit_strip(40);
        keep_between(&mut none, 40, (0, 0), 6.0);
        assert!(none.iter().all(|&value| value == 0));
    }

    #[test]
    fn the_landing_flash_whitens_the_edge_and_leaves_it_as_it_was() {
        let ring = [0.2, 0.8, 1.0];
        assert_eq!(edge_colour(false, ring, 0.0), EDGE);
        assert_eq!(edge_colour(false, ring, 1.0), EDGE_FLASH);
        assert_eq!(edge_colour(true, ring, 1.0), EDGE_FLASH);
        let part = edge_colour(false, ring, 0.5);
        assert!((part & 0xff) > (EDGE & 0xff) && (part & 0xff) < (EDGE_FLASH & 0xff));
    }
}
