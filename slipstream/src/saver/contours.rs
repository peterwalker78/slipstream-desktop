//! `contours`: a pressure map of the air round the logo.
//!
//! The screen is a chart of isobars. Air flowing past the logo piles up in front of it, where the
//! lines crowd into tight rings round a high, and thins over its shoulders and behind it, where
//! they open out. A weather system drifts across the chart from one side to the other, a low whose
//! rings the lines bend round as it passes, and the whole map breathes slowly as the flow
//! strengthens and eases.
//!
//! As on a weather chart, some of the isobars are labelled with their pressure, and the high in
//! front of the logo and the passing low are marked H and L.

use std::f32::consts::TAU;

use super::{
    AMBER, BG, Frame, Grid, Layout, MINT, Variation, WHITE, gradient, line_glyph, mix, palette,
};

/// Steps a second. Isobars move slowly, and anything slower than 20 is cross-faded, so this is
/// plenty.
const HZ: f64 = 10.0;

/// How much pressure lies between one isobar and the next, and the pressure written on the
/// isobar at zero, in hectopascals, with the step between labels.
const BAND: f32 = 0.11;
const BASE_HPA: i32 = 1012;
const HPA_STEP: i32 = 2;

/// How strong the source in front of the logo and the sink behind it are, against the free
/// stream, and how far the flow's strength swings either way and over how many seconds.
const STRENGTH: f32 = 26.0;
const BREATH: f32 = 0.15;
const BREATH_SECS: f32 = 9.0;

/// The drifting low: its depth, its radius in cells (rows counted double), and the seconds it
/// takes to cross the screen.
const LOW_DEPTH: f32 = 0.85;
const LOW_RADIUS: f32 = 34.0;
const LOW_CROSSING: f32 = 150.0;

/// The still picture reduced motion shows.
const STILL: f32 = 40.0;

/// The columns, as shares of the width, where isobars are labelled.
const LABEL_AT: [f32; 3] = [0.1, 0.5, 0.9];

#[derive(Default)]
pub struct Contours {
    /// The pressure at each cell for the step being drawn.
    pressure: Vec<f32>,
}

/// The source and sink that make the logo's shape in the flow: their positions in screen units
/// (rows counted double), and the middle of the logo.
fn body(layout: &Layout) -> ((f32, f32), (f32, f32)) {
    let (lx, ly, lw, lh) = layout.logo;
    let y = (ly as f32 + (lh as f32 - 1.0) / 2.0) * 2.0;
    ((lx as f32 + 4.0, y), ((lx + lw) as f32 - 4.0, y))
}

/// Where the low is at `t`, in screen units.
fn low(layout: &Layout, t: f32) -> (f32, f32) {
    let span = layout.cols as f32 + 2.0 * LOW_RADIUS;
    let x = (t / LOW_CROSSING * span + span * 0.3).rem_euclid(span) - LOW_RADIUS;
    let y = layout.rows as f32 * 2.0 * (0.5 + 0.28 * (TAU * t / LOW_CROSSING).cos());
    (x, y)
}

/// The pressure at a point in screen units at `t`: the pressure coefficient of a steady stream past
/// the logo's source and sink, plus a gentle slope across the whole chart, plus the low.
fn pressure(layout: &Layout, x: f32, y: f32, t: f32) -> f32 {
    let (source, sink) = body(layout);
    let strength = STRENGTH * (1.0 + BREATH * (TAU * t / BREATH_SECS).sin());
    let mut vx = 1.0;
    let mut vy = 0.0;
    for ((px, py), sign) in [(source, 1.0), (sink, -1.0)] {
        let (dx, dy) = (x - px, y - py);
        let r2 = (dx * dx + dy * dy).max(4.0);
        vx += sign * strength / TAU * dx / r2;
        vy += sign * strength / TAU * dy / r2;
    }
    let cp = (1.0 - (vx * vx + vy * vy)).max(-2.5);
    let slope =
        0.0035 * (y - layout.rows as f32) + 0.18 * (x / 61.0 + 0.4 * (t / 23.0).sin()).sin();
    let (lx, ly) = low(layout, t);
    let (dx, dy) = (x - lx, y - ly);
    let dip = LOW_DEPTH * (-(dx * dx + dy * dy) / (LOW_RADIUS * LOW_RADIUS)).exp();
    cp + slope - dip
}

impl Contours {
    fn measure(&mut self, layout: &Layout, t: f32) {
        self.pressure.clear();
        for row in 0..layout.rows {
            for col in 0..layout.cols {
                self.pressure
                    .push(pressure(layout, col as f32, row as f32 * 2.0, t));
            }
        }
    }

    fn band(&self, layout: &Layout, col: i32, row: i32) -> i32 {
        let i =
            (row.clamp(0, layout.rows - 1) * layout.cols + col.clamp(0, layout.cols - 1)) as usize;
        (self.pressure[i] / BAND).floor() as i32
    }

