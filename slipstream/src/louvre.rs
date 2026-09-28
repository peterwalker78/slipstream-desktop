//! Slipstream's own streams: light falling through slits.
//!
//! Streaks of light run down narrow lanes on a minimised app's card, more of them, longer, faster
//! and greener the harder the app is working. The card is cut across by slits in the logo's rhythm,
//! three parts bar to one part slit, so each streak reads as light through a louvre. The app's name
//! is written down the middle, a letter under a letter, and lights up wherever a streak passes over
//! it, so a busy app's name glows and an idle one's is barely there.
//!
//! The lanes and slits are laid out on one unit, a lane's width, so they line up with each other and
//! with the screen's pixels.

use crate::{
    glmatrix::{QUIET, colour_for},
    text::{self, Face},
};

/// The unit in logical pixels: a lane's width and a slit's height.
const UNIT: f64 = 5.6;
/// A bar between two slits is this many units tall, and a slit one.
const BAR_UNITS: usize = 3;
/// How much of the light behind a slit still shows.
const SLIT_LIGHT: f32 = 0.2;
/// The streams move in steps of this many seconds of the animation clock.
const TICK: f64 = 1.0 / 30.0;
/// At most this many steps at once, so a long stall can't hold up the compositor.
const MAX_TICKS: usize = 90;
/// Steps run before a card is first shown, so it looks as if light has been falling for a while.
const WARM_TICKS: usize = 90;
/// How long a change in load takes to settle into the light, in seconds.
const EASE: f64 = 2.0;
/// How long a letter keeps glowing after a streak has passed over it, in seconds.
const GLOW: f64 = 0.55;
/// Streaks' speed in logical pixels a second: at rest, and the extra when flat out.
const SPEED_REST: f32 = 33.0;
const SPEED_BUSY: f32 = 360.0;
/// Streaks begun a second across a card: at rest, and the extra when flat out.
const RATE_REST: f32 = 0.8;
const RATE_BUSY: f32 = 22.0;
/// A streak's tail, in logical pixels, beyond the part that grows with its speed.
const TAIL: f32 = 4.8;
/// A streak's width, in logical pixels, and how many screen rows its bright head takes.
const STREAK_W: f64 = 4.0;
const HEAD_ROWS: usize = 3;
/// The name's letters: their size, how far apart they sit down the card, and where the first
/// starts, in logical pixels.
const NAME_PX: f64 = 22.0;
const NAME_PITCH: f64 = 22.0;
const NAME_TOP: f64 = 10.0;
/// A letter when nothing is lighting it, and at full glow.
const LETTER_DARK: f32 = 0.16;
const LETTER_LIT: f32 = 0.9;

/// The unit in screen pixels at `scale`: whole pixels, and never less than two.
pub fn unit(scale: f64) -> usize {
    ((UNIT * scale).round() as usize).max(2)
}

struct Streak {
    lane: usize,
    /// Its head, in pixels from the top of the card.
    y: f32,
    speed: f32,
    len: f32,
    bright: f32,
}

/// One letter of the name: its coverage, where it sits on the card, and how brightly each lane
/// crossing it is lit.
struct Letter {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    cover: Vec<u8>,
    glow: Vec<f32>,
}

pub struct Louvre {
    name: String,
    width: usize,
    height: usize,
    scale: f64,
    unit: usize,
    lanes: usize,
    /// Pixels left over at the card's left edge, half of what the lanes don't fill.
    offset: usize,
    streaks: Vec<Streak>,
    letters: Vec<Letter>,
    /// The load the light shows now, easing towards `target`.
    load: f32,
    target: f32,
    /// Part of the next streak, so rates below one a step still add up.
    spawn: f32,
    /// Time not yet stepped.
    pending: f64,
    rng: u64,
    name_rgb: [f32; 3],
    still: bool,
}

