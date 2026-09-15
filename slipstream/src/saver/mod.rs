//! The living wallpaper: a terminal-like grid of characters behind everything else, drawn on the
//! CPU, with only the changed cells redrawn and uploaded. It runs on the animation clock, so
//! bullet time slows it too.
//!
//! This module is everything the variations share — the grid, the cell sizing, the logo's place
//! in it, the painting, the cross-fade, and the cycle that hands one variation over to the next.
//! What each variation actually draws is a `Variation` in its own file, and the ids here match
//! `slipstream_config::VARIATIONS`, which is what the settings file and the Settings app know.
//!
//! Adding one is: a new file, an arm in `make`, and a row in that table. A test checks the two
//! agree.

mod attractor;
mod chladni;
mod circuit;
mod contours;
mod contrails;
mod coral;
mod departures;
mod frost;
mod galaxies;
mod glitch;
mod life;
mod maze;
mod murmuration;
mod physarum;
pub mod preview;
mod prompt;
mod slipstream;
mod sonar;
mod tide;
mod vortex;
mod warp;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Buffer, Logical, Rectangle, Size, Transform},
};

use crate::text::{self, Face};

static LOGO: &str = include_str!("../../../assets/slipstream-logo.txt");
static LOGO_STACKED: &str = include_str!("../../../assets/slipstream-logo-stacked.txt");

/// The grid is recomposed at 30 frames a second.
const STEP: f64 = 1.0 / 30.0;
/// How finely a cell's fade from one step to the next is drawn: smooth while bullet time
/// stretches a step over many frames, without redrawing a fading cell on every frame.
const TWEEN_LEVELS: u8 = 16;

/// The cross-fade when one variation hands over to the next, in seconds of animation clock.
const HANDOVER: f64 = 1.2;

/// How often the saver reports what it has cost, in wall-clock time.
const COST_EVERY: Duration = Duration::from_secs(10);

/// What the wallpaper has cost since the last report: CPU time spent composing grids and painting
/// changed cells (thread CPU time, per wall-clock second), and how many cells were repainted. Logged at debug level (`RUST_LOG` with
/// `slipstream::saver=debug`), so a variation's budget is measured on the saver itself rather than
/// inferred from whole-process CPU, which the compositor's own GL work dominates.
#[derive(Default)]
struct Cost {
    since: Option<Instant>,
    compose: Duration,
    paint: Duration,
    steps: u32,
    cells: u64,
}

/// CPU time this thread has used, which is what the wallpaper costs whatever else the machine is
/// doing: wall-clock spans would count the time the thread spent waiting for a core.
fn thread_cpu() -> Duration {
    use smithay::reexports::rustix::time::{ClockId, clock_gettime};
    let at = clock_gettime(ClockId::ThreadCPUTime);
    Duration::new(
        at.tv_sec.max(0) as u64,
        at.tv_nsec.clamp(0, 999_999_999) as u32,
    )
}

impl Cost {
    fn report(&mut self, variation: &'static str) {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        let wall = now - since;
        if wall < COST_EVERY {
            return;
        }
        let secs = wall.as_secs_f64();
        tracing::debug!(
            variation,
            compose_pct = format!("{:.2}", self.compose.as_secs_f64() / secs * 100.0),
            paint_pct = format!("{:.2}", self.paint.as_secs_f64() / secs * 100.0),
            steps_per_sec = format!("{:.1}", self.steps as f64 / secs),
            cells_per_sec = (self.cells as f64 / secs).round() as u64,
            "living wallpaper cost (percent of one core)"
        );
        *self = Cost {
            since: Some(now),
            ..Cost::default()
        };
    }
}

/// What a variation is handed each step.
struct Frame<'a> {
    /// The cell grid, and where the logo's box sits in it.
    layout: &'a Layout,
    /// Seconds of animation clock since this variation took over.
    elapsed: f64,
    /// Animation-clock seconds since the last step; 0 with reduced motion.
    dt: f32,
    /// Reduced motion: nothing may move, and the grid must be this variation's still picture.
    still: bool,
    /// What the machine says: the time, the date, the workspace and the battery.
    readings: &'a Readings,
    /// Local time as seconds since the epoch, to the millisecond, once the readings have given the
    /// time zone away. Variations that react to the clock read the second or the minute from it.
    local: Option<f64>,
}

/// The desktop's own readings, handed to the wallpaper so a variation can show them or react to
/// them. Everything here is read locally, and nothing leaves the machine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Readings {
    /// Local time as HH:MM, as the bar shows it; empty until the first reading.
    pub time: String,
    /// The local date as the bar writes it, e.g. `Fri 11 Sep`.
    pub date: String,
    /// The name of the workspace on this screen.
    pub place: String,
    /// Charge in percent and whether it's charging; `None` without a battery.
    pub battery: Option<(u8, bool)>,
}

impl Readings {
    /// Minutes the local clock is ahead of UTC, worked out from the HH:MM reading against the
    /// system clock. The reading lags by up to a couple of seconds and is truncated to the minute,
    /// so the difference is rounded to the nearest quarter of an hour, which every time zone is a
    /// multiple of.
    fn utc_offset(&self, epoch: f64) -> Option<i64> {
        let (hours, minutes) = self.time.split_once(':')?;
        let local = hours.trim().parse::<i64>().ok()? * 60 + minutes.trim().parse::<i64>().ok()?;
        let utc = (epoch / 60.0).floor() as i64 % 1440;
        let ahead = (local - utc).rem_euclid(1440);
        // Rounded to a quarter hour, then brought into -12 h..+14 h.
        let ahead = ((ahead + 7) / 15 * 15) % 1440;
        Some(if ahead > 14 * 60 { ahead - 1440 } else { ahead })
    }
}

/// Seconds since the epoch by the system clock.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// One living-wallpaper variation: everything that differs between them.
trait Variation {
    /// Its id in the settings file. Matches `slipstream_config::VARIATIONS`.
    fn id(&self) -> &'static str;

    /// Starts over on a grid this size. Called when the variation takes over, and again whenever
    /// a change of output size or scale rebuilds the layout.
    fn reset(&mut self, layout: &Layout, seed: u64);

    /// Advances its own state by `frame.dt` and draws one step into `grid`, which arrives blank.
    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid);

    /// How often it wants a new grid, in steps a second. 30 is the default and the most the
    /// saver will give; anything below 20 is cross-faded between steps.
    fn step_hz(&self) -> f64 {
        30.0
    }

    /// Seconds of animation clock in one turn of its own cycle, if it has one.
    fn turn(&self) -> Option<f64> {
        None
    }

    /// Seconds in at which a preview of it starts: a moment that shows what it is. By default a
    /// little under half way through a turn, which is where most of them are in full swing.
    fn preview_at(&self) -> f64 {
        self.turn().map_or(8.0, |turn| turn * 0.45)
    }
}

