//! `attractor`: the logo unravels into a strange attractor, which slowly changes shape, and then
//! winds back up into the letters.
//!
//! The picture is drawn in Braille dots, two across and four down a cell, so it has four times the
//! grid's detail. Every dot of the logo is a particle. At the start of a turn the particles let go
//! of their letters, left to right, and fall into the orbit of a Clifford attractor:
//!
//! ```text
//! x' = sin(a·y) + c·cos(a·x)
//! y' = sin(b·x) + d·cos(b·y)
//! ```
//!
//! While they are in it, one long orbit is traced tens of thousands of points a step into a light
//! buffer that fades, so the attractor shows as a fine veil of dots, densest where the orbit
//! returns most often. Its four numbers wander slowly, and the veil folds, splits and rejoins as
//! they do. At the end of the turn the particles find their letters again.

use super::{
    AMBER, BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, braille, gradient, hash01, mix,
};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const UNWIND: f64 = 4.0;
const HOLD: f64 = 19.0;
const WIND: f64 = 4.0;
const REST: f64 = 2.0;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + UNWIND + HOLD + WIND + REST + GONE;

/// Steps a second.
const HZ: f64 = 20.0;

/// Orbit points traced into the light each step, and how much of the light survives a step.
const TRACE: usize = 26_000;
const DECAY: f32 = 0.78;
/// Light a dot needs to show.
const SHOWS: f32 = 0.55;

/// The attractors a turn can start from, as (a, b, c, d): each is a different family of shapes.
const SHAPES: [[f32; 4]; 5] = [
    [-1.4, 1.6, 1.0, 0.7],
    [1.7, 1.7, 0.6, 1.2],
    [-1.7, 1.3, -0.1, -1.2],
    [-1.8, -2.0, -0.5, -0.9],
    [1.5, -1.8, 1.6, 0.9],
];

/// How far each number wanders from where the turn started, and how fast, in radians a second.
const WANDER: f32 = 0.16;
const WANDER_SPEED: [f32; 4] = [0.071, 0.053, 0.089, 0.061];

/// Radians a second the whole attractor turns.
const SPIN: f32 = 0.035;

/// The shape at `t` seconds into a turn that started from `base`.
fn shape(base: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|k| base[k] + WANDER * (WANDER_SPEED[k] * t + k as f32 * 1.7).sin())
}

fn step(p: (f32, f32), [a, b, c, d]: [f32; 4]) -> (f32, f32) {
    (
        (a * p.1).sin() + c * (a * p.0).cos(),
        (b * p.0).sin() + d * (b * p.1).cos(),
    )
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy)]
struct Particle {
    /// Its dot in the logo.
    home: (f32, f32),
    /// Its colour there.
    rgb: [f32; 3],
    /// Where it is in the attractor's own coordinates.
    orbit: (f32, f32),
    /// How far across the logo it sits, 0 to 1, which is when it lets go.
    across: f32,
}

#[derive(Default)]
pub struct Attractor {
    /// Dots across and down.
    w: usize,
    h: usize,
    light: Vec<f32>,
    particles: Vec<Particle>,
    /// The long orbit traced into the light.
    orbit: (f32, f32),
    seed: u64,
    turn: Option<u64>,
    base: [f32; 4],
    /// Each cell's lit dots, and the light and colour gathered in it this step.
    dots: Vec<u8>,
    glow: Vec<f32>,
    tint: Vec<[f32; 4]>,
}

