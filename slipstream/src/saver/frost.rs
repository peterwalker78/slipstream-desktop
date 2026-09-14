//! `frost`: frost grows out of the letters in branching ferns until it has crept over the glass,
//! glitters, and then melts back into the letters the way it came.
//!
//! Drawn in Braille dots. The frost is diffusion-limited aggregation: a speck of vapour wanders
//! at random until it touches the frost, and freezes there. Nothing tells the frost to branch; the
//! tips of a branch stick out further into the wandering vapour than the hollows between them, so
//! they catch more of it, grow further, and shade the hollows even more. Each speck takes the
//! colour of the ice it froze on to, so the ferns carry their letter's colour out across the
//! screen, and the newest ice glints white.
//!
//! A speck far from any ice jumps rather than wanders: in a block with no ice in it or round it
//! a random walk would take many steps to go the same distance in some direction, so it takes one.
//! The frost remembers the order it froze in, and melts in the reverse of it.

use super::{
    BG, Frame, Grid, Layout, Variation, WHITE, braille, draw_letters, gradient, hash01, logo_dots,
    mix, smoothstep, xorshift,
};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const GROW: f64 = 20.0;
const GLITTER: f64 = 3.0;
const MELT: f64 = 5.0;
const REST: f64 = 1.5;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + GROW + GLITTER + MELT + REST + GONE;

/// Steps a second.
const HZ: f64 = 20.0;

/// How much of the screen's dots the frost covers when it has finished growing.
const COVER: f32 = 0.14;
/// Steps of wandering a step of the clock may spend, which bounds what growing costs.
const BUDGET: usize = 160_000;
/// Specks wandering at once.
const SPECKS: usize = 64;
/// Specks that must reach a spot beside the ice before it freezes. More than one smooths the
/// random walk's noise out of the branches, so they grow as long needles with side shoots, as
/// frost does, rather than as ragged lightning.
const HITS: u8 = 3;
/// Dots a side of the blocks a speck jumps across.
const BLOCK: usize = 8;
/// Seconds the newest ice glints for.
const GLINT: f32 = 0.5;

/// A dot of the letters, frozen from the start.
const SEED: u32 = 1;

#[derive(Default)]
pub struct Frost {
    seed: u64,
    turn: Option<u64>,
    cols: i32,
    rows: i32,
    /// Dots across and down.
    w: usize,
    h: usize,
    /// 0 for no ice; `SEED` for a letter's dot; otherwise its place in `order`, plus 2.
    ice: Vec<u32>,
    /// How far across the logo the ice's colour comes from, a dot.
    hue: Vec<f32>,
    /// Ice in each block.
    blocks: Vec<u16>,
    /// The dots that froze, in the order they did.
    order: Vec<u32>,
    /// When each froze, in seconds into the turn.
    frozen_at: Vec<f32>,
    /// Specks that have reached each spot without freezing it yet.
    hits: Vec<u8>,
    seeds: Vec<u32>,
    specks: Vec<(i32, i32)>,
    rng: u64,
    stepped: Option<i64>,
    dots: Vec<u8>,
    ink: Vec<[f32; 6]>,
}