impl Louvre {
    /// A card `width`×`height` screen pixels at `scale` for an app called `name`, working at
    /// `load`, 0 to 1.
    pub fn new(name: &str, load: f32, width: usize, height: usize, scale: f64, seed: u64) -> Self {
        let mut louvre = Self {
            name: name.to_string(),
            width: 0,
            height: 0,
            scale: 0.0,
            unit: 1,
            lanes: 1,
            offset: 0,
            streaks: Vec::new(),
            letters: Vec::new(),
            load: load.clamp(0.0, 1.0),
            target: load.clamp(0.0, 1.0),
            spawn: 0.0,
            pending: 0.0,
            rng: seed | 1,
            name_rgb: QUIET,
            still: false,
        };
        louvre.resize(width, height, scale);
        for _ in 0..WARM_TICKS {
            louvre.tick();
        }
        louvre
    }

    /// Fits a card of a new size or scale, keeping what's falling where it can.
    pub fn resize(&mut self, width: usize, height: usize, scale: f64) {
        if (width, height, scale) == (self.width, self.height, self.scale) {
            return;
        }
        // Never so coarse that fewer than five lanes fit across the card.
        let unit = unit(scale).min(width / 5).max(2);
        let lanes = (width / unit).max(1);
        if unit != self.unit || lanes != self.lanes {
            self.streaks.clear();
        }
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.unit = unit;
        self.lanes = lanes;
        self.offset = width.saturating_sub(lanes * unit) / 2;
        self.letters = letters(&self.name, width, scale);
        for letter in &mut self.letters {
            letter.glow = vec![0.0; lanes];
        }
    }

    /// Eases towards `load` from here on.
    pub fn set_load(&mut self, load: f32) {
        self.target = load.clamp(0.0, 1.0);
    }

    /// Lights the name in `rgb`, returning whether that's a change.
    pub fn set_name_colour(&mut self, rgb: [f32; 3]) -> bool {
        let changed = self.name_rgb != rgb;
        self.name_rgb = rgb;
        changed
    }

    /// Holds the light still at `load`, for reduced motion: nothing falls, and the name glows
    /// steadily, more the busier the app.
    pub fn hold(&mut self, load: f32) {
        self.target = load.clamp(0.0, 1.0);
        self.load = self.target;
        self.still = true;
    }

    pub fn is_still(&self) -> bool {
        self.still
    }

    /// Moves on by `seconds` of the animation clock, returning whether anything moved.
    pub fn step(&mut self, seconds: f64) -> bool {
        self.still = false;
        self.pending += seconds.max(0.0);
        let due = (self.pending / TICK).floor() as usize;
        let ticks = due.min(MAX_TICKS);
        self.pending = if due > MAX_TICKS {
            0.0
        } else {
            self.pending - ticks as f64 * TICK
        };
        for _ in 0..ticks {
            self.tick();
        }
        ticks > 0
    }

    fn tick(&mut self) {
        let dt = TICK as f32;
        self.load += (self.target - self.load) * (1.0 - (-TICK / EASE).exp()) as f32;
        let load = self.load.clamp(0.0, 1.0);
        let scale = self.scale as f32;
        self.spawn += (RATE_REST + RATE_BUSY * load) * dt;
        while self.spawn >= 1.0 {
            self.spawn -= 1.0;
            let speed = (SPEED_REST + SPEED_BUSY * load) * scale * (0.7 + 0.6 * self.random());
            let lane = ((self.random() * self.lanes as f32) as usize).min(self.lanes - 1);
            let bright = 0.5 + 0.5 * self.random();
            self.streaks.push(Streak {
                lane,
                y: -2.0 * scale,
                speed,
                len: TAIL * scale + 0.2 * speed,
                bright,
            });
        }
        let fade = (-TICK / GLOW).exp() as f32;
        for letter in &mut self.letters {
            for glow in &mut letter.glow {
                *glow *= fade;
            }
        }
        let (unit, offset) = (self.unit, self.offset);
        for streak in &mut self.streaks {
            let from = streak.y;
            streak.y += streak.speed * dt;
            // Whatever the head passed over this step lights up, in the streak's own lane.
            let lane_x = offset + streak.lane * unit;
            for letter in &mut self.letters {
                let across = lane_x < letter.x + letter.w && letter.x < lane_x + unit;
                let (top, bottom) = (letter.y as f32, (letter.y + letter.h) as f32);
                if across && top <= streak.y && bottom >= from {
                    letter.glow[streak.lane] = 1.0;
                }
            }
        }
        let height = self.height as f32;
        self.streaks.retain(|streak| streak.y - streak.len < height);
    }