    fn draw(&self, layout: &Layout, grid: &mut Grid, t: f32) {
        let (lx, ly, lw, lh) = layout.logo;
        let inside = |col: i32, row: i32| {
            (lx - 2..lx + lw + 2).contains(&col) && (ly - 1..ly + lh + 1).contains(&row)
        };
        for row in 0..layout.rows {
            for col in 0..layout.cols {
                if inside(col, row) {
                    continue;
                }
                let at = |c: i32, r: i32| {
                    self.pressure[(r.clamp(0, layout.rows - 1) * layout.cols
                        + c.clamp(0, layout.cols - 1)) as usize]
                };
                // An isobar crosses between this cell and the next one along the pressure's
                // slope: to the right where the line runs up and down, below where it runs
                // across. Testing only that neighbour keeps the line one cell thick.
                let gx = at(col + 1, row) - at(col - 1, row);
                let gy = (at(col, row + 1) - at(col, row - 1)) / 2.0;
                let here = self.band(layout, col, row);
                let next = if gx.abs() > gy.abs() {
                    self.band(layout, col + 1, row)
                } else {
                    self.band(layout, col, row + 1)
                };
                if here == next {
                    continue;
                }
                // The line runs across the slope.
                let ch = line_glyph(-gy, gx);
                let level = here.max(next);
                grid.put(col as f32, row as f32, ch, band_colour(level));
            }
        }
        self.label(layout, grid);
        // H in front of the logo, where the air piles up; L at the middle of the low.
        let (source, _) = body(layout);
        let (x, y) = low(layout, t);
        for (x, y, ch, rgb) in [
            (source.0 - 9.0, source.1 / 2.0, 'H', AMBER),
            (x, y / 2.0, 'L', MINT),
        ] {
            // A clearing round the letter, so it stands out from the rings.
            for dx in -2..=2 {
                for dy in -1..=1 {
                    grid.put(x + dx as f32, y + dy as f32, ' ', BG);
                }
            }
            grid.put(x, y, ch, mix(rgb, WHITE, 0.35));
        }
        // The logo, faint, as the thing the map is drawn round.
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, gradient(across), 0.6),
            );
        }
    }

    /// Writes an isobar's pressure into a gap cut in it, down a few columns across the chart.
    fn label(&self, layout: &Layout, grid: &mut Grid) {
        for share in LABEL_AT {
            let col = (layout.cols as f32 * share) as i32;
            let mut last_label = -10;
            for row in 1..layout.rows - 1 {
                let here = self.band(layout, col, row);
                let below = self.band(layout, col, row + 1);
                if here == below || row - last_label < 4 {
                    continue;
                }
                let level = here.max(below);
                if level.rem_euclid(2) != 0 {
                    continue;
                }
                // Four figures with a space either side, written without allocating.
                let hpa = (BASE_HPA + level * HPA_STEP).clamp(0, 9999);
                let mut text = [' '; 6];
                for (k, place) in [1000, 100, 10, 1].into_iter().enumerate() {
                    text[k + 1] = char::from_digit((hpa / place % 10) as u32, 10).unwrap_or(' ');
                }
                let start = col - text.len() as i32 / 2;
                let (lx, ly, lw, lh) = layout.logo;
                if (ly - 2..ly + lh + 2).contains(&row)
                    && start + text.len() as i32 > lx - 3
                    && start < lx + lw + 3
                {
                    continue;
                }
                for (k, &ch) in text.iter().enumerate() {
                    grid.put(
                        (start + k as i32) as f32,
                        row as f32,
                        ch,
                        mix(band_colour(level), WHITE, 0.25),
                    );
                }
                last_label = row;
            }
        }
    }
}

/// An isobar's colour.
fn band_colour(level: i32) -> [f32; 3] {
    // Each isobar a step round the palette from the one before, as a relief map tints its
    // heights, so neighbouring lines are told apart.
    mix(BG, palette(level as f32 / 9.0 + 0.66), 0.55)
}

impl Variation for Contours {
    fn id(&self) -> &'static str {
        "contours"
    }

    fn reset(&mut self, _layout: &Layout, _seed: u64) {
        self.pressure.clear();
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            STILL + frame.elapsed as f32
        };
        self.measure(layout, t);
        self.draw(layout, grid, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn map(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let grid = composed(&mut Contours::default(), &layout, elapsed, still);
        (grid, layout)
    }

    #[test]
    fn the_pressure_is_high_in_front_of_the_logo_and_low_over_it() {
        let layout = layout(1920, 1200).unwrap();
        let (lx, ly, lw, _) = layout.logo;
        let (source, _) = body(&layout);
        let front = pressure(&layout, source.0 - 6.0, source.1, 0.0);
        let shoulder = pressure(&layout, (lx + lw / 2) as f32, (ly - 3) as f32 * 2.0, 0.0);
        assert!(front > shoulder + 0.3, "{front} against {shoulder}");
    }

    #[test]
    fn isobars_are_lines_not_areas_and_leave_the_logo_clear() {
        let (grid, layout) = map(3.0, false);
        let cells = (layout.cols * layout.rows) as usize;
        assert!(drawn(&grid) > cells / 30, "{} isobar cells", drawn(&grid));
        assert!(drawn(&grid) < cells / 3, "{} isobar cells", drawn(&grid));
        assert_eq!(at_home(&grid, &layout), layout.letters.len());
        let labels = grid
            .cells
            .iter()
            .filter(|cell| cell.ch.is_ascii_digit())
            .count();
        assert!(labels >= 8, "some isobars carry their pressure: {labels}");
        assert!(grid.cells.iter().any(|cell| cell.ch == 'H'));
    }

    #[test]
    fn the_map_moves_but_only_a_little_each_step() {
        let (a, _) = map(10.0, false);
        let (b, _) = map(10.1, false);
        let changed = a.cells.iter().zip(&b.cells).filter(|(x, y)| x != y).count();
        assert!(
            changed > 0 && changed < drawn(&a),
            "{changed} of {} changed",
            drawn(&a)
        );
    }

    #[test]
    fn reduced_motion_holds_the_map_still() {
        let (first, _) = map(0.0, true);
        let (second, _) = map(25.0, true);
        assert_eq!(first.cells, second.cells);
    }
}
