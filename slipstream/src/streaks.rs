//! Slipstream's own streams: streaks of light falling at different depths.
//!
//! A minimised app's card fills with falling light, more of it, faster and greener the harder the
//! app is working. Each streak falls at a depth of its own: near ones are wider, brighter and faster
//! with a soft glow at the head, far ones fine, dim and slow, and each leaves a faint trail. The
//! light is softened by a little bloom. The app's name runs down the card on its side, like a book's
//! spine, dim until light passes over it, when it glows and holds the glow for a moment.

use resvg::tiny_skia::Pixmap;

use crate::{
    glmatrix::{QUIET, colour_for, speed_for},
    text::{self, Face, Style},
};

/// The streams move in steps of this many seconds of the animation clock.
const TICK: f64 = 1.0 / 30.0;
/// At most this many steps at once, so a long stall can't hold up the compositor.
const MAX_TICKS: usize = 90;
/// Steps run before a card is first shown, so it looks as if light has been falling for a while.
const WARM_TICKS: usize = 90;
/// How long a change in load takes to settle into the light, in seconds.
const EASE: f64 = 2.0;
/// How long the name keeps glowing after light has passed over it, in seconds.
const GLOW: f64 = 0.45;
/// Streaks fall as fast as the code rain's heads do, on its own curve of load: about five of its
/// 15-pixel cells a second at its speed of 1, over the 0.88 an average streak's depth gives.
const SPEED: f32 = 84.0;
/// Streaks begun a second across a card: at rest, and the extra flat out.
const RATE_REST: f32 = 0.5;
const RATE_BUSY: f32 = 11.0;
/// A streak's length in logical pixels, from farthest to nearest, and the part that grows with
/// its speed, in seconds.
const LEN_FAR: f32 = 8.0;
const LEN_NEAR: f32 = 26.0;
const LEN_SPEED: f32 = 0.12;
/// A streak's width in logical pixels, farthest to nearest, and its head's glow radius.
const WIDTH_FAR: f32 = 1.4;
const WIDTH_NEAR: f32 = 4.6;
const HEAD_FAR: f32 = 2.2;
const HEAD_NEAR: f32 = 5.6;
/// The faint trail behind each streak: how many of its lengths long, and how bright.
const TRAIL: f32 = 3.0;
const TRAIL_LIGHT: f32 = 0.12;
/// Bloom: how far the light spreads, in logical pixels, and how much of it is added back.
const BLOOM: f32 = 4.0;
const BLOOM_LIGHT: f32 = 0.6;
/// The name: its size and letter spacing, where it starts below the top of the card in logical
/// pixels, and how bright it is at rest and fully lit.
const NAME_PX: f32 = 11.5;
const NAME_TRACKING: f32 = 0.09;
const NAME_TOP: f32 = 11.0;
const NAME_REST: f32 = 0.2;
const NAME_LIT: f32 = 1.0;

struct Streak {
    x: f32,
    /// Its head, in pixels from the top of the card.
    y: f32,
    /// How near it is, 0 (far) to 1 (near).
    near: f32,
    speed: f32,
    len: f32,
}

/// The name, turned on its side: its coverage, where it sits, and how lit each pixel of it is.
struct Spine {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    cover: Vec<u8>,
    glow: Vec<f32>,
}

pub struct Streaks {
    name: String,
    width: usize,
    height: usize,
    scale: f64,
    streaks: Vec<Streak>,
    spine: Option<Spine>,
    /// The load the light shows now, easing towards `target`.
    load: f32,
    target: f32,
    /// Part of the next streak, so rates below one a step still add up.
    spawn: f32,
    /// Time not yet stepped.
    pending: f64,
    rng: u64,
    name_rgb: [f32; 3],
    /// How bright the name is with no light on it.
    name_rest: f32,
    still: bool,
}

