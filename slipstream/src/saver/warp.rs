//! `warp`: the logo at the still point of a tunnel.
//!
//! Specks stream outwards from a vanishing point, accelerating as they go, and every step the
//! picture before is carried after them by a feedback buffer, zoomed out from the vanishing point
//! and turned a little. So each speck draws its own lengthening, gently curving streak, and the
//! screen reads as the inside of a tunnel at speed. The logo comes up out of the vanishing point,
//! holds while the tunnel streams past it, then blows past the screen and leaves a spiral wake.
//!
//! The tunnel steers: its vanishing point wanders on a slow figure of eight, and it banks into
//! each turn, so the streaks sweep one way and then the other while the logo holds its place.

use super::{
    BG, CYAN, Cell, Feedback, Frame, Grid, Layout, Motion, Variation, WHITE, gradient, hash01,
    hue_byte, hue_colour, mix, smoothstep,
};

/// One turn, in seconds of the animation clock.
const ARRIVE: f64 = 3.0;
const HOLD: f64 = 7.0;
const LEAVE: f64 = 2.5;
const GAP: f64 = 1.4;
const TURN: f64 = ARRIVE + HOLD + LEAVE + GAP;

/// The still picture reduced motion shows: the logo in place, the tunnel as it stood.
const STILL: f64 = ARRIVE + HOLD / 2.0;
/// Steps the still picture is run for before it is kept.
const STILL_STEPS: u32 = 30;

/// How many specks fly at once.
const SPECKS: u64 = 120;
/// Seconds a speck takes to travel from the middle to the edge.
const FLIGHT: f32 = 2.4;

/// How far from the middle the logo starts, and how far past the screen it goes: the scale its
/// distance from the middle is multiplied by at either end of its flight.
const FAR: f32 = 0.06;
const PAST: f32 = 11.0;

/// Steps a second, and how the tunnel carries the picture each step: out from the vanishing point
/// by `ZOOM`, turning by `SPIN` radians, and keeping `DECAY` of its light (half a second to the floor).
const HZ: f64 = 20.0;
const ZOOM: f32 = 1.045;
const SPIN: f32 = 0.0105;
const DECAY: f32 = 0.8;
const FLOOR: f32 = 0.12;
const LOOKS: [char; 4] = ['░', '▒', '▓', '█'];
/// The most cells of its own path a speck inks in one step.
const STREAK: f32 = 5.0;

/// How far the vanishing point wanders, in cells across and down, and the periods of the figure of
/// eight it wanders on. The bank adds up to `BANK` radians a step to the spin on the turns.
const STEER: (f32, f32) = (9.0, 3.0);
const STEER_PERIOD: (f32, f32) = (37.0, 18.5);
const BANK: f32 = 0.014;

/// Where the vanishing point is at time `t`, as an offset from the middle of the screen in cells,
/// and how hard the tunnel is banking there.
fn steer(t: f32) -> ((f32, f32), f32) {
    use std::f32::consts::TAU;
    let (a, b) = (TAU * t / STEER_PERIOD.0, TAU * t / STEER_PERIOD.1);
    // Banks with the sideways swing: hardest where it moves fastest.
    ((STEER.0 * a.sin(), STEER.1 * b.sin()), BANK * a.cos())
}

/// A cell's distance from the middle of the screen, in cells, with rows counted double so that
/// the picture is round on a screen whose cells are twice as tall as they are wide.
fn from_middle(layout: &Layout, x: f32, y: f32) -> (f32, f32) {
    let (cx, cy) = (layout.cols as f32 / 2.0, layout.rows as f32 / 2.0);
    (x - cx, (y - cy) * 2.0)
}

fn to_cell(layout: &Layout, dx: f32, dy: f32) -> (f32, f32) {
    let (cx, cy) = (layout.cols as f32 / 2.0, layout.rows as f32 / 2.0);
    (cx + dx, cy + dy / 2.0)
}

