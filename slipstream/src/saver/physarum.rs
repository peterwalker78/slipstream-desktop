//! `physarum`: a slime mould creeps out of the letters, spreads over the screen as a network of
//! veins that thickens along the best paths and lets the rest wither, and then draws back into the
//! logo.
//!
//! Drawn in Braille dots, two across and four down a cell. The mould is Jeff Jones's model of
//! Physarum polycephalum: thousands of agents each sniff a trail ahead, ahead-left and ahead-right,
//! turn towards the strongest, step forward and leave trail behind them, while the trail spreads a
//! little and fades. Nothing plans the network; it comes out of each agent following the others.
//! The agents start on the logo's dots. Towards the end of the turn the letters give off a scent
//! the agents can smell from anywhere, the veins pull in towards them, and the letters stand again.

use super::{
    AMBER, BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, braille, gradient, hash01, mix,
};

/// One turn, in seconds of the animation clock.
const APPEAR: f64 = 1.5;
const SPILL: f64 = 3.0;
const GROW: f64 = 22.0;
const GATHER: f64 = 5.0;
const REST: f64 = 2.5;
const GONE: f64 = 1.0;
const TURN: f64 = APPEAR + SPILL + GROW + GATHER + REST + GONE;

/// Steps a second, and moves the agents make in each.
const HZ: f64 = 20.0;
const MOVES: usize = 2;

/// Agents for each dot of the screen.
const CROWD: f32 = 0.16;

/// How far ahead an agent smells, as a share of the screen's height in dots, and at what angle to
/// either side; how far it turns towards a scent; how far it steps.
const SNIFF: f32 = 0.045;
const SNIFF_ANGLE: f32 = 0.785;
const TURN_ANGLE: f32 = 0.785;
const STRIDE: f32 = 1.2;

/// Trail an agent leaves each move, and how much of the trail survives a move.
const DEPOSIT: f32 = 1.0;
const DECAY: f32 = 0.85;

/// How much of each dot's trail is swapped for the mean of the dots round it each move.
const SPREAD: f32 = 0.35;

/// Trail a dot needs to show.
const SHOWS: f32 = 0.9;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy)]
struct Agent {
    x: f32,
    y: f32,
    heading: f32,
    /// How far across the logo it started, 0 to 1, which is when it sets off.
    across: f32,
    /// How far it still runs straight out before it starts following trails, in dots.
    run: f32,
}

#[derive(Default)]
pub struct Physarum {
    seed: u64,
    turn: Option<u64>,
    /// Dots across and down.
    w: usize,
    h: usize,
    agents: Vec<Agent>,
    trail: Vec<f32>,
    spare: Vec<f32>,
    /// How near each dot is to the logo, 1 on a letter and falling away from it: the scent the
    /// agents follow home.
    scent: Vec<f32>,
    rng: u64,
}