impl Streaks {
    /// A card `width`×`height` screen pixels at `scale` for an app called `name`, working at
    /// `load`, 0 to 1.
    pub fn new(name: &str, load: f32, width: usize, height: usize, scale: f64, seed: u64) -> Self {
        let mut streaks = Self {
            name: name.to_uppercase(),
            width: 0,
            height: 0,
            scale: 0.0,
            streaks: Vec::new(),
            spine: None,
            load: load.clamp(0.0, 1.0),
            target: load.clamp(0.0, 1.0),
            spawn: 0.0,
            pending: 0.0,
            rng: seed | 1,
            name_rgb: QUIET,
            name_rest: NAME_REST,
            still: false,
        };
        streaks.resize(width, height, scale);
        for _ in 0..WARM_TICKS {
            streaks.tick();
        }
        streaks
    }

    /// Fits a card of a new size or scale, keeping what's falling where it can.
    pub fn resize(&mut self, width: usize, height: usize, scale: f64) {
        if (width, height, scale) == (self.width, self.height, self.scale) {
            return;
        }
        if scale != self.scale {
            self.streaks.clear();
        }
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.spine = spine(&self.name, width, height, scale);
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

    /// Keeps the name this bright with no light on it, where the name is all that says whose
    /// stream it is.
    pub fn set_name_rest(&mut self, rest: f32) {
        self.name_rest = rest.clamp(0.0, NAME_LIT);
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
            // Most streaks are far away, a few near.
            let near = self.random().powf(1.4);
            let speed = SPEED * speed_for(load) * scale * (0.5 + 0.9 * near);
            let x = self.random() * self.width as f32;
            self.streaks.push(Streak {
                x,
                y: -2.0 * scale,
                near,
                speed,
                len: (LEN_FAR + (LEN_NEAR - LEN_FAR) * near) * scale + speed * LEN_SPEED,
            });
        }
        let fade = (-TICK / GLOW).exp() as f32;
        if let Some(spine) = &mut self.spine {
            for glow in &mut spine.glow {
                *glow *= fade;
            }
        }
        for streak in &mut self.streaks {
            let from = streak.y;
            streak.y += streak.speed * dt;
            // The name holds the light where the head passed over it this step.
            if let Some(spine) = &mut self.spine {
                let reach = head_radius(streak.near, scale) + width(streak.near, scale) / 2.0;
                light_spine(spine, streak.x, reach, from, streak.y, bright(streak.near));
            }
        }
        let height = self.height as f32;
        self.streaks
            .retain(|streak| streak.y - streak.len * (1.0 + TRAIL) < height);
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
        let hot = colour.map(|c| c + (1.0 - c) * 0.55);
        let scale = self.scale as f32;
        let mut sum = vec![0f32; w * h * 3];
        for streak in &self.streaks {
            let b = bright(streak.near);
            let half = width(streak.near, scale) / 2.0;
            // The faint trail, then the streak itself, brightening towards its head.
            let trail_top = streak.y - streak.len * (1.0 + TRAIL);
            let tail = streak.y - streak.len;
            let first = trail_top.max(0.0).floor() as usize;
            let last = (streak.y.min(h as f32 - 1.0)).max(0.0) as usize;
            if streak.y >= 0.0 {
                for y in first..=last.min(h.saturating_sub(1)) {
                    let yf = y as f32 + 0.5;
                    let amount = if yf >= tail {
                        ((yf - tail) / streak.len).clamp(0.0, 1.0).powf(1.6) * 0.9
                    } else {
                        ((yf - trail_top) / (tail - trail_top)).clamp(0.0, 1.0) * TRAIL_LIGHT
                    } * b;
                    column(&mut sum, w, h, y, streak.x, half, colour, amount);
                }
            }
            // The head's glow.
            let r = head_radius(streak.near, scale);
            let (cx, cy) = (streak.x, streak.y);
            let (x0, x1) = ((cx - r).floor().max(0.0) as usize, (cx + r).ceil() as usize);
            let (y0, y1) = ((cy - r).floor().max(0.0) as usize, (cy + r).ceil() as usize);
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                    let fall = (1.0 - d / r).max(0.0);
                    add(&mut sum, w, x, y, hot, fall * fall * b);
                }
            }
        }
        bloom(&mut sum, w, h, (BLOOM * scale).round().max(1.0) as usize);
        self.draw_spine(&mut sum, w, h);
        for (pixel, value) in light.chunks_exact_mut(4).zip(sum.chunks_exact(3)) {
            for (channel, v) in pixel[..3].iter_mut().zip(value) {
                // A half and a saturating cast round to nearest without calling `roundf`.
                *channel = (*channel as f32 + v * 255.0 + 0.5) as u8;
            }
        }
    }

    /// The name: dim in the ring's colour, and lit towards white where light has passed.
    fn draw_spine(&self, sum: &mut [f32], w: usize, h: usize) {
        let Some(spine) = &self.spine else {
            return;
        };
        let steady = if self.still {
            0.25 + 0.5 * self.load
        } else {
            0.0
        };
        let lit = self.name_rgb.map(|c| c + (1.0 - c) * 0.25);
        for iy in 0..spine.h {
            for ix in 0..spine.w {
                let at = iy * spine.w + ix;
                let cover = spine.cover[at] as f32 / 255.0;
                if cover == 0.0 {
                    continue;
                }
                let glow = spine.glow[at].max(steady).min(1.0);
                let rgb = [0, 1, 2].map(|c| self.name_rgb[c] + (lit[c] - self.name_rgb[c]) * glow);
                let amount = (self.name_rest + (NAME_LIT - self.name_rest) * glow) * cover;
                let (x, y) = (spine.x + ix, spine.y + iy);
                if x < w && y < h {
                    add(sum, w, x, y, rgb, amount);
                }
            }
        }
    }
}

