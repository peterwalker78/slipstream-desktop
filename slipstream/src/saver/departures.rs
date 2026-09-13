//! `departures`: a split-flap board.
//!
//! The time in large figures on flap tiles, and under them a line of small flaps with the date,
//! the workspace and the battery, with the logo as the board's header. When a value changes, its
//! flaps riffle through their characters and settle one after another, left to right, as a
//! station board does; for most of every minute nothing on the board moves at all. A single
//! stream line under the logo fills with the minute, so the seconds are there too, for anyone who
//! looks.
//!
//! On the hour, every flap on the board riffles once round and settles again.
//!
//! This is the variation that gives back what the idle fade takes away: once the UI has faded, the
//! bar's clock has gone with it.

use super::{AMBER, BG, CYAN, Frame, Grid, Layout, Readings, Variation, WHITE, gradient, mix};

/// Flips a second while a flap riffles, and the shortest and longest a riffle lasts.
const FLIPS_PER_SEC: f64 = 14.0;
const SHORTEST: f64 = 0.5;
const LONGEST: f64 = 1.4;

/// How long each flap waits after the one to its left, so a change settles left to right: a
/// big figure after the figure before it, a small flap after its neighbour.
const FIGURE_DELAY: f64 = 0.12;
const LETTER_DELAY: f64 = 0.012;

/// The figures' characters, in the order the flaps turn through them.
const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
/// The small flaps' characters, in order.
const LETTERS: &[char] = &[
    ' ', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '%',
    ':', '-', '.', '/',
];

/// The figures, five cells of ink by seven rows, `#` for ink.
#[rustfmt::skip]
const FONT: [[&str; 7]; 10] = [
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    ["####.", "....#", "....#", ".###.", "....#", "....#", "####."],
    ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    [".###.", "#....", "#....", "####.", "#...#", "#...#", ".###."],
    ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    [".###.", "#...#", "#...#", ".####", "....#", "....#", ".###."],
];

/// A figure's tile, in cells: each point of the font is two cells wide and one row tall, which is
/// square on screen, with a cell of ground round the figure.
const TILE_W: i32 = 12;
const TILE_H: i32 = 9;
/// The gap between the two figures of the hours or of the minutes, and the colon's width.
const TILE_GAP: i32 = 1;
const COLON_W: i32 = 4;

/// The board's ground behind a figure, a shade lighter than the page.
const GROUND: [f32; 3] = [0.082, 0.102, 0.133];

/// Steps a second while anything riffles, and at rest, when only the minute's line moves.
const RIFFLING_HZ: f64 = 30.0;
const RESTING_HZ: f64 = 4.0;

/// One flap: what it is turning from and to, and when.
#[derive(Debug, Clone, Copy)]
struct Flap {
    from: char,
    to: char,
    start: f64,
    flips: u32,
    lasts: f64,
}

impl Flap {
    fn settled(ch: char) -> Self {
        Self {
            from: ch,
            to: ch,
            start: f64::NEG_INFINITY,
            flips: 0,
            lasts: 0.0,
        }
    }

    /// Turns the flap to `to` from whatever it shows at `now`, starting at `start`. It always
    /// travels forwards through `set`, and at least half a second's worth, going once round if
    /// the way is shorter; `round` sends it once round besides.
    fn turn(&mut self, to: char, set: &[char], now: f64, start: f64, round: bool) {
        let (from, _) = self.at(set, now);
        let n = set.len() as u32;
        let travel = match (position(set, from), position(set, to)) {
            (Some(a), Some(b)) => (b + n - a) % n,
            // Something the flaps don't carry, like a letter with an accent: a turn's worth of
            // riffle, then it shows.
            _ => n / 2,
        };
        let mut flips = travel + if round { n } else { 0 };
        let least = (SHORTEST * FLIPS_PER_SEC).ceil() as u32;
        while flips < least && flips > 0 {
            flips += n;
        }
        if flips == 0 && from == to {
            *self = Self::settled(to);
            return;
        }
        *self = Self {
            from,
            to,
            start,
            flips: flips.max(1),
            lasts: (flips as f64 / FLIPS_PER_SEC).clamp(SHORTEST, LONGEST),
        };
    }

