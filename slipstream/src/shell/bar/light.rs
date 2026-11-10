//! The lit edge: the line along the bar's foot, carrying a charge in the focus ring's colour. It
//! glows faintly all along, brighter around the workspace marker, and the glow follows the
//! marker as it slides. Every few seconds a calm pulse leaves the marker and runs out both ways
//! to the bar's ends, fading gently on the way. Now and then a faster gust leaves the marker
//! towards the side with more room, and the edge sheds a wake behind it: small curls of light,
//! above and below the line in turn, that drift back against the gust, turn slowly and thin into
//! smoke. While a panel comes out of the bar, the line brightens over the slot it comes out of.
//!
//! One shader strip draws all of it, worked out per pixel from when each pulse and the gust set
//! off, so the processor does nothing per curl and nothing in it steps or comes in blocks. The
//! bar's own picture leaves its foot line out in this style, so the line has one owner.
//!
//! Only the screen with the keyboard moves; the others show the still edge. The pulses move in
//! steps of a thirtieth of a second and a gust's head at the display's own rate. With neither
//! out the strip's numbers stay as they are and the screen isn't redrawn for it, but a pulse
//! leaves every few seconds and takes longer than that to run out, so while the desktop is shown
//! there is nearly always one out, and the screen with the keyboard is woken about 30 times a
//! second for the edge (more while a gust's head runs).
//!
//! On battery the edge holds still everywhere, the line and the marker's glow alone, so it never
//! wakes the screen: what only moves for looks waits for the charger.

use smithay::{
    backend::renderer::gles::{GlesPixelProgram, GlesRenderer},
    utils::{Logical, Physical, Rectangle},
};

use super::{HEIGHT, TALL};
use crate::{
    effects::{
        lit_line,
        shader::{self, Values},
    },
    flow::BarSlot,
};

/// How often the pulses move, a second, while no gust is out.
pub const STEPS: f64 = 30.0;

/// A span numbers are picked from, low to high.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Range {
    low: f64,
    high: f64,
}

impl Range {
    const fn new(low: f64, high: f64) -> Self {
        Self { low, high }
    }

    /// `share` of the way from low to high.
    fn at(self, share: f64) -> f64 {
        self.low + (self.high - self.low) * share
    }

    #[cfg(test)]
    fn contains(self, value: f64) -> bool {
        (self.low..=self.high).contains(&value)
    }
}

/// Seconds between one pulse leaving the marker and the next, picked afresh each time.
const PULSE_EVERY: Range = Range::new(4.5, 6.0);
/// How fast a pulse runs out along the edge, in the bar's units a second, picked per pulse.
const PULSE_SPEED: Range = Range::new(110.0, 135.0);
/// How far past the bar's far end a pulse runs before it is let go, so it leaves the bar whole.
const PULSE_TAIL: f64 = 200.0;
/// The furthest a pulse runs, however wide the bar: it fades out over the last stretch of it.
/// The slowest takes under 27 seconds to get there, so `PULSES` is enough to have every one out
/// at once.
const PULSE_LONGEST: f64 = 2950.0;
/// The most pulses out at once: they leave at least 4.5 s apart and none lasts 27 s.
const PULSES: usize = 8;

/// Seconds between one gust and the next.
const GUST_EVERY: Range = Range::new(6.0, 10.0);
/// How fast a gust runs, in the bar's units a second, and how far before it has faded out.
const GUST_SPEED: Range = Range::new(380.0, 500.0);
const GUST_RUN: Range = Range::new(700.0, 1200.0);
/// How far from the marker's middle a gust sets off.
const GUST_FROM: f64 = 20.0;
/// The wake: a curl every `CURL_EVERY` units of the gust's run, shed once the gust is
/// `CURL_BEHIND` past it, each lasting a time within `CURL_LIFE`.
const CURL_EVERY: f64 = 52.0;
const CURL_BEHIND: f64 = 30.0;
const CURL_LIFE: Range = Range::new(1.4, 1.9);

/// How far the strip reaches above and below the line, in logical pixels: room for the glow
/// either side and for the curls, which fade out before they reach either edge.
const ABOVE: i32 = 16;
const BELOW: i32 = 17;