impl Frost {
    fn start(&mut self, layout: &Layout, turn: u64) {
        self.turn = Some(turn);
        self.cols = layout.cols;
        self.rows = layout.rows;
        let (w, h) = (
            layout.cols.max(1) as usize * 2,
            layout.rows.max(1) as usize * 4,
        );
        self.w = w;
        self.h = h;
        self.ice.clear();
        self.ice.resize(w * h, 0);
        self.hue.clear();
        self.hue.resize(w * h, 0.0);
        self.hits.clear();
        self.hits.resize(w * h, 0);
        self.blocks.clear();
        self.blocks.resize(w.div_ceil(BLOCK) * h.div_ceil(BLOCK), 0);
        self.order.clear();
        self.frozen_at.clear();
        self.seeds.clear();
        self.specks.clear();
        self.stepped = None;
        self.rng = (self.seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;
        for dot in logo_dots(layout) {
            if dot.x < w && dot.y < h {
                let i = dot.y * w + dot.x;
                if self.ice[i] == 0 {
                    self.ice[i] = SEED;
                    self.hue[i] = dot.across;
                    self.blocks[(dot.y / BLOCK) * w.div_ceil(BLOCK) + dot.x / BLOCK] += 1;
                    self.seeds.push(i as u32);
                }
            }
        }
    }

    fn most(&self) -> usize {
        ((self.w * self.h) as f32 * COVER) as usize
    }

    /// Whether there is ice in the block holding (`x`, `y`) or any block round it.
    fn near(&self, x: i32, y: i32) -> bool {
        let across = self.w.div_ceil(BLOCK) as i32;
        let down = self.h.div_ceil(BLOCK) as i32;
        let (bx, by) = (x / BLOCK as i32, y / BLOCK as i32);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy) = (bx + dx, by + dy);
                if cx >= 0
                    && cy >= 0
                    && cx < across
                    && cy < down
                    && self.blocks[(cy * across + cx) as usize] > 0
                {
                    return true;
                }
            }
        }
        false
    }

    /// A new speck somewhere away from the ice if there's room for one, and never on the ice
    /// itself, where a speck inside a solid letter could never move again.
    fn launch(&mut self) -> (i32, i32) {
        let (w, h) = (self.w as f32, self.h as f32);
        let mut spot = (0, 0);
        for tries in 0..400 {
            spot = (
                (xorshift(&mut self.rng) * w) as i32,
                (xorshift(&mut self.rng) * h) as i32,
            );
            let on_ice = self.ice[spot.1 as usize * self.w + spot.0 as usize] != 0;
            if !on_ice && (tries >= 40 || !self.near(spot.0, spot.1)) {
                break;
            }
        }
        spot
    }

    /// The colour of a speck's frozen neighbour, if it's touching any ice.
    fn touching(&self, x: i32, y: i32) -> Option<f32> {
        let (w, h) = (self.w as i32, self.h as i32);
        for (dx, dy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (-1, 1),
            (1, -1),
            (-1, -1),
        ] {
            let (nx, ny) = (x + dx, y + dy);
            if nx >= 0 && ny >= 0 && nx < w && ny < h {
                let i = (ny * w + nx) as usize;
                if self.ice[i] != 0 {
                    return Some(self.hue[i]);
                }
            }
        }
        None
    }

    /// Lets specks wander until `target` dots have frozen or `budget` steps are spent.
    fn grow(&mut self, target: usize, mut budget: usize, now: f32) {
        let (w, h) = (self.w as i32, self.h as i32);
        if w < 3 || h < 3 {
            return;
        }
        while self.specks.len() < SPECKS {
            let speck = self.launch();
            self.specks.push(speck);
        }
        let mut k = 0;
        while self.order.len() < target && budget > 0 {
            k = (k + 1) % SPECKS;
            let (mut x, mut y) = self.specks[k];
            // Each speck wanders a while before the next has a turn, so they share the budget.
            for _ in 0..256 {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                if !self.near(x, y) {
                    // Nothing within a block: jump most of one, a random way.
                    let angle = xorshift(&mut self.rng) * std::f32::consts::TAU;
                    let reach = (BLOCK - 1) as f32;
                    x = bounce(x + (angle.cos() * reach) as i32, w);
                    y = bounce(y + (angle.sin() * reach) as i32, h);
                    continue;
                }
                let (nx, ny) = match (xorshift(&mut self.rng) * 4.0) as u32 {
                    0 => (x + 1, y),
                    1 => (x - 1, y),
                    2 => (x, y + 1),
                    _ => (x, y - 1),
                };
                let (nx, ny) = (bounce(nx, w), bounce(ny, h));
                // A step on to the ice is refused, but the speck has touched it all the same, so a
                // speck in a hole walled in by ice still freezes rather than waiting there forever.
                if self.ice[(ny * w + nx) as usize] == 0 {
                    (x, y) = (nx, ny);
                }
                let Some(hue) = self.touching(x, y) else {
                    continue;
                };
                let i = (y * w + x) as usize;
                self.hits[i] = self.hits[i].saturating_add(1);
                if self.hits[i] < HITS {
                    (x, y) = self.launch();
                    continue;
                }
                self.ice[i] = self.order.len() as u32 + 2;
                self.hue[i] = hue;
                let across = self.w.div_ceil(BLOCK);
                self.blocks[(y as usize / BLOCK) * across + x as usize / BLOCK] += 1;
                self.order.push(i as u32);
                self.frozen_at.push(now);
                (x, y) = self.launch();
                if self.order.len() >= target {
                    break;
                }
            }
            self.specks[k] = (x, y);
        }
    }

    /// Draws the letters' dots and the first `shown` dots of ice, glinting where they froze in the
    /// last moments before `now`, and glittering here and there by `glitter`.
    fn draw(&mut self, grid: &mut Grid, shown: usize, now: f32, glitter: f32, fade: f32) {
        let (cols, rows) = (self.cols.max(0) as usize, self.rows.max(0) as usize);
        self.dots.clear();
        self.dots.resize(cols * rows, 0);
        self.ink.clear();
        self.ink.resize(cols * rows, [0.0; 6]);
        let w = self.w;
        let add = |i: u32,
                   glint: f32,
                   bright: f32,
                   dots: &mut Vec<u8>,
                   ink: &mut Vec<[f32; 6]>,
                   hue: f32| {
            let (x, y) = (i as usize % w, i as usize / w);
            let (col, row) = (x / 2, y / 4);
            if col >= cols || row >= rows {
                return;
            }
            let cell = row * cols + col;
            dots[cell] |= 1 << ((y % 4) * 2 + x % 2);
            let rgb = gradient(hue);
            let sum = &mut ink[cell];
            sum[0] += rgb[0];
            sum[1] += rgb[1];
            sum[2] += rgb[2];
            sum[3] += 1.0;
            sum[4] = sum[4].max(glint);
            sum[5] = sum[5].max(bright);
        };
        for &i in &self.seeds {
            add(
                i,
                0.0,
                1.0,
                &mut self.dots,
                &mut self.ink,
                self.hue[i as usize],
            );
        }
        // Older ice is dimmer, so the frost is brightest at the edge it grows or melts from.
        let newest = shown.max(1) as f32;
        for (k, &i) in self.order.iter().take(shown).enumerate() {
            let glint = 1.0 - ((now - self.frozen_at[k]) / GLINT).clamp(0.0, 1.0);
            let bright = 0.4 + 0.6 * (k as f32 / newest).powf(1.5);
            add(
                i,
                glint,
                bright,
                &mut self.dots,
                &mut self.ink,
                self.hue[i as usize],
            );
        }
        // The glitter moves on a few times a second.
        let twinkle = (now * 6.0) as u64;
        for row in 0..rows.min(grid.rows.max(0) as usize) {
            for col in 0..cols.min(grid.cols.max(0) as usize) {
                let cell = row * cols + col;
                let dots = self.dots[cell];
                if dots == 0 {
                    continue;
                }
                let sum = self.ink[cell];
                let rgb = [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3]];
                let sparkle = if hash01(cell as u64 ^ twinkle.wrapping_mul(0x9e37_79b9)) < glitter {
                    0.8
                } else {
                    0.0
                };
                let white = (sum[4] * 0.85).max(sparkle);
                let level = sum[5] * (0.7 + 0.3 * (sum[3] / 4.0).min(1.0));
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, mix(rgb, WHITE, white), level.max(white) * fade),
                );
            }
        }
    }
}