impl Physarum {
    fn random(&mut self) -> f32 {
        // xorshift64*: cheap, and the same every run for the same seed.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn start(&mut self, layout: &Layout, turn: u64) {
        let (cols, rows) = (layout.cols.max(1) as usize, layout.rows.max(1) as usize);
        let (w, h) = (cols * 2, rows * 4);
        self.w = w;
        self.h = h;
        self.trail.clear();
        self.trail.resize(w * h, 0.0);
        self.spare.clear();
        self.spare.resize(w * h, 0.0);
        self.rng = (self.seed ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;

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
                        homes.push((
                            letter.col as usize * 2 + side,
                            letter.row as usize * 4 + down,
                            across,
                        ));
                    }
                }
            }
        }

        // The scent: distance from the nearest letter dot, by two sweeps of a chamfer distance
        // transform, turned into a slope that rises towards the logo.
        let far = (w + h) as f32;
        let mut distance = vec![far; w * h];
        for &(x, y, _) in &homes {
            if x < w && y < h {
                distance[y * w + x] = 0.0;
            }
        }
        const SIDE: f32 = 1.0;
        const CORNER: f32 = std::f32::consts::SQRT_2;
        for y in 0..h {
            for x in 0..w {
                let mut d = distance[y * w + x];
                if x > 0 {
                    d = d.min(distance[y * w + x - 1] + SIDE);
                }
                if y > 0 {
                    d = d.min(distance[(y - 1) * w + x] + SIDE);
                    if x > 0 {
                        d = d.min(distance[(y - 1) * w + x - 1] + CORNER);
                    }
                    if x + 1 < w {
                        d = d.min(distance[(y - 1) * w + x + 1] + CORNER);
                    }
                }
                distance[y * w + x] = d;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let mut d = distance[y * w + x];
                if x + 1 < w {
                    d = d.min(distance[y * w + x + 1] + SIDE);
                }
                if y + 1 < h {
                    d = d.min(distance[(y + 1) * w + x] + SIDE);
                    if x + 1 < w {
                        d = d.min(distance[(y + 1) * w + x + 1] + CORNER);
                    }
                    if x > 0 {
                        d = d.min(distance[(y + 1) * w + x - 1] + CORNER);
                    }
                }
                distance[y * w + x] = d;
            }
        }
        let reach = (w.max(h)) as f32;
        self.scent = distance
            .iter()
            .map(|d| 1.0 - (d / reach).min(1.0))
            .collect();

        // Agents on the logo's dots, facing every way.
        let count = ((w * h) as f32 * CROWD) as usize;
        self.agents.clear();
        if homes.is_empty() {
            return;
        }
        for k in 0..count {
            let (x, y, across) = homes[k % homes.len()];
            let (jx, jy, heading) = (self.random(), self.random(), self.random());
            let run = self.random().sqrt() * w.max(h) as f32 * 0.55;
            self.agents.push(Agent {
                x: x as f32 + jx,
                y: y as f32 + jy,
                heading: heading * std::f32::consts::TAU,
                across,
                run,
            });
        }
        self.turn = Some(turn);
    }

    /// One move of every agent and one spread and fade of the trail, `t` seconds into the turn.
    fn advance(&mut self, t: f64) {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return;
        }
        let reach = (h as f32 * SNIFF).max(3.0);
        let set_off = ((t - APPEAR) / SPILL) as f32;
        // How strongly the logo calls the agents home, and how fast the trail fades as it does.
        let homing = ease(((t - APPEAR - SPILL - GROW) / GATHER) as f32);
        let decay = DECAY - 0.12 * homing;

        let mut agents = std::mem::take(&mut self.agents);
        for agent in &mut agents {
            if set_off < agent.across * 0.9 {
                // Not yet off: it keeps its dot lit.
                let i = (agent.y as usize).min(h - 1) * w + (agent.x as usize).min(w - 1);
                self.trail[i] += DEPOSIT;
                continue;
            }
            if agent.run > 0.0 {
                // Creeping out: straight on, leaving a trail, and turning back off the edges.
                agent.run -= STRIDE;
                let (nx, ny) = (
                    agent.x + agent.heading.cos() * STRIDE,
                    agent.y + agent.heading.sin() * STRIDE,
                );
                if nx < 0.0 || ny < 0.0 || nx >= w as f32 || ny >= h as f32 {
                    agent.heading += std::f32::consts::PI;
                    continue;
                }
                agent.x = nx;
                agent.y = ny;
                self.trail[ny as usize * w + nx as usize] += DEPOSIT;
                continue;
            }
            let smell = |angle: f32, trail: &[f32], scent: &[f32]| {
                let (sx, sy) = (agent.x + angle.cos() * reach, agent.y + angle.sin() * reach);
                if sx < 0.0 || sy < 0.0 || sx >= w as f32 || sy >= h as f32 {
                    return -1.0;
                }
                let i = sy as usize * w + sx as usize;
                trail[i] + scent[i].powi(6) * homing * 40.0
            };
            let ahead = smell(agent.heading, &self.trail, &self.scent);
            let left = smell(agent.heading - SNIFF_ANGLE, &self.trail, &self.scent);
            let right = smell(agent.heading + SNIFF_ANGLE, &self.trail, &self.scent);
            if ahead >= left && ahead >= right {
            } else if ahead < left && ahead < right {
                agent.heading += if self.random() < 0.5 {
                    -TURN_ANGLE
                } else {
                    TURN_ANGLE
                };
            } else if left > right {
                agent.heading -= TURN_ANGLE;
            } else {
                agent.heading += TURN_ANGLE;
            }
            // Arriving home, it slows and stays.
            let (nx, ny) = (
                agent.x + agent.heading.cos() * STRIDE,
                agent.y + agent.heading.sin() * STRIDE,
            );
            if nx < 0.0 || ny < 0.0 || nx >= w as f32 || ny >= h as f32 {
                agent.heading = self.random() * std::f32::consts::TAU;
                continue;
            }
            let i = ny as usize * w + nx as usize;
            if homing > 0.0 && self.scent[i] >= 1.0 && self.random() < homing {
                self.trail[i] += DEPOSIT;
                continue;
            }
            agent.x = nx;
            agent.y = ny;
            self.trail[i] += DEPOSIT;
        }
        self.agents = agents;

        // Spread: each dot moves part of the way to the mean of the nine round it, then fades.
        let trail = &self.trail;
        let spare = &mut self.spare;
        for y in 0..h {
            let (up, down) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let (l, r) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let sum = trail[up * w + l]
                    + trail[up * w + x]
                    + trail[up * w + r]
                    + trail[y * w + l]
                    + trail[y * w + x]
                    + trail[y * w + r]
                    + trail[down * w + l]
                    + trail[down * w + x]
                    + trail[down * w + r];
                let own = trail[y * w + x];
                spare[y * w + x] = (own + (sum / 9.0 - own) * SPREAD) * decay;
            }
        }
        std::mem::swap(&mut self.trail, &mut self.spare);
    }

    fn draw(&self, grid: &mut Grid, fade: f32) {
        // Dots are stippled against the mean of the trail that shows, each at its own steady
        // random share of it, so veins are shaded by how much runs through them rather than
        // filled solid.
        let (sum, lit) = self
            .trail
            .iter()
            .filter(|&&l| l >= SHOWS)
            .fold((0.0, 0usize), |(sum, n), &l| (sum + l, n + 1));
        let mean = if lit > 0 { sum / lit as f32 } else { 1.0 };
        let cols = (grid.cols.max(0) as usize).min(self.w / 2);
        let rows = (grid.rows.max(0) as usize).min(self.h / 4);
        for row in 0..rows {
            for col in 0..cols {
                let (mut dots, mut most) = (0u8, 0.0f32);
                for down in 0..4 {
                    for across in 0..2 {
                        let i = (row * 4 + down) * self.w + col * 2 + across;
                        let light = self.trail[i];
                        if light >= SHOWS && light >= mean * (0.3 + 0.9 * hash01(i as u64 ^ 0x5117))
                        {
                            dots |= 1 << (down * 2 + across);
                            most = most.max(light);
                        }
                    }
                }
                if dots == 0 {
                    continue;
                }
                // Faint veins cyan, through mint to amber and white in the thickest.
                let thick = (most / mean).ln_1p() / 1.1;
                let rgb = if thick < 0.5 {
                    mix(CYAN, MINT, thick * 2.0)
                } else {
                    mix(
                        mix(MINT, AMBER, ((thick - 0.5) * 2.0).min(1.0)),
                        WHITE,
                        (thick - 1.0).clamp(0.0, 0.6),
                    )
                };
                let level = (0.55 + thick * 0.45).min(1.0);
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, rgb, level * fade),
                );
            }
        }
    }
}