/// Builds the variation with this id, or nothing if it isn't one.
fn make(id: &str) -> Option<Box<dyn Variation>> {
    Some(match id {
        "slipstream" => Box::<slipstream::Slipstream>::default(),
        "vortex" => Box::<vortex::Vortex>::default(),
        "departures" => Box::<departures::Departures>::default(),
        "prompt" => Box::<prompt::Prompt>::default(),
        "circuit" => Box::<circuit::Circuit>::default(),
        "life" => Box::<life::Life>::default(),
        "sonar" => Box::<sonar::Sonar>::default(),
        "tide" => Box::<tide::Tide>::default(),
        "warp" => Box::<warp::Warp>::default(),
        "glitch" => Box::<glitch::Glitch>::default(),
        "contours" => Box::<contours::Contours>::default(),
        "contrails" => Box::<contrails::Contrails>::default(),
        "coral" => Box::<coral::Coral>::default(),
        "attractor" => Box::<attractor::Attractor>::default(),
        "murmuration" => Box::<murmuration::Murmuration>::default(),
        "maze" => Box::<maze::Maze>::default(),
        "physarum" => Box::<physarum::Physarum>::default(),
        "galaxies" => Box::<galaxies::Galaxies>::default(),
        "chladni" => Box::<chladni::Chladni>::default(),
        "frost" => Box::<frost::Frost>::default(),
        _ => return None,
    })
}

/// The ids to cycle through, from the settings. Never empty.
fn ids(wallpaper: &slipstream_config::Wallpaper) -> Vec<&'static str> {
    wallpaper
        .chosen()
        .into_iter()
        .map(|variation| variation.id)
        .filter(|id| make(id).is_some())
        .collect()
}

/// The steps a second a variation that asks for `asked` gets on `layout`: never more than the
/// saver's own rate, and on a screen with big cells, where every changed cell costs several times
/// as much ink, no more than 15.
fn step_hz(asked: f64, layout: &Layout) -> f64 {
    let asked = asked.clamp(1.0, 1.0 / STEP);
    if layout.cell_w * layout.cell_h > 300 {
        asked.min(15.0)
    } else {
        asked
    }
}

/// The mockup's page colour.
const BACKGROUND: [u8; 3] = [11, 14, 19];
const BG: [f32; 3] = [11.0 / 255.0, 14.0 / 255.0, 19.0 / 255.0];
const WHITE: [f32; 3] = [0.93, 0.98, 1.0];
const CYAN: [f32; 3] = [0.2, 0.8, 1.0];
const AMBER: [f32; 3] = [1.0, 0.71, 0.28];
const MINT: [f32; 3] = [0.235, 0.94, 0.75];

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The logo's colours, left to right: Slipstream's amber and mint, then cyan.
fn gradient(across: f32) -> [f32; 3] {
    if across < 0.5 {
        mix(AMBER, MINT, across * 2.0)
    } else {
        mix(MINT, CYAN, (across - 0.5) * 2.0)
    }
}

/// The next number from a splitmix64 sequence, advancing `state`.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A steady pseudo-random number in [0, 1) for `n` (splitmix64).
fn hash01(n: u64) -> f32 {
    let mut z = n.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// The line character that draws a nearly level line at `row`, a third of a row at a time: the
/// cell it falls in, and `─` across its middle or `▔`/`▁` a third of the way up or down.
fn level_line(row: f32) -> (f32, char) {
    let thirds = (row * 3.0).round() as i32;
    let cell = thirds.div_euclid(3) as f32;
    match thirds.rem_euclid(3) {
        0 => (cell, '─'),
        1 => (cell, '▁'),
        _ => (cell + 1.0, '▔'),
    }
}

/// The line character that runs in the direction (`dx`, `dy`), with `dy` in rows counted double
/// so the directions are as they look on screen.
fn line_glyph(dx: f32, dy: f32) -> char {
    let angle = dy.atan2(dx).rem_euclid(std::f32::consts::PI);
    let eighth = std::f32::consts::PI / 8.0;
    match angle {
        a if a < eighth || a >= 7.0 * eighth => '─',
        a if a < 3.0 * eighth => '╲',
        a if a < 5.0 * eighth => '│',
        _ => '╱',
    }
}

/// Slipstream's colours as one loop, for ink whose colour travels: amber, mint, cyan, and back
/// to amber. `at` runs round it once from 0 to 1.
fn palette(at: f32) -> [f32; 3] {
    let at = at.rem_euclid(1.0) * 3.0;
    match at {
        a if a < 1.0 => mix(AMBER, MINT, a),
        a if a < 2.0 => mix(MINT, CYAN, a - 1.0),
        a => mix(CYAN, AMBER, a - 2.0),
    }
}

/// A picture that persists between steps: each step starts from the last one, carried through a
/// motion and faded, and new ink is added on top. This is what makes a feedback variation feel
/// alive rather than looped: the motion is simple, and the picture it carries is not.
///
/// It holds light, not glyphs. Light is sampled between cells, so a motion of less than a cell a
/// step still moves it, and it only becomes characters in `draw`, quantised to a few levels and
/// hues, so a cell whose light fades smoothly changes its look only every few steps. That, not the
/// arithmetic, is what keeps the painting cheap.
#[derive(Default)]
struct Feedback {
    cols: i32,
    rows: i32,
    /// 0..1 a cell.
    light: Vec<f32>,
    /// Where on `palette` the ink in a cell came from, as 0..=255: stamped when ink lands and
    /// carried with it.
    hue: Vec<u8>,
    next_light: Vec<f32>,
    next_hue: Vec<u8>,
}

/// How the last step's picture is carried into this one: zoomed and turned about a centre, then
/// shifted. Rows count double, so a turn is round on screen rather than in cells.
#[derive(Debug, Clone, Copy)]
struct Motion {
    /// The centre of the zoom and the turn, in cells.
    centre: (f32, f32),
    /// Above 1 the picture grows outwards from the centre each step; below 1 it is drawn in.
    zoom: f32,
    /// Radians the picture turns each step.
    turn: f32,
    /// Cells the picture moves each step, across and down.
    shift: (f32, f32),
}

impl Motion {
    /// Where the picture that lands on (`x`, `y`) this step was on the last: the motion run
    /// backwards.
    fn source(&self, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (
            x - self.shift.0 - self.centre.0,
            (y - self.shift.1 - self.centre.1) * 2.0,
        );
        let (sin, cos) = self.turn.sin_cos();
        let zoom = self.zoom.max(0.01);
        let (rx, ry) = ((dx * cos + dy * sin) / zoom, (dy * cos - dx * sin) / zoom);
        (self.centre.0 + rx, self.centre.1 + ry / 2.0)
    }
}

impl Feedback {
    /// Sizes the buffer to the grid and empties it.
    fn reset(&mut self, cols: i32, rows: i32) {
        let len = (cols * rows).max(0) as usize;
        self.cols = cols;
        self.rows = rows;
        for buffer in [&mut self.light, &mut self.next_light] {
            buffer.clear();
            buffer.resize(len, 0.0);
        }
        for buffer in [&mut self.hue, &mut self.next_hue] {
            buffer.clear();
            buffer.resize(len, 0);
        }
    }

    /// Carries the picture one step: every cell takes the light from where `source` says it came
    /// from, sampled between cells, and fades by `decay`. The hue comes with it from the nearest
    /// cell.
    fn advance(&mut self, decay: f32, source: impl Fn(f32, f32) -> (f32, f32)) {
        let (cols, rows) = (self.cols, self.rows);
        for row in 0..rows {
            for col in 0..cols {
                let (x, y) = source(col as f32, row as f32);
                let i = (row * cols + col) as usize;
                let (x0, y0) = (x.floor(), y.floor());
                let (fx, fy) = (x - x0, y - y0);
                let (c0, r0) = (x0 as i32, y0 as i32);
                let mut light = 0.0;
                let mut best = (0.0f32, 0u8);
                for (dc, dr, weight) in [
                    (0, 0, (1.0 - fx) * (1.0 - fy)),
                    (1, 0, fx * (1.0 - fy)),
                    (0, 1, (1.0 - fx) * fy),
                    (1, 1, fx * fy),
                ] {
                    let (c, r) = (c0 + dc, r0 + dr);
                    if weight <= 0.0 || !(0..cols).contains(&c) || !(0..rows).contains(&r) {
                        continue;
                    }
                    let j = (r * cols + c) as usize;
                    let lit = self.light[j] * weight;
                    light += lit;
                    if lit > best.0 {
                        best = (lit, self.hue[j]);
                    }
                }
                let light = light * decay;
                self.next_light[i] = if light < 0.004 { 0.0 } else { light };
                self.next_hue[i] = best.1;
            }
        }
        std::mem::swap(&mut self.light, &mut self.next_light);
        std::mem::swap(&mut self.hue, &mut self.next_hue);
    }

    /// Adds ink to the cell at (`x`, `y`): the brighter of what is there and `light` stays, and the
    /// new ink stamps its hue if it is at least as bright.
    fn ink(&mut self, x: f32, y: f32, light: f32, hue: u8) {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if !(0..self.cols).contains(&col) || !(0..self.rows).contains(&row) {
            return;
        }
        let i = (row * self.cols + col) as usize;
        let light = light.clamp(0.0, 1.0);
        if light >= self.light[i] {
            self.light[i] = light;
            self.hue[i] = hue;
        }
    }

    /// Adds ink along the line from `from` to `to`, a cell at a time, brightest at `to`.
    fn line(&mut self, from: (f32, f32), to: (f32, f32), light: f32, hue: u8) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let cells = dx.abs().max(dy.abs()).ceil().clamp(1.0, 64.0) as i32;
        for k in 0..=cells {
            let along = k as f32 / cells as f32;
            self.ink(
                from.0 + dx * along,
                from.1 + dy * along,
                light * (0.55 + 0.45 * along),
                hue,
            );
        }
    }

    /// The light in the cell at (`col`, `row`), and its hue.
    #[cfg(test)]
    fn at(&self, col: i32, row: i32) -> (f32, u8) {
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            let i = (row * self.cols + col) as usize;
            (self.light[i], self.hue[i])
        } else {
            (0.0, 0)
        }
    }

    /// Draws the picture into `grid`: cells under `floor` stay blank, and the rest take one of
    /// `looks` by how bright they are (dimmest first) in the colour `colour` gives for their hue
    /// and that level.
    fn draw(
        &self,
        grid: &mut Grid,
        floor: f32,
        looks: &[char],
        colour: impl Fn(u8, usize) -> [f32; 3],
    ) {
        let levels = looks.len();
        if levels == 0 {
            return;
        }
        for row in 0..self.rows.min(grid.rows) {
            for col in 0..self.cols.min(grid.cols) {
                let i = (row * self.cols + col) as usize;
                let light = self.light[i];
                if light < floor {
                    continue;
                }
                let level = (((light - floor) / (1.0 - floor).max(0.001)) * levels as f32) as usize;
                let level = level.min(levels - 1);
                grid.put(
                    col as f32,
                    row as f32,
                    looks[level],
                    colour(self.hue[i], level),
                );
            }
        }
    }
}

