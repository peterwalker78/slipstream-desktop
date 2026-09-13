//! Code rain: a port of xscreensaver's GLMatrix (`hacks/glx/glmatrix.c`). Strips of glyphs fall
//! through a 3D grid, a brightness wave runs down each one, and glyphs are added as light, green on
//! black. The glyphs come from a font rather than GLMatrix's scanlined atlas, so they're sharp.
//!
//! A band is a narrow slice of that world, drawn on the CPU. Its look carries the machine's
//! load: grey and drifting when the machine is idle, green and quick when it is working.
//!
//! glmatrix, Copyright (c) 2003-2018 Jamie Zawinski <jwz@jwz.org>
//!
//! Permission to use, copy, modify, distribute, and sell this software and its documentation for
//! any purpose is hereby granted without fee, provided that the above copyright notice appear in
//! all copies and that both that copyright notice and this permission notice appear in supporting
//! documentation. No representations are made about the suitability of this software for any
//! purpose. It is provided "as is" without express or implied warranty.

use std::collections::HashMap;

/// Columns of glyphs across a band, each a third of its width. GLMatrix sizes glyphs by the
/// window's height and scatters strips at every depth, which in a narrow, screen-tall stream gives
/// a few huge glyphs. Depth still sets each strip's fog, drift and splash, but not its size or
/// place.
pub const COLUMNS: usize = 3;
/// The middle column rains first.
const COLUMN_ORDER: [usize; COLUMNS] = [1, 0, 2];
/// The most rows a band can have; each has as many as fill its height.
const MAX_ROWS: usize = 160;
const GRID_DEPTH: f32 = 35.0;
const SPLASH_RATIO: f32 = 0.7;
const WAVE_SIZE: usize = 22;
/// GLMatrix's default `--delay`: a frame every 30 ms, here of the animation clock.
const FRAME: f64 = 0.030;
/// A cell is this much taller than wide, as GLMatrix's 32×46 atlas cells are.
const CELL_ASPECT: f32 = 46.0 / 32.0;
/// Ticks run before a band is first shown: about one full strip cycle, so a new stream looks
/// like rain that has been falling for a while.
const WARM_TICKS: usize = 1400;
/// The longest a column rests between strips while its app's network is quiet: about two seconds
/// at normal speed.
const REST_TICKS: f32 = 60.0;
/// How long a change in demand takes to settle into the rain, in seconds of the animation clock.
const EASE: f64 = 2.0;
/// How much of the longest rest a column may take between strips.
const REST_SHARE: f32 = 0.5;
/// At most this many frames per step, so a long stall can't freeze the compositor.
const MAX_TICKS: usize = 600;

/// The rain's glyphs: Noto Sans CJK JP DemiLight, cut down to ASCII and half-width katakana (see
/// `assets/fonts/rain-font.md`). GLMatrix's atlas has CRT scanlines and a phosphor glow drawn in,
/// which at stream size blur and fatten the glyphs, so they're drawn sharply from a font instead.
static RAIN_FONT: &[u8] = include_bytes!("../../assets/fonts/NotoSansCJKjp-DemiLight-Rain.otf");
/// GLMatrix's 16 kana (atlas glyphs 160–175), as half-width katakana.
const KANA: [char; 16] = [
    'ﾊ', 'ﾐ', 'ﾋ', 'ｳ', 'ｼ', 'ﾅ', 'ﾓ', 'ﾆ', 'ｻ', 'ﾜ', 'ﾂ', 'ｵ', 'ﾘ', 'ｱ', 'ﾎ', 'ﾃ',
];
/// The light a glyph adds where it's fully covered: green, a little towards cyan. The focus
/// ring and the panels' keyboard ring take their colour from here, so the desktop's green is
/// the rain's green.
pub const TINT: [f32; 3] = [0.35, 1.0, 0.55];
/// The other end of the scale: the light grey of an idle machine, #9aa3ad.
pub const QUIET: [f32; 3] = [0.60, 0.64, 0.68];
/// How much brighter font glyphs are drawn than GLMatrix's atlas glyphs would be.
const GAIN: f32 = 2.5;
/// A held band's light at no load and at full load: with nothing falling, how bright the still
/// glyphs are is left to say how hard the app is working, alongside their colour.
const STILL_QUIET: f32 = 0.6;
const STILL_BUSY: f32 = 1.0;