    /// What the flap shows at `now`, and whether it is still riffling.
    fn at(&self, set: &[char], now: f64) -> (char, bool) {
        if now < self.start {
            return (self.from, false);
        }
        let along = (now - self.start) / self.lasts.max(0.001);
        if along >= 1.0 || self.flips == 0 {
            return (self.to, false);
        }
        let flipped = (along * self.flips as f64) as u32;
        match position(set, self.from) {
            Some(from) => (set[((from + flipped) % set.len() as u32) as usize], true),
            None => (set[flipped as usize % set.len()], true),
        }
    }

    /// Whether it has anything left to do after `now`.
    fn busy(&self, now: f64) -> bool {
        self.flips > 0 && now < self.start + self.lasts
    }
}

fn position(set: &[char], ch: char) -> Option<u32> {
    set.iter().position(|&c| c == ch).map(|i| i as u32)
}

/// The board's clock, as four digits, from the local clock or the bar's reading.
fn clock_digits(readings: &Readings, local: Option<f64>) -> Option<[char; 4]> {
    let (hours, minutes) = match local {
        Some(local) => {
            let minute = (local / 60.0).floor() as i64;
            (minute.div_euclid(60).rem_euclid(24), minute.rem_euclid(60))
        }
        None => {
            let (h, m) = readings.time.split_once(':')?;
            (h.trim().parse().ok()?, m.trim().parse().ok()?)
        }
    };
    let digit = |n: i64| char::from_digit(n.rem_euclid(10) as u32, 10).unwrap_or('0');
    Some([
        digit(hours / 10),
        digit(hours),
        digit(minutes / 10),
        digit(minutes),
    ])
}

/// The line of small flaps, `width` long: the date on the left, the workspace in the middle and
/// the battery on the right, in capitals.
fn info_line(readings: &Readings, width: usize) -> Vec<char> {
    let mut line = vec![' '; width];
    let mut write = |text: &str, at: usize| {
        for (k, ch) in text.chars().enumerate() {
            if let Some(cell) = line.get_mut(at + k) {
                *cell = ch;
            }
        }
    };
    let date = readings.date.to_uppercase();
    write(&date, 0);
    let battery = match readings.battery {
        Some((percent, true)) => format!("CHARGING {percent}%"),
        Some((percent, false)) => format!("BATTERY {percent}%"),
        None => String::new(),
    };
    let battery_len = battery.chars().count();
    write(&battery, width.saturating_sub(battery_len));
    // The workspace gets what is left in the middle, cut short if it has to be.
    let room = width.saturating_sub(2 * date.chars().count().max(battery_len) + 4);
    // A workspace without a name of its own goes by its number, which says little on its own.
    let place =
        if !readings.place.is_empty() && readings.place.chars().all(|ch| ch.is_ascii_digit()) {
            format!("WORKSPACE {}", readings.place)
        } else {
            readings.place.to_uppercase()
        };
    let place: String = place.chars().take(room).collect();
    let place_len = place.chars().count();
    write(&place, width.saturating_sub(place_len) / 2);
    line
}

#[derive(Default)]
pub struct Departures {
    figures: [Option<Flap>; 4],
    letters: Vec<Flap>,
    /// The hour on the local clock last step, for the riffle on the hour.
    hour: Option<i64>,
    /// When the last flap finishes, on this variation's clock.
    busy_until: f64,
    /// Riffling, so steps come fast; resting, so they don't.
    riffling: bool,
}

/// Where the board's parts go: the clock's top-left cell, and the small flaps' row and first
/// column. Above the logo when there's room, which there is on any ordinary screen; below it
/// otherwise.
fn places(layout: &Layout) -> ((i32, i32), (i32, i32)) {
    let (lx, ly, lw, lh) = layout.logo;
    let clock_w = 4 * TILE_W + 2 * TILE_GAP + COLON_W;
    let clock_x = lx + (lw - clock_w) / 2;
    if ly >= TILE_H + 6 {
        ((clock_x, ly - TILE_H - 6), (lx, ly - 3))
    } else {
        ((clock_x, ly + lh + 5), (lx, ly + lh + 3))
    }
}