/// The strip's shader, its numbers in the bar's units unless they say otherwise. The colours are
/// light added to what's under the strip, apart from the line's own colour, which is laid over.
/// The numbers it shares with the timing here are put in front of it when it's compiled.
const SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Screen pixels to one of the bar's units, the bar's height in screen pixels, and where the
// strip's top-left corner is from the bar's, in screen pixels.
uniform vec4 frame;
// The strip's size in screen pixels.
uniform vec2 extent;
uniform vec3 ring;
// The line's own colour, premultiplied.
uniform vec4 edge;
// The marker's middle, how strongly it glows, the strip's alpha, and how strongly the line is
// charged all along: none when the strip draws a panel's slot alone.
uniform vec4 look;
// How far each pulse has run from the marker; below zero, none.
uniform vec4 pulses;
uniform vec4 pulses_more;
// The gust: where it set off, its speed (signed the way it runs), how far it runs, and how long
// ago it set off; below zero, none.
uniform vec4 gust;
// The gust's seed, and the side its first curl is on: 1 below the line, -1 above.
uniform vec2 wake;
// The gap where a docked pane's frame carries the bar on down, and how far it is from the
// marker to the bar's further end.
uniform vec3 bar;
// The slot a panel is coming out of: its left and right ends, how strongly it is lit, and how far
// its ends taper, as a lit line's do.
uniform vec4 slot;

const float PI = 3.14159265;
// The charge all along the edge, and what the marker adds at its middle.
const float BASE = 0.14;
const float MARKER = 0.34;
const float SPREAD = 420.0;
// What a pulse adds at its middle when it leaves the marker, and its share of that at the bar's
// far end.
const float PULSE = 0.7;
const float FAR = 0.33;
// What a gust adds at its head, and how fast its tail fades behind it and its head ahead.
const float GUST = 0.75;
const float TAIL = 38.0;
const float HEAD = 8.0;
// Above KNEE the line brightens ever more slowly towards CEILING, so it never glares.
const float KNEE = 0.62;
const float CEILING = 0.88;
// What a lit slot adds to the line.
const float SLOT = 0.8;
// A curl: how far it drifts back against the gust over its life, how far it lies from the line
// at first and at last, its radius at least and at most, how much wider it is than tall, how far
// round its spiral goes, how fast it turns in radians a second, and the share of its life it
// spends whole before it breaks into smoke.
const float DRIFT = 30.0;
const float OFF_NEAR = 2.5;
const float OFF_FAR = 6.0;
const float RADIUS_LOW = 5.0;
const float RADIUS_HIGH = 8.0;
const float STRETCH = 1.7;
const float SPIRAL = 1.35 * PI;
const float SPIN = 3.2;
const float WHOLE = 0.45;
// The curl's spiral is followed in this many straight pieces.
const int PIECES = 12;

float sq(float v) {
    return v * v;
}

float hash(float n) {
    return fract(sin(n) * 43758.5453);
}

// What a pulse `run` units out adds at `d` units from the marker: its charge and its white core.
vec2 pulse(float run, float d) {
    if (run < 0.0) return vec2(0.0);
    float wide = 26.0 + 0.06 * min(run, 1400.0);
    float fade = mix(1.0, FAR, pow(clamp(run / max(bar.z, 1.0), 0.0, 1.0), 0.85))
        * smoothstep(0.0, 30.0, run) * (1.0 - smoothstep(LONGEST - 250.0, LONGEST, run));
    float g = exp(-sq((d - run) / wide));
    return vec2(PULSE * look.y * g * fade, pow(g, 8.0) * fade);
}

// What the gust adds on the line at `x`: its charge and its white core, brightest at its head
// with a short tail behind, fading out over its run.
vec2 gust_line(float x) {
    if (gust.w < 0.0) return vec2(0.0);
    float way = sign(gust.y);
    float run = gust.w * abs(gust.y);
    float fade = max(1.0 - run / gust.z, 0.0) * smoothstep(0.0, 30.0, run);
    float behind = way * (gust.x + way * run - x);
    float k = behind < 0.0 ? exp(-sq(behind / HEAD)) : exp(-behind / TAIL);
    return vec2(GUST * k * fade, k * k * fade);
}

// How strongly a stroke `w` screen pixels wide covers a pixel `d` from its middle: thinner than
// a pixel, it fades rather than breaking up.
float stroke(float d, float w) {
    float body = max(w, 1.0);
    return min(w, 1.0) * clamp(0.5 * body + 0.5 - d, 0.0, 1.0);
}