impl Variation for Physarum {
    fn id(&self) -> &'static str {
        "physarum"
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
            // The network grown, from the same start every time.
            self.start(layout, 0);
            let from = APPEAR + SPILL;
            for s in 0..(HZ * 8.0) as usize {
                for _ in 0..MOVES {
                    self.advance(from + s as f64 / HZ);
                }
            }
            self.draw(grid, 1.0);
            return;
        }
        let turn = (frame.elapsed / TURN).floor().max(0.0) as u64;
        let t = frame.elapsed.rem_euclid(TURN);
        if self.turn != Some(turn) || self.w != w || self.h != h {
            self.start(layout, turn);
        }
        for _ in 0..MOVES {
            self.advance(t);
        }
        let fade = if t < APPEAR {
            (t / APPEAR) as f32
        } else if t > TURN - GONE {
            ((TURN - t) / GONE) as f32
        } else {
            1.0
        };
        let home = APPEAR + SPILL + GROW + GATHER;
        if t < APPEAR || t >= home {
            // The letters in their own blocks, with what is left of the mould fading round them.
            if t >= home {
                let mut veins = Grid::new(layout.cols, layout.rows);
                let left = (1.0 - (t - home) / (REST * 0.6)).clamp(0.0, 1.0) as f32;
                self.draw(&mut veins, fade * left);
                for (cell, vein) in grid.cells.iter_mut().zip(&veins.cells) {
                    if vein.ch != ' ' {
                        *cell = *vein;
                    }
                }
            }
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
        } else {
            self.draw(grid, fade);
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        APPEAR + SPILL + 8.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn run(until: f64) -> (Grid, Layout) {
        let layout = layout(1280, 800).unwrap();
        let mut mould = Physarum::default();
        mould.reset(&layout, 3);
        let mut elapsed = 0.0;
        let mut grid = composed(&mut mould, &layout, 0.0, false);
        while elapsed <= until {
            grid = composed(&mut mould, &layout, elapsed, false);
            elapsed += 1.0 / HZ;
        }
        (grid, layout)
    }

    #[test]
    fn the_mould_spreads_out_of_the_logo_and_the_letters_return() {
        let (standing, layout) = run(APPEAR - 0.1);
        assert_eq!(at_home(&standing, &layout), layout.letters.len());
        let (grown, _) = run(APPEAR + SPILL + 10.0);
        assert!(at_home(&grown, &layout) < layout.letters.len() / 4);
        assert!(
            drawn(&grown) > (layout.cols * layout.rows) as usize / 20,
            "a network over the screen: {}",
            drawn(&grown)
        );
        let (home, _) = run(APPEAR + SPILL + GROW + GATHER + REST);
        assert_eq!(at_home(&home, &layout), layout.letters.len());
    }

    #[test]
    fn reduced_motion_shows_the_same_network_each_time() {
        let layout = layout(1280, 800).unwrap();
        let mut mould = Physarum::default();
        mould.reset(&layout, 5);
        let first = composed(&mut mould, &layout, 12.0, true);
        let second = composed(&mut mould, &layout, 12.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > 100);
    }
}
