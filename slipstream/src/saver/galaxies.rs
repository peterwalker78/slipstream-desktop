//! `galaxies`: the two halves of the logo wind up into spiral galaxies, fall together, fling out
//! tidal tails and merge; then the whole collision runs backwards and the letters come together
//! again.
//!
//! Drawn in Braille dots. Each half of the logo turns about a heavy core at its middle, and every
//! dot of its letters becomes a handful of stars on a circular orbit round that core. Inner stars
//! go round faster than outer ones, so the letters shear into spiral arms by themselves. The cores
//! fall towards each other on a wide orbit, slowed by a drag while they are close (the dynamical
//! friction a real galaxy's dark halo gives), so they swing past, fall back and merge.
//!
//! The stars feel the cores and not each other, and are stepped with leapfrog, which is exactly
//! reversible: a step taken backwards undoes a step taken forwards to within rounding. The cores'
//! path is worked out in advance, so running the stars backwards through it brings every one
//! back to its own dot of its own letter, however far it was flung.

use super::{
    Frame, Grid, Layout, Stipple, Variation, draw_letters, gradient, hash01, logo_dots, smoothstep,
};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const HOLD: f64 = 0.8;
const COLLIDE: f64 = 22.0;
const PAUSE: f64 = 0.6;
const REWIND: f64 = 6.0;
const REST: f64 = 2.0;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + HOLD + COLLIDE + PAUSE + REWIND + REST + GONE;

/// Steps a second drawn, and simulation steps a second of collision.
const HZ: f64 = 30.0;
const SIM_HZ: f64 = 60.0;

/// Stars for each dot of the logo.
const STARS_PER_DOT: usize = 4;

/// The physics, in units of a logo 242 dots wide (a 1920-pixel screen): each core's mass times the
/// gravitational constant, how close to a core its pull stops growing, and how fast the cores
/// start towards each other as a share of the speed that would keep them circling.
const GM: f64 = 16_000.0;
const SOFTEN: f64 = 9.0;
const APPROACH: f64 = 0.8;
/// The drag on the cores' relative speed, and how near they are for it to act.
const DRAG: f64 = 1.0;
const DRAG_REACH: f64 = 60.0;
const DESIGN_WIDTH: f64 = 242.0;

#[derive(Clone, Copy)]
struct Star {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    /// Its pull from the cores where it is now, kept from the last step.
    ax: f64,
    ay: f64,
    rgb: [f32; 3],
}

#[derive(Default)]
pub struct Galaxies {
    seed: u64,
    turn: Option<u64>,
    cols: i32,
    rows: i32,
    stars: Vec<Star>,
    /// The cores at every simulation step, (x, y) of each.
    cores: Vec<[f64; 4]>,
    /// Which simulation step the stars are at.
    at: usize,
    /// Dots a design unit, and the logo's middle in dots.
    scale: f64,
    centre: (f64, f64),
    stipple: Stipple,
}

/// The pull of both cores at (`x`, `y`).
fn pull(x: f64, y: f64, cores: &[f64; 4]) -> (f64, f64) {
    let mut a = (0.0, 0.0);
    for core in [(cores[0], cores[1]), (cores[2], cores[3])] {
        let (dx, dy) = (x - core.0, y - core.1);
        let d2 = dx * dx + dy * dy + SOFTEN * SOFTEN;
        let f = GM / (d2 * d2.sqrt());
        a.0 -= dx * f;
        a.1 -= dy * f;
    }
    a
}

impl Galaxies {
    fn steps() -> usize {
        (COLLIDE * SIM_HZ) as usize
    }

