//! `contrails`: aircraft crossing a clear sky, and the trails they leave.
//!
//! A few aircraft cross the screen on long, shallow curves, each a single bright point. The trail
//! behind one starts as a crisp line, then spreads, frays into dashes and specks, drifts on the
//! wind and fades. Where a trail crosses the logo the letters under it light up, so the logo is
//! painted in by the traffic and fades out again behind it. The trails take the light of the time
//! of day by the local clock: white by day, gold round sunrise and sunset, and faint by night.
//!
//! Every so often a formation flies across above the logo skywriting the time, a row of puffs of
//! smoke that hang in the sky and slowly break up.

use super::{
    AMBER, BG, CYAN, Frame, Grid, Layout, Variation, WHITE, gradient, hash01, level_line, mix,
    smoothstep,
};

/// Steps a second.
const HZ: f64 = 20.0;

/// How many aircraft there are, their speeds in columns a second, and the seconds between one
/// leaving the screen and the next coming in.
const CRAFT: u64 = 4;
const SPEEDS: (f32, f32) = (9.0, 14.0);
const GAPS: (f32, f32) = (2.0, 8.0);

/// A trail's life in seconds, and the ages at which it spreads to either side and frays.
const LIFE: f32 = 12.0;
const SPREAD: f32 = 4.0;
const FRAY: f32 = 8.0;
/// How far a trail drifts on the wind, in columns and rows a second.
const WIND: (f32, f32) = (0.35, 0.12);

/// Skywriting: seconds between one message and the next, when the first is written, the
/// formation's speed in columns a second, and how long the puffs hang.
const SKY_EVERY: f64 = 60.0;
const SKY_FIRST: f64 = 6.0;
const SKY_SPEED: f32 = 24.0;
const SKY_LIFE: f32 = 16.0;

/// The still picture reduced motion shows: traffic in the sky and the time written above the logo.
const STILL: f64 = SKY_FIRST + 7.0;

/// Five by seven dot figures for the time, a row to each string, `#` a puff.
const FIGURES: [[&str; 7]; 11] = [
    [
        ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
    ],
    [
        "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
    ],
    [
        ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
    ],
    [
        "####.", "....#", "....#", ".###.", "....#", "....#", "####.",
    ],
    [
        "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
    ],
    [
        "#####", "#....", "####.", "....#", "....#", "#...#", ".###.",
    ],
    [
        ".###.", "#....", "#....", "####.", "#...#", "#...#", ".###.",
    ],
    [
        "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
    ],
    [
        ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
    ],
    [
        ".###.", "#...#", "#...#", ".####", "....#", "....#", ".###.",
    ],
    [
        ".....", "..#..", "..#..", ".....", "..#..", "..#..", ".....",
    ],
];

/// One crossing: when it started, which way it flies, its speed, and the curve it follows.
#[derive(Debug, Clone, Copy)]
struct Flight {
    start: f64,
    rightwards: bool,
    speed: f32,
    /// The row it crosses the logo's middle column on, how steeply it climbs (rows a column),
    /// and its bow: height, wavelength in columns, and phase.
    row: f32,
    climb: f32,
    bow: (f32, f32, f32),
}

impl Flight {
    /// Flight `n` of craft `k` on a screen `cols` wide.
    fn of(layout: &Layout, k: u64, n: i64) -> Self {
        let cols = layout.cols as f32 + 20.0;
        let seed = k.wrapping_mul(0x9e37_79b9) ^ (n as u64).wrapping_mul(0x85eb_ca6b);
        let speed = SPEEDS.0 + (SPEEDS.1 - SPEEDS.0) * hash01(k ^ 0x5eed);
        let gap = GAPS.0 + (GAPS.1 - GAPS.0) * hash01(k ^ 0x6a95);
        // Each craft keeps a steady rhythm of its own, so a flight's start is known without
        // counting the ones before it.
        let period = (cols / speed + gap) as f64;
        let (_, ly, _, lh) = layout.logo;
        // Half the flights cross the logo's rows; the rest anywhere in the sky.
        let row = if hash01(seed ^ 1) < 0.5 {
            ly as f32 + hash01(seed ^ 2) * lh as f32
        } else {
            layout.rows as f32 * (0.06 + 0.88 * hash01(seed ^ 3))
        };
        Flight {
            start: n as f64 * period + hash01(k ^ 0x0ff5) as f64 * period,
            rightwards: hash01(seed ^ 4) < 0.6,
            speed,
            row,
            climb: (hash01(seed ^ 5) - 0.5) * 0.16,
            bow: (
                1.5 + 3.0 * hash01(seed ^ 6),
                90.0 + 90.0 * hash01(seed ^ 7),
                hash01(seed ^ 8) * std::f32::consts::TAU,
            ),
        }
    }

