//! `life`: Conway's Game of Life, seeded with the logo.
//!
//! The logo is set out as living cells and then left to the rules: it holds for a moment, breaks
//! up, throws gliders off across the screen and settles into whatever it settles into. Then the
//! board fades and the logo seeds it again. Cells still standing where the logo was keep its
//! colours, so its shape shows for as long as the rules leave it alone.
//!
//! Every living cell also glows into a feedback buffer, and each turn carries that glow through
//! its own motion, the way a MilkDrop preset does: blooming out from the middle, draining into a
//! whirlpool, rising like warm liquid, or thrown out as a kaleidoscope. The glow's colour travels
//! round a wider loop than the rest of the saver's (amber, rose, violet, cyan, mint) in bands
//! that spread out from the middle, and the whole picture throbs on a slow beat. A glider leaves
//! a smoke trail that curls away behind it, and a still life smoulders.

use super::{
    AMBER, BG, CYAN, Feedback, Frame, Grid, Layout, MINT, Motion, Variation, WHITE, gradient,
    hash01, mix,
};

/// One turn, in seconds of the animation clock.
const SEED: f64 = 1.6;
const RUN: f64 = 11.0;
const FADE: f64 = 1.6;
const GAP: f64 = 1.0;
const TURN: f64 = SEED + RUN + FADE + GAP;

/// Generations a second. Slower than the screen so that each one can be read.
const GENERATIONS: f64 = 10.0;

/// How many R-pentominoes are sown around the logo to start with, so no two turns run the same
/// way. A cell on its own would simply die, so the scatter is made of these.
const SPARKS: u64 = 8;

/// How much of the glow survives each step: a little over a second from full to the floor.
const DECAY: f32 = 0.9;
/// Glow below this is drawn as nothing.
const FLOOR: f32 = 0.15;
/// Three levels of glow as shades, under the solid living cells, in eight colours round the loop.
const LOOKS: [char; 3] = ['░', '▒', '▓'];
const HUES: u8 = 8;

/// Seconds between the beats the picture throbs to: a kick in the zoom and in the glow.
const BEAT: f32 = 1.75;

/// How far the hue bands are apart, in cells from the middle, and how long the colours take to go
/// once round the loop.
const BAND: f32 = 70.0;
const HUE_LOOP: f32 = 18.0;

const ROSE: [f32; 3] = [1.0, 0.32, 0.62];
const VIOLET: [f32; 3] = [0.62, 0.42, 1.0];

/// The glow's colour loop: amber, rose, violet, cyan, mint and back to amber, `at` going round
/// once from 0 to 1.
fn trip(at: f32) -> [f32; 3] {
    const LOOP: [[f32; 3]; 5] = [AMBER, ROSE, VIOLET, CYAN, MINT];
    let at = at.rem_euclid(1.0) * LOOP.len() as f32;
    let from = at.floor() as usize % LOOP.len();
    mix(LOOP[from], LOOP[(from + 1) % LOOP.len()], at.fract())
}

/// The ways a turn can carry the glow, one to a turn, like a MilkDrop preset.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Preset {
    /// Out from the middle, turning gently one way and back.
    Bloom,
    /// Drawn in towards the middle, spinning.
    Whirlpool,
    /// Rising and wobbling, like heat over a road.
    Liquid,
    /// Out and round, with every cell's glow mirrored into all four quarters.
    Kaleidoscope,
}

const PRESETS: [Preset; 4] = [
    Preset::Bloom,
    Preset::Whirlpool,
    Preset::Liquid,
    Preset::Kaleidoscope,
];

