//! `murmuration`: the logo's dots take off as a flock of starlings, wheel round the screen as one
//! body, scatter round a falcon nobody sees, and come down again into the letters.
//!
//! Drawn in Braille dots, two across and four down a cell. Every dot of the logo is a bird. The
//! flock is boids in three dimensions: each bird keeps its distance from the birds round it, and
//! matches the heading of and closes on seven of them, as real starlings do, and the whole flock
//! leans towards a roost that wanders round the screen.
//! Depth only shows as brightness and a little perspective, which is where the flock's shifting
//! density comes from. Now and then a falcon passes through and the flock parts round it. At the
//! end of the turn the birds land back on their own dots, left of the logo first.

use super::{BG, Frame, Grid, Layout, Variation, WHITE, braille, gradient, hash01, mix};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const LIFT: f64 = 4.0;
const FLY: f64 = 21.0;
const LAND: f64 = 5.0;
const REST: f64 = 2.0;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + LIFT + FLY + LAND + REST + GONE;

/// Steps a second.
const HZ: f64 = 30.0;

/// Birds for each dot of the logo, and the most there can be.
const BIRDS_PER_DOT: usize = 1;
const MOST_BIRDS: usize = 5000;

/// The most birds each bird looks at in a step, so an unusually tight knot can't stall a step.
const LOOKS: usize = 96;

/// How many neighbours each bird heeds.
const NEIGHBOURS: usize = 7;

/// How close a bird lets another come, as a share of the screen's height in dots.
const ROOM: f32 = 0.026;

/// How far a bird looks for the neighbours it steers by, as a share of the screen's height.
const SIGHT: f32 = 0.09;

/// Speeds as shares of the height a second.
const FASTEST: f32 = 0.42;
const SLOWEST: f32 = 0.2;

/// Seconds between the falcon's passes, and its reach as a share of the height.
const FALCON_EVERY: f32 = 7.0;
const FALCON_REACH: f32 = 0.16;

/// How much of a dot's light survives a step, so each bird leaves a short streak.
const TRAIL: f32 = 0.35;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy)]
struct Bird {
    /// Its dot in the logo.
    home: (f32, f32),
    rgb: [f32; 3],
    /// How far across the logo it sits, 0 to 1, which is when it takes off and lands.
    across: f32,
    /// Where it is, in dots, with depth in the same units, and how fast it is going.
    at: [f32; 3],
    speed: [f32; 3],
    flying: bool,
}

#[derive(Default)]
pub struct Murmuration {
    seed: u64,
    turn: Option<u64>,
    /// Dots across and down.
    w: usize,
    h: usize,
    birds: Vec<Bird>,
    /// The birds by square of the screen, as a linked list per square: the first bird in each
    /// square, and the next bird after each.
    first: Vec<u32>,
    next: Vec<u32>,
    /// The same in squares as wide as a bird's sight.
    wide_first: Vec<u32>,
    wide_next: Vec<u32>,
    /// The light on each dot and the colour it was last lit in.
    light: Vec<f32>,
    ink: Vec<[f32; 3]>,
    /// The time of the last step, so steps follow the animation clock.
    last: Option<f64>,
}

/// No bird, in the square lists.
const NONE: u32 = u32::MAX;