    /// The flights of craft `k` that could have a trail on screen at `t`: its latest and the one
    /// before.
    fn around(layout: &Layout, k: u64, t: f64) -> [Self; 2] {
        let first = Flight::of(layout, k, 0);
        let period = Flight::of(layout, k, 1).start - first.start;
        let n = ((t - first.start) / period).floor() as i64;
        [Flight::of(layout, k, n), Flight::of(layout, k, n - 1)]
    }

    /// The column the craft is at `t`, measured along its way across from where it came in.
    fn along(&self, t: f64) -> f32 {
        ((t - self.start) as f32) * self.speed - 10.0
    }

    /// The screen column for a distance along its way.
    fn column(&self, layout: &Layout, along: f32) -> f32 {
        if self.rightwards {
            along
        } else {
            layout.cols as f32 - 1.0 - along
        }
    }

    /// The row its path runs on at column `x`.
    fn path(&self, layout: &Layout, x: f32) -> f32 {
        let middle = layout.cols as f32 / 2.0;
        let (height, wavelength, phase) = self.bow;
        self.row
            + self.climb * (x - middle)
            + height * (x / wavelength * std::f32::consts::TAU + phase).sin()
    }
}

/// The colour of smoke in the sky at a local time of day, in seconds, and how brightly it shows:
/// white by day, gold for an hour or so either side of sunrise and sunset, faint blue by night.
fn sky_light(seconds: Option<f64>) -> ([f32; 3], f32) {
    let day = mix(WHITE, CYAN, 0.3);
    let Some(seconds) = seconds else {
        return (day, 1.0);
    };
    let hours = (seconds / 3600.0).rem_euclid(24.0) as f32;
    let near = |at: f32| 1.0 - smoothstep(((hours - at).abs() - 0.6) / 1.2);
    let golden = near(7.0).max(near(19.5));
    let night = if (6.0..21.0).contains(&hours) {
        0.0
    } else {
        1.0 - golden
    };
    let colour = mix(
        mix(day, mix(CYAN, WHITE, 0.15), night),
        mix(AMBER, WHITE, 0.25),
        golden,
    );
    (colour, 1.0 - 0.2 * night)
}

#[derive(Default)]
pub struct Contrails {
    /// For each cell of the logo's box, row by row, the index of the letter in it, if any.
    letter_at: Vec<Option<usize>>,
    /// How brightly each letter has been lit by a trail this step.
    lit: Vec<f32>,
}

impl Contrails {
    /// Lays down one flight's trail at `t` and returns how brightly it lights each letter of the
    /// logo into `lit`.
    #[allow(clippy::too_many_arguments)]
    fn trail(
        letter_at: &[Option<usize>],
        layout: &Layout,
        grid: &mut Grid,
        flight: &Flight,
        t: f64,
        smoke: [f32; 3],
        glow: f32,
        lit: &mut [f32],
    ) {
        let head = flight.along(t);
        let cols = layout.cols as f32;
        let tail = (head - flight.speed * LIFE).max(-10.0);
        let mut along = head.floor();
        while along >= tail {
            let age = (head - along) / flight.speed;
            let x0 = flight.column(layout, along);
            along -= 1.0;
            if !(-1.0..cols + 1.0).contains(&x0) {
                continue;
            }
            let fade = 1.0 - smoothstep(age / LIFE);
            // Drifting downwind, a little to the right and down.
            let (x, row) = (x0 + WIND.0 * age, flight.path(layout, x0) + WIND.1 * age);
            let seed = (along as i64 as u64).wrapping_mul(0x2545_f491) ^ (flight.start as u64);
            if age < SPREAD {
                let (y, ch) = level_line(row);
                let light = glow * (1.0 - 0.2 * age / SPREAD);
                grid.put(x, y, ch, mix(BG, smoke, (light * 6.0).round() / 6.0));
            } else if age < FRAY {
                // Spread to a row either side, broken into dashes that grow as it ages.
                let broken = (age - SPREAD) / (FRAY - SPREAD);
                for (dy, share) in [(-1.0, 0.45), (0.0, 0.8), (1.0, 0.45)] {
                    let gap = hash01(seed ^ (dy as i64 + 7) as u64) < 0.25 + 0.4 * broken;
                    if gap {
                        continue;
                    }
                    let light = glow * share * fade;
                    let ch = if dy == 0.0 { '─' } else { '╌' };
                    grid.put(x, row + dy, ch, mix(BG, smoke, (light * 6.0).round() / 6.0));
                }
            } else {
                // Specks, two rows either side, thinning out.
                for dy in -2..=2 {
                    if hash01(seed ^ (dy + 13) as u64) > 0.35 * fade + 0.1 {
                        continue;
                    }
                    let light = glow * fade * 0.7;
                    let ch = if dy == 0 { '·' } else { '˙' };
                    grid.put(
                        x,
                        row + dy as f32,
                        ch,
                        mix(BG, smoke, (light * 6.0).round() / 6.0),
                    );
                }
            }
            // The letters under the trail, and a couple of rows either side as it spreads.
            let (lx, ly, lw, lh) = layout.logo;
            let col = x.round() as i32 - lx;
            let reach = 2.0 + 2.5 * (age / FRAY).min(1.0);
            if (0..lw).contains(&col) {
                let from = ((row - reach).ceil() as i32 - ly).max(0);
                let to = ((row + reach).floor() as i32 - ly).min(lh - 1);
                for r in from..=to {
                    if let Some(Some(i)) = letter_at.get((r * lw + col) as usize) {
                        // Softer towards the edges of the band the trail lights.
                        let edge = 1.0 - ((r + ly) as f32 - row).abs() / (reach + 1.0);
                        lit[*i] = lit[*i].max(fade * edge * (1.0 - 0.3 * (age / LIFE)));
                    }
                }
            }
        }
        // The craft itself, while it is on screen.
        let x = flight.column(layout, head);
        if (0.0..cols).contains(&x) {
            let ch = if flight.rightwards { '►' } else { '◄' };
            grid.put(x, flight.path(layout, x), ch, WHITE);
        }
    }