    fn start(&mut self, layout: &Layout, turn: u64) {
        self.turn = Some(turn);
        self.cols = layout.cols;
        self.rows = layout.rows;
        self.at = 0;
        let dots = logo_dots(layout);
        let (lx, ly, lw, lh) = layout.logo;
        self.scale = (lw.max(1) * 2) as f64 / DESIGN_WIDTH;
        self.centre = ((lx * 2 + lw) as f64, (ly * 4 + lh * 2) as f64);
        let half = DESIGN_WIDTH / 4.0;

        // The cores' path, worked out once with the drag, so the stars can be stepped through
        // it either way.
        let dt = 1.0 / SIM_HZ;
        let separation = half * 2.0;
        let circling = (2.0 * GM / separation).sqrt();
        // Each turn swings the cores in on a slightly different path.
        let approach =
            APPROACH * (0.92 + 0.16 * hash01(self.seed ^ turn.wrapping_mul(0x51)) as f64);
        let mut c = [-half, 0.0, half, 0.0];
        let mut v = [
            0.0,
            -circling * approach / 2.0,
            0.0,
            circling * approach / 2.0,
        ];
        let core_pull = |c: &[f64; 4], v: &[f64; 4]| {
            let (dx, dy) = (c[0] - c[2], c[1] - c[3]);
            let d2 = dx * dx + dy * dy + SOFTEN * SOFTEN;
            let f = GM / (d2 * d2.sqrt());
            let drag = DRAG * (-d2 / (DRAG_REACH * DRAG_REACH)).exp();
            let (rvx, rvy) = (v[0] - v[2], v[1] - v[3]);
            let (ax, ay) = (-dx * f - rvx * drag / 2.0, -dy * f - rvy * drag / 2.0);
            [ax, ay, -ax, -ay]
        };
        self.cores.clear();
        self.cores.push(c);
        let initial_v = v;
        for _ in 0..Self::steps() {
            let a = core_pull(&c, &v);
            for k in 0..4 {
                v[k] += a[k] * dt / 2.0;
            }
            for k in 0..4 {
                c[k] += v[k] * dt;
            }
            let a = core_pull(&c, &v);
            for k in 0..4 {
                v[k] += a[k] * dt / 2.0;
            }
            self.cores.push(c);
        }

        // Stars on the letters, each circling its own half's core in the sense the cores circle
        // each other, and carried along with it.
        let mut rng = (self.seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;
        self.stars.clear();
        for dot in &dots {
            let rgb = gradient(dot.across);
            for _ in 0..STARS_PER_DOT {
                let x =
                    (dot.x as f64 + super::xorshift(&mut rng) as f64 - self.centre.0) / self.scale;
                let y =
                    (dot.y as f64 + super::xorshift(&mut rng) as f64 - self.centre.1) / self.scale;
                let own = if x < 0.0 { 0 } else { 2 };
                let (rx, ry) = (
                    x - c_at(&self.cores[0], own).0,
                    y - c_at(&self.cores[0], own).1,
                );
                let r2 = rx * rx + ry * ry;
                let r = r2.sqrt().max(1e-6);
                let d2 = r2 + SOFTEN * SOFTEN;
                let speed = (GM * r2 / (d2 * d2.sqrt())).sqrt();
                let (ax, ay) = pull(x, y, &self.cores[0]);
                self.stars.push(Star {
                    x,
                    y,
                    vx: -ry / r * speed + initial_v[own],
                    vy: rx / r * speed + initial_v[own + 1],
                    ax,
                    ay,
                    rgb,
                });
            }
        }
    }

    /// Steps the stars forwards or backwards until they are at simulation step `target`.
    fn go_to(&mut self, target: usize) {
        let dt = 1.0 / SIM_HZ;
        let target = target.min(self.cores.len() - 1);
        while self.at < target {
            let next = self.cores[self.at + 1];
            for star in &mut self.stars {
                star.vx += star.ax * dt / 2.0;
                star.vy += star.ay * dt / 2.0;
                star.x += star.vx * dt;
                star.y += star.vy * dt;
                (star.ax, star.ay) = pull(star.x, star.y, &next);
                star.vx += star.ax * dt / 2.0;
                star.vy += star.ay * dt / 2.0;
            }
            self.at += 1;
        }
        while self.at > target {
            let before = self.cores[self.at - 1];
            for star in &mut self.stars {
                star.vx -= star.ax * dt / 2.0;
                star.vy -= star.ay * dt / 2.0;
                star.x -= star.vx * dt;
                star.y -= star.vy * dt;
                (star.ax, star.ay) = pull(star.x, star.y, &before);
                star.vx -= star.ax * dt / 2.0;
                star.vy -= star.ay * dt / 2.0;
            }
            self.at -= 1;
        }
    }

    fn draw(&mut self, grid: &mut Grid, fade: f32) {
        let (cols, rows) = (self.cols.max(0) as usize, self.rows.max(0) as usize);
        self.stipple.clear(cols, rows);
        for star in &self.stars {
            self.stipple.add(
                (self.centre.0 + star.x * self.scale) as f32,
                (self.centre.1 + star.y * self.scale) as f32,
                star.rgb,
                1.0,
            );
        }
        // An arm holds a few stars a cell; a core holds dozens, and burns white.
        self.stipple
            .draw(grid, (STARS_PER_DOT * 5) as f32, 0.4, fade);
    }
}

fn c_at(cores: &[f64; 4], own: usize) -> (f64, f64) {
    (cores[own], cores[own + 1])
}

impl Variation for Galaxies {
    fn id(&self) -> &'static str {
        "galaxies"
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
            // The first pass: two galaxies torn into tails, in the middle of merging.
            if self.turn != Some(0) || resized {
                self.seed = 0;
                self.start(layout, 0);
            }
            self.go_to((9.0 * SIM_HZ) as usize);
            self.draw(grid, 1.0);
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
        let collide = APPEAR + HOLD;
        let rewind = collide + COLLIDE + PAUSE;
        let home = rewind + REWIND;
        let steps = Self::steps();
        let target = if t < collide {
            0
        } else if t < collide + COLLIDE {
            ((t - collide) * SIM_HZ) as usize
        } else if t < rewind {
            steps
        } else if t < home {
            // Like a tape: gathering speed, then easing into the letters.
            let along = smoothstep(((t - rewind) / REWIND) as f32) as f64;
            ((1.0 - along) * steps as f64).round() as usize
        } else {
            0
        };
        self.go_to(target);
        if self.at == 0 {
            draw_letters(layout, grid, fade);
        } else {
            self.draw(grid, fade);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + HOLD + 4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn run(galaxies: &mut Galaxies, layout: &Layout, from: f64, until: f64) -> Grid {
        let mut elapsed = from;
        let mut grid = composed(galaxies, layout, elapsed, false);
        while elapsed <= until {
            grid = composed(galaxies, layout, elapsed, false);
            elapsed += 1.0 / HZ;
        }
        grid
    }

    #[test]
    fn the_letters_become_galaxies_and_come_back_exactly() {
        let layout = layout(1280, 800).unwrap();
        let mut galaxies = Galaxies::default();
        galaxies.reset(&layout, 3);
        let standing = run(&mut galaxies, &layout, 0.0, APPEAR);
        assert_eq!(at_home(&standing, &layout), layout.letters.len());
        let homes: Vec<(f64, f64)> = galaxies.stars.iter().map(|s| (s.x, s.y)).collect();

        let collided = run(&mut galaxies, &layout, APPEAR, APPEAR + HOLD + 12.0);
        assert!(at_home(&collided, &layout) < layout.letters.len() / 4);
        assert!(drawn(&collided) > layout.letters.len());

        // Running it back to the start puts every star where it began.
        galaxies.go_to(0);
        let furthest = galaxies
            .stars
            .iter()
            .zip(&homes)
            .map(|(s, h)| ((s.x - h.0).powi(2) + (s.y - h.1).powi(2)).sqrt())
            .fold(0.0, f64::max);
        assert!(furthest < 0.01, "a star came back {furthest} away");
        let home = run(
            &mut galaxies,
            &layout,
            TURN - GONE - REST,
            TURN - GONE - 0.5,
        );
        assert_eq!(at_home(&home, &layout), layout.letters.len());
    }

    #[test]
    fn the_cores_merge() {
        let layout = layout(1920, 1200).unwrap();
        let mut galaxies = Galaxies::default();
        galaxies.reset(&layout, 1);
        galaxies.start(&layout, 0);
        let gap = |c: &[f64; 4]| ((c[0] - c[2]).powi(2) + (c[1] - c[3]).powi(2)).sqrt();
        assert!(gap(&galaxies.cores[0]) > 100.0);
        let last = *galaxies.cores.last().unwrap();
        assert!(gap(&last) < 10.0, "still {} apart", gap(&last));
    }

    #[test]
    fn reduced_motion_shows_the_same_collision_each_time() {
        let layout = layout(1280, 800).unwrap();
        let mut galaxies = Galaxies::default();
        galaxies.reset(&layout, 5);
        let first = composed(&mut galaxies, &layout, 0.0, true);
        let second = composed(&mut galaxies, &layout, 7.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > layout.letters.len());
    }
}
