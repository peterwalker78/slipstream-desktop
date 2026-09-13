//! `circuit`: traces reaching the logo, and pulses running along them.
//!
//! Tracks are drawn in from the edges of the screen, one corner at a time, the way a circuit
//! board routes a signal. Each one ends at a letter of the logo, which lights as its track
//! arrives. Once they are all in, pulses run along them; then the tracks withdraw the way they
//! came and the light goes out.
//!
//! Each turn, one track carries a signal of its own: its pulses run amber, and every time one
//! lands, its pad sparks and the letters beside it flash.

use super::{AMBER, BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, gradient, hash01, mix};

/// One turn, in seconds of the animation clock.
const GROW: f64 = 4.0;
const HOLD: f64 = 8.0;
const WITHDRAW: f64 = 2.8;
const GAP: f64 = 1.2;
const TURN: f64 = GROW + HOLD + WITHDRAW + GAP;

/// The still picture reduced motion shows: mid-hold, with the pulses stopped.
const STILL: f64 = GROW + HOLD / 2.0;

/// How many tracks are routed.
const TRACKS: usize = 26;
/// How long a pulse is, in cells, and how fast it runs, in cells a second.
const PULSE: i32 = 5;
const PULSE_SPEED: f32 = 34.0;

/// How long a carried pulse's spark lasts once it lands, in cells of the pulse's travel, and how
/// far round its pad the letters catch it, in columns and rows.
const SPARK: f32 = 16.0;
const SPARK_REACH: (i32, i32) = (7, 3);

/// One cell of a track, with the character that joins it to its neighbours.
#[derive(Clone, Copy)]
struct Step {
    col: i32,
    row: i32,
    ch: char,
}

/// A track from the edge of the screen to one letter, and when it starts to grow.
struct Track {
    steps: Vec<Step>,
    delay: f32,
}

/// The corner or line a cell takes, from the way the track turns at it.
fn joint(into: (i32, i32), out: (i32, i32)) -> char {
    match (into, out) {
        ((_, 0), (_, 0)) => '─',
        ((0, _), (0, _)) => '│',
        // Coming in along a row and leaving along a column, or the other way about: the corner
        // is named for the two directions it joins.
        ((1, 0), (0, 1)) | ((0, -1), (-1, 0)) => '┐',
        ((1, 0), (0, -1)) | ((0, 1), (-1, 0)) => '┘',
        ((-1, 0), (0, 1)) | ((0, -1), (1, 0)) => '┌',
        ((-1, 0), (0, -1)) | ((0, 1), (1, 0)) => '└',
        _ => '─',
    }
}

/// A track through `points`, one leg at a time: along a row to the next point's column, then
/// along that column to its row, which is how a board routes a signal.
fn route(points: &[(i32, i32)]) -> Vec<Step> {
    let Some(&start) = points.first() else {
        return Vec::new();
    };
    let mut cells = vec![start];
    let (mut col, mut row) = start;
    for &point in &points[1..] {
        for leg in [(point.0, row), point] {
            while (col, row) != leg {
                col += (leg.0 - col).signum();
                row += (leg.1 - row).signum();
                cells.push((col, row));
            }
        }
    }
    // Every cell's character comes from the cells either side of it.
    let mut steps = Vec::with_capacity(cells.len());
    for (i, &(col, row)) in cells.iter().enumerate() {
        let before = cells.get(i.wrapping_sub(1)).copied().unwrap_or((col, row));
        let after = cells.get(i + 1).copied().unwrap_or((col, row));
        let into = ((col - before.0).signum(), (row - before.1).signum());
        let out = ((after.0 - col).signum(), (after.1 - row).signum());
        let into = if into == (0, 0) { out } else { into };
        let out = if out == (0, 0) { into } else { out };
        steps.push(Step {
            col,
            row,
            ch: joint(into, out),
        });
    }
    steps
}

/// A point `along` of the way from `from` to `to`.
fn between(from: i32, to: i32, along: f32) -> i32 {
    from + ((to - from) as f32 * along).round() as i32
}

#[derive(Default)]
pub struct Circuit {
    tracks: Vec<Track>,
}

