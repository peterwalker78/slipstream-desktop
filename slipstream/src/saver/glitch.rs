//! `glitch`: the logo as a corrupted video stream.
//!
//! It sits steady for a while, and then the stream starts to break. The picture is carried from
//! step to step in a feedback buffer, and a tear sets bands of it moving sideways without a fresh
//! picture to replace them, the way a video decoder smears the last good frame along stale motion
//! vectors when the keyframe it needed was lost. The smear hangs and fades after the band snaps
//! back, a cyan and an amber echo split either side of it, and characters corrupt. At the end the
//! whole picture is sucked into a swirl and fades out.
//!
//! In one tear each turn, the corruption in a torn band reads the local time.

use std::f32::consts::PI;

use super::{
    BG, Feedback, Frame, Grid, Layout, Motion, Variation, WHITE, gradient, hash01, hue_byte,
    hue_colour, mix,
};

/// One turn, in seconds of the animation clock.
const CLEAN: f64 = 4.0;
const TEARING: f64 = 6.0;
const COLLAPSE: f64 = 2.2;
const GAP: f64 = 1.2;
const TURN: f64 = CLEAN + TEARING + COLLAPSE + GAP;

/// The still picture reduced motion shows: the logo steady, before anything tears.
const STILL: f64 = CLEAN / 2.0;

/// When the picture tears, in seconds into the tearing, and how long each tear lasts.
const TEARS: [(f64, f64); 5] = [
    (0.25, 0.16),
    (1.30, 0.11),
    (2.20, 0.26),
    (3.70, 0.13),
    (4.60, 0.34),
];
/// The tear whose corruption spells the time: the longest, so it can be read.
const TELLING: u64 = 5;

/// How far a band slides at the height of a tear, in cells, and how far its smear is dragged each
/// step.
const SLIDE: f32 = 7.0;
const DRAG: f32 = 2.6;
/// How many rows are in a band that slides together.
const BAND: i32 = 2;
/// How much of a torn band corrupts into other characters.
const CORRUPT: f32 = 0.16;
/// How much of the picture tears at once: the rest of it stays put, so the logo is still
/// readable through a tear rather than being churned up by it.
const TORN: f32 = 0.38;

/// How much of a step's light the smear keeps: 1.4 s to the floor at 30 steps a second.
const DECAY: f32 = 0.95;
const FLOOR: f32 = 0.12;
const LOOKS: [char; 4] = ['░', '▒', '▓', '█'];

fn noise(seed: u64) -> char {
    char::from(b'!' + (hash01(seed) * 94.0) as u8 % 94)
}

/// How far into a tear the picture is at `t`, from 0 to 1, and which tear it is.
fn tearing(t: f64) -> Option<(f32, u64)> {
    if !(CLEAN..CLEAN + TEARING).contains(&t) {
        return None;
    }
    let since = t - CLEAN;
    TEARS
        .iter()
        .enumerate()
        .find(|(_, (at, long))| (*at..at + long).contains(&since))
        .map(|(i, (at, long))| (((since - at) / long) as f32, i as u64 + 1))
}

/// Whether the band a row is in tears in this tear, and if so which way and how far it slides
/// at the tear's height, in cells.
fn torn(row: i32, tear: u64) -> Option<f32> {
    let n = (row / BAND) as u64 ^ tear.wrapping_mul(0x9e37_79b9);
    (hash01(n) < TORN).then(|| (hash01(n ^ 0x511d) - 0.5) * 2.0 * SLIDE)
}

#[derive(Default)]
pub struct Glitch {
    buffer: Feedback,
}