impl Preset {
    /// How the glow moves at time `t`, with `kick` the beat's strength, 1 on the beat and falling.
    fn motion(self, centre: (f32, f32), t: f32, kick: f32) -> Motion {
        let (zoom, turn, shift) = match self {
            Preset::Bloom => (
                1.03 + 0.01 * (0.5 * t).sin(),
                1.4 * (0.21 * t).sin(),
                (0.0, 0.0),
            ),
            Preset::Whirlpool => (0.965, 3.0 + 1.0 * (0.3 * t).sin(), (0.0, 0.0)),
            Preset::Liquid => (1.004, 0.0, (0.25 * (0.4 * t).sin(), -0.45)),
            Preset::Kaleidoscope => (1.022, -1.8 * (0.17 * t + 0.6).cos(), (0.0, 0.0)),
        };
        Motion {
            centre,
            zoom: zoom + 0.035 * kick,
            turn: turn.to_radians(),
            shift,
        }
    }

    /// How far the glow is pushed about, per cell, on top of the motion: its warp.
    fn warp(self) -> f32 {
        match self {
            Preset::Bloom => 0.45,
            Preset::Whirlpool => 0.3,
            Preset::Liquid => 1.1,
            Preset::Kaleidoscope => 0.5,
        }
    }
}

/// The beat's kick: 1 on the beat, falling away fast.
fn kick(t: f32) -> f32 {
    (1.0 - (t / BEAT).fract()).powi(6)
}

/// An R-pentomino: five cells that run for over a thousand generations and throw gliders across
/// the board. Below `SPARSE` living cells another is dropped in, no more often than every
/// `SPARK_EVERY` generations, because Conway's rules settle into still lifes and blinkers within
/// a few hundred and a settled board is nothing to watch.
const PENTOMINO: [(i32, i32); 5] = [(1, 0), (2, 0), (0, 1), (1, 1), (1, 2)];
const SPARSE: usize = 400;
const SPARK_EVERY: u32 = 12;

#[derive(Default)]
pub struct Life {
    cols: i32,
    rows: i32,
    /// The living cells, and how many generations each has survived (255 once it stops counting).
    alive: Vec<bool>,
    age: Vec<u8>,
    /// Where the logo's letters are, and which character each was, for colouring what survives.
    logo: Vec<Option<char>>,
    next: Vec<bool>,
    /// The glow the living cells leave, carried and faded each step.
    glow: Feedback,
    /// How this turn carries the glow.
    preset: Option<Preset>,
    /// The middle of the logo, which the glow turns about.
    centre: (f32, f32),
    /// How many cells are alive, and how many generations since the last spark, so a board that
    /// has settled is given something to do without being flooded.
    population: usize,
    since_spark: u32,
    /// Which turn the board was seeded for, so a new turn seeds it again.
    turn_at: i64,
    seed: u64,
    /// Whether the last step was the half-way step between two generations.
    halfway: bool,
}

impl Life {
    fn at(&self, col: i32, row: i32) -> bool {
        (0..self.cols).contains(&col)
            && (0..self.rows).contains(&row)
            && self.alive[(row * self.cols + col) as usize]
    }

