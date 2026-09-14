//! `chladni`: the logo's dots are sand on a metal plate. A note sounds, the sand is thrown across
//! the plate and dances into the note's figure; the note glides to the next and the figure melts
//! into a new one; then the plate falls quiet and the sand walks home into the letters.
//!
//! Drawn in Braille dots. A plate ringing in one of its modes shakes hardest at its antinodes and
//! not at all along its nodal lines. Each grain is kicked a random way by as much as the plate is
//! shaking under it, and slides a little down the slope of the shaking, so grains leave the
//! loud parts of the plate and pile up along the quiet lines: Chladni's figures, found by the sand
//! rather than drawn. A square plate's mode is
//!
//! ```text
//! cos(n·x)·cos(m·y) ± cos(m·x)·cos(n·y)
//! ```
//!
//! and while one note glides into the next the two modes are mixed, so the lines bend, split and
//! join as they move.

use super::{
    Frame, Grid, Layout, Stipple, Variation, draw_letters, gradient, logo_dots, smoothstep,
    xorshift,
};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const THROW: f64 = 1.5;
const NOTE: f64 = 5.0;
const NOTES: usize = 4;
const HOME: f64 = 3.5;
const REST: f64 = 1.5;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + THROW + NOTE * NOTES as f64 + HOME + REST + GONE;

/// Steps a second.
const HZ: f64 = 20.0;

/// Grains for each dot of the logo.
const GRAINS_PER_DOT: usize = 5;

/// The modes a turn picks its notes from, as (n, m, sign).
const MODES: [(f32, f32, f32); 10] = [
    (2.0, 5.0, 1.0),
    (3.0, 6.0, -1.0),
    (1.0, 6.0, 1.0),
    (4.0, 7.0, 1.0),
    (3.0, 8.0, -1.0),
    (5.0, 6.0, 1.0),
    (2.0, 7.0, -1.0),
    (1.0, 4.0, 1.0),
    (4.0, 5.0, -1.0),
    (3.0, 7.0, 1.0),
];

/// The furthest a grain is kicked in a step, in dots on a screen 264 dots high; how hard it slides
/// down the slope of the shaking; and the shaking left even on a nodal line, so no grain is ever
/// quite stuck.
const KICK: f32 = 8.0;
const SLIDE: f32 = 150.0;
const FLOOR: f32 = 0.01;
/// How hard the plate is struck as each note changes, on top of the note itself.
const STRIKE: f32 = 0.25;
const DESIGN_HEIGHT: f32 = 264.0;

#[derive(Clone, Copy)]
struct Grain {
    x: f32,
    y: f32,
    home: (f32, f32),
    /// Where the throw lands it.
    landing: (f32, f32),
    rgb: [f32; 3],
}

#[derive(Default)]
pub struct Chladni {
    seed: u64,
    turn: Option<u64>,
    cols: i32,
    rows: i32,
    /// Dots across and down.
    w: f32,
    h: f32,
    grains: Vec<Grain>,
    notes: [usize; NOTES],
    rng: u64,
    /// When the grains were last stepped, to step each step of the clock once.
    stepped: Option<i64>,
    stipple: Stipple,
}