// The wake's light at `p` (units along the bar, and below the line's middle), `f` screen pixels
// to a unit: its coloured halo and its near-white core.
vec2 wake_at(vec2 p, float f) {
    vec2 light = vec2(0.0);
    if (gust.w < 0.0) return light;
    float way = sign(gust.y);
    float speed = abs(gust.y);
    // Curls drift back from where they are shed, so this pixel can only be near these three.
    float along = way * (p.x - gust.x);
    float first = floor((along - RADIUS_HIGH * 2.0 * STRETCH) / CURL_EVERY);
    for (int n = 0; n < 3; n++) {
        float k = first + float(n);
        float shed = k * CURL_EVERY + CURL_BEHIND;
        if (k < 1.0 || shed >= gust.z) continue;
        float age = gust.w - shed / speed;
        float life = mix(CURL_LIFE_LOW, CURL_LIFE_HIGH, hash(k * 1.31 + wake.x));
        float u = age / life;
        if (u <= 0.0 || u >= 1.0) continue;
        float side = mod(k, 2.0) < 0.5 ? wake.y : -wake.y;
        float spin = side * way;
        float r = mix(RADIUS_LOW, RADIUS_HIGH, hash(k * 2.17 + wake.x + 11.0)) * (0.8 + 0.9 * u);
        float stretch = STRETCH + 0.4 * u;
        vec2 c = vec2(gust.x + way * (k * CURL_EVERY - DRIFT * u),
                      side * mix(OFF_NEAR, OFF_FAR, u));
        if (abs(p.x - c.x) > r * stretch + 3.0 || abs(p.y - c.y) > 0.5 * r + 3.0) continue;
        float turn = 2.0 * PI * hash(k * 3.73 + wake.x + 23.0) + SPIN * age;
        // The nearest point of its spiral, piece by piece, and how far along the spiral it is.
        float near = 1e6;
        float at_len = 0.0;
        float len = 0.0;
        vec2 a = c + vec2(way * 0.45 * r * cos(turn * spin) * stretch,
                          0.45 * r * sin(turn * spin) * 0.5);
        for (int i = 1; i <= PIECES; i++) {
            float q = float(i) / float(PIECES);
            float th = spin * (q * SPIRAL + turn);
            float rho = r * (0.45 + 0.55 * q);
            vec2 b = c + vec2(way * rho * cos(th) * stretch, rho * sin(th) * 0.5);
            vec2 ab = b - a;
            float piece = length(ab);
            float t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-4), 0.0, 1.0);
            float d = length(p - a - ab * t);
            if (d < near) {
                near = d;
                at_len = len + t * piece;
            }
            len += piece;
            a = b;
        }
        float strength = (1.0 - shed / gust.z) * min(u / 0.08, 1.0) * pow(1.0 - u, 1.4);
        float thin = 1.0 - 0.45 * u;
        // Past its whole stretch it breaks into dots along the spiral, ever further apart.
        float smoke = smoothstep(WHOLE - 0.05, WHOLE + 0.1, u);
        float period = 3.8 + 4.0 * u;
        float from_dot = abs(fract(at_len / period) - 0.5) * period;
        float dots = clamp((0.6 - from_dot) * f + 0.5, 0.0, 1.0);
        // It has no ends to see: it grows out of nothing at its middle and thins away at its
        // outer end, so it reads as a wisp rather than a mark.
        float share = at_len / max(len, 1e-3);
        float ends = smoothstep(0.0, 0.35, share) * (1.0 - smoothstep(0.8, 1.0, share));
        thin *= mix(0.55, 1.0, share);
        float broken = mix(1.0, dots, smoke) * ends;
        float d = near * f;
        light.x += 0.2 * strength * broken * exp(-sq(d / max(1.5 * thin * f, 0.8)));
        light.y += 0.6 * strength * broken * stroke(d, 0.9 * thin * f);
    }
    return light;
}

void main() {
    float f = frame.x;
    // This pixel's middle, in screen pixels from the bar's top-left corner.
    vec2 at = v_coords * extent + frame.zw;
    float x = at.x / f;
    // The line fills the bar's last unit, the same rows its picture would.
    float line_top = frame.y - f;
    float cover = clamp(min(at.y + 0.5, frame.y) - max(at.y - 0.5, line_top), 0.0, 1.0);
    // Units from the line's middle, downwards.
    float below = (at.y - (frame.y - 0.5 * f)) / f;
    float in_gap = clamp(min(at.x + 0.5, bar.y * f) - max(at.x - 0.5, bar.x * f), 0.0, 1.0);
    float open = 1.0 - in_gap;

    float d = abs(x - look.x);
    float charge = look.w * (BASE + MARKER * look.y * exp(-sq(d / SPREAD)));
    vec2 runs = pulse(pulses.x, d) + pulse(pulses.y, d) + pulse(pulses.z, d)
        + pulse(pulses.w, d) + pulse(pulses_more.x, d) + pulse(pulses_more.y, d)
        + pulse(pulses_more.z, d) + pulse(pulses_more.w, d) + gust_line(x);
    charge += runs.x;
    float core = runs.y;
    if (charge > KNEE) {
        float room = CEILING - KNEE;
        charge = KNEE + room * (1.0 - exp(-(charge - KNEE) / room));
    }
    // A lit slot, over the knee so its glint can flare, its ends tapering into the line.
    float lit = 0.0;
    if (slot.z > 0.0) {
        lit = smoothstep(slot.x, slot.x + slot.w, x) * (1.0 - smoothstep(slot.y - slot.w, slot.y, x));
        charge += SLOT * slot.z * lit;
    }

    // The glow beside the line: a little above, into the bar, and further below.
    float glow = below > 0.0
        ? 0.55 * pow(clamp(1.0 - below / 12.0, 0.0, 1.0), 2.2)
        : 0.30 * pow(clamp(1.0 + below / 4.0, 0.0, 1.0), 2.0);
    vec3 light = ring * charge * (cover + (1.0 - cover) * glow);
    // A white-hot touch at a pulse's very core, on the line alone.
    light += vec3((0.25 * min(core, 1.0) + 0.4 * max(slot.z - 1.0, 0.0) * lit) * cover);

    // The wake, in the ring's colour round a near-white core. Nothing reaches the strip's top
    // or bottom, so it has no edge to show.
    vec2 curls = wake_at(vec2(x, below), f);
    float rim = smoothstep(0.0, 2.0 * f, at.y - frame.w)
        * smoothstep(0.0, 2.0 * f, extent.y - (at.y - frame.w));
    light += (ring * curls.x + mix(ring, vec3(1.0), 0.65) * curls.y) * rim;

    vec4 color = vec4(edge.rgb * cover + light, edge.a * cover) * open * look.z * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// The shader with the numbers it shares with the timing here.
fn shader() -> String {
    shader::with_constants(
        SHADER,
        &[
            ("LONGEST", &PULSE_LONGEST),
            ("CURL_EVERY", &CURL_EVERY),
            ("CURL_BEHIND", &CURL_BEHIND),
            ("CURL_LIFE_LOW", &CURL_LIFE.low),
            ("CURL_LIFE_HIGH", &CURL_LIFE.high),
        ],
    )
}

/// The strip's program for `renderer`, compiled once for it; `None` if it didn't compile, which
/// is said once.
fn program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    shader::program::<Program, _>(
        renderer,
        |renderer| renderer.compile_custom_pixel_shader(shader(), &shader::names::<Uniforms>()),
        "the lit edge's",
    )
}

