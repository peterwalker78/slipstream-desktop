//! `vortex`: the logo in the eye of a turning tunnel of light.
//!
//! The logo stands steady in its colours. Its outline and a breathing ring round it glow, and the
//! glow is carried outwards and round, step after step, by a feedback buffer: each step is the
//! last one zoomed about the logo, turned and warped a little, and faded. So the light leaves the
//! logo in a slow spiral, turning from amber through mint to cyan as it goes, and the picture never
//! quite repeats. The spin slows, stops and reverses on its own long beat.
//!
//! On the minute by the local clock, the logo lets go of one bright ring of a single colour, which
//! the tunnel carries out to the edges of the screen.

use std::f32::consts::{PI, TAU};

use super::{
    BG, Cell, Feedback, Frame, Grid, Layout, Motion, Variation, gradient, hue_byte, hue_colour, mix,
};

/// One turn, in seconds of the animation clock. For the last `DIM` of it the sources go out and
/// the buffer empties, so a handover lands on a near-empty screen.
const TURN: f64 = 24.0;
const DIM: f64 = 1.5;
/// How long the sources take to come up at the start of a turn.
const RISE: f64 = 1.0;

/// Steps a second. Feedback moves the picture a fixed amount a step, so this is also its speed.
const HZ: f64 = 20.0;

/// How much of a step's light survives into the next: a second from full to the floor.
const DECAY: f32 = 0.90;
/// Light below this is drawn as nothing.
const FLOOR: f32 = 0.12;
/// Four levels of light as shades, and six colours round the palette: few enough looks that a
/// fading cell changes only every few steps.
const LOOKS: [char; 4] = ['░', '▒', '▓', '█'];
const HUES: u8 = 6;

/// Seconds between the swells of the beat the sources breathe to.
const BEAT: f32 = 2.4;

/// The ring round the logo, in cells with rows counted double: its radius, and how far its two
/// ripples push it in and out.
const RING: f32 = 30.0;

/// Steps the still picture is run for before it is kept.
const STILL_STEPS: u32 = 40;

/// The minute ring's light, and for how many steps it is laid down, which is how thick it is.
const MINUTE_LIGHT: f32 = 1.0;
const MINUTE_STEPS: u32 = 3;

/// How the tunnel carries the picture at time `t`: out from the logo's middle, turning one way
/// and then the other every 48 s.
fn motion(centre: (f32, f32), t: f32) -> Motion {
    Motion {
        centre,
        zoom: 1.04 + 0.015 * (0.37 * t).sin(),
        turn: (1.6f32).to_radians() * (0.13 * t + 1.0).sin(),
        shift: (0.0, 0.0),
    }
}

/// The source's slow swell, from 0 to 1 and back once a `BEAT`.
fn beat(t: f32) -> f32 {
    0.5 - 0.5 * (TAU * t / BEAT).cos()
}

#[derive(Default)]
pub struct Vortex {
    buffer: Feedback,
    /// The logo's outline: letters with a blank beside them, which is where the glow comes from.
    outline: Vec<(f32, f32)>,
    /// The middle of the logo, which the tunnel turns about, and half its width and height.
    centre: (f32, f32),
    outline_half: (f32, f32),
    /// The minute on the local clock at the last step, and how many more steps the minute's ring
    /// is laid down for.
    minute: Option<i64>,
    ringing: u32,
    /// The still picture, made once.
    still: Option<Vec<Cell>>,
}