    fn neighbours(&self, col: i32, row: i32) -> usize {
        let mut count = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy) != (0, 0) && self.at(col + dx, row + dy) {
                    count += 1;
                }
            }
        }
        count
    }

    /// The logo, plus a scatter of cells that differs from turn to turn.
    fn sow(&mut self, seed: u64) {
        self.alive.clear();
        self.alive
            .resize((self.cols * self.rows).max(0) as usize, false);
        self.age.clear();
        self.age.resize(self.alive.len(), 0);
        self.glow.reset(self.cols, self.rows);
        for (i, letter) in self.logo.iter().enumerate() {
            if letter.is_some() {
                self.alive[i] = true;
            }
        }
        self.population = self.alive.iter().filter(|alive| **alive).count();
        self.since_spark = 0;
        for spark in 0..SPARKS {
            self.spark(seed ^ spark.wrapping_mul(0x2545_f491));
        }
    }

    /// Drops an R-pentomino well inside the edges of the board.
    fn spark(&mut self, seed: u64) {
        self.since_spark = 0;
        let col = 3 + (hash01(seed ^ 0x5a5a) * (self.cols - 8).max(1) as f32) as i32;
        let row = 3 + (hash01(seed ^ 0xa5a5) * (self.rows - 8).max(1) as f32) as i32;
        for (dx, dy) in PENTOMINO {
            let (col, row) = (col + dx, row + dy);
            if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
                let i = (row * self.cols + col) as usize;
                self.alive[i] = true;
                self.age[i] = 0;
                self.population += 1;
            }
        }
    }

    /// One generation: three neighbours give birth, two or three carry on, everything else dies.
    fn generation(&mut self) {
        self.next.clear();
        self.next.resize(self.alive.len(), false);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let i = (row * self.cols + col) as usize;
                let neighbours = self.neighbours(col, row);
                let alive = matches!((self.alive[i], neighbours), (true, 2 | 3) | (false, 3));
                self.next[i] = alive;
                self.age[i] = if !alive {
                    0
                } else if self.alive[i] {
                    self.age[i].saturating_add(1)
                } else {
                    0
                };
            }
        }
        std::mem::swap(&mut self.alive, &mut self.next);
        self.population = self.alive.iter().filter(|alive| **alive).count();
        self.since_spark = self.since_spark.saturating_add(1);
    }

    /// The hue of the glow at (`x`, `y`) at time `t`, as the byte a `Feedback` stores: bands
    /// spreading out from the middle while the colours go round.
    fn hue(&self, x: f32, y: f32, t: f32) -> f32 {
        let (cx, cy) = self.centre;
        let distance = (x - cx).hypot((y - cy) * 2.0);
        t / HUE_LOOP - distance / BAND
    }

    /// One step of the glow at time `t`: last step's carried by the preset's motion and warp and
    /// faded, then every living cell glowing into it at `strength`.
    fn flow(&mut self, t: f32, strength: f32) {
        let preset = self.preset.unwrap_or(Preset::Bloom);
        let beat = kick(t);
        let motion = preset.motion(self.centre, t, beat);
        let warp = preset.warp();
        self.glow.advance(DECAY, |x, y| {
            let (sx, sy) = motion.source(x, y);
            (
                sx + warp * (0.17 * y + 1.3 * t).sin(),
                sy + 0.5 * warp * (0.09 * x - 0.9 * t).cos(),
            )
        });
        if strength <= 0.0 {
            return;
        }
        let (cx, cy) = self.centre;
        let mirror = preset == Preset::Kaleidoscope;
        let lift = 0.8 + 0.2 * beat;
        for row in 0..self.rows {
            for col in 0..self.cols {
                let i = (row * self.cols + col) as usize;
                if !self.alive[i] {
                    continue;
                }
                // Something new glows brightest; a still life only smoulders, so the trails are
                // drawn by what moves.
                let light = match self.age[i] {
                    0 => 1.0,
                    1..=8 => 0.8,
                    _ => 0.3,
                } * strength
                    * lift;
                let (x, y) = (col as f32, row as f32);
                let hue = super::hue_byte(self.hue(x, y, t));
                self.glow.ink(x, y, light, hue);
                if mirror {
                    // The reflections a little dimmer than the cell, or four times the glow
                    // floods the screen.
                    let (mx, my, echo) = (2.0 * cx - x, 2.0 * cy - y, 0.7 * light);
                    self.glow.ink(mx, y, echo, hue);
                    self.glow.ink(x, my, echo, hue);
                    self.glow.ink(mx, my, echo, hue);
                }
            }
        }
    }
}