impl Chladni {
    fn start(&mut self, layout: &Layout, turn: u64) {
        self.turn = Some(turn);
        self.cols = layout.cols;
        self.rows = layout.rows;
        self.w = (layout.cols.max(1) * 2) as f32;
        self.h = (layout.rows.max(1) * 4) as f32;
        self.rng = (self.seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;
        self.stepped = None;
        // Four different notes, in a different order each turn.
        let mut modes: Vec<usize> = (0..MODES.len()).collect();
        for i in (1..modes.len()).rev() {
            let j = (xorshift(&mut self.rng) * (i + 1) as f32) as usize;
            modes.swap(i, j.min(i));
        }
        self.notes.copy_from_slice(&modes[..NOTES]);

        let (w, h) = (self.w, self.h);
        self.grains.clear();
        for dot in logo_dots(layout) {
            let rgb = gradient(dot.across);
            for _ in 0..GRAINS_PER_DOT {
                let x = dot.x as f32 + xorshift(&mut self.rng);
                let y = dot.y as f32 + xorshift(&mut self.rng);
                // Thrown out over the whole plate, keeping to its side of it, so the figures carry
                // the logo's colours across the screen.
                let (u, v) = (xorshift(&mut self.rng).max(1e-6), xorshift(&mut self.rng));
                let spread = (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos();
                let landing = (
                    ((dot.across * 1.1 - 0.05) * w + spread * w * 0.045).clamp(0.0, w - 1.0),
                    xorshift(&mut self.rng) * (h - 1.0),
                );
                self.grains.push(Grain {
                    x,
                    y,
                    home: (x, y),
                    landing,
                    rgb,
                });
            }
        }
    }

    /// How hard the plate shakes at (`x`, `y`) in dots, and its slope, at `t` seconds into the
    /// notes: the note playing, gliding into the next over the last part of it.
    fn shaking(&self, x: f32, y: f32, t: f32) -> (f32, f32, f32) {
        let note = ((t / NOTE as f32) as usize).min(NOTES - 1);
        let within = t / NOTE as f32 - note as f32;
        let glide = if note + 1 < NOTES {
            smoothstep((within - 0.7) / 0.3)
        } else {
            0.0
        };
        // The plate is square and as tall as the screen, centred on it.
        let half = self.h / 2.0;
        let to_angle = std::f32::consts::FRAC_PI_2 / half;
        let px = (x - self.w / 2.0) * to_angle + std::f32::consts::FRAC_PI_2;
        let py = (y - half) * to_angle + std::f32::consts::FRAC_PI_2;
        let mode = |index: usize| {
            let (n, m, sign) = MODES[self.notes[index]];
            let (cnx, snx, cmy, smy) = (
                (n * px).cos(),
                (n * px).sin(),
                (m * py).cos(),
                (m * py).sin(),
            );
            let (cmx, smx, cny, sny) = (
                (m * px).cos(),
                (m * px).sin(),
                (n * py).cos(),
                (n * py).sin(),
            );
            (
                cnx * cmy + sign * cmx * cny,
                (-n * snx * cmy - sign * m * smx * cny) * to_angle,
                (-m * cnx * smy - sign * n * cmx * sny) * to_angle,
            )
        };
        let (a, ax, ay) = mode(note);
        if glide <= 0.0 {
            return (a / 2.0, ax / 2.0, ay / 2.0);
        }
        let (b, bx, by) = mode(note + 1);
        let mix = |p: f32, q: f32| (p + (q - p) * glide) / 2.0;
        (mix(a, b), mix(ax, bx), mix(ay, by))
    }

    /// One step of the clock, `t` seconds into the turn.
    fn advance(&mut self, t: f64) {
        let notes_from = APPEAR + THROW;
        let home_from = notes_from + NOTE * NOTES as f64;
        let scale = self.h / DESIGN_HEIGHT;
        let (w, h) = (self.w, self.h);
        let mut grains = std::mem::take(&mut self.grains);
        if t < APPEAR {
            for grain in &mut grains {
                (grain.x, grain.y) = grain.home;
            }
        } else if t < notes_from {
            // The throw: out of the letters and across the plate, landing softly.
            let along = ((t - APPEAR) / THROW) as f32;
            let out = 1.0 - (1.0 - along).powi(3);
            for grain in &mut grains {
                grain.x = grain.home.0 + (grain.landing.0 - grain.home.0) * out;
                grain.y = grain.home.1 + (grain.landing.1 - grain.home.1) * out;
            }
        } else if t < home_from {
            let at = (t - notes_from) as f32;
            let within = (at / NOTE as f32).fract();
            // Each change of note strikes the plate, and the sand leaps before it settles.
            let strike = STRIKE * (-((within - 0.85) / 0.06).powi(2)).exp();
            for grain in &mut grains {
                let (a, ax, ay) = self.shaking(grain.x, grain.y, at);
                let kick = (a.abs() + FLOOR + strike) * KICK * scale * xorshift(&mut self.rng);
                let angle = xorshift(&mut self.rng) * std::f32::consts::TAU;
                // Down the slope of a·a, whose gradient is 2·a·∇a.
                let slide = SLIDE * scale * scale;
                grain.x += angle.cos() * kick - 2.0 * a * ax * slide;
                grain.y += angle.sin() * kick - 2.0 * a * ay * slide;
                grain.x = reflect(grain.x, w - 1.0);
                grain.y = reflect(grain.y, h - 1.0);
            }
        } else {
            // Quiet: each grain skitters home, jittering less the nearer it gets, so the last
            // figure draws in towards the letters before it breaks up into them.
            let along = ((t - home_from) / HOME) as f32;
            let pull = 0.02 + 0.2 * smoothstep(along * 1.2);
            for grain in &mut grains {
                let (dx, dy) = (grain.home.0 - grain.x, grain.home.1 - grain.y);
                if along >= 0.85 {
                    let settle = smoothstep((along - 0.85) / 0.1);
                    grain.x += dx * settle.max(pull);
                    grain.y += dy * settle.max(pull);
                    continue;
                }
                let jitter = (dx * dx + dy * dy).sqrt().min(30.0) * 0.04;
                let angle = xorshift(&mut self.rng) * std::f32::consts::TAU;
                grain.x += dx * pull + angle.cos() * jitter;
                grain.y += dy * pull + angle.sin() * jitter;
            }
        }
        self.grains = grains;
    }

    fn draw(&mut self, grid: &mut Grid, fade: f32) {
        let (cols, rows) = (self.cols.max(0) as usize, self.rows.max(0) as usize);
        self.stipple.clear(cols, rows);
        for grain in &self.grains {
            self.stipple.add(grain.x, grain.y, grain.rgb, 1.0);
        }
        self.stipple.draw(grid, 6.0, 0.12, fade);
    }
}

/// `v` bounced back into 0..=`most`.
fn reflect(v: f32, most: f32) -> f32 {
    let v = v.abs();
    if v > most {
        (2.0 * most - v).max(0.0)
    } else {
        v
    }
}

impl Variation for Chladni {
    fn id(&self) -> &'static str {
        "chladni"
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
            // The first note's figure, settled, from the same start every time.
            if self.turn != Some(0) || resized {
                self.seed = 0;
                self.start(layout, 0);
                let until = APPEAR + THROW + NOTE * 0.6;
                let mut t = 0.0;
                while t < until {
                    self.advance(t);
                    t += 1.0 / HZ;
                }
            }
            self.draw(grid, 1.0);
            return;
        }
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn) || resized {
            self.start(layout, turn);
        }
        // The sand moves once a step of the clock, however often it is drawn.
        let step = (frame.elapsed * HZ).floor() as i64;
        if self.stepped != Some(step) {
            self.stepped = Some(step);
            self.advance(t);
        }
        let fade = if t < APPEAR {
            (t / APPEAR) as f32
        } else if t > TURN - GONE {
            ((TURN - t) / GONE) as f32
        } else {
            1.0
        };
        if t < APPEAR || t >= TURN - REST - GONE {
            draw_letters(layout, grid, fade);
        } else {
            self.draw(grid, fade);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + THROW + NOTE * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn run(until: f64) -> (Grid, Layout, Chladni) {
        let layout = layout(1280, 800).unwrap();
        let mut plate = Chladni::default();
        plate.reset(&layout, 3);
        let mut elapsed = 0.0;
        let mut grid = composed(&mut plate, &layout, 0.0, false);
        while elapsed <= until {
            grid = composed(&mut plate, &layout, elapsed, false);
            elapsed += 1.0 / HZ;
        }
        (grid, layout, plate)
    }

    #[test]
    fn the_sand_finds_the_quiet_lines() {
        let (grid, layout, plate) = run(APPEAR + THROW + NOTE * 0.7);
        assert!(at_home(&grid, &layout) < layout.letters.len() / 4);
        assert!(drawn(&grid) > layout.letters.len());
        let at = (NOTE * 0.7) as f32;
        let quiet = plate
            .grains
            .iter()
            .filter(|g| plate.shaking(g.x, g.y, at).0.abs() < 0.15)
            .count();
        assert!(
            quiet > plate.grains.len() * 3 / 4,
            "{quiet} of {} grains on quiet lines",
            plate.grains.len()
        );
    }

    #[test]
    fn the_sand_walks_home_into_the_letters() {
        let (grid, layout, _) = run(TURN - GONE - REST / 2.0);
        assert_eq!(at_home(&grid, &layout), layout.letters.len());
    }

    #[test]
    fn reduced_motion_shows_the_same_figure_each_time() {
        let layout = layout(1280, 800).unwrap();
        let mut plate = Chladni::default();
        plate.reset(&layout, 5);
        let first = composed(&mut plate, &layout, 0.0, true);
        let second = composed(&mut plate, &layout, 9.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > layout.letters.len());
    }
}