impl Vortex {
    /// One step of the tunnel at time `t`, with the sources at `strength`.
    fn advance(&mut self, t: f32, strength: f32) {
        let movement = motion(self.centre, t);
        self.buffer.advance(DECAY, |x, y| {
            let (sx, sy) = movement.source(x, y);
            (
                sx + 0.6 * (0.23 * y + 1.1 * t).sin(),
                sy + 0.3 * (0.07 * x - 0.8 * t).cos(),
            )
        });
        if strength <= 0.0 {
            return;
        }
        let (cx, cy) = self.centre;
        let swell = beat(t);
        let drift = t / 24.0;
        let angle_of = |x: f32, y: f32| ((y - cy) * 2.0).atan2(x - cx);
        // The outline glows with the beat.
        for &(x, y) in &self.outline {
            let hue = hue_byte(2.0 * angle_of(x, y) / TAU + drift);
            self.buffer.ink(x, y, (0.35 + 0.65 * swell) * strength, hue);
        }
        // The ring breathes and ripples, harder on the beat.
        let samples = 420;
        for k in 0..samples {
            let theta = k as f32 / samples as f32 * TAU;
            let radius = RING
                + 5.0 * (3.0 * theta + 0.9 * t).sin()
                + 3.0 * (7.0 * theta - 1.6 * t).sin() * swell;
            let (x, y) = (cx + radius * theta.cos(), cy + radius * theta.sin() / 2.0);
            let hue = hue_byte(2.0 * theta / TAU + drift);
            self.buffer.ink(x, y, 0.8 * strength, hue);
        }
        // The minute's ring: an ellipse just clear of the logo, in one colour.
        if self.ringing > 0 {
            self.ringing -= 1;
            let (rx, ry) = (self.outline_half.0 + 5.0, (self.outline_half.1 + 2.0) * 2.0);
            let hue = hue_byte(drift + 0.5);
            let samples = 520;
            for k in 0..samples {
                let theta = k as f32 / samples as f32 * TAU;
                let (x, y) = (cx + rx * theta.cos(), cy + ry * theta.sin() / 2.0);
                self.buffer.ink(x, y, MINUTE_LIGHT * strength.max(0.6), hue);
            }
        }
    }

    fn draw(&self, layout: &Layout, grid: &mut Grid, light: f32) {
        // The faint levels are dimmer as well as sparser, so the tunnel's wide outer smoke stays
        // behind the windows rather than competing with them.
        self.buffer.draw(grid, FLOOR, &LOOKS, |hue, level| {
            mix(BG, hue_colour(hue, HUES), [0.6, 0.75, 0.9, 1.0][level])
        });
        if light <= 0.02 {
            return;
        }
        let (lx, _, lw, _) = layout.logo;
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, gradient(across), light),
            );
        }
    }
}

impl Variation for Vortex {
    fn id(&self) -> &'static str {
        "vortex"
    }

    fn reset(&mut self, layout: &Layout, _seed: u64) {
        self.buffer.reset(layout.cols, layout.rows);
        let (lx, ly, lw, lh) = layout.logo;
        self.centre = (lx as f32 + lw as f32 / 2.0, ly as f32 + lh as f32 / 2.0);
        let lit = |col: i32, row: i32| {
            layout
                .letters
                .iter()
                .any(|letter| letter.col == col && letter.row == row)
        };
        self.outline = layout
            .letters
            .iter()
            .filter(|letter| {
                [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(dc, dr)| !lit(letter.col + dc, letter.row + dr))
            })
            .map(|letter| (letter.col as f32, letter.row as f32))
            .collect();
        self.outline_half = (lw as f32 / 2.0, lh as f32 / 2.0);
        self.minute = None;
        self.ringing = 0;
        self.still = None;
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        if frame.still {
            // Reduced motion: the tunnel as it stands after a couple of seconds at the start,
            // made once, so every still step is the same picture.
            if self.still.is_none() {
                self.buffer.reset(layout.cols, layout.rows);
                self.ringing = 0;
                for _ in 0..STILL_STEPS {
                    self.advance(0.0, 1.0);
                }
                let mut picture = Grid::new(layout.cols, layout.rows);
                self.draw(layout, &mut picture, 1.0);
                self.still = Some(picture.cells);
            }
            if let Some(cells) = &self.still {
                grid.cells.clone_from(cells);
            }
            return;
        }
        self.still = None;
        let t = frame.elapsed as f32;
        let within = frame.elapsed.rem_euclid(TURN);
        let strength =
            ((within / RISE).min(1.0) * ((TURN - within) / DIM).min(1.0)).clamp(0.0, 1.0) as f32;
        // The minute's ring, when the local clock turns over. Not on the first reading, which
        // would ring whenever the variation took over.
        if let Some(local) = frame.local {
            let minute = (local / 60.0).floor() as i64;
            if self.minute.is_some_and(|last| last != minute) {
                self.ringing = MINUTE_STEPS;
            }
            self.minute = Some(minute);
        }
        if frame.dt > 0.0 {
            self.advance(t, strength);
        }
        // The logo fades with the sources at the end of a turn, so the handover finds it gone.
        let light = smooth(((strength - 0.1) / 0.9 * 1.4).min(1.0));
        self.draw(layout, grid, light);
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }
}