/// A cell's width and height in pixels for a band `width` pixels wide.
fn cell_size(width: f32) -> (f32, f32) {
    let w = width / COLUMNS as f32;
    (w, w * CELL_ASPECT)
}

/// Rows enough to fill a band's height, plus one.
fn rows_for(width: f32, height: f32) -> usize {
    ((height / cell_size(width).1.max(1.0)).ceil() as usize + 1).clamp(8, MAX_ROWS)
}

/// GLMatrix's brightness_ramp: `0.2 + 0.8·sin((22 − k)/21 · π/2)`, from about 1 down to 0.26.
fn ramp(k: usize) -> f32 {
    let t = (WAVE_SIZE - k) as f32 / (WAVE_SIZE - 1) as f32;
    0.2 + 0.8 * (t * std::f32::consts::FRAC_PI_2).sin()
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Demand → speed: a slow drift on an idle machine, 2.5× flat out, straight in between.
pub fn speed_for(demand: f32) -> f32 {
    0.22 + 2.28 * clamp01(demand)
}

/// Demand → a held band's brightness, from `STILL_QUIET` to `STILL_BUSY` in a straight line.
pub fn still_brightness_for(demand: f32) -> f32 {
    STILL_QUIET + (STILL_BUSY - STILL_QUIET) * clamp01(demand)
}

/// Demand → colour: light grey when nothing is happening, the rain's green when everything is,
/// mixed channel by channel so the shift between them is even.
pub fn colour_for(demand: f32) -> [f32; 3] {
    let share = clamp01(demand);
    let mut colour = [0.0; 3];
    for (channel, (quiet, busy)) in colour.iter_mut().zip(QUIET.into_iter().zip(TINT)) {
        *channel = quiet + (busy - quiet) * share;
    }
    colour
}

/// What a band shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub speed: f32,
    pub colour: [f32; 3],
}

impl Look {
    /// What the machine's load looks like: `demand` is 0 to 1 (`usage::Demand`).
    pub fn for_demand(demand: f32) -> Self {
        Self {
            speed: speed_for(demand),
            colour: colour_for(demand),
        }
    }
}

/// xorshift64: GLMatrix uses the C library's random numbers; any decent generator will do.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn frand(&mut self, f: f32) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 * f
    }

    fn bellrand(&mut self, f: f32) -> f32 {
        (self.frand(f) + self.frand(f) + self.frand(f)) / 3.0
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }

    /// Matrix mode's glyphs: digits 0–9 (atlas 16–25) and the 16 kana (atlas 160–175).
    fn glyph(&mut self) -> i16 {
        let pick = self.below(26) as i16;
        if pick < 10 {
            16 + pick
        } else {
            160 + pick - 10
        }
    }
}

#[derive(Debug, Clone)]
struct Strip {
    /// Frames left resting before the strip starts to fall.
    rest: u32,
    y: f32,
    z: f32,
    dz: f32,
    /// The head: where drawing (or erasing) has reached, in cells from the top.
    spinner_y: f32,
    spinner_speed: f32,
    spin_speed: u32,
    spin_tick: u32,
    wave_pos: usize,
    wave_speed: u32,
    wave_tick: u32,
    erasing: bool,
    /// Atlas index + 1; negative while spinning; 0 for an empty cell.
    glyphs: [i16; MAX_ROWS],
    /// Cells of the app's name, drawn brighter and unmirrored.
    highlight: [bool; MAX_ROWS],
    spinner: i16,
}

