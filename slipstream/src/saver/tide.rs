//! `tide`: swell crossing the screen, with the logo surfacing out of it.
//!
//! Three sine waves at angles to each other make a field over the grid, and the cells where they
//! crest together are drawn as shaded blocks, so bands of light travel across the screen and
//! through each other. The swell rises out of a flat sea, the logo surfaces and rides it, then
//! the sea flattens again.
//!
//! Where the crests pass under the sun they glitter, and the glitter follows the local clock: low on
//! the left in the morning, across the screen through the day, and a paler, fainter moon's by
//! night.

use super::{BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, gradient, hash01, mix};

/// One turn, in seconds of the animation clock.
const RISE: f64 = 4.0;
const HOLD: f64 = 9.0;
const EBB: f64 = 3.5;
const GAP: f64 = 1.2;
const TURN: f64 = RISE + HOLD + EBB + GAP;

/// The still picture reduced motion shows: mid-hold, with the swell stopped.
const STILL: f64 = RISE + HOLD / 2.0;

/// How high the field has to reach before a cell is drawn at all. The waves add up to a little
/// over 1.5 at most, so this leaves about a fifth of the screen lit: the crest of each band and
/// nothing between them.
const CREST: f32 = 1.0;

/// The field at a cell. One set of waves runs across the screen at an angle and decides where the
/// bands are; a slower set cuts across it and a slow swell underneath lifts them in turn, so the
/// bands thicken and thin along their length rather than running ruled. Rows count double because
/// cells are twice as tall as they are wide, so the bands lie at the angle they look like they do.
fn swell(x: f32, y: f32, t: f32) -> f32 {
    let y = y * 2.0;
    (x * 0.075 + y * 0.11 + t * 1.0).sin()
        + 0.3 * (x * 0.02 - y * 0.05 - t * 0.6).sin()
        + 0.22 * (x * 0.013 + t * 0.3).sin()
}

/// The glitter: half its width and height in cells, how far above the crest a cell has to be to
/// catch it, and how many of those cells sparkle at once.
const GLITTER: (f32, f32) = (22.0, 7.0);
const GLINTS: f32 = 0.06;
const SPARKLE: f32 = 0.3;

/// Where the glitter is, as a share of the way across and down the screen, and how bright, from
/// the local time of day in seconds: the sun from six in the morning to eight at night, rising on
/// the left and setting on the right; the moon, dimmer, the rest of the time.
fn glitter(seconds_of_day: f64) -> (f32, f32, f32) {
    let hours = (seconds_of_day / 3600.0).rem_euclid(24.0) as f32;
    let (day, bright) = if (6.0..20.0).contains(&hours) {
        ((hours - 6.0) / 14.0, 1.0)
    } else {
        (((hours + 4.0).rem_euclid(24.0)) / 10.0, 0.45)
    };
    // Highest at midday, lowest at either end of the arc.
    let height = 0.18 + 0.22 * (1.0 - (day * std::f32::consts::PI).sin());
    (0.1 + 0.8 * day, height, bright)
}

/// How a cell that far above the crest is shaded.
fn shade(above: f32) -> char {
    match above {
        a if a < 0.13 => '░',
        a if a < 0.26 => '▒',
        a if a < 0.39 => '▓',
        _ => '█',
    }
}

#[derive(Default)]
pub struct Tide;

