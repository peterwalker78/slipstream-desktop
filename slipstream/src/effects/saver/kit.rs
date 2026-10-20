//! What the variations draw with: the palette, easing, randomness, and the logo in letters
//! or in Braille dots.

use super::*;

pub(super) const BG: [f32; 3] = [11.0 / 255.0, 14.0 / 255.0, 19.0 / 255.0];
pub(super) const WHITE: [f32; 3] = [0.93, 0.98, 1.0];
pub(super) const CYAN: [f32; 3] = [0.2, 0.8, 1.0];
pub(super) const AMBER: [f32; 3] = [1.0, 0.71, 0.28];
pub(super) const MINT: [f32; 3] = [0.235, 0.94, 0.75];

pub(super) fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub(super) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The logo's colours, left to right: Slipstream's amber and mint, then cyan.
pub(super) fn gradient(across: f32) -> [f32; 3] {
    if across < 0.5 {
        mix(AMBER, MINT, across * 2.0)
    } else {
        mix(MINT, CYAN, (across - 0.5) * 2.0)
    }
}

/// The next number from a splitmix64 sequence, advancing `state`.
pub(super) fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A steady pseudo-random number in [0, 1) for `n` (splitmix64).
pub(super) fn hash01(n: u64) -> f32 {
    let mut state = n;
    (next_random(&mut state) >> 40) as f32 / (1u64 << 24) as f32
}

/// A random state for `turn`, different every turn and the same every run for the same seed.
pub(super) fn turn_rng(seed: u64, turn: u64) -> u64 {
    (seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1
}

/// Which turn of `turn` seconds `elapsed` is in, and how far into it.
pub(super) fn into_turn(elapsed: f64, turn: f64) -> (u64, f64) {
    (
        (elapsed / turn).floor().max(0.0) as u64,
        elapsed.rem_euclid(turn),
    )
}

/// How far in the picture has faded at `t` seconds into a turn `turn` long: up over the first
/// `appear` seconds, down over the last `gone`, and fully in between.
pub(super) fn turn_fade(t: f64, appear: f64, turn: f64, gone: f64) -> f32 {
    if t < appear {
        (t / appear) as f32
    } else if t > turn - gone {
        ((turn - t) / gone) as f32
    } else {
        1.0
    }
}

/// A 4×4 ordered dither: the order in which a block's dots light as it fills.
pub(super) const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// The figures 0 to 9, five cells of ink by seven rows, `#` for ink.
#[rustfmt::skip]
pub(super) const FIGURES: [[&str; 7]; 10] = [
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    ["####.", "....#", "....#", ".###.", "....#", "....#", "####."],
    ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    [".###.", "#....", "#....", "####.", "#...#", "#...#", ".###."],
    ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    [".###.", "#...#", "#...#", ".####", "....#", "....#", ".###."],
];

/// The line character that draws a nearly level line at `row`, a third of a row at a time: the
/// cell it falls in, and `─` across its middle or `▔`/`▁` a third of the way up or down.
pub(super) fn level_line(row: f32) -> (f32, char) {
    let thirds = (row * 3.0).round() as i32;
    let cell = thirds.div_euclid(3) as f32;
    match thirds.rem_euclid(3) {
        0 => (cell, '─'),
        1 => (cell, '▁'),
        _ => (cell + 1.0, '▔'),
    }
}

/// The line character that runs in the direction (`dx`, `dy`), with `dy` in rows counted double
/// so the directions are as they look on screen.
pub(super) fn line_glyph(dx: f32, dy: f32) -> char {
    let angle = dy.atan2(dx).rem_euclid(std::f32::consts::PI);
    let eighth = std::f32::consts::PI / 8.0;
    match angle {
        a if a < eighth || a >= 7.0 * eighth => '─',
        a if a < 3.0 * eighth => '╲',
        a if a < 5.0 * eighth => '│',
        _ => '╱',
    }
}

/// Slipstream's colours as one loop, for ink whose colour travels: amber, mint, cyan, and back
/// to amber. `at` runs round it once from 0 to 1.
pub(super) fn palette(at: f32) -> [f32; 3] {
    let at = at.rem_euclid(1.0) * 3.0;
    match at {
        a if a < 1.0 => mix(AMBER, MINT, a),
        a if a < 2.0 => mix(MINT, CYAN, a - 1.0),
        a => mix(CYAN, AMBER, a - 2.0),
    }
}

/// Draws the logo's letters in their own blocks, `fade` of the way in.
pub(super) fn draw_letters(layout: &Layout, grid: &mut Grid, fade: f32) {
    let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
    for letter in &layout.letters {
        let across = (letter.col - lx) as f32 / lw as f32;
        grid.put(
            letter.col as f32,
            letter.row as f32,
            letter.ch,
            mix(BG, gradient(across), fade),
        );
    }
}

/// Many small things gathered into Braille cells: each lights its dot and adds its colour to its
/// cell, and a cell is drawn in the mean of its colours, brighter and whiter the more it holds.
#[derive(Default)]
pub(super) struct Stipple {
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) dots: Vec<u8>,
    /// Red, green and blue summed, and how many were added, a cell.
    pub(super) ink: Vec<[f32; 4]>,
}

impl Stipple {
    pub(super) fn clear(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.dots.clear();
        self.dots.resize(cols * rows, 0);
        self.ink.clear();
        self.ink.resize(cols * rows, [0.0; 4]);
    }

    /// Adds one at (`x`, `y`) in dots, `weight` strong.
    pub(super) fn add(&mut self, x: f32, y: f32, rgb: [f32; 3], weight: f32) {
        if !(x >= 0.0 && y >= 0.0) {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let (col, row) = (x / 2, y / 4);
        if col >= self.cols || row >= self.rows {
            return;
        }
        let cell = row * self.cols + col;
        self.dots[cell] |= 1 << ((y % 4) * 2 + x % 2);
        let ink = &mut self.ink[cell];
        for (sum, channel) in ink.iter_mut().zip(rgb) {
            *sum += channel * weight;
        }
        ink[3] += weight;
    }

    /// Draws every cell holding anything. `full` is how much a cell holds to be drawn at full
    /// brightness; `white` how far past that its colour runs towards white.
    pub(super) fn draw(&self, grid: &mut Grid, full: f32, white: f32, fade: f32) {
        for row in 0..self.rows.min(grid.rows.max(0) as usize) {
            for col in 0..self.cols.min(grid.cols.max(0) as usize) {
                let cell = row * self.cols + col;
                let dots = self.dots[cell];
                let ink = self.ink[cell];
                if dots == 0 || ink[3] <= 0.0 {
                    continue;
                }
                let rgb = [ink[0] / ink[3], ink[1] / ink[3], ink[2] / ink[3]];
                let level = (0.35 + 0.65 * (ink[3] / full).sqrt()).min(1.0);
                let whiter = ((ink[3] / full - 1.0) * white).clamp(0.0, 0.75);
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, mix(rgb, WHITE, whiter), level * fade),
                );
            }
        }
    }
}

/// xorshift64*: cheap, and the same every run for the same seed. Never seed it with 0.
pub(super) fn xorshift(state: &mut u64) -> f32 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
}