impl Variation for Life {
    fn id(&self) -> &'static str {
        "life"
    }

    fn reset(&mut self, layout: &Layout, seed: u64) {
        self.cols = layout.cols;
        self.rows = layout.rows;
        self.logo = vec![None; (layout.cols * layout.rows).max(0) as usize];
        for letter in &layout.letters {
            if (0..layout.cols).contains(&letter.col) && (0..layout.rows).contains(&letter.row) {
                self.logo[(letter.row * layout.cols + letter.col) as usize] = Some(letter.ch);
            }
        }
        let (lx, ly, lw, lh) = layout.logo;
        self.centre = (lx as f32 + lw as f32 / 2.0, ly as f32 + lh as f32 / 2.0);
        self.seed = seed;
        self.turn_at = i64::MIN;
        self.sow(seed);
    }

    /// Two steps to a generation, and the glow moves on both. At 20 steps a second the saver
    /// doesn't cross-fade between steps, which at 10 it would do on every frame for every cell
    /// that changed; the glow is the fade instead, and much cheaper.
    fn step_hz(&self) -> f64 {
        GENERATIONS * 2.0
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        if frame.still {
            // The seed itself, standing still: the logo as living cells.
            let (lx, _, lw, _) = layout.logo;
            for letter in &layout.letters {
                let across = (letter.col - lx) as f32 / lw.max(1) as f32;
                grid.put(letter.col as f32, letter.row as f32, '█', gradient(across));
            }
            return;
        }
        let turn = (frame.elapsed / TURN).floor() as i64;
        if turn != self.turn_at {
            self.turn_at = turn;
            let seed = self.seed ^ (turn as u64).wrapping_mul(0x9e37_79b9);
            self.sow(seed);
            self.preset = Some(PRESETS[(self.seed.wrapping_add(turn as u64) % 4) as usize]);
        }
        let t = frame.elapsed.rem_euclid(TURN);
        if t >= SEED + RUN + FADE {
            return;
        }
        // The board dims away at the end of its turn rather than blinking out.
        let light = if t < SEED + RUN {
            1.0
        } else {
            1.0 - ((t - SEED - RUN) / FADE) as f32
        };
        let clock = frame.elapsed as f32;
        if frame.dt > 0.0 {
            self.halfway = !self.halfway;
            if !self.halfway && (SEED..SEED + RUN).contains(&t) {
                self.generation();
                if self.population < SPARSE && self.since_spark >= SPARK_EVERY {
                    self.spark(self.seed ^ (frame.elapsed * 1000.0) as u64);
                }
            }
            self.flow(clock, light * light);
        }
        // The glow, the faint levels dimmer as well as sparser so the smoke stays behind windows.
        self.glow.draw(grid, FLOOR, &LOOKS, |hue, level| {
            let steps = HUES as u16;
            let at = (hue as u16 * steps / 256) as f32 / steps as f32;
            mix(BG, trip(at), [0.45, 0.7, 0.95][level] * light)
        });
        let (lx, _, lw, _) = layout.logo;
        // The logo behind the board, faint: what the colony grew out of.
        for letter in &layout.letters {
            if self.glow_at(letter.col, letter.row) >= FLOOR {
                continue;
            }
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, gradient(across), 0.16 * light),
            );
        }
        for row in 0..self.rows.min(layout.rows) {
            for col in 0..self.cols.min(layout.cols) {
                let i = (row * self.cols + col) as usize;
                if !self.alive[i] {
                    continue;
                }
                let age = self.age[i];
                // A cell that has stood where the logo was keeps the logo's colour. The rest take
                // the glow's colour where they stand, white the generation they are born.
                let rgb = if self.logo[i].is_some() {
                    gradient((col - lx) as f32 / lw.max(1) as f32)
                } else if age == 0 {
                    WHITE
                } else {
                    mix(trip(self.hue(col as f32, row as f32, clock)), WHITE, 0.25)
                };
                // Solid blocks, so a colony reads as cells rather than as a smudge.
                grid.put(col as f32, row as f32, '█', mix(BG, rgb, light));
            }
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }
}

impl Life {
    fn glow_at(&self, col: i32, row: i32) -> f32 {
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            self.glow.light[(row * self.cols + col) as usize]
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{layout, tests::drawn};

    /// Runs a variation from the start of its turn to `elapsed`, one step at a time, the way the
    /// saver does, and gives back the last grid: `life` carries its board between steps.
    fn run_to(elapsed: f64) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut life = Life::default();
        life.reset(&layout, 7);
        let mut grid = Grid::new(layout.cols, layout.rows);
        let step = 1.0 / (GENERATIONS * 2.0);
        let mut at = 0.0;
        while at <= elapsed {
            grid = Grid::new(layout.cols, layout.rows);
            life.compose(
                Frame {
                    layout: &layout,
                    elapsed: at,
                    dt: step as f32,
                    still: false,
                    readings: &Default::default(),
                    local: None,
                },
                &mut grid,
            );
            at += step;
        }
        (grid, layout)
    }

