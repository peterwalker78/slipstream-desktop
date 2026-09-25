//! The wake a window leaves when it moves fast: streaks of light trailing from its back edge, and
//! a faint band of air behind it, strongest at the start of a move and gone as it settles.
//!
//! Each streak is one small gradient picture stretched to length, so a wake costs a dozen
//! elements and no painting after the first frame.

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
};

/// Below this speed, in logical pixels a second, a window leaves no wake.
const QUIET: f64 = 400.0;
/// The speed at which a wake is as bright as it gets.
const FULL: f64 = 9000.0;
/// Streaks behind each window.
const STREAKS: usize = 11;
/// The longest a streak gets, in logical pixels.
const LONGEST: f64 = 700.0;
const BAND_LONGEST: f64 = 520.0;
/// The streaks' colours: the logo's mint, and the focus ring's cyan.
const MINT: [u8; 3] = [60, 240, 192];
const CYAN: [u8; 3] = [66, 211, 255];

/// Which way a streak runs from the edge it leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trail {
    Right,
    Left,
    Down,
    Up,
}

/// One gradient picture: which way it runs, which colour, and its size in pixels.
type Streak = (Trail, usize, MemoryRenderBuffer, (i32, i32));

/// The gradient pictures, one per colour and direction.
#[derive(Default)]
pub struct Wakes {
    buffers: Option<Vec<Streak>>,
}

/// A streak's picture: bright at the edge it leaves, gone at the far end, soft across.
fn streak(trail: Trail, rgb: [u8; 3]) -> Option<(MemoryRenderBuffer, (i32, i32))> {
    const LONG: u32 = 256;
    const WIDE: u32 = 12;
    let (w, h) = match trail {
        Trail::Right | Trail::Left => (LONG, WIDE),
        Trail::Down | Trail::Up => (WIDE, LONG),
    };
    let mut pixmap = Pixmap::new(w, h)?;
    let data = pixmap.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (along, across) = match trail {
                Trail::Right => (x, y),
                Trail::Left => (LONG - 1 - x, y),
                Trail::Down => (y, x),
                Trail::Up => (LONG - 1 - y, x),
            };
            let fade = (1.0 - along as f32 / (LONG - 1) as f32).powf(1.5);
            let off = (across as f32 + 0.5 - WIDE as f32 / 2.0) / 2.2;
            let soft = (-off * off).exp();
            let a = fade * soft;
            let at = ((y * w + x) * 4) as usize;
            data[at] = (rgb[0] as f32 * a).round() as u8;
            data[at + 1] = (rgb[1] as f32 * a).round() as u8;
            data[at + 2] = (rgb[2] as f32 * a).round() as u8;
            data[at + 3] = (255.0 * a).round() as u8;
        }
    }
    Some((crate::paint::buffer(&pixmap), (w as i32, h as i32)))
}

/// A small, fixed sequence of numbers for one window, so its streaks keep their places from frame
/// to frame.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / (1u64 << 31) as f64
    }
}

impl Wakes {
    fn buffers(&mut self) -> &[Streak] {
        if self.buffers.is_none() {
            let mut made = Vec::new();
            for trail in [Trail::Right, Trail::Left, Trail::Down, Trail::Up] {
                for (colour, rgb) in [MINT, CYAN].into_iter().enumerate() {
                    if let Some((buffer, size)) = streak(trail, rgb) {
                        made.push((trail, colour, buffer, size));
                    }
                }
            }
            self.buffers = Some(made);
        }
        self.buffers.as_deref().unwrap_or(&[])
    }

    /// The wake of a window drawn at `rect` (the screen's logical pixels) moving at `velocity`
    /// logical pixels a second, at `scale`, drawn at `alpha`. `seed` keeps each window's streaks
    /// where they were last frame. Nothing when it's barely moving.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        rect: Rectangle<f64, Logical>,
        velocity: (f64, f64),
        seed: u64,
        scale: f64,
        alpha: f32,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (vx, vy) = velocity;
        let across = vx.abs() >= vy.abs();
        let speed = if across { vx.abs() } else { vy.abs() };
        if speed < QUIET || alpha <= 0.0 {
            return Vec::new();
        }
        let strength = (speed / FULL).min(1.0) * 0.9 * alpha as f64;
        // Streaks trail from the back edge, opposite the way it's going.
        let trail = match (across, if across { vx } else { vy } < 0.0) {
            (true, true) => Trail::Right,
            (true, false) => Trail::Left,
            (false, true) => Trail::Down,
            (false, false) => Trail::Up,
        };
        let (x, y, w, h) = (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
        let mut random = Seeded(seed.wrapping_mul(2654435761).wrapping_add(7));
        let mut pieces = Vec::with_capacity(STREAKS + 1);
        // (start along the travel, position across it, length, thickness, colour, strength)
        let mut lay = |pos: f64, len: f64, thick: f64, colour: usize, a: f64| {
            let (loc, size) = match trail {
                Trail::Right => ((x + w, pos - thick / 2.0), (len, thick)),
                Trail::Left => ((x - len, pos - thick / 2.0), (len, thick)),
                Trail::Down => ((pos - thick / 2.0, y + h), (thick, len)),
                Trail::Up => ((pos - thick / 2.0, y - len), (thick, len)),
            };
            pieces.push((
                Point::<f64, Logical>::from(loc),
                Size::<f64, Logical>::from(size),
                colour,
                a,
            ));
        };
        // A faint band of disturbed air the height of the window.
        let (side, start) = if across { (h, y) } else { (w, x) };
        let band = (speed * 0.03).min(BAND_LONGEST);
        lay(start + side / 2.0, band, side * 0.88, 1, 0.12 * strength);
        for _ in 0..STREAKS {
            let pos = start + side * (0.04 + 0.92 * random.next());
            let len = (speed * 0.035).min(LONGEST) * (0.45 + random.next());
            let thick = (1.5 + 2.5 * random.next()) * 3.0;
            let colour = usize::from(random.next() >= 0.5);
            lay(pos, len, thick, colour, strength);
        }
        let buffers = self.buffers();
        pieces
            .into_iter()
            .filter_map(|(loc, size, colour, a)| {
                let (_, _, buffer, device) = buffers
                    .iter()
                    .find(|(t, c, ..)| *t == trail && *c == colour)?;
                let logical = Size::<i32, Logical>::from((
                    size.w.round().max(1.0) as i32,
                    size.h.round().max(1.0) as i32,
                ));
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    loc.to_physical(scale),
                    buffer,
                    Some(a.clamp(0.0, 1.0) as f32),
                    Some(Rectangle::from_size(
                        (device.0 as f64, device.1 as f64).into(),
                    )),
                    Some(logical),
                    Kind::Unspecified,
                )
                .ok()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streak_is_brightest_at_the_edge_it_leaves() {
        let (_, (w, h)) = streak(Trail::Right, MINT).unwrap();
        assert_eq!((w, h), (256, 12));
        let (_, (w, h)) = streak(Trail::Up, CYAN).unwrap();
        assert_eq!((w, h), (12, 256));
    }

    #[test]
    fn the_same_window_gets_the_same_streaks() {
        let mut a = Seeded(42);
        let mut b = Seeded(42);
        for _ in 0..5 {
            let (x, y) = (a.next(), b.next());
            assert_eq!(x, y);
            assert!((0.0..1.0).contains(&x));
        }
    }
}