impl Variation for Glitch {
    fn id(&self) -> &'static str {
        "glitch"
    }

    fn reset(&mut self, layout: &Layout, _seed: u64) {
        self.buffer.reset(layout.cols, layout.rows);
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let (lx, ly, lw, lh) = layout.logo;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed.rem_euclid(TURN)
        };
        let tear = tearing(t);
        let collapse = if t < CLEAN + TEARING {
            0.0
        } else {
            ((t - CLEAN - TEARING) / COLLAPSE).min(1.0) as f32
        };

        if !frame.still && frame.dt > 0.0 {
            if collapse > 0.0 {
                // The collapse: the picture is drawn in and turned, with nothing new laid down
                // after the logo itself goes in at the start.
                let swirl = Motion {
                    centre: (lx as f32 + lw as f32 / 2.0, ly as f32 + lh as f32 / 2.0),
                    zoom: 0.95,
                    turn: (4.0f32).to_radians(),
                    shift: (0.0, 0.0),
                };
                self.buffer.advance(0.95, |x, y| swirl.source(x, y));
                // The logo pours into the swirl while it fades, not just once.
                let pouring = 1.0 - collapse / 0.3;
                if pouring > 0.0 {
                    for letter in &layout.letters {
                        let across = (letter.col - lx) as f32 / lw.max(1) as f32;
                        self.buffer.ink(
                            letter.col as f32,
                            letter.row as f32,
                            pouring,
                            hue_byte(across * 2.0 / 3.0),
                        );
                    }
                }
            } else {
                // A torn band's picture is dragged along its stale motion; the rest stays put.
                let (along, number) = tear.unwrap_or((0.0, 0));
                let amount = (along * PI).sin();
                self.buffer.advance(DECAY, |x, y| {
                    match (number > 0).then(|| torn(y as i32, number)).flatten() {
                        Some(slide) => (x - slide.signum() * DRAG * amount, y),
                        None => (x, y),
                    }
                });
            }
        }

        // The buffer's smear and echoes, under the letters.
        self.buffer.draw(grid, FLOOR, &LOOKS, |hue, level| {
            mix(BG, hue_colour(hue, 6), [0.6, 0.75, 0.9, 1.0][level])
        });

        if t >= CLEAN + TEARING + COLLAPSE {
            return;
        }
        let beat = (t * 14.0) as u64;
        // The time, as the one tear's corruption spells it.
        let told: Vec<char> = frame.readings.time.chars().collect();
        let telling = tear.is_some_and(|(_, number)| number == TELLING) && !told.is_empty();
        // The torn row nearest the middle of the logo carries it, from a third of the way in.
        let told_row = tear.and_then(|(_, number)| {
            (0..lh)
                .map(|k| ly + lh / 2 + if k % 2 == 0 { k / 2 } else { -(k / 2) - 1 })
                .find(|&row| torn(row, number).is_some())
        });

        // Crisp letters: at home where the picture holds, slid where it tears, gone into the swirl.
        let light = 1.0 - (collapse / 0.3).min(1.0);
        if light <= 0.0 {
            return;
        }
        for (i, letter) in layout.letters.iter().enumerate() {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            let home = gradient(across);
            let mut ch = letter.ch;
            let mut x = letter.col as f32;
            let y = letter.row as f32;
            let mut rgb = home;
            let hue = hue_byte(across * 2.0 / 3.0);

            let slid = tear.and_then(|(along, number)| {
                torn(letter.row, number).map(|slide| (slide * (along * PI).sin(), along, number))
            });
            if let Some((slide, along, number)) = slid {
                x += slide;
                let amount = (along * PI).sin();
                if !frame.still && frame.dt > 0.0 {
                    // No fresh picture in a torn band: only the echoes go into the buffer, and
                    // the smear of what was there is dragged on.
                    self.buffer
                        .ink(x + 1.0, y, 0.45 * amount, hue_byte(2.0 / 3.0));
                    self.buffer.ink(x - 1.0, y, 0.45 * amount, hue_byte(0.0));
                    self.buffer.ink(x, y, 0.7, hue);
                }
                if hash01((letter.row / BAND) as u64 ^ number ^ (i as u64) ^ beat) < CORRUPT {
                    ch = noise(i as u64 ^ beat);
                    rgb = mix(home, WHITE, 0.6);
                }
            } else if !frame.still && frame.dt > 0.0 && collapse <= 0.0 {
                // A good picture refreshes the buffer where it holds.
                self.buffer.ink(x, y, 0.9, hue);
            }
            grid.put(x, y, ch, mix(BG, rgb, light));
        }
        // The time, over the corruption in its band, a third of the way along the logo.
        if let (true, Some(row), Some((along, number))) = (telling, told_row, tear) {
            let slide = torn(row, number).unwrap_or(0.0) * (along * PI).sin();
            let start = (lx + lw / 3) as f32 + slide;
            for (k, &digit) in told.iter().enumerate() {
                grid.put(
                    start + k as f32,
                    row as f32,
                    digit,
                    mix(BG, WHITE, 0.8 * light),
                );
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
        Readings, layout,
        tests::{at_home, drawn},
    };

    /// Runs the stream a step at a time from the start of its turn to `elapsed`, with the clock
    /// reading `time`; with `still`, one reduced-motion compose instead.
    fn run(elapsed: f64, still: bool, time: &str) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut glitch = Glitch::default();
        glitch.reset(&layout, 0);
        let readings = Readings {
            time: time.into(),
            ..Default::default()
        };
        let mut grid = Grid::new(layout.cols, layout.rows);
        let mut at = if still { elapsed } else { 0.0 };
        while at <= elapsed + 1e-9 {
            grid = Grid::new(layout.cols, layout.rows);
            glitch.compose(
                Frame {
                    layout: &layout,
                    elapsed: at,
                    dt: if still { 0.0 } else { 1.0 / 30.0 },
                    still,
                    readings: &readings,
                    local: None,
                },
                &mut grid,
            );
            at += 1.0 / 30.0;
        }
        (grid, layout)
    }

    fn glitch(elapsed: f64, still: bool) -> (Grid, Layout) {
        run(elapsed, still, "")
    }

    #[test]
    fn the_tears_come_when_they_are_due_and_never_outside_the_tearing() {
        assert!(tearing(CLEAN / 2.0).is_none(), "not while it is clean");
        let (at, long) = TEARS[2];
        assert!(tearing(CLEAN + at + long / 2.0).is_some());
        assert!(tearing(CLEAN + at + long + 0.01).is_none(), "between tears");
        assert!(tearing(CLEAN + TEARING + 0.1).is_none(), "not after them");
    }

    #[test]
    fn the_picture_is_whole_until_a_tear_slides_bands_of_it_sideways() {
        let (clean, layout) = glitch(CLEAN / 2.0, false);
        assert_eq!(
            at_home(&clean, &layout),
            layout.letters.len(),
            "every letter is in its place until something tears"
        );
        let (at, long) = TEARS[2];
        let (torn, layout) = glitch(CLEAN + at + long / 2.0, false);
        assert!(
            at_home(&torn, &layout) < layout.letters.len(),
            "some of the picture has slid off its place"
        );
        assert!(
            at_home(&torn, &layout) > layout.letters.len() / 4,
            "and some of it is still where it was"
        );
    }

    #[test]
    fn it_collapses_and_the_turn_ends_on_an_empty_screen() {
        let (going, layout) = glitch(CLEAN + TEARING + COLLAPSE * 0.8, false);
        assert!(
            at_home(&going, &layout) < layout.letters.len() / 2,
            "most of it has broken up"
        );
        let (gap, _) = glitch(TURN - 0.1, false);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn a_torn_band_leaves_a_smear_after_it_snaps_back() {
        let (at, long) = TEARS[4];
        let (after, layout) = glitch(CLEAN + at + long + 0.1, false);
        assert_eq!(
            at_home(&after, &layout),
            layout.letters.len(),
            "the band is back"
        );
        assert!(
            drawn(&after) > layout.letters.len() + 40,
            "and the smear it left hangs beside the logo: {} cells",
            drawn(&after)
        );
        let (settled, _) = glitch(CLEAN + at + long + 0.95, false);
        assert!(drawn(&settled) < drawn(&after), "then fades");
    }

    #[test]
    fn one_tear_s_corruption_reads_the_time() {
        let (at, long) = TEARS[TELLING as usize - 1];
        let (grid, layout) = run(CLEAN + at + long / 2.0, false, "21:47");
        let reads = (0..layout.rows).any(|row| {
            let text: String = (0..layout.cols)
                .map(|col| grid.at(col as f32, row as f32).ch)
                .collect();
            text.contains("21:47")
        });
        assert!(reads, "the time is spelled in a torn band");
        let (at, long) = TEARS[0];
        let (grid, layout) = run(CLEAN + at + long / 2.0, false, "21:47");
        let reads = (0..layout.rows).any(|row| {
            let text: String = (0..layout.cols)
                .map(|col| grid.at(col as f32, row as f32).ch)
                .collect();
            text.contains("21:47")
        });
        assert!(!reads, "but only in the one tear");
    }

    #[test]
    fn reduced_motion_shows_a_picture_with_nothing_wrong_with_it() {
        let (first, layout) = glitch(0.0, true);
        let (second, _) = glitch(12.0, true);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
        assert_eq!(drawn(&first), layout.letters.len(), "and nothing else");
    }
}