impl Departures {
    fn draw(&self, frame: &Frame<'_>, grid: &mut Grid, now: f64) {
        let layout = frame.layout;
        let (lx, ly, lw, lh) = layout.logo;
        let settled = mix(BG, WHITE, 0.95);
        let riffling = mix(BG, AMBER, 0.9);
        let ((cx, cy), (ix, iy)) = places(layout);

        // The clock: four tiles, the colon between the hours and the minutes.
        for (k, flap) in self.figures.iter().enumerate() {
            let x =
                cx + k as i32 * (TILE_W + TILE_GAP) + if k >= 2 { COLON_W - TILE_GAP } else { 0 };
            let (ch, moving) = flap.map_or((' ', false), |flap| flap.at(DIGITS, now));
            let glyph = ch.to_digit(10).map(|d| &FONT[d as usize]);
            let ink = if moving { riffling } else { settled };
            for row in 0..TILE_H {
                for col in 0..TILE_W {
                    let (fx, fy) = (col - 1, row - 1);
                    let lit = glyph.is_some_and(|glyph| {
                        (0..10).contains(&fx)
                            && (0..7).contains(&fy)
                            && glyph[fy as usize].as_bytes()[(fx / 2) as usize] == b'#'
                    });
                    // The hinge across the middle of the tile shows as a dark line through the
                    // top of that row, figure and ground alike.
                    let hinge = row == TILE_H / 2;
                    let ch = if hinge { '▄' } else { '█' };
                    grid.put(
                        (x + col) as f32,
                        (cy + row) as f32,
                        ch,
                        if lit { ink } else { GROUND },
                    );
                }
            }
        }
        let colon_x = cx + 2 * TILE_W + TILE_GAP + 1;
        for dot in [TILE_H / 2 - 2, TILE_H / 2 + 2] {
            for col in 0..2 {
                grid.put((colon_x + col) as f32, (cy + dot) as f32, '█', settled);
            }
        }

        // The small flaps.
        for (k, flap) in self.letters.iter().enumerate() {
            let (ch, moving) = flap.at(LETTERS, now);
            if ch != ' ' {
                grid.put(
                    (ix + k as i32) as f32,
                    iy as f32,
                    ch,
                    if moving { riffling } else { settled },
                );
            }
        }

        // The header.
        for letter in &layout.letters {
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                gradient(across),
            );
        }

        // The stream line under it, filled with the minute so far.
        let row = (ly + lh + 1) as f32;
        let filled = match (frame.still, frame.local) {
            (false, Some(local)) => (local.rem_euclid(60.0) / 60.0) as f32 * lw as f32,
            _ => -1.0,
        };
        for col in 0..lw {
            let x = col as f32;
            let light = if x < filled - 3.0 {
                0.26
            } else if x <= filled {
                // The head of the fill, brightest at its tip.
                0.26 + 0.6 * (1.0 - (filled - x) / 3.0)
            } else {
                0.12
            };
            let rgb = mix(BG, mix(CYAN, WHITE, (light - 0.5).max(0.0)), light);
            grid.put((lx + col) as f32, row, '─', rgb);
        }
    }
}

