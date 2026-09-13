//! `coral`: a reaction–diffusion that grows out from round the logo and over the screen.
//!
//! Two chemicals diffuse across a grid of half cells, one feeding on the other (the Gray–Scott
//! model). Seeded in specks round the logo, the reaction spreads outwards as a living pattern: a
//! labyrinth, broad branching stripes or a honeycomb, depending on how fast the one is fed and the
//! other removed. Each turn takes a different mix, and the rates drift a little across the screen
//! and carry a grain of jitter, so fronts break up and branch.
//!
//! The pattern is coloured in rings of the logo's palette out from its middle, travelling slowly
//! outwards. At the end of a turn the removal rate climbs past what the pattern can live on, and
//! it breaks up into spots and dies back.

use super::{BG, Frame, Grid, Layout, Variation, WHITE, braille, gradient, hash01, mix, palette};

/// One turn, in seconds of the animation clock: growing, holding, dying back, and a gap.
const GROW: f64 = 14.0;
const HOLD: f64 = 9.0;
const DIE: f64 = 5.0;
const GAP: f64 = 1.0;
const TURN: f64 = GROW + HOLD + DIE + GAP;

/// Steps a second, and reaction steps a second of animation clock: the speed the pattern grows.
const HZ: f64 = 20.0;
const RATE: f64 = 260.0;

/// Seconds the seeds are fed at the start of a turn.
const FEEDING: f64 = 2.0;

/// Where the reaction shows: a half cell is drawn once the second chemical passes this.
const SHOWS: f32 = 0.22;

/// Where the fainter ring inside the outline runs.
const DEEP: f32 = SHOWS + 0.12;

/// The most of the pattern's inside the dither lights, where the chemistry is strongest.
const FILL: f32 = 0.3;

/// A 4×4 ordered dither: the order in which a block's dots light as it fills.
const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Half cells out from the logo in one lap of the palette, and laps a second the rings travel out.
const RING_WIDTH: f32 = 90.0;
const RING_SPEED: f32 = 0.02;

/// Reaction steps run for the still picture.
const STILL_STEPS: usize = 2600;

/// Feed and removal rates, for one kind of growth: each makes its own texture.
#[derive(Debug, Clone, Copy)]
struct Mix {
    feed: f32,
    kill: f32,
}

const MIXES: [Mix; 3] = [
    // A labyrinth of even stripes.
    Mix {
        feed: 0.029,
        kill: 0.057,
    },
    // Broad stripes that branch.
    Mix {
        feed: 0.039,
        kill: 0.058,
    },
    // A fine maze.
    Mix {
        feed: 0.025,
        kill: 0.055,
    },
];

/// How fast the two chemicals spread, as a share of the usual rates. Slower spreading makes a finer
/// pattern: half gives stripes about four half cells wide.
const DIFFUSE: f32 = 0.5;

/// How much the rates wander across the screen, from one edge to the other.
const DRIFT: (f32, f32) = (0.003, 0.001);

/// How far each half cell's feed rate is jittered either way.
const GRAIN: f32 = 0.003;

/// How far past its turn's rate the removal climbs while the pattern dies back.
const DEATH: f32 = 0.018;

#[derive(Default)]
pub struct Coral {
    /// Half cells across and down: the grid's columns, and twice its rows.
    w: usize,
    h: usize,
    a: Vec<f32>,
    b: Vec<f32>,
    next_a: Vec<f32>,
    next_b: Vec<f32>,
    /// The second chemical sampled smoothly at every Braille dot, two across and two down a
    /// half cell, for drawing.
    smooth: Vec<f32>,
    /// How far round the palette the rings have travelled.
    drift: f32,
    /// Each half cell's feed and removal rates, before the dying back.
    feed: Vec<f32>,
    kill: Vec<f32>,
    /// Specks round the window that seed the reaction and feed it while it starts.
    seeds: Vec<usize>,
    /// The half cells round the logo that are kept clear, so it stays readable.
    window: Vec<usize>,
    seed: u64,
    /// Which turn the grid holds, and the reaction steps owed from fractions of a step.
    turn: Option<u64>,
    owed: f64,
    /// The box of half cells the reaction is worked in: columns and rows, each end exclusive.
    reach: (usize, usize, usize, usize),
    still: bool,
}