    /// The time skywritten above the logo, if a message is in the sky at `t`.
    fn skywriting(
        layout: &Layout,
        grid: &mut Grid,
        time: &str,
        t: f64,
        smoke: [f32; 3],
        glow: f32,
    ) {
        if t < SKY_FIRST {
            return;
        }
        let since = ((t - SKY_FIRST).rem_euclid(SKY_EVERY)) as f32;
        let mut figures = [0usize; 8];
        let mut count = 0;
        for ch in time.chars() {
            let figure = match ch {
                ':' => 10,
                digit => match digit.to_digit(10) {
                    Some(d) => d as usize,
                    None => continue,
                },
            };
            if count < figures.len() {
                figures[count] = figure;
                count += 1;
            }
        }
        if count == 0 {
            return;
        }
        let figures = &figures[..count];
        // Each figure five puffs wide at two columns a puff, with a gap between figures.
        let width = figures.len() as f32 * 12.0 - 2.0;
        let (_, ly, _, _) = layout.logo;
        let left = (layout.cols as f32 - width) / 2.0;
        let top = (ly as f32 - 13.0).max(1.0);
        let front = since * SKY_SPEED - 20.0;
        for (f, &figure) in figures.iter().enumerate() {
            for (r, line) in FIGURES[figure].iter().enumerate() {
                for (c, dot) in line.chars().enumerate() {
                    if dot != '#' {
                        continue;
                    }
                    let x = left + f as f32 * 12.0 + c as f32 * 2.0;
                    let age = (front - x) / SKY_SPEED;
                    if age < 0.0 || age > SKY_LIFE {
                        continue;
                    }
                    let fade = 1.0 - smoothstep((age - SKY_LIFE * 0.5) / (SKY_LIFE * 0.5));
                    // Puffs swell and drift as they hang.
                    let seed = (f * 100 + r * 10 + c) as u64;
                    let drift = WIND.0 * age + (hash01(seed) - 0.5) * age * 0.15;
                    let ch = match age {
                        a if a < 3.0 => '●',
                        a if a < 9.0 => '•',
                        _ => '·',
                    };
                    grid.put(
                        x + drift,
                        top + r as f32 + WIND.1 * age * 0.5,
                        ch,
                        mix(BG, smoke, glow * fade * 0.9),
                    );
                }
            }
        }
        // The formation, one craft for each row of puffs, while it is crossing.
        if (-1.0..layout.cols as f32).contains(&front) && front < left + width + 4.0 {
            for r in 0..7 {
                grid.put(front + 1.0, top + r as f32, '►', mix(BG, WHITE, 0.9));
            }
        }
    }
}