    /// From 0 to 1: xorshift64.
    fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Adds the light to `light`, `w`×`h` pixels of RGBA; only the colour channels are written.
    pub fn draw(&self, light: &mut [u8], w: usize, h: usize) {
        let colour = colour_for(self.load);
        let hot = colour.map(|c| c + (1.0 - c) * 0.5);
        let core = ((STREAK_W * self.scale).round() as usize).max(1);
        for streak in &self.streaks {
            let lane_x = self.offset + streak.lane * self.unit;
            let x = lane_x + (self.unit.saturating_sub(core)) / 2;
            let tail = streak.y - streak.len;
            let first = tail.max(0.0).floor() as usize;
            let last = streak.y.min(h as f32 - 1.0);
            if streak.y < 0.0 || last < 0.0 {
                continue;
            }
            for y in first..=last as usize {
                let along = ((y as f32 - tail) / streak.len).clamp(0.0, 1.0);
                let amount = along * 0.85 * streak.bright;
                for cx in x..x + core {
                    add(light, w, h, cx, y, colour, amount);
                }
                // A soft edge either side, inside the lane.
                if x > lane_x {
                    add(light, w, h, x - 1, y, colour, amount * 0.25);
                }
                if x + core < lane_x + self.unit {
                    add(light, w, h, x + core, y, colour, amount * 0.25);
                }
            }
            let head = streak.y.round() as usize;
            for y in head.saturating_sub(HEAD_ROWS - 1)..=head.min(h.saturating_sub(1)) {
                for cx in x..x + core {
                    add(light, w, h, cx, y, hot, streak.bright);
                }
            }
        }
        // The slits: most of the light behind them is cut off.
        let period = (BAR_UNITS + 1) * self.unit;
        for y in 0..h {
            if y % period < BAR_UNITS * self.unit {
                continue;
            }
            for pixel in light[y * w * 4..(y + 1) * w * 4].chunks_exact_mut(4) {
                for channel in &mut pixel[..3] {
                    *channel = (*channel as f32 * SLIT_LIGHT).round() as u8;
                }
            }
        }
        // The name over the slits, so none of its letters are cut, each part lit as brightly as
        // the lane it's in.
        let steady = if self.still {
            0.25 + 0.5 * self.load
        } else {
            0.0
        };
        for letter in &self.letters {
            for iy in 0..letter.h {
                for ix in 0..letter.w {
                    let cover = letter.cover[iy * letter.w + ix];
                    if cover == 0 {
                        continue;
                    }
                    let x = letter.x + ix;
                    let lane = (x.saturating_sub(self.offset) / self.unit).min(self.lanes - 1);
                    let glow = letter.glow[lane].max(steady);
                    let share = (glow * 2.0).min(1.0);
                    let rgb = [0, 1, 2].map(|c| QUIET[c] + (self.name_rgb[c] - QUIET[c]) * share);
                    let amount =
                        (LETTER_DARK + (LETTER_LIT - LETTER_DARK) * glow) * cover as f32 / 255.0;
                    add(light, w, h, x, letter.y + iy, rgb, amount);
                }
            }
        }
    }
}

/// Adds `rgb` at `amount` to the pixel at (`x`, `y`), if it's in the buffer.
fn add(light: &mut [u8], w: usize, h: usize, x: usize, y: usize, rgb: [f32; 3], amount: f32) {
    if x >= w || y >= h || amount <= 0.0 {
        return;
    }
    let at = (y * w + x) * 4;
    for (channel, c) in light[at..at + 3].iter_mut().zip(rgb) {
        let added = *channel as f32 + c * amount * 255.0;
        *channel = added.round().min(255.0) as u8;
    }
}