impl Murmuration {
    fn start(&mut self, layout: &Layout, turn: u64) {
        let (cols, rows) = (layout.cols.max(1) as usize, layout.rows.max(1) as usize);
        self.w = cols * 2;
        self.h = rows * 4;
        self.light.clear();
        self.light.resize(self.w * self.h, 0.0);
        self.ink.clear();
        self.ink.resize(self.w * self.h, BG);
        self.last = None;

        let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
        let mut homes = vec![];
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw as f32;
            for down in 0..4 {
                for side in 0..2 {
                    let on = match letter.ch {
                        '▀' => down < 2,
                        '▄' => down >= 2,
                        '▌' => side == 0,
                        '▐' => side == 1,
                        _ => true,
                    };
                    if on {
                        let home = (
                            (letter.col * 2 + side) as f32,
                            (letter.row * 4 + down) as f32,
                        );
                        homes.push((home, across));
                    }
                }
            }
        }
        let each = BIRDS_PER_DOT.min(MOST_BIRDS / homes.len().max(1)).max(1);
        let salt = self.seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.birds.clear();
        for (k, &(home, across)) in homes.iter().enumerate() {
            for copy in 0..each {
                let n = ((k * each + copy) as u64) ^ salt;
                // Each bird leaves a little to one side of the others, and mostly upwards.
                let unit = self.h as f32 * FASTEST;
                self.birds.push(Bird {
                    home,
                    rgb: gradient(across),
                    across: across + (hash01(n ^ 0x51) - 0.5) * 0.04,
                    at: [home.0 + 0.5, home.1 + 0.5, 0.0],
                    speed: [
                        (hash01(n ^ 0xa1) - 0.5) * unit * 0.8,
                        -(0.4 + 0.5 * hash01(n ^ 0xb2)) * unit,
                        (hash01(n ^ 0xc3) - 0.5) * unit * 0.8,
                    ],
                    flying: false,
                });
            }
        }
        self.turn = Some(turn);
    }

    /// Where the flock is drawn towards `t` seconds into the turn: a slow wander round the middle
    /// of the screen, in and out of it too.
    fn roost(&self, t: f32) -> [f32; 3] {
        let (w, h) = (self.w as f32, self.h as f32);
        let phase = hash01(self.seed ^ self.turn.unwrap_or(0).wrapping_mul(0x5bd1_e995))
            * std::f32::consts::TAU;
        [
            w * 0.5 + w * 0.27 * (0.19 * t + phase).sin(),
            h * 0.5 + h * 0.22 * (0.31 * t + phase * 1.7).sin(),
            h * 0.25 * (0.23 * t + phase * 0.6).sin(),
        ]
    }

    /// The falcon at `t`: where it is, if it is on screen. It crosses on a straight line through
    /// somewhere near the middle, each pass from a different side.
    fn falcon(&self, t: f32) -> Option<[f32; 3]> {
        let pass = (t / FALCON_EVERY).floor();
        let into = t / FALCON_EVERY - pass;
        // Across in the middle third of each pass; out of sight the rest of the time.
        if !(0.3..0.7).contains(&into) {
            return None;
        }
        let n = pass as u64 ^ self.seed ^ 0xfa1c;
        let angle = hash01(n) * std::f32::consts::TAU;
        let (w, h) = (self.w as f32, self.h as f32);
        let through = [
            w * (0.35 + 0.3 * hash01(n ^ 1)),
            h * (0.35 + 0.3 * hash01(n ^ 2)),
        ];
        let along = (into - 0.5) / 0.2 * w * 0.7;
        Some([
            through[0] + angle.cos() * along,
            through[1] + angle.sin() * along,
            0.0,
        ])
    }

    /// Advances every bird in the air by `dt` seconds, `t` seconds into the turn.
    fn fly(&mut self, t: f32, dt: f32) {
        let (w, h) = (self.w as f32, self.h as f32);
        let room = (h * ROOM).max(1.0);
        let sight = (h * SIGHT).max(room);
        let (fastest, slowest) = (h * FASTEST, h * SLOWEST);

        // File the birds by square, each square as wide as the room a bird keeps, so the nine
        // squares round a bird hold every bird it must keep clear of, and only a few more.
        let across = (self.w as f32 / room).ceil() as usize + 1;
        let down = (self.h as f32 / room).ceil() as usize + 1;
        let square = |at: [f32; 3]| {
            let x = (at[0] / room).clamp(0.0, (across - 1) as f32) as usize;
            let y = (at[1] / room).clamp(0.0, (down - 1) as f32) as usize;
            (x, y)
        };
        self.first.clear();
        self.first.resize(across * down, NONE);
        self.next.clear();
        self.next.resize(self.birds.len(), NONE);
        for (i, bird) in self.birds.iter().enumerate() {
            if bird.flying {
                let (x, y) = square(bird.at);
                self.next[i] = self.first[y * across + x];
                self.first[y * across + x] = i as u32;
            }
        }

        // And again in wide squares, as wide as a bird's sight, for finding the birds it steers by.
        let wide_across = (self.w as f32 / sight).ceil() as usize + 1;
        let wide_down = (self.h as f32 / sight).ceil() as usize + 1;
        let wide_square = |at: [f32; 3]| {
            let x = (at[0] / sight).clamp(0.0, (wide_across - 1) as f32) as usize;
            let y = (at[1] / sight).clamp(0.0, (wide_down - 1) as f32) as usize;
            (x, y)
        };
        self.wide_first.clear();
        self.wide_first.resize(wide_across * wide_down, NONE);
        self.wide_next.clear();
        self.wide_next.resize(self.birds.len(), NONE);
        for (i, bird) in self.birds.iter().enumerate() {
            if bird.flying {
                let (x, y) = wide_square(bird.at);
                self.wide_next[i] = self.wide_first[y * wide_across + x];
                self.wide_first[y * wide_across + x] = i as u32;
            }
        }

        // The middle of the whole flock, which stray clumps are drawn back to.
        let (mut centre, mut aloft) = ([0.0f32; 3], 0.0f32);
        for bird in self.birds.iter().filter(|b| b.flying) {
            for (sum, at) in centre.iter_mut().zip(bird.at) {
                *sum += at;
            }
            aloft += 1.0;
        }
        let centre = centre.map(|c| c / aloft.max(1.0));
        let roost = self.roost(t);
        let falcon = self.falcon(t);
        for i in 0..self.birds.len() {
            let bird = self.birds[i];
            if !bird.flying {
                continue;
            }
            let (sx, sy) = square(bird.at);
            let (mut away, mut heading, mut middle) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
            let (mut seen, mut looked) = (0, 0);
            // Every bird in the nine squares round it keeps its distance.
            'near: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (x, y) = (sx as i32 + dx, sy as i32 + dy);
                    if x < 0 || y < 0 || x as usize >= across || y as usize >= down {
                        continue;
                    }
                    let mut j = self.first[y as usize * across + x as usize];
                    while j != NONE {
                        looked += 1;
                        if looked > LOOKS {
                            break 'near;
                        }
                        if j as usize != i {
                            let other = &self.birds[j as usize];
                            let gap: [f32; 3] = std::array::from_fn(|k| other.at[k] - bird.at[k]);
                            let d2 = gap[0] * gap[0] + gap[1] * gap[1] + gap[2] * gap[2];
                            if d2 < room * room {
                                let d = d2.sqrt().max(0.05);
                                let push = (1.0 - d / room) / d;
                                for k in 0..3 {
                                    away[k] -= gap[k] * push;
                                }
                            }
                        }
                        j = self.next[j as usize];
                    }
                }
            }
            // Its heading and closeness come from the first few birds within sight, looking in
            // its own wide square first.
            let (wx, wy) = wide_square(bird.at);
            'sight: for (dx, dy) in [
                (0i32, 0i32),
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
            ] {
                let (x, y) = (wx as i32 + dx, wy as i32 + dy);
                if x < 0 || y < 0 || x as usize >= wide_across || y as usize >= wide_down {
                    continue;
                }
                let mut j = self.wide_first[y as usize * wide_across + x as usize];
                while j != NONE {
                    if j as usize != i {
                        let other = &self.birds[j as usize];
                        let gap: [f32; 3] = std::array::from_fn(|k| other.at[k] - bird.at[k]);
                        if gap[0] * gap[0] + gap[1] * gap[1] + gap[2] * gap[2] < sight * sight {
                            for k in 0..3 {
                                heading[k] += other.speed[k];
                                middle[k] += gap[k];
                            }
                            seen += 1;
                            if seen >= NEIGHBOURS {
                                break 'sight;
                            }
                        }
                    }
                    j = self.wide_next[j as usize];
                }
            }

            let mut pull = [0.0f32; 3];
            if seen > 0 {
                let n = seen as f32;
                for k in 0..3 {
                    pull[k] += (heading[k] / n - bird.speed[k]) * 2.5 + (middle[k] / n) * 1.2;
                }
            }
            for k in 0..3 {
                pull[k] += away[k] * h * 6.0;
            }
            for k in 0..3 {
                pull[k] += (centre[k] - bird.at[k]) * 0.06;
            }
            // Towards the roost, harder the further out a bird has strayed.
            let to_roost: [f32; 3] = std::array::from_fn(|k| roost[k] - bird.at[k]);
            let far = (to_roost[0].powi(2) + to_roost[1].powi(2) + to_roost[2].powi(2)).sqrt();
            for k in 0..3 {
                pull[k] += to_roost[k] / far.max(1.0) * h * (0.25 + far / h);
            }
            // Away from the falcon, hard.
            if let Some(falcon) = falcon {
                let gap: [f32; 3] = std::array::from_fn(|k| bird.at[k] - falcon[k]);
                let d = (gap[0] * gap[0] + gap[1] * gap[1]).sqrt();
                let reach = h * FALCON_REACH;
                if d < reach {
                    let fear = (1.0 - d / reach) * h * 9.0;
                    pull[0] += gap[0] / d.max(0.5) * fear;
                    pull[1] += gap[1] / d.max(0.5) * fear;
                }
            }
            // Back from the screen's edges.
            let margin = h * 0.08;
            if bird.at[0] < margin {
                pull[0] += h * 2.0;
            } else if bird.at[0] > w - margin {
                pull[0] -= h * 2.0;
            }
            if bird.at[1] < margin {
                pull[1] += h * 2.0;
            } else if bird.at[1] > h - margin {
                pull[1] -= h * 2.0;
            }

            let bird = &mut self.birds[i];
            for (speed, pull) in bird.speed.iter_mut().zip(pull) {
                *speed += pull * dt;
            }
            let speed = (bird.speed[0].powi(2) + bird.speed[1].powi(2) + bird.speed[2].powi(2))
                .sqrt()
                .max(1e-3);
            let clamped = speed.clamp(slowest, fastest);
            for k in 0..3 {
                bird.speed[k] *= clamped / speed;
                bird.at[k] += bird.speed[k] * dt;
            }
        }
    }

    /// The share of the way home a bird is `t` seconds into the turn: 0 in the air, 1 on its dot.
    fn landed(bird: &Bird, t: f64) -> f32 {
        let since = ((t - APPEAR - LIFT - FLY) / LAND) as f32;
        ease(since * 1.8 - bird.across * 0.8)
    }

    /// Where a bird shows on screen, and how brightly for its depth.
    fn shown(&self, bird: &Bird, t: f64) -> (f32, f32, f32) {
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        let near = 1.0 / (1.0 + bird.at[2] / (self.h as f32 * 1.6)).max(0.3);
        let seen = (cx + (bird.at[0] - cx) * near, cy + (bird.at[1] - cy) * near);
        let home = Self::landed(bird, t);
        let depth = (0.55 + 0.45 * near.min(1.4)).min(1.0);
        (
            seen.0 + (bird.home.0 + 0.5 - seen.0) * home,
            seen.1 + (bird.home.1 + 0.5 - seen.1) * home,
            depth + (1.0 - depth) * home,
        )
    }

    fn step(&mut self, t: f64, dt: f32) {
        // Taking off, left of the logo first.
        let lifted = ((t - APPEAR) / LIFT) as f32;
        for bird in &mut self.birds {
            if !bird.flying && lifted > bird.across * 0.85 {
                bird.flying = true;
            }
        }
        self.fly(t as f32, dt);
    }

    fn draw(&mut self, grid: &mut Grid, t: f64, fade: f32, trail: f32) {
        self.light.iter_mut().for_each(|l| *l *= trail);
        for k in 0..self.birds.len() {
            let bird = self.birds[k];
            let (x, y, bright) = if bird.flying {
                self.shown(&bird, t)
            } else {
                (bird.home.0 + 0.5, bird.home.1 + 0.5, 1.0)
            };
            if x < 0.0 || y < 0.0 || x as usize >= self.w || y as usize >= self.h {
                continue;
            }
            let i = y as usize * self.w + x as usize;
            self.light[i] += bright;
            self.ink[i] = bird.rgb;
        }

        let cols = (grid.cols.max(0) as usize).min(self.w / 2);
        let rows = (grid.rows.max(0) as usize).min(self.h / 4);
        for row in 0..rows {
            for col in 0..cols {
                let (mut dots, mut sum, mut rgb, mut weight) = (0u8, 0.0f32, [0.0f32; 3], 0.0);
                for down in 0..4 {
                    for across in 0..2 {
                        let i = (row * 4 + down) * self.w + col * 2 + across;
                        let light = self.light[i];
                        if light < 0.3 {
                            continue;
                        }
                        dots |= 1 << (down * 2 + across);
                        sum += light;
                        for (sum, ink) in rgb.iter_mut().zip(self.ink[i]) {
                            *sum += ink * light;
                        }
                        weight += light;
                    }
                }
                if dots == 0 {
                    continue;
                }
                let rgb = rgb.map(|c| c / weight);
                // Where birds crowd, the colour runs towards white.
                let crowd = ((sum - 2.0) / 10.0).clamp(0.0, 0.5);
                let level = (0.2 + sum * 0.22).min(1.0);
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, mix(rgb, WHITE, crowd), level * fade),
                );
            }
        }
    }
}