    #[test]
    fn the_logo_seeds_the_board_and_the_rules_take_it_apart() {
        let (seeded, layout) = run_to(SEED * 0.5);
        let logo_cells = layout
            .letters
            .iter()
            .filter(|letter| seeded.at(letter.col as f32, letter.row as f32).ch != ' ')
            .count();
        assert_eq!(
            logo_cells,
            layout.letters.len(),
            "the logo is alive to begin with"
        );
        let (running, _) = run_to(SEED + 4.0);
        assert_ne!(running.cells, seeded.cells, "the board has moved on");
        assert!(drawn(&running) > 0, "something is still alive");
    }

    #[test]
    fn every_preset_carries_a_cells_glow_away_from_it() {
        let layout = layout(1920, 1200).unwrap();
        for preset in PRESETS {
            let mut life = Life::default();
            life.reset(&layout, 7);
            life.preset = Some(preset);
            life.alive.iter_mut().for_each(|alive| *alive = false);
            // A cell off the middle, so a zoom about the middle moves it, and near enough that the
            // beat's kick doesn't throw it off the screen.
            let (col, row) = (life.centre.0 as i32 - 24, life.centre.1 as i32 - 6);
            let home = (row * life.cols + col) as usize;
            life.alive[home] = true;
            life.flow(0.0, 1.0);
            life.alive[home] = false;
            for step in 1..8 {
                life.flow(step as f32 / 20.0, 1.0);
            }
            let lit: Vec<usize> = (0..life.glow.light.len())
                .filter(|&i| life.glow.light[i] > 0.01)
                .collect();
            assert!(!lit.is_empty(), "{preset:?}: the glow lasts");
            assert!(
                lit.iter().any(|&i| i != home),
                "{preset:?}: the glow has moved off the cell"
            );
            if preset == Preset::Kaleidoscope {
                let (cx, cy) = life.centre;
                let far = lit
                    .iter()
                    .filter(|&&i| {
                        let (c, r) = ((i as i32 % life.cols) as f32, (i as i32 / life.cols) as f32);
                        c > cx && r > cy
                    })
                    .count();
                assert!(far > 0, "the kaleidoscope mirrors it into the far quarter");
            }
        }
    }

    #[test]
    fn a_colony_leaves_glow_where_nothing_is_alive() {
        let (running, _) = run_to(SEED + 4.0);
        let smoke = running
            .cells
            .iter()
            .filter(|cell| LOOKS.contains(&cell.ch))
            .count();
        assert!(smoke > 50, "trails behind the colony: {smoke} cells");
    }

    #[test]
    fn the_board_fades_and_the_turn_ends_empty() {
        let (gap, _) = run_to(TURN - 0.1);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn a_new_turn_seeds_the_logo_again() {
        let (again, layout) = run_to(TURN + SEED * 0.5);
        let logo_cells = layout
            .letters
            .iter()
            .filter(|letter| again.at(letter.col as f32, letter.row as f32).ch != ' ')
            .count();
        assert_eq!(logo_cells, layout.letters.len());
    }

    #[test]
    fn reduced_motion_shows_the_seed_and_nothing_else() {
        let layout = layout(1920, 1200).unwrap();
        let mut life = Life::default();
        life.reset(&layout, 1);
        let still = |life: &mut Life, elapsed: f64| {
            let mut grid = Grid::new(layout.cols, layout.rows);
            life.compose(
                Frame {
                    layout: &layout,
                    elapsed,
                    dt: 0.0,
                    still: true,
                    readings: &Default::default(),
                    local: None,
                },
                &mut grid,
            );
            grid
        };
        let first = still(&mut life, 0.0);
        let second = still(&mut life, 30.0);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(drawn(&first), layout.letters.len());
    }
}