impl Attractor {
    fn start(&mut self, layout: &Layout, turn: u64) {
        let (cols, rows) = (layout.cols.max(1) as usize, layout.rows.max(1) as usize);
        self.w = cols * 2;
        self.h = rows * 4;
        self.light.clear();
        self.light.resize(self.w * self.h, 0.0);
        let pick = hash01(self.seed ^ turn.wrapping_mul(0x2545_f491_4f6c_dd1d));
        self.base = SHAPES[((pick * SHAPES.len() as f32) as usize).min(SHAPES.len() - 1)];
        self.orbit = (0.1, 0.1);

        // A particle for every dot of every letter.
        let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
        self.particles.clear();
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw as f32;
            let rgb = gradient(across);
            for down in 0..4 {
                for side in 0..2 {
                    let on = match letter.ch {
                        '▀' => down < 2,
                        '▄' => down >= 2,
                        '▌' => side == 0,
                        '▐' => side == 1,
                        _ => true,
                    };
                    if !on {
                        continue;
                    }
                    let home = (
                        (letter.col * 2 + side) as f32,
                        (letter.row * 4 + down) as f32,
                    );
                    let n = self.particles.len() as u64;
                    self.particles.push(Particle {
                        home,
                        rgb,
                        orbit: (
                            hash01(n ^ self.seed) * 2.0 - 1.0,
                            hash01(n.wrapping_mul(31) ^ self.seed) * 2.0 - 1.0,
                        ),
                        across,
                    });
                }
            }
        }
        self.turn = Some(turn);
    }

    /// Maps the attractor's coordinates to dots: centred, turned by `angle`, and scaled so the
    /// furthest the turn's shape can reach fits the screen's height.
    fn placer(&self, angle: f32) -> impl Fn((f32, f32)) -> (f32, f32) + use<> {
        let reach = ((1.0 + WANDER + self.base[2].abs()).powi(2)
            + (1.0 + WANDER + self.base[3].abs()).powi(2))
        .sqrt();
        // Stretched a little across, since the screen is wider than it is tall.
        let scale_y = (self.h as f32 * 0.5).min(self.w as f32 * 0.5) * 0.98 / reach;
        let scale_x = (scale_y * 1.4).min(self.w as f32 * 0.49 / reach);
        let centre = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        let (sin, cos) = angle.sin_cos();
        move |(x, y)| {
            (
                centre.0 + (x * cos - y * sin) * scale_x,
                centre.1 + (x * sin + y * cos) * scale_y,
            )
        }
    }

    fn add_light(&mut self, (x, y): (f32, f32), amount: f32) {
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (col, row) = (x as usize, y as usize);
        if col < self.w && row < self.h {
            self.light[row * self.w + col] += amount;
        }
    }

    /// One step, `t` seconds into the turn; `still` runs the tracing long enough for a full
    /// picture in one go.
    fn advance(&mut self, t: f32, still: bool) {
        let now = shape(self.base, t);
        let place = self.placer(t * SPIN);
        // How far out of their letters the particles are, as a whole.
        let out = attractor_share(t as f64);

        if still {
            self.light.iter_mut().for_each(|l| *l = 0.0);
        } else {
            self.light.iter_mut().for_each(|l| *l *= DECAY);
        }
        if out > 0.0 {
            let traces = if still { TRACE * 6 } else { TRACE };
            let amount = if still { 0.25 } else { 1.0 } * ease(out * 1.4 - 0.2);
            let mut p = self.orbit;
            for _ in 0..traces {
                p = step(p, now);
                self.add_light(place(p), amount);
            }
            self.orbit = p;
        }
        for particle in &mut self.particles {
            particle.orbit = step(particle.orbit, now);
        }
    }

    fn draw(&mut self, layout: &Layout, grid: &mut Grid, t: f32, fade: f32) {
        let (cols, rows) = (layout.cols.max(0) as usize, layout.rows.max(0) as usize);
        let cells = cols * rows;
        self.dots.clear();
        self.dots.resize(cells, 0);
        self.glow.clear();
        self.glow.resize(cells, 0.0);
        self.tint.clear();
        self.tint.resize(cells, [0.0; 4]);

        let w = self.w;
        // Dots are stippled by how often the orbit passes them against the average: a dot shows
        // when its light beats its own steady random share of that, so a dense attractor is
        // shaded by its density rather than filled solid.
        let (sum, lit) = self
            .light
            .iter()
            .filter(|&&l| l >= SHOWS)
            .fold((0.0, 0usize), |(sum, n), &l| (sum + l, n + 1));
        let mean = if lit > 0 { sum / lit as f32 } else { 0.0 };
        for (i, &light) in self.light.iter().enumerate() {
            if light < SHOWS || light < mean * (0.15 + 1.2 * hash01(i as u64 ^ 0xd07)) {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let cell = (y / 4) * cols + x / 2;
            if cell < cells {
                self.dots[cell] |= 1 << ((y % 4) * 2 + x % 2);
                self.glow[cell] += light;
            }
        }

        let out = attractor_share(t as f64);
        let place = self.placer(t * SPIN);
        for particle in &self.particles {
            // The left of the logo lets go first and comes home first.
            let own = ease(out * 1.8 - particle.across * 0.8);
            let (x, y) = if own <= 0.0 {
                particle.home
            } else {
                let there = place(particle.orbit);
                (
                    particle.home.0 + (there.0 - particle.home.0) * own,
                    particle.home.1 + (there.1 - particle.home.1) * own,
                )
            };
            if x < 0.0 || y < 0.0 {
                continue;
            }
            let (x, y) = (x as usize, y as usize);
            if x >= cols * 2 || y >= rows * 4 {
                continue;
            }
            let cell = (y / 4) * cols + x / 2;
            self.dots[cell] |= 1 << ((y % 4) * 2 + x % 2);
            let tint = &mut self.tint[cell];
            for (sum, channel) in tint.iter_mut().zip(particle.rgb) {
                *sum += channel;
            }
            tint[3] += 1.0;
        }

        for row in 0..rows {
            for col in 0..cols {
                let cell = row * cols + col;
                let dots = self.dots[cell];
                if dots == 0 {
                    continue;
                }
                // The veil: cyan where the orbit passes rarely, through mint to amber and nearly
                // white where it keeps coming back.
                let dense = (self.glow[cell] / 24.0).ln_1p() / 1.5;
                let veil = if dense < 0.5 {
                    mix(CYAN, MINT, dense * 2.0)
                } else {
                    mix(
                        mix(MINT, AMBER, (dense - 0.5) * 2.0),
                        WHITE,
                        (dense - 1.0).max(0.0),
                    )
                };
                let tint = self.tint[cell];
                let rgb = if tint[3] > 0.0 {
                    let own = [tint[0] / tint[3], tint[1] / tint[3], tint[2] / tint[3]];
                    mix(veil, own, (tint[3] / 3.0).min(1.0))
                } else {
                    veil
                };
                grid.put(col as f32, row as f32, braille(dots), mix(BG, rgb, fade));
            }
        }
    }
}