/// Marks the lit edge's program in a renderer's keeping.
struct Program;

/// What the bar tells the edge each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Where the bar is, in logical pixels from the screen's corner, and the screen's scale.
    pub bar: Rectangle<f64, Logical>,
    pub scale: f64,
    /// The marker's middle as drawn, in the bar's units.
    pub marker: Option<f32>,
    /// How strongly the marker glows: less on a screen the keyboard isn't on.
    pub strength: f32,
    /// The focus ring's colour and the line's own, 0xRRGGBBAA.
    pub ring: u32,
    pub line: u32,
    /// Whether the line carries its charge, or the strip draws a panel's slot alone over a bar
    /// whose picture has its own foot line.
    pub charged: bool,
    /// Where a docked pane's frame carries the bar on down, in the bar's units.
    pub gap: Option<(f32, f32)>,
    /// The slot a panel is coming out of, its ends in logical pixels across the screen.
    pub slot: Option<BarSlot>,
    pub alpha: f32,
}

/// The shader's numbers, all plain values, so working them out and drawing them allocate
/// nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    frame: [f32; 4],
    extent: [f32; 2],
    ring: [f32; 3],
    edge: [f32; 4],
    look: [f32; 4],
    pulses: [f32; 4],
    pulses_more: [f32; 4],
    gust: [f32; 4],
    wake: [f32; 2],
    bar: [f32; 3],
    slot: [f32; 4],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("extent", self.extent)
            .with("ring", self.ring)
            .with("edge", self.edge)
            .with("look", self.look)
            .with("pulses", self.pulses)
            .with("pulses_more", self.pulses_more)
            .with("gust", self.gust)
            .with("wake", self.wake)
            .with("bar", self.bar)
            .with("slot", self.slot)
    }
}

/// One gust: when it set off, from where and which way, how fast and how far it runs, and the
/// seed its curls are picked from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Gust {
    start: f64,
    from: f64,
    /// 1 to the right, -1 to the left.
    way: f64,
    speed: f64,
    run: f64,
    seed: f32,
    /// The side of the line its first curl is on: 1 below, -1 above.
    side: f32,
}

impl Gust {
    /// The gust numbered `n`, setting off at `start` from the marker at `marker` on a bar `wide`
    /// units long, towards the side with more room.
    fn new(n: u64, start: f64, marker: f64, wide: f64) -> Self {
        let mut pick = Picker::new(n ^ 0x6a09_e667);
        let way = if marker <= wide / 2.0 { 1.0 } else { -1.0 };
        Self {
            start,
            from: marker + way * GUST_FROM,
            way,
            speed: pick.within(GUST_SPEED),
            run: pick.within(GUST_RUN),
            seed: (n % 997) as f32,
            side: if pick.next() < 0.5 { 1.0 } else { -1.0 },
        }
    }

    /// When its head has run its course.
    fn head_gone(&self) -> f64 {
        self.start + self.run / self.speed
    }

    /// When its last curl has gone.
    fn over(&self) -> f64 {
        self.start + self.run / self.speed + CURL_LIFE.high
    }
}

