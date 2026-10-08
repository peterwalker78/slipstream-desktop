//! The docked window's slot in the bar: a stream laid on its side.
//!
//! A minimised window's stream falls down the edge of the screen and says how hard its app is
//! working. A docked window has no stream there, so the bar carries one for it: the same light,
//! running along a short strip instead of down a card, with the app's name in it. Streaks
//! (`streaks.rs`) or code rain (`glmatrix.rs`), whichever the streams are drawn in: more of it,
//! faster and greener, the busier the app.

use slipstream_config::Effects;
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Size},
};

use crate::{
    glmatrix::{Band, Glyphs, Look},
    paint::{self, Painter},
    streaks::Streaks,
    text::Face,
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
/// How much of a stream's light the slot shows. It sits among the bar's small text, where a
/// stream's full brightness would shout.
const GAIN: u32 = 150;
/// The name in the bar's own small capitals, as its pills are lettered: the face, its size in
/// logical pixels and its letter spacing.
const NAME: (Face, f32, f32) = (Face::MonoBold, 9.6, 0.06);
/// How bright the name is with no light passing over it: brighter than down a stream, which has
/// its app's icon above it to say whose it is.
const NAME_REST: f32 = 0.5;
/// How much of the strip's height a glyph of code rain takes.
const GLYPH: f32 = 0.8;
/// The load is shown in this many steps, so a reading that holds still repaints nothing.
const STEPS: f32 = 40.0;

/// What the painted strip shows, beyond the light itself.
#[derive(Clone, Copy, PartialEq)]
struct Shown {
    device: (i32, i32),
    scale: f64,
    keyboard: bool,
    ring: [f32; 3],
    effects: Effects,
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
        let shown = Shown {
            device,
            scale,
            keyboard,
            ring,
            effects,
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
            // Streaks fall down a card as tall as the strip is long and as wide as it is high,
            // which is then laid on its side.
            Effects::Slipstream => Light::Streaks(Streaks::new(name, load, ih, iw, scale, seed)),
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
                streaks.resize(ih, iw, scale);
                streaks.set_name_style(NAME.0, NAME.1, NAME.2);
                streaks.set_name_rest(NAME_REST);
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
            match light {
                Light::Streaks(streaks) => {
                    let mut fallen = vec![0u8; ih * iw * 4];
                    streaks.draw(&mut fallen, ih, iw);
                    lay_on_its_side(&fallen, &mut lit, iw, ih);
                }
                Light::Rain(band) => {
                    if let Some(glyphs) = glyphs {
                        band.draw_along(&mut lit, iw, ih, glyphs);
                    }
                }
            }
            let pixmap = paint_strip(&lit, device, inset, (iw, ih), scale, keyboard, ring)?;
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

/// Turns a falling card's light a quarter anticlockwise into a strip `iw`×`ih`: the card, `ih`
/// wide and `iw` tall, has its top at the strip's left end and its right-hand side along the
/// strip's top. What fell now runs left to right, and the name down the card's spine reads along
/// the strip.
fn lay_on_its_side(fallen: &[u8], strip: &mut [u8], iw: usize, ih: usize) {
    for sy in 0..ih {
        for sx in 0..iw {
            let from = (sx * ih + (ih - 1 - sy)) * 4;
            let to = (sy * iw + sx) * 4;
            strip[to..to + 4].copy_from_slice(&fallen[from..from + 4]);
        }
    }
}

/// The strip's card `device` screen pixels big with `light`, `inner.0`×`inner.1`, added inside
/// its edge at `GAIN`, softened along the corners.
fn paint_strip(
    light: &[u8],
    device: (i32, i32),
    inset: usize,
    inner: (usize, usize),
    scale: f64,
    keyboard: bool,
    ring: [f32; 3],
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
        let cover = cover[i * 4 + 3] as u32 * GAIN / 255;
        if cover == 0 || light[..3] == [0, 0, 0] {
            continue;
        }
        let at = ((i / iw + inset) * w as usize + i % iw + inset) * 4;
        for (channel, &light) in data[at..at + 3].iter_mut().zip(&light[..3]) {
            let added = *channel as u32 + (light as u32 * cover + 127) / 255;
            *channel = added.min(255) as u8;
        }
    }
    let edge = if keyboard {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        (channel(ring[0]) << 24) | (channel(ring[1]) << 16) | (channel(ring[2]) << 8) | EDGE_HELD
    } else {
        EDGE
    };
    let line = (scale.round() as f32).max(1.0);
    p.border(0.0, 0.0, w as f32, h as f32, radius, line, edge);
    Some(p.pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A falling card 4 wide and 6 tall with one lit pixel, as RGBA.
    fn card_with(x: usize, y: usize) -> Vec<u8> {
        let mut light = vec![0u8; 4 * 6 * 4];
        light[(y * 4 + x) * 4] = 200;
        light
    }

    /// Where the strip, 6 long and 4 high, is lit.
    fn lit(strip: &[u8]) -> Vec<(usize, usize)> {
        strip
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[0] > 0)
            .map(|(i, _)| (i % 6, i / 6))
            .collect()
    }

    #[test]
    fn what_falls_down_the_card_runs_left_to_right_along_the_strip() {
        let strip = |x, y| {
            let mut strip = vec![0u8; 6 * 4 * 4];
            lay_on_its_side(&card_with(x, y), &mut strip, 6, 4);
            lit(&strip)
        };
        // Further down the card is further along the strip, in the same row.
        assert_eq!(strip(1, 1), vec![(1, 2)]);
        assert_eq!(strip(1, 4), vec![(4, 2)]);
        // The card's right-hand side is the strip's top.
        assert_eq!(strip(2, 1), vec![(1, 1)]);
    }

    #[test]
    fn the_light_is_shown_quieter_than_a_streams_and_only_inside_the_edge() {
        let mut light = vec![0u8; 6 * 4 * 4];
        // Every pixel fully lit in red.
        for pixel in light.chunks_exact_mut(4) {
            pixel[0] = 255;
        }
        let strip = paint_strip(&light, (8, 6), 1, (6, 4), 1.0, false, [0.0; 3]).unwrap();
        let dark = paint_strip(&[0u8; 96], (8, 6), 1, (6, 4), 1.0, false, [0.0; 3]).unwrap();
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
        let quiet = paint_strip(&dark, (8, 6), 1, (6, 4), 1.0, false, ring).unwrap();
        let held = paint_strip(&dark, (8, 6), 1, (6, 4), 1.0, true, ring).unwrap();
        let edge = |p: &resvg::tiny_skia::Pixmap| p.pixel(4, 0).unwrap();
        assert_ne!(edge(&quiet), edge(&held));
        assert!(edge(&held).blue() > edge(&held).red());
        // Inside the edge nothing differs.
        assert_eq!(quiet.pixel(4, 3), held.pixel(4, 3));
    }
}