impl Variation for Murmuration {
    fn id(&self) -> &'static str {
        "murmuration"
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
            // The flock in the air, from the same start every time.
            self.start(layout, 0);
            let steps = (HZ * 4.0) as usize;
            for s in 0..steps {
                self.step(APPEAR + LIFT + s as f64 / HZ, 1.0 / HZ as f32);
            }
            self.draw(grid, APPEAR + LIFT + 4.0, 1.0, 0.0);
            return;
        }
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn) || self.w != w || self.h != h {
            self.start(layout, turn);
        }
        // Steps follow the clock, but a long gap (a pause, a skipped frame) is taken as one step.
        let dt =
            self.last
                .map_or(1.0 / HZ, |last| (frame.elapsed - last).clamp(0.0, 0.1)) as f32;
        self.last = Some(frame.elapsed);
        self.step(t, dt);

        let fade = if t < APPEAR {
            (t / APPEAR) as f32
        } else if t > TURN - GONE {
            ((TURN - t) / GONE) as f32
        } else {
            1.0
        };
        if t < APPEAR || t >= APPEAR + LIFT + FLY + LAND {
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
            self.light.iter_mut().for_each(|l| *l = 0.0);
        } else {
            self.draw(grid, t, fade, TRAIL);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + LIFT + 3.0
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
        let mut flock = Murmuration::default();
        flock.reset(&layout, 3);
        let mut grid = Grid::new(layout.cols, layout.rows);
        let readings = Readings::default();
        let mut elapsed = 0.0;
        while elapsed <= until {
            grid.blank(layout.cols, layout.rows);
            flock.compose(
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
    fn the_logo_takes_off_as_a_flock_and_lands_again() {
        let (standing, layout) = run(APPEAR - 0.1);
        assert_eq!(at_home(&standing, &layout), layout.letters.len());
        let (flying, _) = run(APPEAR + LIFT + 6.0);
        assert!(at_home(&flying, &layout) < layout.letters.len() / 10);
        assert!(drawn(&flying) > layout.letters.len() / 4);
        let (home, _) = run(APPEAR + LIFT + FLY + LAND + REST / 2.0);
        assert_eq!(at_home(&home, &layout), layout.letters.len());
    }

    #[test]
    fn the_flock_stays_on_screen() {
        let layout = layout(1280, 800).unwrap();
        let mut flock = Murmuration::default();
        flock.reset(&layout, 9);
        flock.start(&layout, 0);
        for s in 0..(HZ * 20.0) as usize {
            flock.step(APPEAR + LIFT + s as f64 / HZ, 1.0 / HZ as f32);
        }
        let (w, h) = (flock.w as f32, flock.h as f32);
        let inside = flock
            .birds
            .iter()
            .filter(|b| (0.0..w).contains(&b.at[0]) && (0.0..h).contains(&b.at[1]))
            .count();
        assert!(
            inside > flock.birds.len() * 9 / 10,
            "{inside} of {}",
            flock.birds.len()
        );
    }

    #[test]
    fn reduced_motion_shows_the_same_flock_each_time() {
        let layout = layout(1280, 800).unwrap();
        let mut flock = Murmuration::default();
        flock.reset(&layout, 5);
        let draw = |flock: &mut Murmuration| {
            let mut grid = Grid::new(layout.cols, layout.rows);
            let readings = Readings::default();
            flock.compose(
                Frame {
                    layout: &layout,
                    elapsed: 12.0,
                    dt: 0.0,
                    still: true,
                    readings: &readings,
                    local: None,
                },
                &mut grid,
            );
            grid
        };
        let (first, second) = (draw(&mut flock), draw(&mut flock));
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > 100);
    }
}