impl Coral {
    /// Empties the grid and seeds it for turn `turn`, whose mix it picks.
    fn start(&mut self, layout: &Layout, turn: u64) {
        let (w, h) = (layout.cols.max(1) as usize, layout.rows.max(1) as usize * 2);
        self.w = w;
        self.h = h;
        let len = w * h;
        for buffer in [&mut self.a, &mut self.next_a] {
            buffer.clear();
            buffer.resize(len, 1.0);
        }
        for buffer in [&mut self.b, &mut self.next_b] {
            buffer.clear();
            buffer.resize(len, 0.0);
        }

        let pick = hash01(self.seed ^ turn.wrapping_mul(0x51_7cc1_b727_220a)) * MIXES.len() as f32;
        let mix = MIXES[(pick as usize).min(MIXES.len() - 1)];
        // Which way the rates drift, so the same mix differs from one turn to the next.
        let lean = hash01(self.seed ^ turn ^ 0xa5a5) * 2.0 - 1.0;
        self.feed.clear();
        self.kill.clear();
        for y in 0..h {
            for x in 0..w {
                let across = x as f32 / w as f32 - 0.5;
                let down = y as f32 / h as f32 - 0.5;
                // A grain of jitter in the rates, so a front breaks up and branches rather than
                // copying its own outline outwards ring after ring.
                let grain = hash01(self.seed ^ turn << 32 ^ (y * w + x) as u64) - 0.5;
                self.feed
                    .push(mix.feed + DRIFT.0 * (across * lean + down * 0.5) + GRAIN * grain);
                self.kill
                    .push(mix.kill + DRIFT.1 * (down * lean - across * 0.5));
            }
        }

        // A rounded window round the logo is kept clear, a superellipse so it hugs the corners of
        // a long box without square edges, which would print themselves into the pattern as ruled
        // lines. The reaction is seeded in specks just outside it and grows outwards from there.
        self.window.clear();
        self.seeds.clear();
        let (lx, ly, lw, lh) = layout.logo;
        let (cx, cy) = (
            lx as f32 + lw as f32 / 2.0,
            (ly as f32 + lh as f32 / 2.0) * 2.0,
        );
        let (rx, ry) = (lw as f32 / 2.0 + 3.0, lh as f32 + 4.0);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
                let i = y * w + x;
                let round = dx.powi(4) + dy.powi(4);
                if round <= 1.0 {
                    self.window.push(i);
                } else if round <= 1.2 && hash01(self.seed ^ turn << 40 ^ i as u64 ^ 0x5eed) < 0.3 {
                    self.seeds.push(i);
                }
            }
        }
        for &i in &self.seeds {
            self.a[i] = 0.5;
            self.b[i] = 0.5;
        }
        self.turn = Some(turn);
        self.owed = 0.0;
        self.reach = (w, 0, h, 0);
    }

    /// Runs the reaction `steps` times, with the seeds fed if `feeding`, and the removal raised by
    /// `dying`.
    fn react(&mut self, steps: usize, feeding: bool, dying: f32) {
        let (w, h) = (self.w, self.h);
        if w < 3 || h < 3 {
            return;
        }
        for step in 0..steps {
            // Only the box the reaction has reached is worked, which early in a turn is a small
            // part of the screen. The front moves far less than a half cell a step, so finding
            // the box every eight steps with a margin of three is always ahead of it.
            if step % 8 == 0 {
                self.grow_reach();
            }
            let (x0, x1, y0, y1) = self.reach;
            for y in y0.max(1)..y1.min(h - 1) {
                let row = |_v: &[f32], y: usize| -> std::ops::Range<usize> { y * w..(y + 1) * w };
                let (a_up, a_mid, a_down) = (
                    &self.a[row(&self.a, y - 1)],
                    &self.a[row(&self.a, y)],
                    &self.a[row(&self.a, y + 1)],
                );
                let (b_up, b_mid, b_down) = (
                    &self.b[row(&self.b, y - 1)],
                    &self.b[row(&self.b, y)],
                    &self.b[row(&self.b, y + 1)],
                );
                let feeds = &self.feed[y * w..(y + 1) * w];
                let kills = &self.kill[y * w..(y + 1) * w];
                let next_a = &mut self.next_a[y * w..(y + 1) * w];
                let next_b = &mut self.next_b[y * w..(y + 1) * w];
                for x in x0.max(1)..x1.min(w - 1) {
                    // The nine-point Laplacian: sides weigh a fifth, corners a twentieth.
                    let lap = |up: &[f32], mid: &[f32], down: &[f32]| {
                        0.2 * (mid[x - 1] + mid[x + 1] + up[x] + down[x])
                            + 0.05 * (up[x - 1] + up[x + 1] + down[x - 1] + down[x + 1])
                            - mid[x]
                    };
                    let (a, b) = (a_mid[x], b_mid[x]);
                    let abb = a * b * b;
                    let (feed, kill) = (feeds[x], kills[x] + dying);
                    next_a[x] = (a + DIFFUSE * lap(a_up, a_mid, a_down) - abb + feed * (1.0 - a))
                        .clamp(0.0, 1.0);
                    next_b[x] = (b + DIFFUSE * 0.5 * lap(b_up, b_mid, b_down) + abb
                        - (kill + feed) * b)
                        .clamp(0.0, 1.0);
                }
            }
            std::mem::swap(&mut self.a, &mut self.next_a);
            std::mem::swap(&mut self.b, &mut self.next_b);
            // Nothing flows through the screen's edges: each edge takes the value beside it, so
            // the pattern meets the edge at right angles instead of lining up along it.
            for v in [&mut self.a, &mut self.b] {
                v.copy_within(w..2 * w, 0);
                v.copy_within((h - 2) * w..(h - 1) * w, (h - 1) * w);
                for y in 0..h {
                    v[y * w] = v[y * w + 1];
                    v[y * w + w - 1] = v[y * w + w - 2];
                }
            }
            if feeding {
                for &i in &self.seeds {
                    self.b[i] = self.b[i].max(0.35);
                }
            }
            for &i in &self.window {
                self.a[i] = 1.0;
                self.b[i] = 0.0;
            }
        }
    }

    /// Widens `reach` to take in every half cell the second chemical has reached, with a margin.
    /// It never shrinks within a turn: cells outside it must be the same in both buffers.
    fn grow_reach(&mut self) {
        let (w, h) = (self.w, self.h);
        let (mut x0, mut x1, mut y0, mut y1) = self.reach;
        for y in 0..h {
            let row = &self.b[y * w..(y + 1) * w];
            let Some(first) = row.iter().position(|&b| b > 1e-5) else {
                continue;
            };
            let last = row.iter().rposition(|&b| b > 1e-5).unwrap_or(first);
            x0 = x0.min(first.saturating_sub(3));
            x1 = x1.max((last + 4).min(w));
            y0 = y0.min(y.saturating_sub(3));
            y1 = y1.max((y + 4).min(h));
        }
        self.reach = (x0, x1, y0, y1);
    }

    fn draw(&mut self, layout: &Layout, grid: &mut Grid, fade: f32) {
        let w = self.w;
        // Rings in the palette out from the logo's middle, which is roughly the order the pattern
        // arrived in, turning slowly outwards. Taken from where a half cell is rather than when it
        // was reached, so neighbouring stripes share a colour instead of speckling.
        let (lx, ly, lw, lh) = layout.logo;
        let centre = (
            (lx as f32 + lw as f32 / 2.0),
            (ly as f32 + lh as f32 / 2.0) * 2.0,
        );
        let squash = lw as f32 / (lh as f32 * 2.0).max(1.0);
        let ring = |i: usize| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let (dx, dy) = ((x - centre.0) / squash.sqrt(), y - centre.1);
            let at = (dx * dx + dy * dy).sqrt() / RING_WIDTH - self.drift;
            palette(((at * 8.0).floor() + 0.5) / 8.0)
        };
        // The chemistry is drawn in Braille dots, two across and two down each half cell, sampled
        // smoothly between half cells so the pattern's edges curve rather than step. Its outline
        // is a fine bright line of dots, with a fainter ring inside it where the chemistry runs
        // deeper; the rest of the inside is a sparse dither that thickens with the chemistry.
        let h = self.h;
        // The chemistry between half cells, once for every dot.
        let (dw, dh) = (w * 2, h * 2);
        self.smooth.clear();
        self.smooth.resize(dw * dh, 0.0);
        for gy in 0..dh {
            let fy = ((gy as f32 + 0.5) / 2.0 - 0.5).max(0.0);
            let (y, ty) = (fy as usize, fy.fract());
            let y1 = (y + 1).min(h - 1);
            for gx in 0..dw {
                let fx = ((gx as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let (x, tx) = (fx as usize, fx.fract());
                let x1 = (x + 1).min(w - 1);
                let (a, b, c, d) = (
                    self.b[y * w + x],
                    self.b[y * w + x1],
                    self.b[y1 * w + x],
                    self.b[y1 * w + x1],
                );
                if a.max(b).max(c).max(d) <= SHOWS {
                    continue;
                }
                let top = a + (b - a) * tx;
                let bottom = c + (d - c) * tx;
                self.smooth[gy * dw + gx] = top + (bottom - top) * ty;
            }
        }
        let smooth = &self.smooth;
        let dot = |gx: usize, gy: usize| smooth[gy.min(dh - 1) * dw + gx.min(dw - 1)];
        for row in 0..(layout.rows.max(0) as usize).min(h / 2) {
            for col in 0..(layout.cols.max(0) as usize).min(w) {
                let (mut dots, mut outline, mut ring_line, mut inside) =
                    (0u8, false, false, 0.0f32);
                for down in 0..4 {
                    for across in 0..2 {
                        let (gx, gy) = (col * 2 + across, row * 4 + down);
                        let b = dot(gx, gy);
                        if b <= SHOWS {
                            continue;
                        }
                        // On the outline when a neighbouring dot is outside the pattern.
                        let near = [
                            dot(gx + 1, gy),
                            dot(gx.saturating_sub(1), gy),
                            dot(gx, gy + 1),
                            dot(gx, gy.saturating_sub(1)),
                        ];
                        let edge = near.iter().any(|&n| n <= SHOWS);
                        let inner = !edge && b > DEEP && near.iter().any(|&n| n <= DEEP);
                        let strength = ((b - SHOWS) / 0.25).min(1.0);
                        let dither = (BAYER[(gy % 4) * 4 + gx % 4] as f32 + 0.5) / 16.0;
                        if edge || inner || dither < FILL * (0.4 + 0.6 * strength) {
                            dots |= 1 << (down * 2 + across);
                            outline |= edge;
                            ring_line |= inner;
                            inside = inside.max(strength);
                        }
                    }
                }
                if dots == 0 {
                    continue;
                }
                let colour = ring(row * 2 * w + col);
                let (rgb, level) = if outline {
                    (mix(colour, WHITE, 0.2), 1.0)
                } else if ring_line {
                    (colour, 0.75)
                } else {
                    (colour, 0.4 + 0.2 * inside)
                };
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, rgb, level * fade),
                );
            }
        }
        // The letters on top, steady in their own colours.
        let (lx, lw) = (layout.logo.0, layout.logo.2);
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, mix(gradient(across), WHITE, 0.2), fade.max(0.0)),
            );
        }
    }
}