/// A hue on `palette` as stored in a `Feedback`, quantised to one of `steps` colours so the look
/// of a cell changes rarely.
fn hue_colour(hue: u8, steps: u8) -> [f32; 3] {
    let steps = steps.max(1) as u16;
    let step = (hue as u16 * steps / 256) as f32;
    palette(step / steps as f32)
}

/// The byte a `Feedback` stores for a position on `palette`.
fn hue_byte(at: f32) -> u8 {
    (at.rem_euclid(1.0) * 256.0).min(255.0) as u8
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    rgb: [u8; 3],
}

const BLANK: Cell = Cell {
    ch: ' ',
    rgb: BACKGROUND,
};

/// How a cell is drawn: fading from one cell to another, this many `TWEEN_LEVELS` of the way.
type Look = (Cell, Cell, u8);

struct Grid {
    cols: i32,
    rows: i32,
    cells: Vec<Cell>,
}

impl Grid {
    fn new(cols: i32, rows: i32) -> Self {
        Self {
            cols,
            rows,
            cells: vec![BLANK; (cols * rows).max(0) as usize],
        }
    }

    /// Empties every cell, ready for the next step, and sizes the grid to the layout. Cells are
    /// swapped in and out of the saver between steps, so the vector it comes back with may be the
    /// wrong length or none at all; it keeps whatever memory it has.
    fn blank(&mut self, cols: i32, rows: i32) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize((cols * rows).max(0) as usize, BLANK);
    }

    /// The cell at (`x`, `y`), or a blank one where that's off the grid.
    #[cfg(test)]
    fn at(&self, x: f32, y: f32) -> Cell {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            self.cells[(row * self.cols + col) as usize]
        } else {
            BLANK
        }
    }

    fn put(&mut self, x: f32, y: f32, ch: char, rgb: [f32; 3]) {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            self.cells[(row * self.cols + col) as usize] = Cell {
                ch,
                rgb: rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
            };
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Letter {
    col: i32,
    row: i32,
    ch: char,
}

#[derive(Debug, Clone)]
struct Layout {
    cols: i32,
    rows: i32,
    cell_w: usize,
    cell_h: usize,
    /// Where the grid starts in the buffer, centring it.
    origin: (usize, usize),
    letters: Vec<Letter>,
    /// The logo's column, row, width and height in cells.
    logo: (i32, i32, i32, i32),
}

fn art(text: &str) -> Vec<Vec<char>> {
    text.lines().map(|line| line.chars().collect()).collect()
}

fn art_width(lines: &[Vec<char>]) -> usize {
    lines.iter().map(Vec::len).max().unwrap_or(1).max(1)
}

/// A terminal-like grid for a `w`×`h` pixel screen, with cells sized so the logo spans most of
/// its width. Narrow screens get the logo on two lines.
fn layout(w: usize, h: usize) -> Option<Layout> {
    let full = art(LOGO);
    let mut cell_w = (w as f32 * 0.62 / art_width(&full) as f32).floor() as usize;
    let lines = if cell_w >= 5 {
        full
    } else {
        let stacked = art(LOGO_STACKED);
        cell_w = (w as f32 * 0.8 / art_width(&stacked) as f32).floor() as usize;
        stacked
    };
    let cell_w = cell_w.clamp(3, 18);
    let cell_h = cell_w * 2;
    let (cols, rows) = ((w / cell_w) as i32, (h / cell_h) as i32);
    if cols < 1 || rows < 1 {
        return None;
    }
    let (art_w, art_h) = (art_width(&lines) as i32, lines.len() as i32);
    let (col0, row0) = ((cols - art_w) / 2, (rows - art_h) / 2);
    let letters = lines
        .iter()
        .enumerate()
        .flat_map(|(r, line)| {
            line.iter()
                .enumerate()
                .filter(|(_, ch)| !ch.is_whitespace())
                .map(move |(c, &ch)| Letter {
                    col: col0 + c as i32,
                    row: row0 + r as i32,
                    ch,
                })
        })
        .collect();
    Some(Layout {
        cols,
        rows,
        cell_w,
        cell_h,
        origin: (
            (w - cols as usize * cell_w) / 2,
            (h - rows as usize * cell_h) / 2,
        ),
        letters,
        logo: (col0, row0, art_w, art_h),
    })
}

/// Characters other than blocks and lines, rasterised once per size.
#[derive(Default)]
struct GlyphCache {
    px: f32,
    bitmaps: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl GlyphCache {
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        pixels: &mut [u8],
        width: usize,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        ch: char,
        rgb: [u8; 3],
        weight: f32,
    ) {
        let px = h as f32 * 0.72;
        if (self.px - px).abs() > 0.01 {
            self.bitmaps.clear();
            self.px = px;
        }
        let (ascent, descent) = text::line_metrics(Face::Mono, px);
        let (metrics, coverage) = self
            .bitmaps
            .entry(ch)
            .or_insert_with(|| text::glyph(Face::Mono, ch, px));
        let baseline = y0 as f32 + h as f32 / 2.0 + (ascent + descent) / 2.0;
        let left = x0 as i32 + (w as i32 - metrics.width as i32) / 2;
        let top = baseline.round() as i32 - metrics.height as i32 - metrics.ymin;
        for gy in 0..metrics.height {
            let py = top + gy as i32;
            if py < y0 as i32 || py >= (y0 + h) as i32 {
                continue;
            }
            for gx in 0..metrics.width {
                let px = left + gx as i32;
                let cover = coverage[gy * metrics.width + gx] as u32;
                if cover == 0 || px < x0 as i32 || px >= (x0 + w) as i32 {
                    continue;
                }
                let at = (py as usize * width + px as usize) * 4;
                let amount = cover as f32 / 255.0 * weight;
                for ((channel, &ink), &ground) in
                    pixels[at..at + 3].iter_mut().zip(&rgb).zip(&BACKGROUND)
                {
                    *channel = add_ink(*channel, ink, ground, amount);
                }
            }
        }
    }
}

fn fill(pixels: &mut [u8], width: usize, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for pixel in pixels[start..start + w * 4].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
}

/// Adds `rgb` over a rectangle, `weight` of the way from the background (1 paints it solid).
#[allow(clippy::too_many_arguments)]
fn ink_fill(
    pixels: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rgb: [u8; 3],
    weight: f32,
) {
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for pixel in pixels[start..start + w * 4].chunks_exact_mut(4) {
            for ((channel, &ink), &ground) in pixel[..3].iter_mut().zip(&rgb).zip(&BACKGROUND) {
                *channel = add_ink(*channel, ink, ground, weight);
            }
        }
    }
}