impl Strip {
    fn empty() -> Self {
        Self {
            rest: 0,
            y: 0.0,
            z: 0.0,
            dz: 0.0,
            spinner_y: 0.0,
            spinner_speed: 0.0,
            spin_speed: 1,
            spin_tick: 0,
            wave_pos: 0,
            wave_speed: 1,
            wave_tick: 0,
            erasing: false,
            glyphs: [0; MAX_ROWS],
            highlight: [false; MAX_ROWS],
            spinner: 0,
        }
    }
}

pub struct Band {
    /// Written into strips, as GLMatrix's `--clock` writes the time.
    name: Vec<u8>,
    /// The slice's width in pixels, a third of it per column, and the rows that fill its height.
    width: f32,
    rows: usize,
    frames: f64,
    target: Look,
    colour: [f32; 3],
    speed: f32,
    strips: Vec<Strip>,
    rng: Rng,
    /// Held still, for reduced motion: the light every glyph is drawn at, in place of falling.
    still: Option<f32>,
}

impl Band {
    pub fn new(name: &str, look: Look, width: f32, height: f32, seed: u64) -> Self {
        let mut band = Self {
            name: name.bytes().collect(),
            width,
            rows: rows_for(width, height),
            frames: 0.0,
            target: look,
            // A new stream starts at the machine's present look rather than easing in from
            // nothing.
            colour: look.colour,
            speed: look.speed,
            strips: (0..COLUMNS).map(|_| Strip::empty()).collect(),
            rng: Rng(seed | 1),
            still: None,
        };
        for i in 0..COLUMNS {
            band.reset(i, true);
        }
        for _ in 0..WARM_TICKS {
            for i in 0..COLUMNS {
                band.tick(i);
            }
        }
        band
    }

    pub fn set_look(&mut self, look: Look) {
        self.target = look;
    }

    /// Holds the band still at `demand`, for reduced motion: nothing falls, spins or eases, and
    /// the glyphs where they stand take that demand's colour and brightness at once. The next
    /// `step` lets it fall again.
    pub fn hold(&mut self, demand: f32) {
        let look = Look::for_demand(demand);
        self.target = look;
        self.colour = look.colour;
        self.speed = look.speed;
        self.still = Some(still_brightness_for(demand));
    }

    pub fn is_still(&self) -> bool {
        self.still.is_some()
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width;
        self.rows = rows_for(width, height);
    }

    fn reset(&mut self, i: usize, startup: bool) {
        // Between strips a column rests. It used to be the network's reading; now nothing rides
        // on it, so it is simply a pause of up to half the longest rest.
        let rest = if startup {
            0
        } else {
            self.rng.frand(REST_TICKS * REST_SHARE) as u32
        };
        let Band {
            name,
            rows,
            strips,
            rng,
            ..
        } = self;
        let rows = *rows;
        let s = &mut strips[i];
        s.rest = rest;
        s.z = GRID_DEPTH * 0.2 - rng.frand(GRID_DEPTH * 0.7);
        // Rows sit on whole cells, so the three columns line up.
        s.y = (rows / 2) as f32;
        s.dz = rng.bellrand(0.02);
        s.spinner_y = 0.0;
        s.spinner_speed = rng.bellrand(0.3);
        s.spin_speed = rng.bellrand(2.0) as u32 + 1;
        s.spin_tick = 0;
        s.wave_pos = 0;
        s.wave_speed = rng.bellrand(3.0) as u32 + 1;
        s.wave_tick = 0;
        s.erasing = false;
        s.glyphs = [0; MAX_ROWS];
        s.highlight = [false; MAX_ROWS];
        let mut name_shown = false;
        let mut cell = 0;
        while cell < rows {
            // GLMatrix's clock text: written into about one strip in five, at a random height.
            if !name.is_empty()
                && !name_shown
                && cell < rows - 5
                && rng.below(((rows - 5) * 5) as u32) == 0
            {
                for &byte in name.iter() {
                    if cell >= rows {
                        break;
                    }
                    s.glyphs[cell] = if (32..128).contains(&byte) {
                        (byte - 32 + 1) as i16
                    } else {
                        0
                    };
                    s.highlight[cell] = true;
                    cell += 1;
                }
                name_shown = true;
                continue;
            }
            let draw = rng.below(7) != 0;
            let spin = draw && rng.below(20) == 0;
            let glyph = if draw { rng.glyph() + 1 } else { 0 };
            s.glyphs[cell] = if spin { -glyph } else { glyph };
            cell += 1;
        }
        s.spinner = -(rng.glyph() + 1);
        if startup {
            // New strips start out erasing and empty from a random height, so rain builds up
            // gradually, as it does when GLMatrix starts.
            s.glyphs = [0; MAX_ROWS];
            s.erasing = true;
            s.spinner_y = rng.frand(rows as f32);
        }
    }