impl Variation for Coral {
    fn id(&self) -> &'static str {
        "coral"
    }

    fn reset(&mut self, layout: &Layout, seed: u64) {
        self.seed = seed;
        self.turn = None;
        self.still = false;
        self.w = 0;
        self.h = 0;
        let _ = layout;
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        if frame.still {
            // The still picture: the first mix, grown for a while, made once.
            if !self.still || self.w != layout.cols.max(1) as usize {
                self.start(layout, 0);
                self.react(STILL_STEPS, true, 0.0);
                self.still = true;
            }
            self.draw(layout, grid, 1.0);
            return;
        }
        self.still = false;
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn)
            || self.w != layout.cols.max(1) as usize
            || self.h != layout.rows.max(1) as usize * 2
        {
            self.start(layout, turn);
        }
        let dying = if t < GROW + HOLD {
            0.0
        } else {
            DEATH * smoothstep_f64((t - GROW - HOLD) / DIE)
        };
        // Full speed while it grows and dies back; a slow creep while it holds.
        let speed = if (GROW..GROW + HOLD).contains(&t) {
            0.25
        } else {
            1.0
        };
        self.owed += frame.dt as f64 * RATE * speed;
        let steps = self.owed.floor() as usize;
        self.owed -= steps as f64;
        if t < GROW + HOLD + DIE {
            self.react(steps.min(64), t < FEEDING, dying);
        }
        self.drift += frame.dt * RING_SPEED;
        // Up from nothing in the first second, and out over the last of the dying.
        let fade = if t < 1.0 {
            t as f32
        } else if t < GROW + HOLD + DIE - 1.0 {
            1.0
        } else {
            ((GROW + HOLD + DIE - t) as f32).max(0.0)
        };
        if fade > 0.0 {
            self.draw(layout, grid, fade);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        GROW * 0.6
    }
}