/// `channel` plus `amount` of the step from the background to `ink`. A cell fading out and the
/// one fading in, drawn over the background, add up to a blend of the two.
fn add_ink(channel: u8, ink: u8, ground: u8, amount: f32) -> u8 {
    (channel as f32 + (ink as f32 - ground as f32) * amount)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Draws a cell as `look` says: one cell, or two cross-fading between steps.
fn paint_cell(
    pixels: &mut [u8],
    width: usize,
    layout: &Layout,
    glyphs: &mut GlyphCache,
    col: i32,
    row: i32,
    (from, to, along): Look,
) {
    let (w, h) = (layout.cell_w, layout.cell_h);
    let x0 = layout.origin.0 + col as usize * w;
    let y0 = layout.origin.1 + row as usize * h;
    fill(pixels, width, x0, y0, w, h, BACKGROUND);
    let weight = along as f32 / TWEEN_LEVELS as f32;
    if from != to {
        ink_cell(pixels, width, glyphs, (x0, y0, w, h), from, 1.0 - weight);
    }
    ink_cell(pixels, width, glyphs, (x0, y0, w, h), to, weight);
}

/// Blocks and lines are drawn as exact rectangles, so rows join up as they do in a terminal.
fn ink_cell(
    pixels: &mut [u8],
    width: usize,
    glyphs: &mut GlyphCache,
    (x0, y0, w, h): (usize, usize, usize, usize),
    cell: Cell,
    weight: f32,
) {
    let rgb = cell.rgb;
    let (half_w, half_h) = (w / 2, h / 2);
    match cell.ch {
        ' ' => {}
        '█' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight),
        '▀' => ink_fill(pixels, width, x0, y0, w, half_h, rgb, weight),
        '▄' => ink_fill(pixels, width, x0, y0 + half_h, w, h - half_h, rgb, weight),
        '▌' => ink_fill(pixels, width, x0, y0, half_w, h, rgb, weight),
        '▐' => ink_fill(pixels, width, x0 + half_w, y0, w - half_w, h, rgb, weight),
        // Shades fill the cell at a fraction of the ink, which is what they look like in a
        // terminal and several times cheaper than a glyph.
        '░' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.25),
        '▒' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.5),
        '▓' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.75),
        '─' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h,
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        // A thin line a third of the way up or down the cell, so a line that climbs slowly can
        // step a third of a row at a time rather than a whole one.
        '▔' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h - h / 3,
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        '▁' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h + h / 3 - (h / 14).max(1),
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        '━' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h - h / 12,
            w,
            (h / 6).max(2),
            rgb,
            weight,
        ),
        // Braille patterns are drawn as their dots, two across and four down: a picture made of
        // them has four times the grid's detail, and the monospace font has none of them.
        ch @ '\u{2801}'..='\u{28ff}' => {
            let bits = ch as u32 - 0x2800;
            let (dot_w, dot_h) = ((w * 3 / 10).max(1), (h * 3 / 20).max(1));
            for (bit, (across, down)) in BRAILLE_DOTS.iter().enumerate() {
                if bits & (1 << bit) != 0 {
                    let x = x0 + (w * (1 + 2 * across)) / 4 - dot_w / 2;
                    let y = y0 + (h * (1 + 2 * down)) / 8 - dot_h / 2;
                    ink_fill(pixels, width, x, y, dot_w, dot_h, rgb, weight);
                }
            }
        }
        ch => glyphs.draw(pixels, width, x0, y0, w, h, ch, rgb, weight),
    }
}

/// Where each of a Braille pattern's eight dots sits, by bit: which of the two columns and which
/// of the four rows.
const BRAILLE_DOTS: [(usize, usize); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (0, 3),
    (1, 3),
];

/// The Braille pattern with a dot at each of (`across` 0..2, `down` 0..4) set in `dots`, a bit a
/// dot, row by row from the top left.
fn braille(dots: u8) -> char {
    let mut bits = 0u32;
    for (bit, (across, down)) in BRAILLE_DOTS.iter().enumerate() {
        if dots & (1 << (down * 2 + across)) != 0 {
            bits |= 1 << bit;
        }
    }
    char::from_u32(0x2800 + bits).unwrap_or(' ')
}

/// One Braille dot of the logo: where it is, in dots across and down the screen, and how far
/// across the logo it sits, 0 to 1.
#[derive(Debug, Clone, Copy)]
struct LogoDot {
    x: usize,
    y: usize,
    across: f32,
}

/// Every Braille dot the logo's letters cover, with half blocks covering half their cell.
fn logo_dots(layout: &Layout) -> Vec<LogoDot> {
    let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
    let mut dots = Vec::new();
    for letter in &layout.letters {
        if letter.col < 0 || letter.row < 0 {
            continue;
        }
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
                    dots.push(LogoDot {
                        x: letter.col as usize * 2 + side,
                        y: letter.row as usize * 4 + down,
                        across,
                    });
                }
            }
        }
    }
    dots
}

/// Draws the logo's letters in their own blocks, `fade` of the way in.
fn draw_letters(layout: &Layout, grid: &mut Grid, fade: f32) {
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
}