/// How bright a streak at depth `near` is.
fn bright(near: f32) -> f32 {
    0.3 + 0.7 * near
}

fn width(near: f32, scale: f32) -> f32 {
    (WIDTH_FAR + (WIDTH_NEAR - WIDTH_FAR) * near) * scale
}

fn head_radius(near: f32, scale: f32) -> f32 {
    (HEAD_FAR + (HEAD_NEAR - HEAD_FAR) * near) * scale
}

/// Adds `rgb` at `amount` to pixel (`x`, `y`) of the float buffer.
fn add(sum: &mut [f32], w: usize, x: usize, y: usize, rgb: [f32; 3], amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let at = (y * w + x) * 3;
    for (channel, c) in sum[at..at + 3].iter_mut().zip(rgb) {
        *channel += c * amount;
    }
}

/// One row of a streak `half` pixels either side of `x`, each pixel covered as far as the streak
/// covers it, so fine streaks between pixels stay smooth.
#[allow(clippy::too_many_arguments)]
fn column(
    sum: &mut [f32],
    w: usize,
    h: usize,
    y: usize,
    x: f32,
    half: f32,
    rgb: [f32; 3],
    amount: f32,
) {
    if y >= h {
        return;
    }
    let (left, right) = (x - half, x + half);
    let first = left.floor().max(0.0) as usize;
    let last = (right.ceil() as usize).min(w);
    for px in first..last {
        let cover = (right.min(px as f32 + 1.0) - left.max(px as f32)).clamp(0.0, 1.0);
        add(sum, w, px, y, rgb, amount * cover);
    }
}

/// Softens the light: a box blur `radius` pixels each way, across then down, added back on top.
/// Both passes keep a running total, so a pixel costs the same whatever the radius, and the pass
/// down walks the rows in memory order.
fn bloom(sum: &mut [f32], w: usize, h: usize, radius: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let mut across = vec![0f32; sum.len()];
    let span = (2 * radius + 1) as f32;
    for y in 0..h {
        let row = y * w * 3;
        let mut total = [0f32; 3];
        for x in 0..(radius + 1).min(w) {
            for c in 0..3 {
                total[c] += sum[row + x * 3 + c];
            }
        }
        for x in 0..w {
            for c in 0..3 {
                across[row + x * 3 + c] = total[c] / span;
            }
            if x + radius + 1 < w {
                for c in 0..3 {
                    total[c] += sum[row + (x + radius + 1) * 3 + c];
                }
            }
            if x >= radius {
                for c in 0..3 {
                    total[c] -= sum[row + (x - radius) * 3 + c];
                }
            }
        }
    }
    let stride = w * 3;
    let mut totals = vec![0f32; stride];
    for y in 0..(radius + 1).min(h) {
        for (total, value) in totals.iter_mut().zip(&across[y * stride..(y + 1) * stride]) {
            *total += value;
        }
    }
    for y in 0..h {
        for (out, total) in sum[y * stride..(y + 1) * stride].iter_mut().zip(&totals) {
            *out += total / span * BLOOM_LIGHT;
        }
        if y + radius + 1 < h {
            let add = &across[(y + radius + 1) * stride..(y + radius + 2) * stride];
            for (total, value) in totals.iter_mut().zip(add) {
                *total += value;
            }
        }
        if y >= radius {
            let gone = &across[(y - radius) * stride..(y - radius + 1) * stride];
            for (total, value) in totals.iter_mut().zip(gone) {
                *total -= value;
            }
        }
    }
}