/// Numbers from 0 to 1, the same ones for the same seed.
struct Picker(u64);

impl Picker {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> f64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn within(&mut self, range: Range) -> f64 {
        range.at(self.next())
    }
}

/// The edge's timing, shared by every screen, since only the one with the keyboard moves: the
/// pulses out, the next to leave, and the gust.
#[derive(Debug, Default)]
pub struct Charge {
    /// When each pulse out left the marker and how fast it runs; `None` for none.
    pulses: [Option<(f64, f64)>; PULSES],
    next_pulse: Option<f64>,
    pulse_count: u64,
    gust: Option<Gust>,
    next_gust: Option<f64>,
    gust_count: u64,
    /// A gust asked for now, by a debug step.
    gust_now: bool,
}

impl Charge {
    /// Sends a gust off on the next frame that moves, so it can be looked at without waiting.
    pub fn gust_now(&mut self) {
        self.gust_now = true;
    }

    /// Moves on to `now`, with the marker at `marker` on a bar `wide` units long, and says which
    /// time the edge is drawn at: `now` itself while a gust's head is running, so it runs at the
    /// display's rate, and otherwise `now` held to the last thirtieth of a second.
    fn advance(&mut self, now: f64, marker: f64, wide: f64) -> f64 {
        let done = done_at(marker, wide);
        if std::mem::take(&mut self.gust_now) {
            self.next_gust = Some(now);
        }
        let next_gust = *self
            .next_gust
            .get_or_insert_with(|| now + Picker::new(self.gust_count).within(GUST_EVERY));
        if now >= next_gust {
            // One missed by much, while the edge was still, is skipped rather than sent late.
            if now - next_gust < 0.25 {
                self.gust = Some(Gust::new(self.gust_count, next_gust, marker, wide));
            }
            self.gust_count += 1;
            let after = Picker::new(self.gust_count ^ 0x51ed).within(GUST_EVERY);
            self.next_gust = Some(next_gust.max(now - 0.25) + after);
        }
        if self.gust.is_some_and(|gust| now >= gust.over()) {
            self.gust = None;
        }
        // Only a gust's head is quick enough to need every frame; its curls drift and turn slowly
        // enough for the pulses' steps once it has gone.
        let at = if self.gust.is_some_and(|gust| now < gust.head_gone()) {
            now
        } else {
            (now * STEPS).floor() / STEPS
        };

        let next_pulse = *self.next_pulse.get_or_insert(at + 0.5);
        if at >= next_pulse {
            let mut pick = Picker::new(self.pulse_count ^ 0x7f4a_0001);
            let speed = pick.within(PULSE_SPEED);
            let gap = pick.within(PULSE_EVERY);
            self.pulse_count += 1;
            // Any that has run its course makes room; the oldest goes if none has.
            let slot = self
                .pulses
                .iter()
                .position(|pulse| pulse.is_none_or(|(left, speed)| (at - left) * speed > done))
                .unwrap_or_else(|| {
                    (0..PULSES)
                        .min_by(|&a, &b| {
                            let left = |n: usize| self.pulses[n].map_or(f64::MIN, |(left, _)| left);
                            left(a).total_cmp(&left(b))
                        })
                        .unwrap_or(0)
                });
            // One that should have left while the edge was still leaves now instead.
            let left = if at - next_pulse < PULSE_EVERY.high {
                next_pulse
            } else {
                at
            };
            self.pulses[slot] = Some((left, speed));
            self.next_pulse = Some(left + gap);
        }
        at
    }

    /// How far each pulse has run at `at`, with the marker at `marker` on a bar `wide` units
    /// long, in the bar's units; -1 for none.
    fn runs(&self, at: f64, marker: f64, wide: f64) -> [f32; PULSES] {
        let done = done_at(marker, wide);
        std::array::from_fn(|n| match self.pulses[n] {
            Some((left, speed)) if at >= left && (at - left) * speed <= done => {
                ((at - left) * speed) as f32
            }
            _ => -1.0,
        })
    }
}

/// How far it is from the marker at `marker` to the further end of a bar `wide` units long.
fn reach(marker: f64, wide: f64) -> f64 {
    marker.max(wide - marker).max(1.0)
}

/// How far a pulse runs before it is let go: past the bar's further end, or as far as any runs.
fn done_at(marker: f64, wide: f64) -> f64 {
    (reach(marker, wide) + PULSE_TAIL).min(PULSE_LONGEST)
}

/// One screen's strip, kept from frame to frame: it counts as changed only when its numbers do.
#[derive(Default)]
pub struct Light(shader::Kept);