/// Many small things gathered into Braille cells: each lights its dot and adds its colour to its
/// cell, and a cell is drawn in the mean of its colours, brighter and whiter the more it holds.
#[derive(Default)]
struct Stipple {
    cols: usize,
    rows: usize,
    dots: Vec<u8>,
    /// Red, green and blue summed, and how many were added, a cell.
    ink: Vec<[f32; 4]>,
}

impl Stipple {
    fn clear(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.dots.clear();
        self.dots.resize(cols * rows, 0);
        self.ink.clear();
        self.ink.resize(cols * rows, [0.0; 4]);
    }

    /// Adds one at (`x`, `y`) in dots, `weight` strong.
    fn add(&mut self, x: f32, y: f32, rgb: [f32; 3], weight: f32) {
        if !(x >= 0.0 && y >= 0.0) {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let (col, row) = (x / 2, y / 4);
        if col >= self.cols || row >= self.rows {
            return;
        }
        let cell = row * self.cols + col;
        self.dots[cell] |= 1 << ((y % 4) * 2 + x % 2);
        let ink = &mut self.ink[cell];
        for (sum, channel) in ink.iter_mut().zip(rgb) {
            *sum += channel * weight;
        }
        ink[3] += weight;
    }

    /// Draws every cell holding anything. `full` is how much a cell holds to be drawn at full
    /// brightness; `white` how far past that its colour runs towards white.
    fn draw(&self, grid: &mut Grid, full: f32, white: f32, fade: f32) {
        for row in 0..self.rows.min(grid.rows.max(0) as usize) {
            for col in 0..self.cols.min(grid.cols.max(0) as usize) {
                let cell = row * self.cols + col;
                let dots = self.dots[cell];
                let ink = self.ink[cell];
                if dots == 0 || ink[3] <= 0.0 {
                    continue;
                }
                let rgb = [ink[0] / ink[3], ink[1] / ink[3], ink[2] / ink[3]];
                let level = (0.35 + 0.65 * (ink[3] / full).sqrt()).min(1.0);
                let whiter = ((ink[3] / full - 1.0) * white).clamp(0.0, 0.75);
                grid.put(
                    col as f32,
                    row as f32,
                    braille(dots),
                    mix(BG, mix(rgb, WHITE, whiter), level * fade),
                );
            }
        }
    }
}

/// xorshift64*: cheap, and the same every run for the same seed. Never seed it with 0.
fn xorshift(state: &mut u64) -> f32 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
}

pub struct Saver {
    buffer: Option<MemoryRenderBuffer>,
    /// The buffer's size in screen pixels.
    device: (usize, usize),
    layout: Option<Layout>,
    /// The grid composed at the step before the latest, which cells fade from while tweening —
    /// and, during a handover, the last grid of the variation being replaced, held still.
    from: Vec<Cell>,
    /// The grid composed at the latest step.
    to: Vec<Cell>,
    /// What the buffer shows, cell by cell.
    shown: Vec<Look>,
    /// Composed into and swapped with `to`, so a step allocates nothing.
    scratch: Grid,
    glyphs: GlyphCache,
    /// The variations to cycle through, in the settings file's order.
    chosen: Vec<&'static str>,
    /// Which of them is showing.
    index: usize,
    /// The chosen variations still to come this round, popped from the end. A round is every
    /// chosen variation but the one showing, shuffled, so each takes a turn before any comes
    /// back and none follows itself.
    upcoming: Vec<&'static str>,
    /// The shuffle's random state (splitmix64).
    shuffle: u64,
    current: Box<dyn Variation>,
    /// Seconds one variation holds before the next takes over; 0 never changes.
    change_every: f64,
    /// When the variation showing now took over, on the wallpaper's own clock.
    started: Option<f64>,
    /// The same moment on the animation clock, which a slowed wallpaper doesn't hold back, so
    /// each variation holds for `change_every` whatever its pace.
    shown_at: Option<f64>,
    /// A cross-fade from the variation before, and when it started.
    handover: Option<f64>,
    stepped_to: f64,
    /// Counts the variations built so far, so each gets its own seed.
    spawned: u64,
    cost: Cost,
    /// The machine's readings, and how far the local clock is ahead of UTC once they say.
    readings: Readings,
    utc_offset: Option<i64>,
    pub reduced_motion: bool,
    /// How fast the wallpaper runs against the animation clock: 1 normally, less to save power.
    pub pace: f64,
    /// How far the wallpaper's own clock has fallen behind the animation clock while slowed.
    lag: f64,
    /// The animation clock when it was last drawn, for the lag.
    last_drawn: Option<f64>,
}

impl Saver {
    pub fn new(reduced_motion: bool, wallpaper: &slipstream_config::Wallpaper) -> Self {
        let chosen = ids(wallpaper);
        let mut shuffle = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let index = (next_random(&mut shuffle) % chosen.len().max(1) as u64) as usize;
        let current = chosen
            .get(index)
            .and_then(|id| make(id))
            .unwrap_or_else(|| Box::<slipstream::Slipstream>::default());
        Self {
            buffer: None,
            device: (0, 0),
            layout: None,
            from: Vec::new(),
            to: Vec::new(),
            shown: Vec::new(),
            scratch: Grid::new(0, 0),
            glyphs: GlyphCache::default(),
            chosen,
            index,
            upcoming: Vec::new(),
            shuffle,
            current,
            change_every: wallpaper.change_every_mins as f64 * 60.0,
            started: None,
            shown_at: None,
            handover: None,
            stepped_to: 0.0,
            spawned: 0,
            cost: Cost::default(),
            readings: Readings::default(),
            utc_offset: None,
            reduced_motion,
            pace: 1.0,
            lag: 0.0,
            last_drawn: None,
        }
    }

    /// The wallpaper's own clock for animation-clock time `now`: behind it by however much it has
    /// been slowed, so a slowed wallpaper steps and moves at its pace without jumping when the
    /// pace changes.
    fn paced(&mut self, now: f64) -> f64 {
        if let Some(last) = self.last_drawn {
            let dt = (now - last).clamp(0.0, 0.1);
            self.lag += dt * (1.0 - self.pace.clamp(0.0, 1.0));
        }
        self.last_drawn = Some(now);
        now - self.lag
    }

    /// The settings changed. Ticking a variation on shows it at once, so the picture behind the
    /// Settings window is the preview; anything else just changes what comes next.
    pub fn set_wallpaper(&mut self, wallpaper: &slipstream_config::Wallpaper, now: f64) {
        let now = now - self.lag;
        self.change_every = wallpaper.change_every_mins as f64 * 60.0;
        let chosen = ids(wallpaper);
        if chosen == self.chosen {
            return;
        }
        let added = chosen.iter().position(|id| !self.chosen.contains(id));
        let showing = chosen.iter().position(|id| *id == self.current.id());
        self.chosen = chosen;
        match (added, showing) {
            (Some(index), _) => self.change_to(index, now),
            // What's on screen is still chosen: leave it up and carry on from where it is.
            (None, Some(index)) => self.index = index,
            (None, None) => self.change_to(0, now),
        }
    }

    /// The time, date, workspace or battery changed.
    pub fn set_readings(&mut self, readings: &Readings) {
        if *readings != self.readings {
            self.readings.clone_from(readings);
            self.utc_offset = self.readings.utc_offset(epoch_now()).or(self.utc_offset);
        }
    }

    /// Shows one variation now, whatever the settings say (the `wallpaper:` debug step).
    pub fn show(&mut self, id: &str, now: f64) -> bool {
        let now = now - self.lag;
        let Some(id) = make(id).map(|variation| variation.id()) else {
            return false;
        };
        self.chosen = vec![id];
        self.change_to(0, now);
        true
    }