impl Variation for Departures {
    fn id(&self) -> &'static str {
        "departures"
    }

    fn reset(&mut self, layout: &Layout, _seed: u64) {
        self.figures = [None; 4];
        self.letters = vec![Flap::settled(' '); layout.logo.2.max(0) as usize];
        self.hour = None;
        self.busy_until = 0.0;
        self.riffling = true;
    }

    fn step_hz(&self) -> f64 {
        if self.riffling {
            RIFFLING_HZ
        } else {
            RESTING_HZ
        }
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let now = frame.elapsed;
        let width = frame.layout.logo.2.max(0) as usize;
        if self.letters.len() != width {
            self.letters = vec![Flap::settled(' '); width];
        }
        let digits = clock_digits(frame.readings, frame.local);
        let line = info_line(frame.readings, width);

        if frame.still {
            // Reduced motion: the board settled at the current values, nothing turning.
            for (flap, digit) in self.figures.iter_mut().zip(digits.unwrap_or([' '; 4])) {
                *flap = digits.map(|_| Flap::settled(digit));
            }
            for (flap, &ch) in self.letters.iter_mut().zip(&line) {
                *flap = Flap::settled(ch);
            }
            self.riffling = false;
            self.draw(&frame, grid, now);
            return;
        }

        // On the hour, every flap goes once round.
        let hour = frame.local.map(|local| (local / 3600.0).floor() as i64);
        let on_the_hour = hour.is_some() && self.hour.is_some() && hour != self.hour;
        if hour.is_some() {
            self.hour = hour;
        }

        let mut order = 0.0;
        if let Some(digits) = digits {
            for (flap, digit) in self.figures.iter_mut().zip(digits) {
                let start = now + order * FIGURE_DELAY;
                order += 1.0;
                match flap {
                    Some(flap) if flap.to == digit && !on_the_hour => {}
                    Some(flap) => flap.turn(digit, DIGITS, now, start, on_the_hour),
                    None => {
                        // The board comes up by riffling in from the first figure.
                        let mut fresh = Flap::settled('0');
                        fresh.turn(digit, DIGITS, now, start, true);
                        *flap = Some(fresh);
                    }
                }
                if let Some(flap) = flap {
                    self.busy_until = self.busy_until.max(flap.start + flap.lasts);
                }
            }
        }
        for (k, (flap, &ch)) in self.letters.iter_mut().zip(&line).enumerate() {
            if flap.to != ch || (on_the_hour && ch != ' ') {
                let start = now + 0.3 + k as f64 * LETTER_DELAY;
                flap.turn(ch, LETTERS, now, start, on_the_hour);
                self.busy_until = self.busy_until.max(flap.start + flap.lasts);
            }
        }
        self.riffling =
            now < self.busy_until || self.figures.iter().flatten().any(|flap| flap.busy(now));
        self.draw(&frame, grid, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{layout, tests::at_home};

    fn readings(time: &str) -> Readings {
        Readings {
            time: time.into(),
            date: "Sat 12 Sep".into(),
            place: "Mail".into(),
            battery: Some((84, false)),
        }
    }

    /// Composes the board at each of `times` in turn, with the local clock at `local` plus the
    /// time, and gives back the last grid.
    fn board(
        departures: &mut Departures,
        layout: &Layout,
        readings: &Readings,
        local: Option<f64>,
        times: impl IntoIterator<Item = f64>,
    ) -> Grid {
        let mut grid = Grid::new(layout.cols, layout.rows);
        for at in times {
            grid = Grid::new(layout.cols, layout.rows);
            departures.compose(
                Frame {
                    layout,
                    elapsed: at,
                    dt: 1.0 / 30.0,
                    still: false,
                    readings,
                    local: local.map(|local| local + at),
                },
                &mut grid,
            );
        }
        grid
    }

    fn row_text(grid: &Grid, row: i32) -> String {
        (0..grid.cols)
            .map(|col| grid.at(col as f32, row as f32).ch)
            .collect::<String>()
            .trim()
            .to_string()
    }

    #[test]
    fn a_flap_travels_forwards_riffles_and_settles_on_its_value() {
        let mut flap = Flap::settled('3');
        flap.turn('4', DIGITS, 0.0, 0.0, false);
        assert!(
            flap.flips >= 7,
            "at least half a second of riffle: {}",
            flap.flips
        );
        assert_eq!(flap.flips % 10, 1, "forwards, from 3 to 4, going round");
        let (_, moving) = flap.at(DIGITS, 0.2);
        assert!(moving);
        assert_eq!(flap.at(DIGITS, flap.lasts + 0.01), ('4', false));
        assert!(flap.lasts <= LONGEST && flap.lasts >= SHORTEST);
        // A turn caught mid-riffle starts from what the flap shows then.
        let showing = flap.at(DIGITS, 0.2).0;
        flap.turn('9', DIGITS, 0.2, 0.2, false);
        assert_eq!(flap.from, showing);
    }

    #[test]
    fn the_board_shows_the_time_date_workspace_and_battery_once_settled() {
        let layout = layout(1920, 1200).unwrap();
        let mut departures = Departures::default();
        departures.reset(&layout, 0);
        let readings = readings("09:41");
        let grid = board(
            &mut departures,
            &layout,
            &readings,
            None,
            (0..120).map(|k| k as f64 / 30.0),
        );
        let (_, (_, info_row)) = places(&layout);
        let info = row_text(&grid, info_row);
        assert!(info.starts_with("SAT 12 SEP"), "{info:?}");
        assert!(info.contains("MAIL"), "{info:?}");
        assert!(info.ends_with("BATTERY 84%"), "{info:?}");
        let numbered = Readings {
            place: "3".into(),
            ..readings.clone()
        };
        let line: String = info_line(&numbered, 121).into_iter().collect();
        assert!(line.contains("WORKSPACE 3"), "{line:?}");
        assert_eq!(at_home(&grid, &layout), layout.letters.len(), "the header");
        assert!(!departures.riffling, "and everything has settled");
        assert_eq!(departures.step_hz(), RESTING_HZ);
        let settled_again = board(&mut departures, &layout, &readings, None, [4.5]);
        assert_eq!(grid.cells, settled_again.cells, "nothing moves at rest");
    }

    #[test]
    fn a_change_riffles_only_the_flaps_that_changed() {
        let layout = layout(1920, 1200).unwrap();
        let mut departures = Departures::default();
        departures.reset(&layout, 0);
        board(
            &mut departures,
            &layout,
            &readings("09:41"),
            None,
            (0..120).map(|k| k as f64 / 30.0),
        );
        board(&mut departures, &layout, &readings("09:42"), None, [4.1]);
        assert!(departures.riffling);
        let moving: Vec<bool> = departures
            .figures
            .iter()
            .map(|flap| flap.unwrap().busy(4.3))
            .collect();
        assert_eq!(moving, [false, false, false, true]);
        assert!(departures.letters.iter().all(|flap| !flap.busy(4.3)));
    }

    #[test]
    fn on_the_hour_every_flap_goes_round() {
        let layout = layout(1920, 1200).unwrap();
        let mut departures = Departures::default();
        departures.reset(&layout, 0);
        // Local time five seconds before 10:00.
        let local = Some(10.0 * 3600.0 - 5.0 - 4.0);
        let readings = readings("09:59");
        board(
            &mut departures,
            &layout,
            &readings,
            local,
            (0..120).map(|k| k as f64 / 30.0),
        );
        assert!(!departures.riffling, "settled before the hour");
        board(&mut departures, &layout, &readings, local, [9.05]);
        let busy = departures
            .letters
            .iter()
            .filter(|flap| flap.busy(9.6))
            .count();
        let written = info_line(&readings, layout.logo.2 as usize)
            .iter()
            .filter(|ch| **ch != ' ')
            .count();
        assert_eq!(busy, written, "every small flap with something on it");
        assert!(
            departures
                .figures
                .iter()
                .all(|flap| flap.unwrap().busy(9.3)),
            "and every figure"
        );
    }

    #[test]
    fn reduced_motion_shows_the_board_settled_and_still() {
        let layout = layout(1920, 1200).unwrap();
        let mut departures = Departures::default();
        departures.reset(&layout, 0);
        let readings = readings("18:05");
        let mut still = |elapsed: f64| {
            let mut grid = Grid::new(layout.cols, layout.rows);
            departures.compose(
                Frame {
                    layout: &layout,
                    elapsed,
                    dt: 0.0,
                    still: true,
                    readings: &readings,
                    local: Some(18.0 * 3600.0 + 5.0 * 60.0 + elapsed),
                },
                &mut grid,
            );
            grid
        };
        let first = still(0.0);
        let second = still(20.0);
        assert_eq!(first.cells, second.cells, "nothing moves");
        let (_, (_, info_row)) = places(&layout);
        assert!(row_text(&first, info_row).contains("MAIL"));
    }

    #[test]
    fn the_board_fits_above_the_logo_on_a_laptop_and_below_a_narrow_one() {
        let wide = layout(1920, 1200).unwrap();
        let ((_, clock_row), (_, info_row)) = places(&wide);
        assert!(clock_row >= 1 && info_row < wide.logo.1);
        for (w, h) in [(700, 1000), (2560, 1440), (1280, 800)] {
            let layout = layout(w, h).unwrap();
            let ((cx, cy), (_, iy)) = places(&layout);
            assert!(cx >= 0 && cy >= 0 && cy + TILE_H <= layout.rows, "{w}x{h}");
            assert!(iy >= 0 && iy < layout.rows, "{w}x{h}");
        }
    }
}