/// The character a streak takes, from the direction it is travelling.
fn streak(dx: f32, dy: f32) -> char {
    let slope = if dx.abs() < 0.001 {
        f32::INFINITY
    } else {
        (dy / dx).abs()
    };
    if slope < 0.5 {
        '─'
    } else if slope > 2.0 {
        '│'
    } else if (dx > 0.0) == (dy > 0.0) {
        '╲'
    } else {
        '╱'
    }
}

#[derive(Default)]
pub struct Warp {
    buffer: Feedback,
    /// The still picture, made once.
    still: Option<Vec<Cell>>,
}

impl Warp {
    /// One step of the tunnel at `t` seconds into the turn, `clock` seconds since it took over
    /// (which steers it, so the steering carries on across turns): carry the last picture on,
    /// then ink the specks, and the logo's streaks if it is blowing past.
    fn advance(&mut self, layout: &Layout, t: f64, clock: f32, envelope: f32) {
        let ((ox, oy), bank) = steer(clock);
        let middle = (layout.cols as f32 / 2.0, layout.rows as f32 / 2.0);
        let vanishing = (middle.0 + ox, middle.1 + oy);
        let motion = Motion {
            centre: vanishing,
            zoom: ZOOM,
            turn: SPIN + bank,
            shift: (0.0, 0.0),
        };
        self.buffer.advance(DECAY, |x, y| motion.source(x, y));
        if envelope <= 0.0 {
            return;
        }
        let edge = (layout.cols as f32 / 2.0).hypot(layout.rows as f32);
        let step = (1.0 / HZ) as f32;
        for speck in 0..SPECKS {
            let angle = hash01(speck) * std::f32::consts::TAU;
            let along = (t as f32 / FLIGHT + hash01(speck ^ 0x0f1e)).rem_euclid(1.0);
            let before = along - step / FLIGHT;
            if before < 0.0 {
                continue;
            }
            // Cyan, with one speck in eleven mint and one in seventeen amber.
            let hue = if speck % 17 == 0 {
                0.0
            } else if speck % 11 == 0 {
                1.0 / 3.0
            } else {
                2.0 / 3.0
            };
            let light = (0.3 + along * 0.8).min(1.0) * envelope;
            let (cos, sin) = (angle.cos(), angle.sin() / 2.0);
            // The way it came this step, so a fast speck draws a line rather than a dotted one,
            // but no more than a few cells of it: the buffer's zoom stretches the rest.
            let head = (
                vanishing.0 + cos * along * along * edge,
                vanishing.1 + sin * along * along * edge,
            );
            let tail = (
                vanishing.0 + cos * before * before * edge,
                vanishing.1 + sin * before * before * edge,
            );
            self.buffer
                .line(shortened(tail, head, STREAK), head, light, hue_byte(hue));
        }
        if let (Some(scale), Some(was)) = (logo_scale(t), logo_scale(t - f64::from(step))) {
            if scale > 1.2 {
                // A third of the letters leave streaks: all of them draw a lattice of dashes.
                let (lx, _, lw, _) = layout.logo;
                for (i, letter) in layout.letters.iter().enumerate() {
                    if hash01(i as u64 ^ 0x57ea) > 0.35 {
                        continue;
                    }
                    let (dx, dy) = from_middle(layout, letter.col as f32, letter.row as f32);
                    let across = (letter.col - lx) as f32 / lw.max(1) as f32;
                    let head = to_cell(layout, dx * scale, dy * scale);
                    let tail = to_cell(layout, dx * was, dy * was);
                    self.buffer.line(
                        shortened(tail, head, STREAK * 2.0),
                        head,
                        0.75 * envelope,
                        hue_byte(across * 2.0 / 3.0),
                    );
                }
            }
        }
    }