impl Light {
    /// The strip along the bar's foot at `now` seconds, moving on `charge` when it is given and
    /// otherwise the still edge, the line and the marker's glow alone. `None` if the shader
    /// didn't compile.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        edge: &Edge,
        charge: Option<&mut Charge>,
        now: f64,
    ) -> Option<shader::Shaded> {
        let program = program(renderer)?;
        let (area, uniforms) = uniforms(edge, charge, now);
        Some(self.0.pixel(program, area, &uniforms))
    }
}

/// The strip's place and its shader's numbers for `edge` at `now`, moving on `charge` if given.
fn uniforms(
    edge: &Edge,
    charge: Option<&mut Charge>,
    now: f64,
) -> (Rectangle<i32, Logical>, Uniforms) {
    let (bar, scale) = (edge.bar, edge.scale);
    // The bar's picture: one pixel per screen pixel from its rounded corner.
    let (left, top) = ((bar.loc.x * scale).round(), (bar.loc.y * scale).round());
    let tall = (HEIGHT as f64 * scale).round();
    let f = tall / TALL as f64;
    let wide = (bar.size.w * scale).round() / f;
    let area = Rectangle::<i32, Logical>::new(
        (bar.loc.x as i32, bar.loc.y as i32 + HEIGHT - ABOVE).into(),
        (bar.size.w as i32, ABOVE + BELOW).into(),
    );
    let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
    let marker = edge.marker.unwrap_or(-1e4);
    let strength = if edge.marker.is_some() {
        edge.strength
    } else {
        0.0
    };
    let rgb = |c: u32| {
        [
            (c >> 24) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
        ]
    };
    let line_alpha = (edge.line & 0xff) as f32 / 255.0;
    let line = rgb(edge.line).map(|c| c * line_alpha);
    let (gap_left, gap_right) = edge.gap.unwrap_or((-1.0, -1.0));
    let units = |x: f64| (((x - bar.loc.x) * scale) / f) as f32;
    let mut uniforms = Uniforms {
        frame: [
            f as f32,
            tall as f32,
            (screen.loc.x as f64 - left) as f32,
            (screen.loc.y as f64 - top) as f32,
        ],
        extent: [screen.size.w as f32, screen.size.h as f32],
        ring: rgb(edge.ring),
        edge: [line[0], line[1], line[2], line_alpha],
        look: [
            marker,
            strength,
            edge.alpha,
            f32::from(u8::from(edge.charged)),
        ],
        pulses: [-1.0; 4],
        pulses_more: [-1.0; 4],
        gust: [0.0, 0.0, 0.0, -1.0],
        wake: [0.0; 2],
        bar: [gap_left, gap_right, reach(marker as f64, wide) as f32],
        slot: edge.slot.map_or([0.0; 4], |slot| {
            let (left, right) = (units(slot.left), units(slot.right));
            // Tapered as a lit line is, at least a unit long so its ends never meet.
            let length = f64::from(right - left).max(1.0) * f;
            let taper = lit_line::taper(length, scale) / f;
            [left, right, slot.strength, taper as f32]
        }),
    };
    let Some(charge) = charge.filter(|_| edge.charged) else {
        return (area, uniforms);
    };
    let at = charge.advance(now, marker as f64, wide);
    let runs = charge.runs(at, marker as f64, wide);
    uniforms.pulses = [runs[0], runs[1], runs[2], runs[3]];
    uniforms.pulses_more = [runs[4], runs[5], runs[6], runs[7]];
    if let Some(gust) = charge.gust {
        let signed = (gust.way * gust.speed) as f32;
        let age = (at - gust.start).max(0.0) as f32;
        uniforms.gust = [gust.from as f32, signed, gust.run as f32, age];
        uniforms.wake = [gust.seed, gust.side];
    }
    (area, uniforms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge() -> Edge {
        Edge {
            bar: Rectangle::new((0.0, 0.0).into(), (1536.0, HEIGHT as f64).into()),
            scale: 1.0,
            marker: Some(80.0),
            strength: 1.0,
            ring: 0x00ceffff,
            line: 0xffffff12,
            charged: true,
            gap: None,
            slot: None,
            alpha: 1.0,
        }
    }

    /// Steps the strip at 60 frames a second from `from` for `secs`, moving on `charge` if there
    /// is one, and gives the frames whose numbers changed, each with whether a gust's head was
    /// running then.
    fn changed_frames(
        charge: &mut Option<Charge>,
        edge: &Edge,
        from: f64,
        secs: f64,
    ) -> Vec<(f64, bool)> {
        let mut kept = shader::Kept::default();
        let mut last = kept.commit();
        let mut changed = Vec::new();
        for frame in 0..(secs * 60.0) as usize {
            let now = from + frame as f64 / 60.0;
            let (area, uniforms) = uniforms(edge, charge.as_mut(), now);
            kept.note(area, &uniforms);
            if kept.commit() != last {
                last = kept.commit();
                let head = charge
                    .as_ref()
                    .and_then(|charge| charge.gust)
                    .is_some_and(|gust| now < gust.head_gone());
                changed.push((now, head));
            }
        }
        changed
    }

    #[test]
    fn gusts_leave_every_six_to_ten_seconds_towards_the_side_with_room() {
        let mut charge = Charge::default();
        let mut gusts: Vec<Gust> = Vec::new();
        let mut now = 0.0;
        while now < 600.0 {
            // The marker near the left end, then near the right.
            let marker = if now < 300.0 { 200.0 } else { 1700.0 };
            charge.advance(now, marker, 1900.0);
            if let Some(gust) = charge.gust
                && gusts.last().is_none_or(|last| last.start != gust.start)
            {
                gusts.push(gust);
                assert_eq!(gust.way, if marker < 950.0 { 1.0 } else { -1.0 });
                assert!(GUST_SPEED.contains(gust.speed));
                assert!(GUST_RUN.contains(gust.run));
            }
            now += 1.0 / 60.0;
        }
        assert!(gusts.len() > 55, "{} gusts", gusts.len());
        for pair in gusts.windows(2) {
            let apart = pair[1].start - pair[0].start;
            assert!((5.99..=10.01).contains(&apart), "{apart} s between gusts");
        }
        // The same gust every time for the same number.
        assert_eq!(Gust::new(7, 1.0, 2.0, 900.0), Gust::new(7, 1.0, 2.0, 900.0));
    }

    #[test]
    fn a_gusts_head_runs_at_the_displays_rate_and_the_rest_steps() {
        let mut charge = Charge::default();
        let (mut stepped, mut smooth) = (0, 0);
        let mut now = 0.0;
        while now < 120.0 {
            let at = charge.advance(now, 400.0, 1900.0);
            if charge.gust.is_some_and(|gust| now < gust.head_gone()) {
                assert_eq!(at, now);
                smooth += 1;
            } else {
                assert_eq!(at, (now * STEPS).floor() / STEPS);
                stepped += 1;
            }
            now += 1.0 / 60.0;
        }
        assert!(
            smooth > 0 && stepped > smooth,
            "{smooth} smooth, {stepped} stepped"
        );
    }

    #[test]
    fn pulses_leave_every_few_seconds_and_run_out_slowly() {
        let mut charge = Charge::default();
        let mut leaving = Vec::new();
        let mut now = 0.0;
        while now < 300.0 {
            charge.advance(now, 400.0, 1900.0);
            for (left, speed) in charge.pulses.iter().flatten() {
                if !leaving.iter().any(|(l, _)| l == left) {
                    leaving.push((*left, *speed));
                }
            }
            now += 1.0 / 60.0;
        }
        assert!(leaving.len() > 45);
        for pair in leaving.windows(2) {
            let apart = pair[1].0 - pair[0].0;
            assert!((4.49..=6.01).contains(&apart), "{apart} s between pulses");
        }
        for (_, speed) in &leaving {
            // A step of the pulses moves one under a quarter of its narrowest width.
            assert!(speed / STEPS < 26.0 / 4.0);
        }
    }

    #[test]
    fn pulses_run_to_the_far_end_of_a_wide_bar_with_several_out_at_once() {
        // A 2560-pixel screen at 1.25 with the marker on the first workspace.
        let (marker, wide) = (60.0, 2560.0);
        let mut charge = Charge::default();
        let (mut furthest, mut most_out) = (0.0_f32, 0);
        let mut now = 0.0;
        while now < 120.0 {
            let at = charge.advance(now, marker, wide);
            let runs = charge.runs(at, marker, wide);
            furthest = runs.iter().fold(furthest, |far, &run| far.max(run));
            most_out = most_out.max(runs.iter().filter(|&&run| run >= 0.0).count());
            now += 1.0 / 60.0;
        }
        assert!(
            furthest as f64 >= wide - marker,
            "they reach the far end: {furthest}"
        );
        assert!((3..=5).contains(&most_out), "{most_out} out at once");
        // However wide the bar, no pulse is let go while it still shows.
        assert!(PULSE_LONGEST / PULSE_SPEED.low <= PULSES as f64 * PULSE_EVERY.low);
    }

    #[test]
    fn a_pulse_is_nearly_always_out_so_the_moving_edge_wakes_its_screen_thirty_times_a_second() {
        // A 2560-pixel screen at 1.25, a minute in, measured for a minute.
        let mut edge = edge();
        edge.bar.size.w = 2048.0;
        edge.scale = 1.25;
        let mut charge = Some(Charge::default());
        changed_frames(&mut charge, &edge, 0.0, 60.0);
        let changed = changed_frames(&mut charge, &edge, 60.0, 60.0);
        // It never rests: no step of the pulses goes by without a change.
        for pair in changed.windows(2) {
            let apart = pair[1].0 - pair[0].0;
            assert!(apart <= 1.0 / STEPS + 1.0 / 60.0 + 1e-9, "rested {apart} s");
        }
        // Outside a gust's head that is once a step, never every frame.
        let stepped = changed.iter().filter(|(_, head)| !head).count();
        let heads = changed.len() - stepped;
        assert!(heads > 0, "gusts ran");
        assert!(
            stepped as f64 <= STEPS * 60.0 + 1.0 && stepped as f64 >= 0.7 * STEPS * 60.0,
            "{stepped} stepped changes in a minute"
        );
    }

    #[test]
    fn the_still_edge_never_changes_and_has_no_pulses_or_gusts() {
        assert_eq!(
            changed_frames(&mut None, &edge(), 5.0, 10.0).len(),
            1,
            "the still edge never changes"
        );
        let (_, first) = uniforms(&edge(), None, 5.0);
        assert_eq!(first.pulses, [-1.0; 4]);
        assert_eq!(first.pulses_more, [-1.0; 4]);
        assert!(first.gust[3] < 0.0, "no gust");
        assert!(first.look[1] > 0.0, "the marker still glows");
        // And it follows the marker by jumping with it.
        let mut moved = edge();
        moved.marker = Some(300.0);
        assert_eq!(uniforms(&moved, None, 20.0).1.look[0], 300.0);
    }

    #[test]
    fn a_gust_asked_for_leaves_now_and_its_wake_is_gone_when_it_is_over() {
        let mut charge = Charge::default();
        charge.gust_now();
        let (_, sent) = uniforms(&edge(), Some(&mut charge), 3.0);
        let gust = charge.gust.expect("a gust");
        assert_eq!(sent.gust[3], 0.0, "it sets off now");
        assert_eq!(sent.gust[1].signum(), 1.0, "towards the room on the right");
        assert_eq!(sent.gust[0], 80.0 + GUST_FROM as f32);
        let (_, after) = uniforms(&edge(), Some(&mut charge), gust.over() + 0.01);
        assert!(after.gust[3] < 0.0, "nothing left of it");
    }

    #[test]
    fn a_panels_slot_is_passed_in_the_bars_units_whether_or_not_the_line_is_charged() {
        let mut edge = edge();
        edge.scale = 1.25;
        edge.slot = Some(BarSlot {
            left: 100.0,
            right: 420.0,
            strength: 1.0,
        });
        // At 1.25 a unit of the bar is one screen pixel; its ends taper as a lit line's do.
        let taper = lit_line::taper(400.0, 1.25) as f32;
        assert_eq!(
            uniforms(&edge, None, 1.0).1.slot,
            [125.0, 525.0, 1.0, taper]
        );
        edge.charged = false;
        let (_, alone) = uniforms(&edge, Some(&mut Charge::default()), 1.0);
        assert_eq!(alone.slot, [125.0, 525.0, 1.0, taper]);
        assert_eq!(alone.look[3], 0.0, "the line itself isn't charged");
        assert_eq!(alone.pulses, [-1.0; 4], "and carries nothing");
        edge.slot = None;
        assert_eq!(uniforms(&edge, None, 1.0).1.slot[2], 0.0, "nothing at rest");
    }

    /// The furthest a curl reaches from the line's middle, in the bar's units, as the shader
    /// draws it: its drift away from the line, half its widest turn (each is flattened to half
    /// its height, and grows to 1.7 times its radius), and its halo.
    const CURL_REACH: f64 = 6.0 + 8.0 * 1.7 * 0.5 + 2.0;

    #[test]
    fn the_shared_numbers_come_after_the_shaders_precision() {
        let source = shader();
        let precision = source.find("precision highp float;").unwrap();
        assert!(source.find("const float CURL_EVERY = 52.0;").unwrap() > precision);
        assert_eq!(source.matches("precision highp float;").count(), 1);
    }

    #[test]
    fn the_curls_fit_inside_the_strip_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let tall = (HEIGHT as f64 * scale).round();
            let unit = tall / TALL as f64 / scale;
            // Two units of fade at the strip's rim, either side.
            let room = |side: i32| side as f64 / unit - 2.0 - 0.5;
            assert!(CURL_REACH < room(ABOVE - 1), "above at {scale}");
            assert!(CURL_REACH < room(BELOW), "below at {scale}");
        }
    }

    #[test]
    fn the_strip_lines_up_with_the_bars_foot_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let mut edge = edge();
            edge.scale = scale;
            let (area, u) = uniforms(&edge, None, 1.0);
            let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
            // The line's rows are the bar's last unit, within the strip.
            let line_top = u.frame[1] - u.frame[0];
            assert!(line_top > u.frame[3]);
            assert!(u.frame[1] < u.frame[3] + screen.size.h as f32);
            assert_eq!(u.frame[1], (HEIGHT as f64 * scale).round() as f32);
        }
    }
}