    /// Hands over to the variation at `index`: the old one's last grid stays frozen while the new
    /// one fades in over it.
    fn change_to(&mut self, index: usize, now: f64) {
        let Some(mut next) = self.chosen.get(index).and_then(|id| make(id)) else {
            return;
        };
        self.spawned += 1;
        if let Some(layout) = self.layout.as_ref() {
            next.reset(layout, self.spawned.wrapping_mul(0x9e37_79b9));
        }
        tracing::info!(variation = next.id(), "living wallpaper");
        let id = next.id();
        self.upcoming.retain(|upcoming| *upcoming != id);
        self.index = index;
        self.current = next;
        self.started = Some(now);
        self.shown_at = Some(now + self.lag);
        if self.reduced_motion {
            // No cross-fade: one still picture replaces another on the next step.
            self.handover = None;
        } else {
            self.from.clone_from(&self.to);
            self.handover = Some(now);
        }
        // Compose the new variation on this pass rather than waiting out a step.
        self.stepped_to = f64::NEG_INFINITY;
    }

    /// The dwell has run out: hand over now, wherever the variation is in its cycle, so every
    /// variation holds for the same time. Never while bullet time is slowing the clock, where a
    /// cross-fade at quarter speed would read as a glitch. `now` is on the wallpaper's own clock.
    fn maybe_hand_over(&mut self, now: f64, slowed: bool) {
        if self.chosen.len() < 2 || self.change_every <= 0.0 || slowed || self.handover.is_some() {
            return;
        }
        let clock = now + self.lag;
        if clock - self.shown_at.unwrap_or(clock) < self.change_every {
            return;
        }
        let next = self.next_index();
        self.change_to(next, now);
    }

    /// Which chosen variation takes the next turn: the next of this round's shuffle, dealing a
    /// new round once it runs out.
    fn next_index(&mut self) -> usize {
        let chosen = &self.chosen;
        self.upcoming.retain(|id| chosen.contains(id));
        if self.upcoming.is_empty() {
            let showing = self.current.id();
            self.upcoming = chosen.iter().copied().filter(|id| *id != showing).collect();
            for i in (1..self.upcoming.len()).rev() {
                let j = (next_random(&mut self.shuffle) % (i as u64 + 1)) as usize;
                self.upcoming.swap(i, j);
            }
        }
        self.upcoming
            .pop()
            .and_then(|id| self.chosen.iter().position(|chosen| *chosen == id))
            .unwrap_or((self.index + 1) % self.chosen.len())
    }

    /// How long a step lasts. A variation asks for a rate; a screen with big cells, where every
    /// changed cell costs several times as much ink, is held to a slower one.
    fn step_secs(&self) -> f64 {
        match self.layout.as_ref() {
            Some(layout) => 1.0 / step_hz(self.current.step_hz(), layout),
            None => 1.0 / self.current.step_hz().clamp(1.0, 1.0 / STEP),
        }
    }

    /// The bottom of the logo in logical pixels from the top of the screen, once laid out.
    pub fn logo_bottom(&self, scale: f64) -> Option<f64> {
        let layout = self.layout.as_ref()?;
        let rows = (layout.logo.1 + layout.logo.3).max(0) as usize;
        Some((layout.origin.1 + rows * layout.cell_h) as f64 / scale)
    }

    /// The wallpaper for a screen `size` logical pixels big, at `alpha`. While `paused` (under a
    /// fullscreen window) it keeps its last frame. With `tween` (bullet time slowing the clock, so
    /// a step lasts many frames) cells fade from one step to the next rather than switching.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        now: f64,
        alpha: f32,
        paused: bool,
        tween: bool,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let now = self.paced(now);
        let device = (
            (size.w as f64 * scale).round() as usize,
            (size.h as f64 * scale).round() as usize,
        );
        let fresh = self.buffer.is_none() || self.device != device;
        if fresh {
            let layout = layout(device.0, device.1)?;
            self.buffer = Some(MemoryRenderBuffer::from_slice(
                &vec![0u8; device.0 * device.1 * 4],
                Fourcc::Abgr8888,
                (device.0 as i32, device.1 as i32),
                1,
                Transform::Normal,
                None,
            ));
            self.device = device;
            self.shown.clear();
            self.from.clear();
            self.to.clear();
            self.handover = None;
            self.scratch = Grid::new(layout.cols, layout.rows);
            self.spawned += 1;
            self.current
                .reset(&layout, self.spawned.wrapping_mul(0x9e37_79b9));
            self.layout = Some(layout);
            self.started.get_or_insert(now);
            self.shown_at.get_or_insert(now + self.lag);
            self.stepped_to = now;
        }
        if !paused {
            self.maybe_hand_over(now, tween);
        }
        if (fresh || now - self.stepped_to >= self.step_secs()) && !paused {
            let started = thread_cpu();
            self.step(now);
            self.cost.compose += thread_cpu().saturating_sub(started);
            self.cost.steps += 1;
        }
        // A handover fades the whole screen from the old variation's last grid to the new one's
        // latest, over 1.2 s rather than over one step.
        let along = if let Some(at) = self.handover {
            let along = ((now - at) / HANDOVER).clamp(0.0, 1.0);
            if along >= 1.0 {
                self.handover = None;
            }
            along
        } else if (tween || self.step_secs() > 1.0 / 20.0) && !paused {
            // Tweening shows the grid one step behind, fading towards the latest as the next
            // nears: for bullet time, and for any variation slower than 20 steps a second.
            ((now - self.stepped_to) / self.step_secs()).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let started = thread_cpu();
        self.paint(along);
        self.cost.paint += thread_cpu().saturating_sub(started);
        self.cost.report(self.current.id());
        self.again(renderer, size, alpha)
    }

