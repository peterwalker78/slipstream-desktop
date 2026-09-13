//! `sonar`: a sweep going round, and returns fading behind it.
//!
//! A beam turns about the middle of the screen. Everything it touches answers — the logo's
//! letters and a scatter of contacts around them — and each return fades over the rest of the
//! turn, so the picture is brightest just behind the beam and almost gone in front of it.
//!
//! One contact is under way. It answers in amber, and each time the beam finds it, it has moved on
//! a little, so its last few returns stand behind it as a fading track across the screen.

use std::f32::consts::TAU;

use super::{AMBER, BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, gradient, hash01, mix};

/// Seconds the beam takes to go round once, and how many times it goes round in a turn.
const SWEEP: f64 = 4.2;
const SWEEPS: f64 = 4.0;
const TURN: f64 = SWEEP * SWEEPS;

/// The still picture reduced motion shows: the beam stopped part of the way round.
const STILL: f64 = SWEEP * 1.35;

/// How far behind the beam a return is still lit, as a share of one time round.
const LINGER: f32 = 0.92;

/// Contacts besides the logo, scattered over the screen.
const CONTACTS: u64 = 70;

/// The beam's soft edge: one dimmer ray just behind the bright one, this far behind it in
/// radians. Any more and the beam reads as a fan of lines rather than as one turning.
const TRAIL: i32 = 1;
const TRAIL_STEP: f32 = 0.014;

/// The range rings, as a share of the way to the corner of the screen.
const RINGS: [f32; 2] = [0.42, 0.84];

/// The contact under way: how many cells it makes good a second, how long it holds a course
/// before it is somewhere else on a new one, and how many of its past returns stay on screen.
const VESSEL_SPEED: f32 = 0.8;
const COURSE: f64 = 96.0;
const TRACK: u32 = 5;

/// Where the contact under way is at `time` seconds on the variation's own clock: a straight
/// course across the screen, a new one every `COURSE` seconds.
fn vessel(layout: &Layout, time: f64) -> (f32, f32) {
    let course = (time / COURSE).floor() as u64;
    let into = (time - course as f64 * COURSE) as f32;
    let (cols, rows) = (layout.cols as f32, layout.rows as f32);
    let n = course.wrapping_mul(0x9e37_79b9) ^ 0x7e55e1;
    // Half way through its course it passes somewhere near the middle of the screen, heading any
    // way at all, so the whole course stays on screen.
    let middle = (
        cols * (0.3 + 0.4 * hash01(n ^ 1)),
        rows * (0.35 + 0.3 * hash01(n ^ 2)),
    );
    let heading = hash01(n ^ 3) * TAU;
    let made_good = VESSEL_SPEED * (into - COURSE as f32 / 2.0);
    // Rows count double, so the course is straight and the speed even on screen.
    (
        middle.0 + heading.cos() * made_good,
        middle.1 + heading.sin() * made_good / 2.0,
    )
}

/// Where a cell sits from the middle of the screen: its angle, and how far out it is as a share
/// of the way to the corner. Rows count double, so the sweep is round rather than oval.
fn bearing(layout: &Layout, x: f32, y: f32) -> (f32, f32) {
    let (cx, cy) = (layout.cols as f32 / 2.0, layout.rows as f32 / 2.0);
    let (dx, dy) = (x - cx, (y - cy) * 2.0);
    let reach = cx.hypot(cy * 2.0).max(1.0);
    (dy.atan2(dx).rem_euclid(TAU), dx.hypot(dy) / reach)
}

/// How brightly something at `angle` is still answering a beam that has turned to `beam`.
fn answer(angle: f32, beam: f32) -> f32 {
    let since = (beam - angle).rem_euclid(TAU) / TAU;
    (1.0 - since / LINGER).max(0.0)
}