    fn draw(&self, layout: &Layout, grid: &mut Grid, t: f64, clock: f32, envelope: f32) {
        // The brightest streaks go most of the way to white, as things near the edge of a tunnel
        // do; the faint ones are dimmer as well as sparser.
        self.buffer.draw(grid, FLOOR, &LOOKS, |hue, level| {
            let colour = mix(
                hue_colour(hue, 6),
                WHITE,
                if level == 3 { 0.35 } else { 0.0 },
            );
            mix(BG, colour, [0.55, 0.7, 0.85, 1.0][level])
        });
        if envelope <= 0.0 {
            return;
        }
        // The specks' own heads, crisp, over their streaks.
        let ((ox, oy), _) = steer(clock);
        let vanishing = (layout.cols as f32 / 2.0 + ox, layout.rows as f32 / 2.0 + oy);
        let edge = (layout.cols as f32 / 2.0).hypot(layout.rows as f32);
        for speck in 0..SPECKS {
            let angle = hash01(speck) * std::f32::consts::TAU;
            let along = (t as f32 / FLIGHT + hash01(speck ^ 0x0f1e)).rem_euclid(1.0);
            if along < 0.2 {
                continue;
            }
            let out = along * along * edge;
            let (dx, dy) = (angle.cos() * out, angle.sin() * out);
            let ch = if along < 0.45 { '·' } else { streak(dx, dy) };
            grid.put(
                vanishing.0 + dx,
                vanishing.1 + dy / 2.0,
                ch,
                mix(BG, mix(CYAN, WHITE, along), envelope * (0.3 + 0.7 * along)),
            );
        }

        let Some(scale) = logo_scale(t) else {
            return;
        };
        let fade = if t < ARRIVE {
            ((t / ARRIVE) as f32 * 2.0).min(1.0)
        } else {
            1.0
        };
        // Rushing past, the letters give way to the streaks they leave in the buffer.
        let fade = fade * (1.0 - (scale - 1.0) / 0.8).clamp(0.0, 1.0);
        if fade <= 0.0 {
            return;
        }
        let (lx, _, lw, _) = layout.logo;
        for letter in &layout.letters {
            let (dx, dy) = from_middle(layout, letter.col as f32, letter.row as f32);
            let (x, y) = to_cell(layout, dx * scale, dy * scale);
            if x < -1.0 || y < -1.0 || x > layout.cols as f32 || y > layout.rows as f32 {
                continue;
            }
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            // Coming in it is a white speck that takes the logo's colours as it grows.
            let rgb = mix(
                BG,
                mix(WHITE, gradient(across), smoothstep(scale.min(1.0))),
                fade * envelope,
            );
            if scale > 1.05 {
                // Bigger than its cell, a letter covers the cells it has grown over, so the logo
                // doesn't break into dots on its way past.
                let size = scale.ceil() as i32;
                for row in 0..size {
                    for col in 0..size {
                        grid.put(x.floor() + col as f32, y.floor() + row as f32, '█', rgb);
                    }
                }
            } else {
                grid.put(x, y, letter.ch, rgb);
            }
        }
    }
}

/// `from`, moved along the line towards `to` until it is at most `cells` away.
fn shortened(from: (f32, f32), to: (f32, f32), cells: f32) -> (f32, f32) {
    let (dx, dy) = (from.0 - to.0, from.1 - to.1);
    let long = dx.abs().max(dy.abs());
    if long <= cells {
        from
    } else {
        (to.0 + dx * cells / long, to.1 + dy * cells / long)
    }
}

/// How far the logo is from where it sits at `t` into the turn: tiny and far off during the
/// arrival, 1 while it holds, then rushing past the screen; nothing once it has gone.
fn logo_scale(t: f64) -> Option<f32> {
    if t < ARRIVE {
        let e = smoothstep((t / ARRIVE) as f32);
        Some(FAR + (1.0 - FAR) * e)
    } else if t < ARRIVE + HOLD {
        Some(1.0)
    } else if t < ARRIVE + HOLD + LEAVE {
        let u = ((t - ARRIVE - HOLD) / LEAVE) as f32;
        Some(1.0 + (PAST - 1.0) * u * u)
    } else {
        None
    }
}

/// Everything fades in at the start of a turn and out at the end, so the screen the next
/// variation fades over is an empty one.
fn envelope(t: f64) -> f32 {
    (t / 0.8)
        .min(1.0)
        .min((TURN - 0.4 - t) / 1.0)
        .clamp(0.0, 1.0) as f32
}