/// Lights the name wherever a head `reach` pixels round, at `x`, passed from `from` to `to`.
fn light_spine(spine: &mut Spine, x: f32, reach: f32, from: f32, to: f32, bright: f32) {
    let (left, right) = (x - reach, x + reach);
    let (top, bottom) = (from - reach, to + reach);
    if right < spine.x as f32
        || left > (spine.x + spine.w) as f32
        || bottom < spine.y as f32
        || top > (spine.y + spine.h) as f32
    {
        return;
    }
    let ix0 = (left - spine.x as f32).floor().max(0.0) as usize;
    let ix1 = ((right - spine.x as f32).ceil().max(0.0) as usize).min(spine.w);
    let iy0 = (top - spine.y as f32).floor().max(0.0) as usize;
    let iy1 = ((bottom - spine.y as f32).ceil().max(0.0) as usize).min(spine.h);
    for iy in iy0..iy1 {
        for ix in ix0..ix1 {
            let dx = (spine.x + ix) as f32 + 0.5 - x;
            let near = (1.0 - dx.abs() / reach).max(0.0);
            let glow = &mut spine.glow[iy * spine.w + ix];
            *glow = glow.max(near * (0.5 + 0.5 * bright));
        }
    }
}

/// `name` in the body face, bold, turned a quarter clockwise to read down a card `width`×`height`
/// screen pixels at `scale`, centred across it and starting `NAME_TOP` below its top. Cut off at
/// the card's foot if it's too long. None if nothing of it can be drawn.
fn spine(name: &str, width: usize, height: usize, scale: f64) -> Option<Spine> {
    let name = name.trim();
    if name.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let mut style = Style::new(Face::BodyBold, NAME_PX * scale as f32, 0xffffffff);
    style.tracking = NAME_TRACKING;
    let (ascent, descent) = text::line_metrics(Face::BodyBold, style.px);
    // Laid out along a line first, then turned.
    let along = (text::width(name, &style).ceil() as usize + 2).max(1);
    let across = ((ascent - descent).ceil() as usize + 2).max(1);
    let mut line = Pixmap::new(along as u32, across as u32)?;
    text::draw(&mut line, name, 1.0, 1.0 + ascent, &style);
    let top = (NAME_TOP * scale as f32).round() as usize;
    let h = along.min(height.saturating_sub(top));
    let w = across.min(width);
    if h == 0 {
        return None;
    }
    // A quarter clockwise: the line's left end at the top, its top edge towards the right.
    let data = line.data();
    let mut cover = vec![0u8; w * h];
    for (ty, row) in cover.chunks_exact_mut(w).enumerate() {
        for (tx, value) in row.iter_mut().enumerate() {
            let (lx, ly) = (ty, across - 1 - tx);
            if ly < across && lx < along {
                *value = data[(ly * along + lx) * 4 + 3];
            }
        }
    }
    Some(Spine {
        x: width.saturating_sub(w) / 2,
        y: top,
        w,
        h,
        glow: vec![0.0; w * h],
        cover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blur written the plain way, every neighbour added for every pixel.
    fn bloom_plainly(sum: &mut [f32], w: usize, h: usize, radius: usize) {
        let mut across = vec![0f32; sum.len()];
        let span = (2 * radius + 1) as f32;
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let total: f32 = (x.saturating_sub(radius)..(x + radius + 1).min(w))
                        .map(|sx| sum[(y * w + sx) * 3 + c])
                        .sum();
                    across[(y * w + x) * 3 + c] = total / span;
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let total: f32 = (y.saturating_sub(radius)..(y + radius + 1).min(h))
                        .map(|sy| across[(sy * w + x) * 3 + c])
                        .sum();
                    sum[(y * w + x) * 3 + c] += total / span * BLOOM_LIGHT;
                }
            }
        }
    }

    #[test]
    fn the_running_bloom_matches_the_plain_one() {
        for (w, h, radius) in [(39, 300, 5), (7, 9, 4), (3, 2, 6), (1, 1, 1)] {
            let mut seed = 0x2545_f491_4f6c_dd1du64;
            let light: Vec<f32> = (0..w * h * 3)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 40) as f32 / (1u64 << 24) as f32
                })
                .collect();
            let (mut fast, mut plain) = (light.clone(), light);
            bloom(&mut fast, w, h, radius);
            bloom_plainly(&mut plain, w, h, radius);
            for (a, b) in fast.iter().zip(&plain) {
                assert!((a - b).abs() < 1e-4, "{w}x{h} r{radius}: {a} against {b}");
            }
        }
    }

    const W: usize = 40;
    const H: usize = 700;

    fn lit(streaks: &Streaks) -> Vec<u8> {
        let mut light = vec![0u8; W * H * 4];
        streaks.draw(&mut light, W, H);
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
        let idle = Streaks::new("Foot", 0.0, W, H, 1.25, 7);
        let busy = Streaks::new("Foot", 1.0, W, H, 1.25, 7);
        assert!(busy.streaks.len() > 3 * idle.streaks.len().max(1));
        assert!(brightness(&lit(&busy)) > 2 * brightness(&lit(&idle)));
    }

    #[test]
    fn near_streaks_are_wider_brighter_and_faster() {
        assert!(width(1.0, 1.25) > 2.0 * width(0.0, 1.25));
        assert!(bright(1.0) > 2.0 * bright(0.0));
        let mut streaks = Streaks::new("Foot", 1.0, W, H, 1.25, 11);
        streaks.step(1.0);
        let (far, near): (Vec<_>, Vec<_>) = streaks.streaks.iter().partition(|s| s.near < 0.5);
        let mean = |v: &[&Streak]| v.iter().map(|s| s.speed).sum::<f32>() / v.len().max(1) as f32;
        assert!(mean(&near) > mean(&far));
    }

    #[test]
    fn the_name_runs_down_the_middle_on_its_side_and_catches_the_light() {
        let mut streaks = Streaks::new("Konsole", 1.0, W, H, 1.25, 3);
        let spine = streaks.spine.as_ref().expect("the name is drawn");
        assert!(
            spine.h > 2 * spine.w,
            "turned on its side: {}×{}",
            spine.w,
            spine.h
        );
        let (left, right) = (spine.x, W - spine.x - spine.w);
        assert!(left.abs_diff(right) <= 1, "centred: {left} and {right}");
        assert!(spine.cover.iter().any(|&c| c > 200), "has ink");
        streaks.step(2.0);
        let spine = streaks.spine.as_ref().unwrap();
        assert!(
            spine.glow.iter().any(|&g| g > 0.3),
            "lit where light passed"
        );
    }

    #[test]
    fn a_long_name_stops_at_the_foot_of_the_card() {
        let streaks = Streaks::new("An Application With A Very Long Name", 0.5, W, 120, 1.25, 1);
        let spine = streaks.spine.as_ref().expect("the name is drawn");
        assert!(spine.y + spine.h <= 120);
    }

    #[test]
    fn reduced_motion_holds_the_light_still() {
        let mut streaks = Streaks::new("Foot", 0.5, W, H, 1.25, 5);
        streaks.hold(0.5);
        let before = lit(&streaks);
        assert!(streaks.is_still());
        assert_eq!(before, lit(&streaks));
    }

    #[test]
    fn light_stays_inside_the_buffer_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let w = (31.0 * scale) as usize;
            let mut streaks = Streaks::new("Foot", 1.0, w, 300, scale, 9);
            streaks.step(3.0);
            let mut light = vec![0u8; w * 300 * 4];
            streaks.draw(&mut light, w, 300);
        }
    }
}