/// The character a ray takes where it is pointing, so the beam joins up like a drawn line.
fn ray(angle: f32) -> char {
    let (dx, dy) = (angle.cos(), angle.sin());
    let slope = if dx.abs() < 0.001 {
        f32::INFINITY
    } else {
        (dy / dx).abs()
    };
    if slope < 0.45 {
        '─'
    } else if slope > 2.2 {
        '│'
    } else if (dx > 0.0) == (dy > 0.0) {
        '╲'
    } else {
        '╱'
    }
}

#[derive(Default)]
pub struct Sonar;

impl Variation for Sonar {
    fn id(&self) -> &'static str {
        "sonar"
    }

    fn reset(&mut self, _layout: &Layout, _seed: u64) {}

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed.rem_euclid(TURN)
        };
        // The beam comes up at the start of a turn and dies away at the end of the last time
        // round, so the screen the next variation fades over is an empty one.
        let envelope = ((t / 0.9).min(1.0) * ((TURN - 0.5 - t) / 1.4).clamp(0.0, 1.0)) as f32;
        if envelope <= 0.0 {
            return;
        }
        let beam = (t / SWEEP).rem_euclid(1.0) as f32 * TAU;
        let (cx, cy) = (layout.cols as f32 / 2.0, layout.rows as f32 / 2.0);
        let reach = cx.hypot(cy * 2.0);

        // The range rings, faint and still, so the sweep reads as a screen and not as a line
        // going round on its own.
        for ring in RINGS {
            let radius = ring * reach;
            let step = (1.1 / radius.max(1.0)).max(0.01);
            let mut angle = 0.0;
            while angle < TAU {
                let (x, y) = (cx + angle.cos() * radius, cy + angle.sin() * radius / 2.0);
                grid.put(x, y, '·', mix(BG, CYAN, 0.13 * envelope));
                angle += step;
            }
        }

        // The contacts the beam picks up besides the logo.
        for contact in 0..CONTACTS {
            let x = hash01(contact ^ 0x51de) * layout.cols as f32;
            let y = hash01(contact ^ 0x7e51) * layout.rows as f32;
            let (angle, _) = bearing(layout, x, y);
            let light = answer(angle, beam) * envelope;
            if light <= 0.05 {
                continue;
            }
            let ch = if light > 0.75 { '*' } else { '·' };
            grid.put(x, y, ch, mix(BG, mix(MINT, WHITE, light * 0.4), light));
        }

        // The contact under way: its return where the beam last found it, and the ones before
        // that, dimmer the older they are. Found at the bearing it had, which it barely changes
        // in the time a sweep takes.
        let clock = if frame.still { STILL } else { frame.elapsed };
        let (x, y) = vessel(layout, clock);
        let (angle, _) = bearing(layout, x, y);
        let mut latest = ((clock / SWEEP).floor() + angle as f64 / TAU as f64) * SWEEP;
        if latest > clock {
            latest -= SWEEP;
        }
        for back in 0..TRACK {
            let found = latest - back as f64 * SWEEP;
            if found < 0.0 {
                break;
            }
            let (x, y) = vessel(layout, found);
            let light = if back == 0 {
                answer(bearing(layout, x, y).0, beam).max(0.25)
            } else {
                0.6 * 0.7f32.powi(back as i32 - 1)
            } * envelope;
            if light <= 0.04 {
                continue;
            }
            let ch = if back == 0 { '◆' } else { '·' };
            grid.put(x, y, ch, mix(BG, mix(AMBER, WHITE, light * 0.3), light));
        }

        // The logo answers letter by letter as the beam crosses it.
        let (lx, _, lw, _) = layout.logo;
        for letter in &layout.letters {
            let (angle, _) = bearing(layout, letter.col as f32, letter.row as f32);
            let light = answer(angle, beam) * envelope;
            if light <= 0.04 {
                continue;
            }
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            let home = gradient(across);
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, mix(home, WHITE, (light - 0.8).max(0.0) * 3.0), light),
            );
        }

        // The beam itself, drawn from the middle outwards, with a couple of dimmer rays just
        // behind it for a soft edge.
        for back in 0..=TRAIL {
            let angle = beam - back as f32 * TRAIL_STEP;
            let light = (1.0 - back as f32 / (TRAIL + 1) as f32).powi(2) * envelope;
            let ch = ray(angle);
            let (dx, dy) = (angle.cos(), angle.sin() / 2.0);
            let mut out = 1.0;
            while out < reach {
                let (x, y) = (cx + dx * out, cy + dy * out);
                if x < 0.0 || y < 0.0 || x >= layout.cols as f32 || y >= layout.rows as f32 {
                    break;
                }
                let rgb = if back == 0 { WHITE } else { MINT };
                // The beam dims towards the rim, as the light spreads out along it.
                let far = 1.0 - 0.45 * (out / reach).min(1.0);
                grid.put(x, y, ch, mix(BG, mix(rgb, CYAN, 0.15), light * far));
                out += 0.7;
            }
        }
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
        tests::{composed, drawn},
    };

    fn sonar(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let grid = composed(&mut Sonar, &layout, elapsed, still);
        (grid, layout)
    }

    /// How many of the logo's letters are answering.
    fn answering(grid: &Grid, layout: &Layout) -> usize {
        layout
            .letters
            .iter()
            .filter(|letter| grid.at(letter.col as f32, letter.row as f32).ch == letter.ch)
            .count()
    }

    #[test]
    fn a_return_is_brightest_behind_the_beam_and_gone_in_front_of_it() {
        assert!(answer(1.0, 1.01) > 0.99, "just behind the beam");
        assert!(
            answer(1.0, 1.0 + TAU * 0.5) < 0.5,
            "half a turn behind it, well down"
        );
        assert_eq!(
            answer(1.0, 1.0 + TAU * 0.95),
            0.0,
            "almost the way round, gone"
        );
        assert_eq!(answer(1.0, 1.0 - 0.2), 0.0, "in front of it, nothing");
    }

    #[test]
    fn the_beam_goes_round_and_the_logo_answers_as_it_passes() {
        let (quarter, layout) = sonar(SWEEP * 1.25, false);
        let (half, _) = sonar(SWEEP * 1.5, false);
        assert_ne!(quarter.cells, half.cells, "the beam has turned");
        // Going round, all of it has answered at one point or another.
        let most = (0..8)
            .map(|step| {
                let (grid, layout) = sonar(SWEEP * (1.0 + step as f64 / 8.0), false);
                answering(&grid, &layout)
            })
            .max()
            .unwrap();
        assert!(
            most > layout.letters.len() / 2,
            "the logo answers: {most} of {}",
            layout.letters.len()
        );
    }

    #[test]
    fn the_sweep_dies_away_at_the_end_of_a_turn() {
        let (gap, _) = sonar(TURN - 0.1, false);
        assert_eq!(drawn(&gap), 0, "the turn ends on an empty screen");
    }

    #[test]
    fn one_contact_is_under_way_and_leaves_a_track() {
        let layout = layout(1920, 1200).unwrap();
        let (x0, y0) = vessel(&layout, 10.0);
        let (x1, y1) = vessel(&layout, 10.0 + SWEEP);
        let moved = (x1 - x0).hypot((y1 - y0) * 2.0);
        assert!(
            (moved - VESSEL_SPEED * SWEEP as f32).abs() < 0.01,
            "it makes good its speed: {moved}"
        );
        // Well into a course, its returns stand in a line behind it.
        let (grid, _) = sonar(SWEEP * 3.6, false);
        let track = grid.cells.iter().filter(|cell| cell.ch == '◆').count()
            + grid
                .cells
                .iter()
                .filter(|cell| cell.ch == '·' && cell.rgb[0] > cell.rgb[2])
                .count();
        assert!(
            track >= 3,
            "the latest return and the track behind it: {track}"
        );
    }

    #[test]
    fn reduced_motion_stops_the_beam() {
        let (first, _) = sonar(0.0, true);
        let (second, _) = sonar(9.0, true);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert!(drawn(&first) > 0, "the beam is still drawn");
    }
}