impl Variation for Warp {
    fn id(&self) -> &'static str {
        "warp"
    }

    fn reset(&mut self, layout: &Layout, _seed: u64) {
        self.buffer.reset(layout.cols, layout.rows);
        self.still = None;
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        if frame.still {
            if self.still.is_none() {
                self.buffer.reset(layout.cols, layout.rows);
                for k in 0..STILL_STEPS {
                    let t = STILL - (STILL_STEPS - k) as f64 / HZ;
                    self.advance(layout, t, t as f32, 1.0);
                }
                let mut picture = Grid::new(layout.cols, layout.rows);
                self.draw(layout, &mut picture, STILL, STILL as f32, 1.0);
                self.still = Some(picture.cells);
            }
            if let Some(cells) = &self.still {
                grid.cells.clone_from(cells);
            }
            return;
        }
        self.still = None;
        let t = frame.elapsed.rem_euclid(TURN);
        let clock = frame.elapsed as f32;
        let envelope = envelope(t);
        if frame.dt > 0.0 {
            self.advance(layout, t, clock, envelope);
        }
        self.draw(layout, grid, t, clock, envelope);
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    /// Runs the tunnel a step at a time from the start of its turn to `elapsed`, as the saver
    /// does; with `still`, one reduced-motion compose instead.
    fn warp(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut warp = Warp::default();
        warp.reset(&layout, 0);
        if still {
            return (composed(&mut warp, &layout, elapsed, true), layout);
        }
        let mut grid = Grid::new(layout.cols, layout.rows);
        let mut at = 0.0;
        while at <= elapsed + 1e-9 {
            grid = composed(&mut warp, &layout, at, false);
            at += 1.0 / HZ;
        }
        (grid, layout)
    }

    #[test]
    fn the_logo_comes_out_of_the_middle_and_holds_in_its_place() {
        let (arriving, layout) = warp(ARRIVE * 0.2, false);
        assert!(
            at_home(&arriving, &layout) < layout.letters.len() / 8,
            "on its way in it is nowhere near its place"
        );
        let (lx, ly, lw, lh) = layout.logo;
        let (middle, _) = (lx + lw / 2, ly + lh / 2);
        let near = arriving
            .cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.ch != ' ')
            .filter(|(i, _)| {
                let col = (*i as i32) % layout.cols;
                (col - middle).abs() < lw / 4
            })
            .count();
        assert!(near > 0, "it is drawn near the middle instead");

        let (holding, layout) = warp(ARRIVE + HOLD / 2.0, false);
        assert_eq!(
            at_home(&holding, &layout),
            layout.letters.len(),
            "it holds in its place with every letter home"
        );
    }

    #[test]
    fn the_specks_fly_and_the_turn_ends_on_an_empty_screen() {
        let (early, _) = warp(1.0, false);
        let (later, _) = warp(1.2, false);
        assert_ne!(early.cells, later.cells, "the specks are moving");
        // The buffer draws streaks behind the specks: far more is lit than the specks alone.
        let (holding, _) = warp(ARRIVE + HOLD / 2.0, false);
        assert!(
            drawn(&holding) > SPECKS as usize * 3,
            "{} cells lit",
            drawn(&holding)
        );
        let (gap, _) = warp(TURN - 0.2, false);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn the_tunnel_steers_and_banks_into_its_turns() {
        let ((x0, _), bank0) = steer(0.0);
        let ((x1, _), bank1) = steer(STEER_PERIOD.0 / 4.0);
        assert_eq!(x0, 0.0, "straight ahead to begin with");
        assert!((x1 - STEER.0).abs() < 0.01, "then off to one side");
        assert!(bank0 > bank1.abs() * 10.0, "banking hardest mid-swing");
    }

    #[test]
    fn reduced_motion_holds_the_logo_still_among_stopped_specks() {
        let (first, layout) = warp(0.0, true);
        let (second, _) = warp(11.0, true);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
    }
}