    fn tick(&mut self, i: usize) {
        let rows = self.rows as f32;
        let s = &mut self.strips[i];
        if s.rest > 0 {
            s.rest -= 1;
            return;
        }
        s.z += s.dz;
        let splashed = s.z > GRID_DEPTH * SPLASH_RATIO;
        if !splashed {
            s.spinner_y += s.spinner_speed;
        }
        let erased = !splashed && s.spinner_y >= rows && s.erasing;
        if splashed || erased {
            self.reset(i, false);
            return;
        }
        let Band { strips, rng, .. } = self;
        let s = &mut strips[i];
        if s.spinner_y >= rows {
            s.erasing = true;
            s.spinner_y = 0.0;
            // Erasing goes slower than drawing did.
            s.spinner_speed /= 2.0;
        }
        s.spin_tick += 1;
        if s.spin_tick > s.spin_speed {
            s.spin_tick = 0;
            s.spinner = -(rng.glyph() + 1);
            for glyph in s.glyphs.iter_mut().filter(|glyph| **glyph < 0) {
                *glyph = -(rng.glyph() + 1);
                // Sometimes a spinner stops spinning.
                if rng.below(800) == 0 {
                    *glyph = -*glyph;
                }
            }
        }
        s.wave_tick += 1;
        if s.wave_tick > s.wave_speed {
            s.wave_tick = 0;
            s.wave_pos = (s.wave_pos + 1) % WAVE_SIZE;
        }
    }

    /// Advances by `seconds` of the animation clock, in whole GLMatrix frames at the app's CPU
    /// speed. Returns whether any frame passed, so the caller knows to draw again.
    pub fn step(&mut self, seconds: f64) -> bool {
        self.still = None;
        let seconds = seconds.max(0.0);
        // Changes settle over a couple of seconds, so a spike doesn't make the rain jump: the
        // colour drifts between grey and green rather than switching.
        let ease = (1.0 - (-seconds / EASE).exp()) as f32;
        for (channel, target) in self.colour.iter_mut().zip(self.target.colour) {
            *channel += (target - *channel) * ease;
        }
        self.speed += (self.target.speed - self.speed) * ease;
        self.frames += seconds / FRAME * self.speed as f64;
        let ticks = (self.frames.floor() as usize).min(MAX_TICKS);
        self.frames -= self.frames.floor();
        for _ in 0..ticks {
            for i in 0..self.strips.len() {
                self.tick(i);
            }
        }
        ticks > 0
    }