impl Variation for Tide {
    fn id(&self) -> &'static str {
        "tide"
    }

    fn reset(&mut self, _layout: &Layout, _seed: u64) {}

    /// The swell is a whole screen of shaded cells, and every one of them changes as it moves.
    /// Twenty steps a second is as fast as that is worth painting.
    fn step_hz(&self) -> f64 {
        20.0
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed.rem_euclid(TURN)
        };
        // How high the sea is running: flat, up, held, and down again. A low swell reaches the
        // crest nowhere, so the screen empties itself.
        let height = if t < RISE {
            (t / RISE) as f32
        } else if t < RISE + HOLD {
            1.0
        } else if t < RISE + HOLD + EBB {
            1.0 - ((t - RISE - HOLD) / EBB) as f32
        } else {
            0.0
        };
        if height <= 0.0 {
            return;
        }
        let (lx, lw) = (layout.logo.0, layout.logo.2);
        // Midday when the clock hasn't been read yet.
        let (across, down, bright) = glitter(
            frame
                .local
                .map_or(13.0 * 3600.0, |local| local.rem_euclid(86_400.0)),
        );
        let sun = (
            across * layout.cols as f32,
            down * layout.rows as f32,
            bright,
        );
        // Sparkles change five times a second rather than every step, so they twinkle.
        let glint_beat = ((t * 5.0) as u64).wrapping_mul(0x9e37_79b9);
        // The swell runs right through the logo's box: thinning it there, however softly, showed
        // as a patch of flat background riding along with the letters. The logo is drawn over the
        // sea instead, and the sea between its strokes is the same sea as everywhere else.
        for row in 0..layout.rows {
            for col in 0..layout.cols {
                let (x, y) = (col as f32, row as f32);
                let level = swell(x, y, t as f32) * height;
                let above = level - CREST;
                if above <= 0.0 {
                    continue;
                }
                // Deep cyan in the shallows, mint at the top of a crest.
                let mut rgb = mix(mix(BG, CYAN, 0.5), MINT, (above / 0.5).min(1.0));
                // Under the sun, the top of a crest catches it, a few cells at a time.
                let (dx, dy) = ((x - sun.0) / GLITTER.0, (y - sun.1) / GLITTER.1);
                let near = 1.0 - (dx * dx + dy * dy);
                if near > 0.0 && above > GLINTS && !frame.still {
                    let sparkle = hash01((row * layout.cols + col) as u64 ^ glint_beat);
                    if sparkle < SPARKLE * near {
                        rgb = mix(rgb, WHITE, 0.75 * sun.2);
                    }
                }
                grid.put(x, y, shade(above), rgb);
            }
        }

        // The logo, lit by whatever the swell is doing under it, so light runs along it.
        let light = if t < RISE {
            ((t / RISE) as f32 * 1.6 - 0.6).clamp(0.0, 1.0)
        } else if t < RISE + HOLD {
            1.0
        } else if t < RISE + HOLD + EBB {
            (1.0 - ((t - RISE - HOLD) / EBB) as f32 * 1.6).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if light <= 0.0 {
            return;
        }
        for letter in &layout.letters {
            let level = swell(letter.col as f32, letter.row as f32, t as f32);
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            let riding = 0.55 + 0.45 * ((level + 3.0) / 6.0);
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, gradient(across), light * riding),
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

    fn tide(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let grid = composed(&mut Tide, &layout, elapsed, still);
        (grid, layout)
    }

    #[test]
    fn the_swell_rises_and_flattens_again() {
        let (flat, _) = tide(0.05, false);
        let (running, _) = tide(RISE + HOLD / 2.0, false);
        assert!(
            drawn(&running) > drawn(&flat) * 4,
            "the sea gets up: {} then {}",
            drawn(&flat),
            drawn(&running)
        );
        let (gap, _) = tide(TURN - 0.1, false);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn the_swell_runs_through_the_logo_rather_than_around_it() {
        let (holding, layout) = tide(RISE + HOLD / 2.0, false);
        assert_eq!(
            at_home(&holding, &layout),
            layout.letters.len(),
            "every letter is up"
        );
        // Between the letters is sea, not background: a cleared box rode along with the logo and
        // read as a hole cut in the swell.
        let (lx, ly, lw, lh) = layout.logo;
        let between: Vec<(i32, i32)> = (ly..ly + lh)
            .flat_map(|row| (lx..lx + lw).map(move |col| (col, row)))
            .filter(|&(col, row)| {
                !layout
                    .letters
                    .iter()
                    .any(|letter| (letter.col, letter.row) == (col, row))
            })
            .collect();
        let swell = between
            .iter()
            .filter(|&&(col, row)| holding.at(col as f32, row as f32).ch != ' ')
            .count();
        // The swell is bands, so only some of the gaps are crested at any moment; none of them
        // would be if the logo were still clearing a space for itself.
        // The swell is bands, so at any one moment only some of the gaps are crested — and at
        // some moments none of them are. Over the hold, the sea crosses the logo freely.
        let most = (0..8)
            .map(|k| {
                let (grid, _) = tide(RISE + HOLD * k as f64 / 8.0, false);
                between
                    .iter()
                    .filter(|&&(col, row)| grid.at(col as f32, row as f32).ch != ' ')
                    .count()
            })
            .max()
            .unwrap_or(0);
        assert!(
            most > between.len() / 10,
            "at most {most} of {} cells between the letters were ever sea ({swell} at the hold's \
             midpoint)",
            between.len()
        );
    }

    #[test]
    fn a_cell_is_only_drawn_where_the_waves_crest_together() {
        // The field never reaches the crest everywhere at once, so the bands stay bands.
        let layout = layout(1920, 1200).unwrap();
        let grid = composed(&mut Tide, &layout, RISE + HOLD / 2.0, false);
        let cells = (layout.cols * layout.rows) as usize;
        assert!(
            drawn(&grid) < cells / 3,
            "{} of {cells} cells drawn",
            drawn(&grid)
        );
    }

    #[test]
    fn the_glitter_follows_the_sun_across_the_day_and_dims_at_night() {
        let (morning, _, day) = glitter(8.0 * 3600.0);
        let (evening, _, _) = glitter(18.0 * 3600.0);
        let (_, noon_height, _) = glitter(13.0 * 3600.0);
        let (_, early_height, _) = glitter(7.0 * 3600.0);
        let (_, _, night) = glitter(2.0 * 3600.0);
        assert!(
            morning < 0.4 && evening > 0.6,
            "left to right: {morning} {evening}"
        );
        assert!(noon_height < early_height, "higher at midday");
        assert!(night < day, "the moon is fainter");
        // And it lands on the sea: some crest cells under it are drawn white.
        let layout = layout(1920, 1200).unwrap();
        let mut glinting = 0;
        for k in 0..20 {
            let grid = composed(&mut Tide, &layout, RISE + 1.0 + k as f64 * 0.37, false);
            glinting += grid
                .cells
                .iter()
                .filter(|cell| cell.ch != ' ' && cell.rgb[0] > 180 && cell.rgb[2] > 180)
                .count();
        }
        assert!(glinting > 0, "the crests glitter");
    }

    #[test]
    fn reduced_motion_stops_the_swell() {
        let (first, layout) = tide(0.0, true);
        let (second, _) = tide(7.0, true);
        assert_eq!(first.cells, second.cells, "nothing moves");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
    }
}