    /// The picture last painted, as another element at `alpha`, without stepping or painting.
    pub fn again<R>(
        &self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let buffer = self.buffer.as_ref()?;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            buffer,
            Some(alpha),
            Some(Rectangle::from_size(
                (self.device.0 as f64, self.device.1 as f64).into(),
            )),
            Some(size),
            Kind::Unspecified,
        )
        .ok()
    }

    fn step(&mut self, now: f64) {
        let Some(layout) = self.layout.take() else {
            return;
        };
        let dt = if self.reduced_motion {
            0.0
        } else {
            (now - self.stepped_to).clamp(0.0, 0.1) as f32
        };
        self.stepped_to = now;
        let frame = Frame {
            layout: &layout,
            elapsed: now - self.started.unwrap_or(now),
            dt,
            still: self.reduced_motion,
            readings: &self.readings,
            local: self
                .utc_offset
                .map(|minutes| epoch_now() + minutes as f64 * 60.0),
        };
        self.scratch.blank(layout.cols, layout.rows);
        self.current.compose(frame, &mut self.scratch);
        // While a handover is fading, `from` holds the old variation's last grid and must stay
        // where it is; otherwise the step before the latest is what cells fade from.
        if self.handover.is_none() {
            std::mem::swap(&mut self.from, &mut self.to);
        }
        std::mem::swap(&mut self.to, &mut self.scratch.cells);
        self.layout = Some(layout);
    }

    /// Brings the buffer `along` the way from the previous step's grid to the latest: draws the
    /// cells whose look changed, and tells the buffer which rows of pixels to upload.
    fn paint(&mut self, along: f64) {
        let Saver {
            buffer,
            device,
            layout,
            from,
            to,
            shown,
            glyphs,
            cost,
            ..
        } = self;
        let (Some(buffer), Some(layout)) = (buffer.as_mut(), layout.as_ref()) else {
            return;
        };
        let level = (along * TWEEN_LEVELS as f64).round() as u8;
        let looks: Vec<Look> = to
            .iter()
            .enumerate()
            .map(|(i, &target)| {
                let start = from.get(i).copied().unwrap_or(target);
                if start == target || level >= TWEEN_LEVELS {
                    (target, target, TWEEN_LEVELS)
                } else if level == 0 {
                    (start, start, TWEEN_LEVELS)
                } else {
                    (start, target, level)
                }
            })
            .collect();
        let full = shown.len() != looks.len();
        if !full && looks == *shown {
            return;
        }
        let blank = (BLANK, BLANK, TWEEN_LEVELS);
        let (width, height) = *device;
        let mut repainted = 0u64;
        let mut context = buffer.render();
        let _ = context.draw(|pixels| {
            let mut damage: Vec<Rectangle<i32, Buffer>> = Vec::new();
            if full {
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
                }
            }
            for row in 0..layout.rows {
                let mut span: Option<(i32, i32)> = None;
                for col in 0..layout.cols {
                    let i = (row * layout.cols + col) as usize;
                    let look = looks[i];
                    let changed = if full {
                        look != blank
                    } else {
                        look != shown[i]
                    };
                    if changed {
                        repainted += 1;
                        paint_cell(pixels, width, layout, glyphs, col, row, look);
                        span = Some(span.map_or((col, col), |(first, _)| (first, col)));
                    }
                }
                if let (false, Some((first, last))) = (full, span) {
                    damage.push(Rectangle::new(
                        (
                            (layout.origin.0 + first as usize * layout.cell_w) as i32,
                            (layout.origin.1 + row as usize * layout.cell_h) as i32,
                        )
                            .into(),
                        (
                            ((last - first + 1) as usize * layout.cell_w) as i32,
                            layout.cell_h as i32,
                        )
                            .into(),
                    ));
                }
            }
            if full {
                damage.push(Rectangle::from_size((width as i32, height as i32).into()));
            }
            Ok::<_, ()>(damage)
        });
        *shown = looks;
        cost.cells += repainted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slowed_wallpaper_falls_behind_smoothly_and_keeps_its_place_at_full_pace() {
        let mut saver = Saver::new(true, &slipstream_config::Wallpaper::default());
        saver.pace = 0.5;
        let mut shown = 0.0;
        for frame in 0..=20 {
            shown = saver.paced(frame as f64 * 0.05);
        }
        assert!((shown - 0.5).abs() < 1e-9, "half of a second: {shown}");
        saver.pace = 1.0;
        assert!(
            (saver.paced(1.05) - 0.55).abs() < 1e-9,
            "no jump back to the clock"
        );
    }

    /// One step of `variation` on a blank grid, `elapsed` seconds into its own turn, as the saver
    /// composes one. Variations that carry state between steps need stepping in order instead.
    pub(super) fn composed(
        variation: &mut dyn Variation,
        layout: &Layout,
        elapsed: f64,
        still: bool,
    ) -> Grid {
        let mut grid = Grid::new(layout.cols, layout.rows);
        variation.compose(
            Frame {
                layout,
                elapsed,
                dt: if still { 0.0 } else { 1.0 / 30.0 },
                still,
                readings: &Readings::default(),
                local: None,
            },
            &mut grid,
        );
        grid
    }

    /// How many of the logo's letters are drawn where they belong.
    pub(super) fn at_home(grid: &Grid, layout: &Layout) -> usize {
        layout
            .letters
            .iter()
            .filter(|letter| grid.at(letter.col as f32, letter.row as f32).ch == letter.ch)
            .count()
    }

    /// How many cells are drawn at all.
    pub(super) fn drawn(grid: &Grid) -> usize {
        grid.cells.iter().filter(|cell| cell.ch != ' ').count()
    }

    /// A grid the shared code can paint without involving a variation: the logo's letters at
    /// home.
    fn logo_grid(layout: &Layout) -> Grid {
        let mut grid = Grid::new(layout.cols, layout.rows);
        for letter in &layout.letters {
            grid.put(letter.col as f32, letter.row as f32, letter.ch, WHITE);
        }
        grid
    }

    #[test]
    fn the_logo_spans_a_laptop_screen_and_stacks_on_a_narrow_one() {
        let wide = layout(1920, 1200).unwrap();
        assert_eq!(wide.logo.2, 121);
        assert!(wide.logo.0 >= 0 && wide.logo.1 >= 0);
        let letters = LOGO.chars().filter(|ch| !ch.is_whitespace()).count();
        assert_eq!(wide.letters.len(), letters);
        let narrow = layout(700, 1000).unwrap();
        assert_eq!(narrow.logo.3, art(LOGO_STACKED).len() as i32);
    }

    #[test]
    fn every_variation_in_the_settings_table_can_be_built_and_nothing_else() {
        for variation in slipstream_config::VARIATIONS {
            let built = make(variation.id)
                .unwrap_or_else(|| panic!("{} is in the table but can't be built", variation.id));
            assert_eq!(built.id(), variation.id, "a variation knows its own id");
        }
        assert!(make("no-such-variation").is_none());
    }

    #[test]
    fn the_settings_choose_which_variations_cycle() {
        let all = slipstream_config::Wallpaper {
            variations: Vec::new(),
            ..Default::default()
        };
        assert_eq!(ids(&all).len(), slipstream_config::VARIATIONS.len());
        let two = slipstream_config::Wallpaper {
            variations: vec!["vortex".into(), "nonsense".into(), "slipstream".into()],
            ..Default::default()
        };
        assert_eq!(
            ids(&two),
            vec!["vortex", "slipstream"],
            "in the file's order"
        );
    }

    fn saver_for(layout: &Layout) -> Saver {
        let mut saver = Saver::new(false, &slipstream_config::Wallpaper::default());
        saver.device = (640, 400);
        saver.buffer = Some(MemoryRenderBuffer::from_slice(
            &vec![0u8; 640 * 400 * 4],
            Fourcc::Abgr8888,
            (640, 400),
            1,
            Transform::Normal,
            None,
        ));
        saver.layout = Some(layout.clone());
        saver
    }

    #[test]
    fn only_changed_cells_are_redrawn() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.to = logo_grid(&layout).cells;
        saver.paint(1.0);
        let again = logo_grid(&layout);
        assert!(
            saver
                .shown
                .iter()
                .zip(&again.cells)
                .all(|(look, &cell)| *look == (cell, cell, TWEEN_LEVELS)),
            "nothing to redraw the second time"
        );
    }

    #[test]
    fn tweening_fades_cells_from_one_step_to_the_next() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.from = Grid::new(layout.cols, layout.rows).cells;
        saver.to = logo_grid(&layout).cells;
        let letter = layout.letters[0];
        let i = (letter.row * layout.cols + letter.col) as usize;
        let (before, after) = (saver.from[i], saver.to[i]);
        assert_ne!(before, after, "the letter arrives between the two steps");

        saver.paint(0.0);
        assert_eq!(saver.shown[i], (before, before, TWEEN_LEVELS));
        saver.paint(0.5);
        assert_eq!(saver.shown[i], (before, after, TWEEN_LEVELS / 2));
        saver.paint(1.0);
        assert_eq!(saver.shown[i], (after, after, TWEEN_LEVELS));
    }

    #[test]
    fn a_handover_holds_the_old_picture_still_while_the_new_one_fades_in() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.chosen = vec!["slipstream", "vortex"];
        saver.to = logo_grid(&layout).cells;
        let old = saver.to.clone();
        saver.change_to(1, 100.0);
        assert_eq!(saver.from, old, "the old variation's last grid is frozen");
        assert!(saver.handover.is_some());
        // A step during the handover replaces the latest grid, never the frozen one.
        saver.step(100.1);
        assert_eq!(saver.from, old);
        saver.step(100.2);
        assert_eq!(saver.from, old);
        // Once the fade is over, steps go back to fading from the step before.
        saver.handover = None;
        saver.step(100.3);
        assert_ne!(saver.from, old);
    }

    #[test]
    fn ticking_a_variation_on_shows_it_at_once() {
        let mut saver = Saver::new(false, &slipstream_config::Wallpaper::default());
        assert_eq!(saver.current.id(), "slipstream");
        saver.set_wallpaper(
            &slipstream_config::Wallpaper {
                variations: vec!["slipstream".into(), "vortex".into()],
                ..Default::default()
            },
            10.0,
        );
        assert_eq!(saver.current.id(), "vortex", "the one just added");
        // Unticking the other one leaves what's on screen alone.
        saver.set_wallpaper(
            &slipstream_config::Wallpaper {
                variations: vec!["vortex".into()],
                ..Default::default()
            },
            20.0,
        );
        assert_eq!(saver.current.id(), "vortex");
    }

    #[test]
    fn a_variation_holds_for_its_dwell_and_hands_over_as_it_ends() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.chosen = vec!["slipstream", "vortex"];
        saver.change_every = 60.0;
        saver.started = Some(0.0);
        saver.shown_at = Some(0.0);
        saver.handover = None;
        saver.maybe_hand_over(59.9, false);
        assert_eq!(saver.current.id(), "slipstream", "still inside its dwell");
        // Bullet time never changes the picture under it.
        saver.maybe_hand_over(60.0, true);
        assert_eq!(saver.current.id(), "slipstream");
        // Mid-cycle or not, it goes as soon as the dwell is up.
        saver.maybe_hand_over(60.0, false);
        assert_eq!(saver.current.id(), "vortex");
    }

    #[test]
    fn a_slowed_wallpaper_still_changes_on_time() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.chosen = vec!["slipstream", "vortex"];
        saver.change_every = 60.0;
        saver.started = Some(0.0);
        saver.shown_at = Some(0.0);
        saver.handover = None;
        // A minute at half pace leaves the wallpaper's clock 30 s behind.
        saver.lag = 30.0;
        saver.maybe_hand_over(29.9, false);
        assert_eq!(saver.current.id(), "slipstream");
        saver.maybe_hand_over(30.0, false);
        assert_eq!(saver.current.id(), "vortex");
    }

    #[test]
    fn variations_take_turns_in_a_shuffled_order() {
        let layout = layout(640, 400).unwrap();
        let mut saver = saver_for(&layout);
        saver.chosen = vec!["slipstream", "vortex", "life", "warp", "tide"];
        saver.shuffle = 7;
        let mut shown = vec![saver.current.id()];
        for turn in 0..40 {
            let next = saver.next_index();
            saver.change_to(next, turn as f64);
            shown.push(saver.current.id());
        }
        for pair in shown.windows(2) {
            assert_ne!(pair[0], pair[1], "none follows itself");
        }
        // Each round is the four not showing when it was dealt, each once.
        for (at, round) in shown[1..].chunks(4).enumerate() {
            let mut ids = round.to_vec();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(
                ids.len(),
                4,
                "each takes a turn before any comes back: {shown:?}"
            );
            assert!(!round.contains(&shown[at * 4]), "{shown:?}");
        }
        let in_file_order = shown.windows(2).all(|pair| {
            let at = |id| {
                saver
                    .chosen
                    .iter()
                    .position(|chosen| *chosen == id)
                    .unwrap()
            };
            at(pair[1]) == (at(pair[0]) + 1) % saver.chosen.len()
        });
        assert!(!in_file_order, "the order is shuffled: {shown:?}");
    }

    #[test]
    fn the_local_clock_s_offset_comes_from_the_reading() {
        let at = |time: &str| Readings {
            time: time.into(),
            ..Default::default()
        };
        // 12:00:30 UTC on some day.
        let epoch = 20_000.0 * 86_400.0 + 12.0 * 3600.0 + 30.0;
        assert_eq!(at("12:00").utc_offset(epoch), Some(0));
        assert_eq!(at("13:00").utc_offset(epoch), Some(60), "summer time");
        assert_eq!(at("17:30").utc_offset(epoch), Some(330), "India");
        assert_eq!(at("07:00").utc_offset(epoch), Some(-300), "New York");
        // The reading lags the clock by a second or two, across a minute.
        assert_eq!(at("13:00").utc_offset(epoch + 31.0), Some(60));
        assert_eq!(at("").utc_offset(epoch), None);
        let mut saver = Saver::new(false, &slipstream_config::Wallpaper::default());
        saver.set_readings(&Readings {
            time: "not a time".into(),
            ..Default::default()
        });
        assert_eq!(saver.utc_offset, None);
    }

    #[test]
    fn the_feedback_buffer_carries_ink_outwards_and_fades_it() {
        let mut buffer = Feedback::default();
        buffer.reset(40, 20);
        buffer.ink(24.0, 10.0, 1.0, 7);
        let outwards = Motion {
            centre: (20.0, 10.0),
            zoom: 2.0,
            turn: 0.0,
            shift: (0.0, 0.0),
        };
        buffer.advance(0.5, |x, y| outwards.source(x, y));
        let (light, hue) = buffer.at(28, 10);
        assert!(
            (light - 0.5).abs() < 0.01,
            "twice as far out, half as bright: {light}"
        );
        assert_eq!(hue, 7, "the hue travels with it");
        assert!(buffer.at(24, 10).0 < 0.3, "and it has left where it was");
        // A shift moves it without zooming.
        let across = Motion {
            centre: (20.0, 10.0),
            zoom: 1.0,
            turn: 0.0,
            shift: (3.0, 0.0),
        };
        buffer.advance(1.0, |x, y| across.source(x, y));
        assert!(buffer.at(31, 10).0 > 0.49);
        let mut grid = Grid::new(40, 20);
        buffer.draw(&mut grid, 0.12, &['░', '█'], |_, _| WHITE);
        assert_eq!(grid.at(31.0, 10.0).ch, '░', "half light is the dimmer look");
        assert_eq!(
            drawn_cells(&grid),
            9,
            "with the cells round it, sampled between cells"
        );
    }

    fn drawn_cells(grid: &Grid) -> usize {
        grid.cells.iter().filter(|cell| cell.ch != ' ').count()
    }

    #[test]
    fn a_cross_fade_blends_the_two_cells_colours() {
        let (ink, ground) = (200, BACKGROUND[0]);
        let solid = add_ink(ground, ink, ground, 1.0);
        assert_eq!(solid, ink, "full weight paints the ink");
        let half = add_ink(ground, ink, ground, 0.5);
        assert!(half > ground && half < ink);
        // Half of one colour then half of another: their average, give or take rounding.
        let out_then_in = add_ink(add_ink(ground, 100, ground, 0.5), ink, ground, 0.5);
        assert!(
            (out_then_in as i32 - 150).abs() <= 1,
            "{out_then_in} should be about 150"
        );
    }
}