    /// Adds the rain's light to premultiplied RGBA `pixels`, `w`×`h`, centred on column `cx`
    /// and clipped to the band's width. Light is added, as GLMatrix blends, so over a dark
    /// backdrop it glows.
    pub fn draw(&self, pixels: &mut [u8], w: usize, h: usize, cx: f32, glyphs: &mut Glyphs) {
        let clip = (
            (cx - self.width / 2.0).max(0.0) as i32,
            ((cx + self.width / 2.0) as i32).min(w as i32),
        );
        let (step_x, step_y) = cell_size(self.width);
        let size = (step_x.round() as usize, step_y.round() as usize);
        if size.0 < 2 {
            return;
        }
        for (i, s) in self.strips.iter().enumerate().filter(|(_, s)| s.rest == 0) {
            let left = cx - self.width / 2.0 + COLUMN_ORDER[i] as f32 * step_x;
            let screen_y = |y: f32| h as f32 / 2.0 - y * step_y;
            let fog = 0.2 + 0.8 * (s.z / GRID_DEPTH + 0.5);
            let splash = if s.z > GRID_DEPTH / 2.0 {
                let ratio =
                    (s.z - GRID_DEPTH / 2.0) / (GRID_DEPTH * SPLASH_RATIO - GRID_DEPTH / 2.0);
                ramp(((ratio * WAVE_SIZE as f32) as usize).min(WAVE_SIZE - 1))
            } else {
                1.0
            };
            let light = fog * self.still.unwrap_or(1.0);
            let mut target = Target {
                pixels: &mut *pixels,
                w,
                h,
                clip,
                colour: self.colour,
            };
            for cell in 0..self.rows {
                let glyph = s.glyphs[cell];
                let reached = s.spinner_y >= cell as f32;
                let shown = if s.erasing { !reached } else { reached };
                if glyph == 0 || !shown {
                    continue;
                }
                // glmatrix reads one entry past its wave table here; that cell stays dark.
                let j = WAVE_SIZE - ((cell + WAVE_SIZE - s.wave_pos) % WAVE_SIZE);
                if j >= WAVE_SIZE {
                    continue;
                }
                target.glyph(
                    glyphs,
                    glyph,
                    s.highlight[cell],
                    left,
                    screen_y(s.y - cell as f32 + 1.0),
                    size,
                    ramp(j) * light,
                    splash,
                );
            }
            if !s.erasing {
                target.glyph(
                    glyphs,
                    s.spinner,
                    false,
                    left,
                    screen_y(s.y - s.spinner_y + 1.0),
                    size,
                    light,
                    splash,
                );
            }
        }
    }
}

struct Target<'a> {
    pixels: &'a mut [u8],
    w: usize,
    h: usize,
    /// Left inclusive, right exclusive.
    clip: (i32, i32),
    /// What the rain is lit in, from the machine's demand.
    colour: [f32; 3],
}

impl Target<'_> {
    #[allow(clippy::too_many_arguments)]
    fn glyph(
        &mut self,
        glyphs: &mut Glyphs,
        glyph: i16,
        highlight: bool,
        x: f32,
        y: f32,
        size: (usize, usize),
        brightness: f32,
        splash: f32,
    ) {
        let (x, y) = (x.round() as i32, y.round() as i32);
        if y > self.h as i32 || y + (size.1 as i32) < 0 {
            return;
        }
        let mut brightness = brightness;
        if glyph < 0 {
            // Spinners and the head.
            brightness *= 1.5;
        }
        if highlight {
            brightness *= 2.0;
        }
        // Fixed-function GL clamps colour at 1. Font strokes are thinner than the atlas's glowing
        // glyphs, so they carry more light to look as bright.
        let alpha = (brightness * GAIN).min(1.0) * splash;
        if alpha < 0.01 {
            return;
        }
        let index = (glyph.unsigned_abs() - 1) as usize;
        // Matrix mode mirrors every glyph; text stays readable.
        let Some(light) = glyphs.scaled(index, !highlight, size) else {
            return;
        };
        // Coverage is cached uncoloured, so one cached glyph serves every colour the rain takes.
        let k = self.colour.map(|channel| (alpha * channel * 256.0) as u32);
        for row in 0..size.1 {
            let py = y + row as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for col in 0..size.0 {
                let px = x + col as i32;
                if px < self.clip.0 || px >= self.clip.1 {
                    continue;
                }
                let cover = light[row * size.0 + col] as u32;
                if cover == 0 {
                    continue;
                }
                let at = (py as usize * self.w + px as usize) * 4;
                for (channel, k) in self.pixels[at..at + 3].iter_mut().zip(k) {
                    let added = *channel as u32 + ((cover * k) >> 8);
                    *channel = added.min(255) as u8;
                }
            }
        }
    }
}