/// `v` bounced back inside 0..`len`.
fn bounce(v: i32, len: i32) -> i32 {
    if v < 0 {
        (-v).min(len - 1)
    } else if v >= len {
        (2 * (len - 1) - v).max(0)
    } else {
        v
    }
}

impl Variation for Frost {
    fn id(&self) -> &'static str {
        "frost"
    }

    fn reset(&mut self, _layout: &Layout, seed: u64) {
        self.seed = seed;
        self.turn = None;
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let resized = self.cols != layout.cols || self.rows != layout.rows;
        if frame.still {
            // Frost most of the way over the glass, from the same start every time.
            if self.turn != Some(0) || resized {
                self.seed = 0;
                self.start(layout, 0);
                let most = self.most();
                self.grow(most * 3 / 4, BUDGET * 2000, -10.0);
            }
            let shown = self.order.len();
            self.draw(grid, shown, 0.0, 0.0, 1.0);
            return;
        }
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn) || resized {
            self.start(layout, turn);
        }
        let fade = if t < APPEAR {
            (t / APPEAR) as f32
        } else if t > TURN - GONE {
            ((TURN - t) / GONE) as f32
        } else {
            1.0
        };
        let grown = APPEAR + GROW;
        let melt = grown + GLITTER;
        let gone = melt + MELT;
        let most = self.most();
        let step = (frame.elapsed * HZ).floor() as i64;
        if t >= APPEAR && t < grown && self.stepped != Some(step) {
            self.stepped = Some(step);
            // Slow at first, then faster as there's more frost to catch the vapour.
            let along = ((t - APPEAR) / GROW) as f32;
            let target = (most as f32 * along.powf(1.4)) as usize;
            self.grow(target, BUDGET, t as f32);
        }
        if t < APPEAR || t >= gone {
            draw_letters(layout, grid, fade);
            return;
        }
        let (shown, glitter) = if t < grown {
            (self.order.len(), 0.0)
        } else if t < melt {
            let along = ((t - grown) / GLITTER) as f32;
            (
                self.order.len(),
                0.02 * (along * std::f32::consts::PI).sin(),
            )
        } else {
            let along = smoothstep(((t - melt) / MELT) as f32);
            (((1.0 - along) * self.order.len() as f32) as usize, 0.0)
        };
        self.draw(grid, shown, t as f32, glitter, fade);
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + GROW * 0.6
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn run(frost: &mut Frost, layout: &Layout, until: f64) -> Grid {
        let mut elapsed = 0.0;
        let mut grid = composed(frost, layout, 0.0, false);
        while elapsed <= until {
            grid = composed(frost, layout, elapsed, false);
            elapsed += 1.0 / HZ;
        }
        grid
    }

    #[test]
    fn frost_branches_out_of_the_letters_and_melts_back() {
        let layout = layout(1280, 800).unwrap();
        let mut frost = Frost::default();
        frost.reset(&layout, 3);
        let grown = run(&mut frost, &layout, APPEAR + GROW);
        assert!(
            frost.order.len() > frost.most() * 3 / 4,
            "only {} of {} froze",
            frost.order.len(),
            frost.most()
        );
        assert!(drawn(&grown) > layout.letters.len() * 3);
        // Every dot of ice froze next to ice that was there before it.
        for (k, &i) in frost.order.iter().enumerate() {
            let (x, y) = ((i as usize % frost.w) as i32, (i as usize / frost.w) as i32);
            let older = [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (-1, 1),
                (1, -1),
                (-1, -1),
            ]
            .iter()
            .any(|(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < frost.w && (ny as usize) < frost.h && {
                    let ice = frost.ice[ny as usize * frost.w + nx as usize];
                    ice == SEED || (ice >= 2 && (ice - 2) < k as u32)
                }
            });
            assert!(older, "dot {k} froze on to nothing");
        }
        let home = run(&mut frost, &layout, TURN - GONE - REST / 2.0);
        assert_eq!(at_home(&home, &layout), layout.letters.len());
    }

    #[test]
    fn reduced_motion_shows_the_same_frost_each_time() {
        let layout = layout(1280, 800).unwrap();
        let mut frost = Frost::default();
        frost.reset(&layout, 5);
        let first = composed(&mut frost, &layout, 0.0, true);
        let second = composed(&mut frost, &layout, 9.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > layout.letters.len() * 3);
    }
}