impl Variation for Circuit {
    fn id(&self) -> &'static str {
        "circuit"
    }

    fn reset(&mut self, layout: &Layout, seed: u64) {
        self.tracks.clear();
        if layout.letters.is_empty() {
            return;
        }
        let (lx, ly, lw, lh) = layout.logo;
        for track in 0..TRACKS as i32 {
            let n = seed ^ (track as u64).wrapping_mul(0x9e37_79b9);
            // Every track ends on a pad just outside the logo's box and comes at it from the
            // side of the screen that pad faces, turning a corner or two on the way. Nothing is
            // ever routed through the box itself, so the logo is never drawn over.
            let points = if track % 4 == 0 {
                let left = (track / 4) % 2 == 0;
                let pad = (
                    if left { lx - 2 } else { lx + lw + 1 },
                    ly + (hash01(n ^ 0x11) * lh as f32) as i32,
                );
                let start = (
                    if left { 0 } else { layout.cols - 1 },
                    (hash01(n ^ 0x22) * layout.rows as f32) as i32,
                );
                // The corner is somewhere between the edge and the pad, so no two tracks on a
                // side turn in the same place.
                let via = (
                    between(start.0, pad.0, 0.25 + hash01(n ^ 0x55) * 0.5),
                    between(start.1, pad.1, 0.3 + hash01(n ^ 0x66) * 0.4),
                );
                [start, via, pad]
            } else {
                // The rest come down from the top or up from the bottom, spread along the box
                // with a little play, so they don't line up like a comb.
                let above = track % 2 == 0;
                let along = (track as f32 + 0.5) / TRACKS as f32;
                let pad = (
                    lx + (along * lw as f32) as i32 + ((hash01(n ^ 0x77) - 0.5) * 7.0) as i32,
                    if above { ly - 2 } else { ly + lh + 1 },
                );
                let start = (
                    (hash01(n ^ 0x33) * layout.cols as f32) as i32,
                    if above { 0 } else { layout.rows - 1 },
                );
                let via = (
                    between(start.0, pad.0, 0.3 + hash01(n ^ 0x55) * 0.45),
                    between(start.1, pad.1, 0.35 + hash01(n ^ 0x66) * 0.4),
                );
                [start, via, pad]
            };
            self.tracks.push(Track {
                steps: route(&points),
                delay: hash01(n ^ 0x44) * 0.55,
            });
        }
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed.rem_euclid(TURN)
        };
        if t >= GROW + HOLD + WITHDRAW {
            return;
        }
        let (lx, _, lw, _) = layout.logo;
        // The track carrying this turn's signal, and how brightly its pad is sparking.
        let turn = if frame.still {
            0
        } else {
            (frame.elapsed / TURN).floor() as u64
        };
        let carrier = (hash01(turn ^ 0xca77) * self.tracks.len() as f32) as usize;
        let mut spark: Option<(i32, i32, f32)> = None;

        for (index, track) in self.tracks.iter().enumerate() {
            let len = track.steps.len() as f32;
            if len < 1.0 {
                continue;
            }
            // How much of the track is drawn: out from the edge while it grows, back to the edge
            // while it withdraws.
            let out = if t < GROW {
                let grown =
                    (((t / GROW) as f32 - track.delay) / (1.0 - track.delay)).clamp(0.0, 1.0);
                grown * len
            } else if t < GROW + HOLD {
                len
            } else {
                let gone =
                    (((t - GROW - HOLD) / WITHDRAW) as f32 - track.delay * 0.5).clamp(0.0, 1.0);
                len * (1.0 - gone)
            };
            let head = out.floor() as usize;
            for step in track.steps.iter().take(head) {
                grid.put(
                    step.col as f32,
                    step.row as f32,
                    step.ch,
                    mix(BG, CYAN, 0.42),
                );
            }
            // The growing end is brighter than the track behind it.
            if head > 0 && head < track.steps.len() {
                let step = track.steps[head - 1];
                grid.put(
                    step.col as f32,
                    step.row as f32,
                    step.ch,
                    mix(BG, WHITE, 0.8),
                );
            }
            // Pulses only run once the track has arrived, and they run towards the logo.
            if (GROW..GROW + HOLD).contains(&t) {
                let along = ((t - GROW) as f32 * PULSE_SPEED + track.delay * 90.0) % (len * 1.6);
                for back in 0..PULSE {
                    let at = along - back as f32;
                    if at < 0.0 || at >= len {
                        continue;
                    }
                    let step = track.steps[at as usize];
                    let light = 1.0 - back as f32 / PULSE as f32;
                    let pulse = if index == carrier {
                        mix(AMBER, WHITE, 0.25)
                    } else {
                        mix(MINT, AMBER, 0.3)
                    };
                    grid.put(
                        step.col as f32,
                        step.row as f32,
                        step.ch,
                        mix(mix(BG, CYAN, 0.42), pulse, light),
                    );
                }
                // The carried pulse has landed: its pad sparks for a moment.
                let since = along - (len - 1.0);
                if index == carrier && (0.0..SPARK).contains(&since) {
                    if let Some(pad) = track.steps.last() {
                        spark = Some((pad.col, pad.row, 1.0 - since / SPARK));
                    }
                }
            }
        }

        // The logo lights letter by letter as the tracks land on their pads, and goes out as
        // they withdraw.
        let arrived = if t < GROW {
            (t / GROW) as f32
        } else if t < GROW + HOLD {
            1.0
        } else {
            1.0 - ((t - GROW - HOLD) / WITHDRAW) as f32
        };
        for (i, letter) in layout.letters.iter().enumerate() {
            let rank = hash01(i as u64 ^ 0xc1c1);
            if rank > arrived {
                continue;
            }
            // Each letter comes up as the light reaches it, and the whole logo is up for as long
            // as every track is in.
            let light = if (GROW..GROW + HOLD).contains(&t) {
                1.0
            } else {
                ((arrived - rank) * 6.0).clamp(0.0, 1.0)
            };
            if light <= 0.0 {
                continue;
            }
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            let mut rgb = gradient(across);
            if let Some((col, row, glow)) = spark {
                let (dc, dr) = ((letter.col - col).abs(), (letter.row - row).abs());
                if dc <= SPARK_REACH.0 && dr <= SPARK_REACH.1 {
                    let near = 1.0
                        - (dc as f32 / SPARK_REACH.0 as f32).max(dr as f32 / SPARK_REACH.1 as f32)
                            * 0.6;
                    rgb = mix(rgb, mix(AMBER, WHITE, 0.55), glow * near);
                }
            }
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, rgb, light),
            );
        }
        if let Some((col, row, glow)) = spark {
            grid.put(
                col as f32,
                row as f32,
                '◆',
                mix(BG, mix(AMBER, WHITE, 0.5), 0.4 + 0.6 * glow),
            );
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
        tests::{at_home, composed, drawn},
    };

    fn circuit(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut circuit = Circuit::default();
        circuit.reset(&layout, 3);
        let grid = composed(&mut circuit, &layout, elapsed, still);
        (grid, layout)
    }

    #[test]
    fn a_track_runs_from_the_edge_of_the_screen_to_its_pad_one_leg_at_a_time() {
        let steps = route(&[(0, 5), (6, 8), (10, 9)]);
        assert_eq!(steps.first().map(|s| (s.col, s.row)), Some((0, 5)));
        assert_eq!(steps.last().map(|s| (s.col, s.row)), Some((10, 9)));
        for pair in steps.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert_eq!(
                (a.col - b.col).abs() + (a.row - b.row).abs(),
                1,
                "each step is one cell from the last"
            );
        }
        assert!(
            steps.iter().any(|step| step.ch == '┐' || step.ch == '┘'),
            "it turns a corner on the way"
        );
    }

    #[test]
    fn no_track_is_routed_through_the_logo() {
        let layout = layout(1920, 1200).unwrap();
        let (lx, ly, lw, lh) = layout.logo;
        for seed in 0..8 {
            let mut circuit = Circuit::default();
            circuit.reset(&layout, seed);
            for track in &circuit.tracks {
                for step in &track.steps {
                    assert!(
                        !((lx - 1..lx + lw + 1).contains(&step.col)
                            && (ly - 1..ly + lh + 1).contains(&step.row)),
                        "a track crosses the logo's box at {}, {}",
                        step.col,
                        step.row
                    );
                }
            }
        }
    }

    #[test]
    fn the_tracks_grow_in_light_the_logo_and_withdraw() {
        let (early, layout) = circuit(0.2, false);
        let (grown, _) = circuit(GROW + HOLD / 2.0, false);
        assert!(
            drawn(&grown) > drawn(&early) * 2,
            "the tracks reach further: {} then {}",
            drawn(&early),
            drawn(&grown)
        );
        assert_eq!(
            at_home(&grown, &layout),
            layout.letters.len(),
            "the logo is lit once they have all arrived"
        );
        assert!(
            at_home(&early, &layout) < layout.letters.len() / 4,
            "at the start hardly any of it is lit"
        );
        let (gap, _) = circuit(TURN - 0.1, false);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn a_carried_pulse_sparks_its_pad_when_it_lands() {
        let layout = layout(1920, 1200).unwrap();
        let mut circuit = Circuit::default();
        circuit.reset(&layout, 3);
        let carrier = (hash01(0 ^ 0xca77) * circuit.tracks.len() as f32) as usize;
        let track = &circuit.tracks[carrier];
        let len = track.steps.len() as f32;
        // The first time its pulse reaches the pad, a little after.
        let lands = ((len - 1.0) + 4.0 - track.delay * 90.0).rem_euclid(len * 1.6) / PULSE_SPEED;
        let grid = composed(&mut circuit, &layout, GROW + lands as f64, false);
        let pad = circuit.tracks[carrier].steps.last().copied().unwrap();
        assert_eq!(
            grid.at(pad.col as f32, pad.row as f32).ch,
            '◆',
            "the pad sparks"
        );
        let quiet = composed(&mut circuit, &layout, GROW + lands as f64 + 1.0, false);
        assert_ne!(
            quiet.at(pad.col as f32, pad.row as f32).ch,
            '◆',
            "and then it's over"
        );
    }

    #[test]
    fn reduced_motion_stops_the_pulses() {
        let (first, layout) = circuit(0.0, true);
        let (second, _) = circuit(6.0, true);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
    }
}