/// `name`'s letters in the name face, each centred across a card `width` screen pixels wide at
/// `scale` and set one under another down it. A space leaves a letter's room; a character the face
/// can't draw is left out.
fn letters(name: &str, width: usize, scale: f64) -> Vec<Letter> {
    let px = (NAME_PX * scale) as f32;
    let pitch = NAME_PITCH * scale;
    let (ascent, _) = text::line_metrics(Face::MonoBold, px);
    let mut letters = Vec::new();
    let mut place = 0;
    for c in name.chars() {
        if c == ' ' {
            place += 1;
            continue;
        }
        let (metrics, cover) = text::glyph(Face::MonoBold, c, px);
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let baseline = NAME_TOP * scale + place as f64 * pitch + ascent as f64;
        let top = baseline - (metrics.height as i32 + metrics.ymin) as f64;
        letters.push(Letter {
            x: width.saturating_sub(metrics.width) / 2,
            y: top.max(0.0).round() as usize,
            w: metrics.width,
            h: metrics.height,
            cover,
            glow: Vec::new(),
        });
        place += 1;
    }
    letters
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 40;
    const H: usize = 700;

    fn lit(louvre: &Louvre) -> Vec<u8> {
        let mut light = vec![0u8; W * H * 4];
        louvre.draw(&mut light, W, H);
        light
    }

    fn brightness(light: &[u8]) -> u64 {
        light
            .chunks_exact(4)
            .map(|p| p[..3].iter().map(|&c| c as u64).sum::<u64>())
            .sum()
    }

    #[test]
    fn a_busy_app_sends_more_light_than_an_idle_one() {
        let idle = Louvre::new("FooT", 0.0, W, H, 1.25, 7);
        let busy = Louvre::new("FooT", 1.0, W, H, 1.25, 7);
        assert!(busy.streaks.len() > 3 * idle.streaks.len().max(1));
        assert!(brightness(&lit(&busy)) > 3 * brightness(&lit(&idle)));
    }

    #[test]
    fn the_slits_cut_the_light() {
        let mut louvre = Louvre::new("", 1.0, W, H, 1.25, 11);
        louvre.step(1.0);
        let light = lit(&louvre);
        let unit = louvre.unit;
        let period = 4 * unit;
        let row = |y: usize| brightness(&light[y * W * 4..(y + 1) * W * 4]);
        let (mut bars, mut slits) = (0, 0);
        for y in 0..H {
            if y % period < 3 * unit {
                bars += row(y);
            } else {
                slits += row(y);
            }
        }
        // Three times as many bar rows as slit rows, and a fifth of the light through each.
        assert!(bars > 8 * slits, "bars {bars}, slits {slits}");
    }

    #[test]
    fn the_name_sits_down_the_middle_and_lights_where_light_passes() {
        let mut louvre = Louvre::new("KoNSoLe", 1.0, W, H, 1.25, 3);
        assert_eq!(louvre.letters.len(), 7);
        for letter in &louvre.letters {
            let (left, right) = (letter.x, W - letter.x - letter.w);
            assert!(left.abs_diff(right) <= 1, "centred: {left} and {right}");
        }
        let tops: Vec<usize> = louvre.letters.iter().map(|letter| letter.y).collect();
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "one under another"
        );
        louvre.step(2.0);
        assert!(
            louvre
                .letters
                .iter()
                .any(|letter| letter.glow.iter().any(|&g| g > 0.5))
        );
    }

    #[test]
    fn reduced_motion_holds_the_light_still() {
        let mut louvre = Louvre::new("Foot", 0.5, W, H, 1.25, 5);
        louvre.hold(0.5);
        let before = lit(&louvre);
        assert!(louvre.is_still());
        assert_eq!(before, lit(&louvre));
        assert!(louvre.letters.iter().all(|letter| letter.y + letter.h < H));
    }

    #[test]
    fn units_are_whole_pixels_at_every_scale() {
        // A card's inner width at each scale, and the unit it gets.
        for (scale, width, unit) in [(1.0, 31, 6), (1.25, 40, 7), (1.5, 47, 8), (2.0, 62, 11)] {
            let louvre = Louvre::new("A", 0.0, width, H, scale, 1);
            assert_eq!(louvre.unit, unit, "at {scale}×");
            assert_eq!(louvre.lanes, 5, "a letter's width across, at {scale}×");
            assert!(louvre.offset + louvre.lanes * louvre.unit <= width);
        }
    }

    #[test]
    fn a_space_leaves_a_letter_s_room() {
        let spaced = letters("A B", W, 1.25);
        let joined = letters("AB", W, 1.25);
        assert_eq!(spaced.len(), 2);
        let pitch = (NAME_PITCH * 1.25).round() as usize;
        assert!((spaced[1].y - joined[1].y).abs_diff(pitch) <= 1);
    }
}