/// How far into the attractor the turn is at `t`: 0 while the logo stands, rising through the
/// unwinding, 1 while it holds, and falling as it winds back.
fn attractor_share(t: f64) -> f32 {
    if t < APPEAR {
        0.0
    } else if t < APPEAR + UNWIND {
        ((t - APPEAR) / UNWIND) as f32
    } else if t < APPEAR + UNWIND + HOLD {
        1.0
    } else if t < APPEAR + UNWIND + HOLD + WIND {
        1.0 - ((t - APPEAR - UNWIND - HOLD) / WIND) as f32
    } else {
        0.0
    }
}

impl Variation for Attractor {
    fn id(&self) -> &'static str {
        "attractor"
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
        let (w, h) = (
            layout.cols.max(1) as usize * 2,
            layout.rows.max(1) as usize * 4,
        );
        if frame.still {
            // The attractor in full, with no letters: the one picture that says what this is.
            let t = (APPEAR + UNWIND + HOLD / 2.0) as f32;
            if self.turn != Some(0) || self.w != w || self.h != h {
                self.start(layout, 0);
            }
            self.orbit = (0.1, 0.1);
            self.advance(t, true);
            // The particles are left out: they only settle into the orbit by being stepped.
            let particles = std::mem::take(&mut self.particles);
            self.draw(layout, grid, t, 1.0);
            self.particles = particles;
            return;
        }
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn) || self.w != w || self.h != h {
            self.start(layout, turn);
        }
        self.advance(t as f32, false);
        let fade = if t < APPEAR {
            (t / APPEAR) as f32
        } else if t > TURN - GONE {
            ((TURN - t) / GONE) as f32
        } else {
            1.0
        };
        let resting = attractor_share(t) <= 0.0;
        if resting {
            // Home: the letters in their own blocks, crisper than their dots.
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
            // What is left of the veil fades out round them.
            self.particles_hidden(layout, grid, t as f32, fade);
        } else {
            self.draw(layout, grid, t as f32, fade);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + UNWIND * 0.6
    }
}