/// The rain's glyphs, rasterised from its font once per size and cached as coverage — one byte a
/// pixel, no colour, because the colour changes with the machine's load and the cache must not.
pub struct Glyphs {
    font: fontdue::Font,
    scaled: HashMap<(usize, bool, (usize, usize)), Vec<u8>>,
}

impl Glyphs {
    pub fn load() -> Option<Self> {
        let font = fontdue::Font::from_bytes(RAIN_FONT, fontdue::FontSettings::default()).ok()?;
        Some(Self {
            font,
            scaled: HashMap::new(),
        })
    }

    /// The character at GLMatrix's atlas `index`: ASCII from 0, kana from 160.
    fn char_for(index: usize) -> Option<char> {
        match index {
            0..=94 => char::from_u32(index as u32 + 32),
            160..=175 => Some(KANA[index - 160]),
            _ => None,
        }
    }

    /// Glyph `index` centred in a `size` cell (width, height), as coverage.
    fn scaled(&mut self, index: usize, mirrored: bool, size: (usize, usize)) -> Option<&[u8]> {
        if self.scaled.len() > 4000 {
            self.scaled.clear();
        }
        let key = (index, mirrored, size);
        if !self.scaled.contains_key(&key) {
            let ch = Self::char_for(index)?;
            let (w, h) = size;
            // Half-width characters are half an em wide: this leaves a gap between columns.
            let px = (h as f32 * 0.95).min(w as f32 * 1.7);
            let (metrics, coverage) = self.font.rasterize(ch, px);
            let mut out = vec![0u8; w * h];
            let left = (w as i32 - metrics.width as i32) / 2;
            // Capitals and kana stand about 0.72 em tall; centre that in the cell.
            let baseline = (h as f32 / 2.0 + px * 0.36).round() as i32;
            let top = baseline - metrics.ymin - metrics.height as i32;
            for gy in 0..metrics.height {
                let oy = top + gy as i32;
                if oy < 0 || oy >= h as i32 {
                    continue;
                }
                for gx in 0..metrics.width {
                    let ox = left + gx as i32;
                    if ox < 0 || ox >= w as i32 {
                        continue;
                    }
                    // Matrix mode mirrors every glyph.
                    let ox = if mirrored { w as i32 - 1 - ox } else { ox };
                    out[oy as usize * w + ox as usize] = coverage[gy * metrics.width + gx];
                }
            }
            self.scaled.insert(key, out);
        }
        self.scaled.get(&key).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: Look = Look {
        speed: 1.0,
        colour: TINT,
    };

    #[test]
    fn the_brightness_wave_runs_from_full_to_a_quarter() {
        assert!((ramp(0) - 1.0).abs() < 0.01);
        assert!((ramp(21) - 0.26).abs() < 0.01);
    }

    #[test]
    fn demand_maps_onto_the_rain_in_a_straight_line() {
        assert_eq!(speed_for(0.0), 0.22);
        assert!((speed_for(1.0) - 2.5).abs() < 1e-4);
        // Halfway along is halfway between, for both readings.
        assert!((speed_for(0.5) - 1.36).abs() < 1e-4);
        assert_eq!(colour_for(0.0), QUIET);
        assert_eq!(colour_for(1.0), TINT);
        for (channel, (quiet, busy)) in colour_for(0.25)
            .into_iter()
            .zip(QUIET.into_iter().zip(TINT))
        {
            assert!((channel - (quiet + (busy - quiet) * 0.25)).abs() < 1e-6);
        }
        // Out of range readings clamp rather than running off either end.
        assert_eq!(colour_for(-1.0), QUIET);
        assert_eq!(speed_for(9.0), speed_for(1.0));
    }

    #[test]
    fn an_idle_machine_rains_grey_and_a_busy_one_green() {
        let mut glyphs = Glyphs::load().expect("the font loads");
        let (w, h) = (100, 600);
        let mut lit = |look: Look| {
            let band = Band::new("FIREFOX", look, 60.0, h as f32, 42);
            let mut pixels = vec![0u8; w * h * 4];
            band.draw(&mut pixels, w, h, 50.0, &mut glyphs);
            pixels
                .chunks_exact(4)
                .filter(|pixel| pixel[1] > 0)
                .fold((0u64, 0u64), |(r, g), p| (r + p[0] as u64, g + p[1] as u64))
        };
        let (quiet_red, quiet_green) = lit(Look::for_demand(0.0));
        let (busy_red, busy_green) = lit(Look::for_demand(1.0));
        assert!(
            quiet_red * 4 > quiet_green * 3,
            "grey: red keeps up with green ({quiet_red} vs {quiet_green})"
        );
        assert!(
            busy_green > busy_red * 2,
            "green: ({busy_red} vs {busy_green})"
        );
    }

    #[test]
    fn frames_follow_the_animation_clock_at_the_app_speed() {
        let mut band = Band::new("FOOT", LOOK, 60.0, 1000.0, 7);
        assert!(!band.step(0.01), "a third of a frame draws nothing new");
        assert!(band.step(0.02), "the rest of the frame does");
        let mut busy = Band::new("FOOT", Look { speed: 2.5, ..LOOK }, 60.0, 1000.0, 7);
        busy.step(0.3);
        assert!(busy.frames < 1.0);
    }

    #[test]
    fn a_held_band_stays_put_and_lights_by_its_demand() {
        let mut glyphs = Glyphs::load().expect("the font loads");
        let (w, h) = (100, 600);
        let mut lit = |band: &Band| {
            let mut pixels = vec![0u8; w * h * 4];
            band.draw(&mut pixels, w, h, 50.0, &mut glyphs);
            pixels
        };
        let mut band = Band::new("FOOT", Look::for_demand(0.0), 60.0, h as f32, 9);
        band.hold(0.0);
        let quiet = lit(&band);
        band.hold(0.0);
        assert_eq!(lit(&band), quiet, "holding again changes nothing");
        band.hold(1.0);
        assert!(band.is_still());
        let busy = lit(&band);
        let total = |pixels: &[u8]| pixels.iter().map(|&channel| channel as u64).sum::<u64>();
        assert!(
            total(&busy) > total(&quiet),
            "a busy app's still rain is brighter"
        );
        let moved = quiet
            .chunks_exact(4)
            .zip(busy.chunks_exact(4))
            .filter(|(quiet, busy)| quiet[..3] != [0, 0, 0] && busy[..3] == [0, 0, 0])
            .count();
        assert_eq!(moved, 0, "the same glyphs, in the same places");
        assert_eq!(
            band.colour, TINT,
            "the colour is there at once, not eased into"
        );
        band.step(1.0);
        assert!(!band.is_still(), "stepping lets it fall again");
    }

    #[test]
    fn a_warmed_band_draws_green_light_inside_its_slice() {
        let band = Band::new("FIREFOX", LOOK, 60.0, 600.0, 42);
        let mut glyphs = Glyphs::load().expect("the atlas loads");
        let (w, h) = (100, 600);
        let mut pixels = vec![0u8; w * h * 4];
        band.draw(&mut pixels, w, h, 50.0, &mut glyphs);
        let lit: Vec<(usize, &[u8])> = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[1] > 0)
            .collect();
        assert!(lit.len() > 500, "rain is falling: {} lit pixels", lit.len());
        assert!(
            lit.iter().all(|(i, _)| (20..80).contains(&(i % w))),
            "clipped to the slice"
        );
        let (red, green): (u64, u64) = lit
            .iter()
            .fold((0, 0), |(r, g), (_, p)| (r + p[0] as u64, g + p[1] as u64));
        assert!(green > red * 2, "green on black");
    }
}