fn smoothstep_f64(t: f64) -> f32 {
    super::smoothstep(t as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        Readings, layout,
        tests::{at_home, drawn},
    };

    /// Steps a coral from the start of a turn to `until`, as the saver would.
    fn grown(until: f64) -> (Grid, Layout) {
        let layout = layout(1280, 800).unwrap();
        let mut coral = Coral::default();
        coral.reset(&layout, 7);
        let mut grid = Grid::new(layout.cols, layout.rows);
        let readings = Readings::default();
        let mut elapsed = 0.0;
        while elapsed < until {
            grid.blank(layout.cols, layout.rows);
            coral.compose(
                Frame {
                    layout: &layout,
                    elapsed,
                    dt: 1.0 / HZ as f32,
                    still: false,
                    readings: &readings,
                    local: None,
                },
                &mut grid,
            );
            elapsed += 1.0 / HZ;
        }
        (grid, layout)
    }

    #[test]
    fn the_pattern_grows_out_of_the_letters() {
        let (early, layout) = grown(2.0);
        let (later, _) = grown(12.0);
        assert_eq!(at_home(&later, &layout), layout.letters.len());
        let beyond = |grid: &Grid| drawn(grid) - at_home(grid, &layout);
        assert!(
            beyond(&later) > beyond(&early) * 3 && beyond(&later) > layout.letters.len(),
            "it spreads: {} then {}",
            beyond(&early),
            beyond(&later)
        );
    }

    #[test]
    fn it_dies_back_to_an_empty_screen_before_the_next_turn() {
        let (gap, _) = grown(TURN - 0.2);
        assert_eq!(drawn(&gap), 0);
    }

    #[test]
    fn reduced_motion_holds_one_grown_picture() {
        let layout = layout(1280, 800).unwrap();
        let mut coral = Coral::default();
        coral.reset(&layout, 7);
        let first = crate::saver::tests::composed(&mut coral, &layout, 0.0, true);
        let second = crate::saver::tests::composed(&mut coral, &layout, 9.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > layout.letters.len() * 2);
    }
}