impl Attractor {
    /// Draws the fading veil alone, leaving the letters' cells to the letters.
    fn particles_hidden(&mut self, layout: &Layout, grid: &mut Grid, t: f32, fade: f32) {
        let mut letter = vec![false; grid.cells.len()];
        for l in &layout.letters {
            if let Some(cell) = letter.get_mut((l.row.max(0) * layout.cols + l.col.max(0)) as usize)
            {
                *cell = true;
            }
        }
        let saved = std::mem::take(&mut self.particles);
        let mut veil = Grid::new(layout.cols, layout.rows);
        self.draw(layout, &mut veil, t, fade);
        self.particles = saved;
        for (i, cell) in veil.cells.iter().enumerate() {
            if cell.ch != ' ' && !letter[i] {
                grid.cells[i] = *cell;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        Readings, layout,
        tests::{at_home, drawn},
    };

    fn run(until: f64) -> (Grid, Layout) {
        let layout = layout(1280, 800).unwrap();
        let mut attractor = Attractor::default();
        attractor.reset(&layout, 3);
        let mut grid = Grid::new(layout.cols, layout.rows);
        let readings = Readings::default();
        let mut elapsed = 0.0;
        while elapsed <= until {
            grid.blank(layout.cols, layout.rows);
            attractor.compose(
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
    fn the_logo_stands_then_unravels_and_comes_home_again() {
        let (standing, layout) = run(APPEAR - 0.1);
        assert_eq!(at_home(&standing, &layout), layout.letters.len());
        let (unravelled, _) = run(APPEAR + UNWIND + 3.0);
        assert!(at_home(&unravelled, &layout) < layout.letters.len() / 10);
        assert!(drawn(&unravelled) > layout.letters.len());
        let (home, _) = run(APPEAR + UNWIND + HOLD + WIND + REST / 2.0);
        assert_eq!(at_home(&home, &layout), layout.letters.len());
    }

    #[test]
    fn the_attractor_changes_shape_as_it_holds() {
        let (early, _) = run(APPEAR + UNWIND + 1.0);
        let (later, _) = run(APPEAR + UNWIND + 9.0);
        let differ = early
            .cells
            .iter()
            .zip(&later.cells)
            .filter(|(a, b)| a.ch != b.ch)
            .count();
        assert!(differ > drawn(&later) / 3, "{differ} cells changed");
    }

    #[test]
    fn braille_patterns_put_their_dots_where_they_are_asked() {
        assert_eq!(braille(0), '\u{2800}');
        assert_eq!(braille(0b1), '⠁');
        assert_eq!(braille(0b10), '⠈');
        assert_eq!(braille(0b1100_0000), '⣀');
        assert_eq!(braille(0xff), '⣿');
    }

    #[test]
    fn reduced_motion_shows_the_attractor_still() {
        let layout = layout(1280, 800).unwrap();
        let mut attractor = Attractor::default();
        attractor.reset(&layout, 3);
        let first = crate::saver::tests::composed(&mut attractor, &layout, 0.0, true);
        let second = crate::saver::tests::composed(&mut attractor, &layout, 5.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > layout.letters.len());
    }
}