fn smooth(t: f32) -> f32 {
    0.5 - 0.5 * (PI * t.clamp(0.0, 1.0)).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        Readings, layout,
        tests::{at_home, drawn},
    };

    /// Runs the tunnel from the start of its turn to `until`, a step at a time, with the local
    /// clock at `local` plus the time run.
    fn run(until: f64, local: Option<f64>) -> (Vortex, Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut vortex = Vortex::default();
        vortex.reset(&layout, 1);
        let readings = Readings::default();
        let mut grid = Grid::new(layout.cols, layout.rows);
        let mut at = 0.0;
        while at <= until {
            grid = Grid::new(layout.cols, layout.rows);
            vortex.compose(
                Frame {
                    layout: &layout,
                    elapsed: at,
                    dt: (1.0 / HZ) as f32,
                    still: false,
                    readings: &readings,
                    local: local.map(|local| local + at),
                },
                &mut grid,
            );
            at += 1.0 / HZ;
        }
        (vortex, grid, layout)
    }

    #[test]
    fn the_glow_is_carried_out_from_the_logo_and_keeps_moving() {
        let (_, early, layout) = run(0.3, None);
        let (_, later, _) = run(6.0, None);
        let (_, next, _) = run(6.05, None);
        assert_eq!(
            at_home(&later, &layout),
            layout.letters.len(),
            "the logo stands"
        );
        assert!(
            drawn(&later) > drawn(&early) * 3,
            "the tunnel fills out: {} then {}",
            drawn(&early),
            drawn(&later)
        );
        // Out past the logo's box, well away from anything inked directly.
        let (lx, ly, lw, lh) = layout.logo;
        let far = later
            .cells
            .iter()
            .enumerate()
            .filter(|(i, cell)| {
                let (col, row) = (*i as i32 % layout.cols, *i as i32 / layout.cols);
                cell.ch != ' '
                    && ((col < lx - 20 || col > lx + lw + 20)
                        || (row < ly - 10 || row > ly + lh + 10))
            })
            .count();
        assert!(
            far > 100,
            "the light has been carried outwards: {far} cells"
        );
        assert_ne!(later.cells, next.cells, "and it keeps moving");
    }

    #[test]
    fn the_turn_ends_on_a_nearly_empty_screen() {
        let (_, end, layout) = run(TURN - 0.05, None);
        assert!(
            drawn(&end) < layout.letters.len() / 4,
            "{} cells still drawn",
            drawn(&end)
        );
    }

    #[test]
    fn the_minute_rings_when_the_local_clock_turns_over_and_not_before() {
        // The clock three seconds short of a minute.
        let minute = 60.0 * 29_000_000.0 - 3.0;
        let (_, before, _) = run(2.5, Some(minute));
        let (_, without, _) = run(2.5, None);
        assert_eq!(
            before.cells, without.cells,
            "nothing rings within the minute"
        );
        let (_, after, _) = run(3.5, Some(minute));
        let (_, without, _) = run(3.5, None);
        assert_ne!(
            after.cells, without.cells,
            "the ring goes out on the minute"
        );
    }

    #[test]
    fn reduced_motion_shows_one_still_tunnel() {
        let layout = layout(1920, 1200).unwrap();
        let mut vortex = Vortex::default();
        vortex.reset(&layout, 1);
        let readings = Readings::default();
        let mut still = |elapsed: f64| {
            let mut grid = Grid::new(layout.cols, layout.rows);
            vortex.compose(
                Frame {
                    layout: &layout,
                    elapsed,
                    dt: 0.0,
                    still: true,
                    readings: &readings,
                    local: None,
                },
                &mut grid,
            );
            grid
        };
        let first = still(0.0);
        let second = still(17.0);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
        assert!(
            drawn(&first) > layout.letters.len() * 2,
            "the tunnel is there"
        );
    }
}