impl Variation for Contrails {
    fn id(&self) -> &'static str {
        "contrails"
    }

    fn reset(&mut self, layout: &Layout, _seed: u64) {
        let (lx, ly, lw, lh) = layout.logo;
        self.letter_at = vec![None; (lw * lh).max(0) as usize];
        for (i, letter) in layout.letters.iter().enumerate() {
            let (col, row) = (letter.col - lx, letter.row - ly);
            if (0..lw).contains(&col) && (0..lh).contains(&row) {
                self.letter_at[(row * lw + col) as usize] = Some(i);
            }
        }
        self.lit = vec![0.0; layout.letters.len()];
    }

    fn step_hz(&self) -> f64 {
        HZ
    }

    fn preview_at(&self) -> f64 {
        STILL
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed + 30.0
        };
        let (smoke, glow) = sky_light(frame.local);
        // How brightly each letter of the logo has been lit by a trail.
        self.lit.clear();
        self.lit.resize(layout.letters.len(), 0.0);
        for k in 0..CRAFT {
            for flight in Flight::around(layout, k, t) {
                Contrails::trail(
                    &self.letter_at,
                    layout,
                    grid,
                    &flight,
                    t,
                    smoke,
                    glow,
                    &mut self.lit,
                );
            }
        }
        let time = if frame.readings.time.is_empty() {
            "12:00"
        } else {
            frame.readings.time.as_str()
        };
        // Skywriting keeps its own clock, from when the variation took over.
        let sky = if frame.still { STILL } else { frame.elapsed };
        Contrails::skywriting(layout, grid, time, sky, smoke, glow);
        let (lx, _, lw, _) = layout.logo;
        for (letter, &light) in layout.letters.iter().zip(&self.lit) {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            // A ghost of the logo always, painted in where the traffic has been.
            let light = 0.32 + 0.68 * ((light * 5.0).round() / 5.0);
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                mix(BG, gradient(across), light),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        Readings, layout,
        tests::{at_home, composed, drawn},
    };

    fn sky(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let mut sky = Contrails::default();
        sky.reset(&layout, 1);
        let grid = composed(&mut sky, &layout, elapsed, still);
        (grid, layout)
    }

    #[test]
    fn a_flight_follows_on_from_the_one_before() {
        let layout = layout(1920, 1200).unwrap();
        for k in 0..CRAFT {
            let [latest, before] = Flight::around(&layout, k, 500.0);
            assert!(latest.start <= 500.0 && before.start < latest.start);
            assert!(latest.along(500.0) >= -10.0);
        }
    }

    #[test]
    fn trails_age_and_the_logo_is_always_there() {
        let (grid, layout) = sky(20.0, false);
        assert!(drawn(&grid) > 300, "{} cells of sky", drawn(&grid));
        assert_eq!(at_home(&grid, &layout), layout.letters.len());
        let crisp = grid
            .cells
            .iter()
            .filter(|cell| matches!(cell.ch, '─' | '▁' | '▔'))
            .count();
        let frayed = grid
            .cells
            .iter()
            .filter(|cell| matches!(cell.ch, '╌' | '·' | '˙'))
            .count();
        assert!(crisp > 20 && frayed > 20, "{crisp} crisp, {frayed} frayed");
    }

    #[test]
    fn the_traffic_lights_the_logo_where_it_crosses() {
        // Over a minute, some letters are lit well above the ghost and later fade again.
        let layout = layout(1920, 1200).unwrap();
        let brightest = |grid: &Grid| {
            layout
                .letters
                .iter()
                .map(|letter| {
                    let cell = grid.at(letter.col as f32, letter.row as f32);
                    cell.rgb.iter().map(|&v| v as u32).sum::<u32>()
                })
                .max()
                .unwrap_or(0)
        };
        let ghost = brightest(&sky(0.0, true).0).min(300);
        let lit = (0..60)
            .map(|s| brightest(&sky(s as f64, false).0))
            .max()
            .unwrap();
        assert!(lit > ghost + 150, "lit {lit} against the ghost's {ghost}");
    }

    #[test]
    fn the_time_is_skywritten_above_the_logo() {
        let layout = layout(1920, 1200).unwrap();
        let mut grid = Grid::new(layout.cols, layout.rows);
        let readings = Readings {
            time: "10:47".into(),
            ..Default::default()
        };
        let mut sky = Contrails::default();
        sky.reset(&layout, 1);
        sky.compose(
            Frame {
                layout: &layout,
                elapsed: SKY_FIRST + 8.0,
                dt: 0.05,
                still: false,
                readings: &readings,
                local: None,
            },
            &mut grid,
        );
        let puffs = grid
            .cells
            .iter()
            .enumerate()
            .filter(|(i, cell)| {
                matches!(cell.ch, '●' | '•') && (*i as i32 / layout.cols) < layout.logo.1
            })
            .count();
        assert!(puffs > 40, "{puffs} puffs above the logo");
    }

    #[test]
    fn the_smoke_is_gold_at_sunset_and_faint_at_night() {
        let (day, day_glow) = sky_light(Some(13.0 * 3600.0));
        let (dusk, _) = sky_light(Some(19.5 * 3600.0));
        let (_, night_glow) = sky_light(Some(2.0 * 3600.0));
        assert!(dusk[0] > dusk[2] && day[2] >= day[0], "{dusk:?} {day:?}");
        assert!(night_glow < day_glow);
    }

    #[test]
    fn reduced_motion_stops_the_sky() {
        let (first, _) = sky(0.0, true);
        let (second, _) = sky(45.0, true);
        assert_eq!(first.cells, second.cells);
    }
}
