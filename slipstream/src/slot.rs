//! The docked window's slot in the bar: a stream laid on its side.
//!
//! A minimised window's stream falls down the edge of the screen and says how hard its app is
//! working. A docked window has no stream there, so the bar carries one for it: the same light
//! (`streaks.rs`), running along a short strip instead of down a card, with the app's name
//! across it. More light, faster and greener, the busier the app.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Size},
};

use crate::{
    paint::{self, Painter},
    streaks::Streaks,
    usage::Meter,
};

/// The slot's height in logical pixels, and how far below the top of the bar it sits.
pub const HEIGHT: i32 = 20;
pub const TOP: i32 = 6;
/// How round its corners are, in logical pixels.
const RADIUS: f32 = 5.0;
/// The dark card the light runs over: the streams' own.
const CARD: u32 = 0x0b0d12ff;
/// Its edge, when the keyboard is elsewhere.
const EDGE: u32 = 0xffffff24;
/// How far inside the edge the light stops, in logical pixels.
const INSET: f64 = 1.0;
/// How bright the name is with no light passing over it: brighter than down a stream, which has
/// its app's icon above it to say whose it is.
const NAME_REST: f32 = 0.55;
/// The load is shown in this many steps, so a reading that holds still repaints nothing.
const STEPS: f32 = 40.0;

/// What the painted strip shows, beyond the light itself.
#[derive(Clone, Copy, PartialEq)]
struct Shown {
    device: (i32, i32),
    scale: f64,
    keyboard: bool,
    ring: [f32; 3],
}

pub struct Slot {
    name: String,
    /// How hard the docked app is working, 0 to 1.
    meter: Meter,
    streaks: Option<Streaks>,
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
            streaks: None,
            stepped_to: now,
            seed: now.to_bits() | 1,
            look_load: None,
            painted: None,
        }
    }

    /// The strip `width` logical pixels wide with its corner at `at`, lit for the app's load now
    /// (or `demand`, when a load is being shown for every app). Its edge and the name are in
    /// `ring`'s colour while the `keyboard` is in the docked pane.
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
        // The light falls down a card as tall as the strip is long and as wide as it is high,
        // which is then laid on its side.
        let streaks = self
            .streaks
            .get_or_insert_with(|| Streaks::new(&self.name, load, ih, iw, scale, self.seed));
        streaks.resize(ih, iw, scale);
        streaks.set_name_rest(NAME_REST);
        let recoloured = streaks.set_name_colour(ring);
        let changed = if reduced_motion {
            let changed = self.look_load != Some(step) || !streaks.is_still();
            if changed {
                streaks.hold(load);
                self.look_load = Some(step);
            }
            changed
        } else {
            let was_still = streaks.is_still();
            if self.look_load != Some(step) {
                streaks.set_load(load);
                self.look_load = Some(step);
            }
            streaks.step(now - self.stepped_to) || was_still
        };
        self.stepped_to = now;
        let shown = Shown {
            device,
            scale,
            keyboard,
            ring,
        };
        let stale = self
            .painted
            .as_ref()
            .is_none_or(|(painted, _)| *painted != shown);
        if changed || recoloured || stale {
            let mut light = vec![0u8; ih * iw * 4];
            streaks.draw(&mut light, ih, iw);
            let pixmap = paint_strip(&light, device, inset, (iw, ih), scale, keyboard, ring)?;
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

/// The strip's card `device` screen pixels big with `light` added inside its edge. `light` is
/// the falling card's, `inner.1` wide and `inner.0` tall, turned a quarter anticlockwise on the
/// way: the top of the card is the strip's left end, so what fell now runs left to right, and
/// the name down the card's spine reads along the strip.
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
    // The shape inside the edge that the light may fall on, softened along its corners.
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
    for sy in 0..ih {
        for sx in 0..iw {
            let cover = cover[(sy * iw + sx) * 4 + 3] as u32;
            // The card is `ih` wide: its right-hand side is the strip's top.
            let from = (sx * ih + (ih - 1 - sy)) * 4;
            let light = &light[from..from + 3];
            if cover == 0 || light == [0, 0, 0] {
                continue;
            }
            let at = ((sy + inset) * w as usize + sx + inset) * 4;
            for (channel, &light) in data[at..at + 3].iter_mut().zip(light) {
                let added = *channel as u32 + (light as u32 * cover + 127) / 255;
                *channel = added.min(255) as u8;
            }
        }
    }
    let edge = if keyboard {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        (channel(ring[0]) << 24) | (channel(ring[1]) << 16) | (channel(ring[2]) << 8) | 0xe6
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

    fn lit(pixmap: &resvg::tiny_skia::Pixmap) -> Vec<(u32, u32)> {
        let base = paint_strip(&[0u8; 4 * 6 * 4], (8, 6), 1, (6, 4), 1.0, false, [0.0; 3]).unwrap();
        let mut out = Vec::new();
        for y in 0..pixmap.height() {
            for x in 0..pixmap.width() {
                if pixmap.pixel(x, y) != base.pixel(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn what_falls_down_the_card_runs_left_to_right_along_the_strip() {
        let strip = |x, y| {
            let light = card_with(x, y);
            lit(&paint_strip(&light, (8, 6), 1, (6, 4), 1.0, false, [0.0; 3]).unwrap())
        };
        // Further down the card is further along the strip, in the same row.
        let (near_top, lower) = (strip(1, 1), strip(1, 4));
        assert_eq!(near_top, vec![(2, 3)]);
        assert_eq!(lower, vec![(5, 3)]);
        // The card's right-hand side is the strip's top.
        assert_eq!(strip(2, 1), vec![(2, 2)]);
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
