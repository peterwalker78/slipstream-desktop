//! The lit edge: the line along the bar's foot, carrying a charge in the focus ring's colour. It
//! glows faintly all along, brighter around the workspace marker, and the glow follows the
//! marker as it slides. Every few seconds a soft pulse leaves the marker and runs out both ways
//! to the bar's ends, fading gently on the way, so several are always out; the line shimmers
//! faintly as a live wire does, and now and then a short electric arc jumps along a stretch of
//! it, restriking two to four times in a tenth of a second or so, and leaves a brief afterglow.
//! Every part of an arc fades in and out along its stretch, so it has no ends to see. While a
//! panel comes out of the bar, the line brightens over the slot it comes out of.
//!
//! One shader strip draws the line, its glow and the arcs, worked out per pixel from continuous
//! time and sub-pixel positions, so nothing in it steps or comes in blocks. The bar's own picture
//! leaves its foot line out in this style, so the line has one owner and no seam. The slow parts
//! move in steps of a thirtieth of a second, well under a pixel each, so between steps a frame
//! has nothing new and the screen isn't redrawn for it; while an arc is live the strip changes
//! every frame, so the arc runs at the display's own rate.

use smithay::{
    backend::renderer::{
        element::Kind,
        gles::{GlesRenderer, Uniform, UniformName, UniformType, element::PixelShaderElement},
    },
    utils::{Logical, Physical, Rectangle},
};

use super::{HEIGHT, TALL};

/// How often the glow and the pulses move, a second.
pub const STEPS: f64 = 30.0;

/// Seconds between one pulse leaving the marker and the next, picked afresh each time.
const PULSE_EVERY: (f64, f64) = (4.5, 6.0);
/// How fast a pulse runs out along the edge, in the bar's units a second, picked per pulse.
const PULSE_SPEED: (f64, f64) = (110.0, 135.0);
/// How far past the bar's far end a pulse runs before it is let go, so it leaves the bar whole.
const PULSE_TAIL: f64 = 200.0;
/// The furthest a pulse runs, however wide the bar: it fades out over the last stretch of it.
/// The slowest takes under 27 seconds to get there, so `PULSES` is enough to have every one out
/// at once.
const PULSE_LONGEST: f64 = 2950.0;
/// The most pulses out at once: they leave at least 4.5 s apart and none lasts 27 s.
const PULSES: usize = 8;

/// Seconds between one arc and the next.
const ARC_EVERY: (f64, f64) = (3.0, 7.0);
/// How long an arc lasts, in seconds: a little slower than a real one, so it can be followed.
const ARC_LIFE: (f64, f64) = (0.18, 0.32);
/// How long each strike lasts before the arc restrikes along a new path, in seconds.
const RESTRIKE: (f64, f64) = (0.055, 0.09);
/// How many strikes an arc has.
const STRIKES: (usize, usize) = (2, 4);
/// How long a stretch of the edge an arc spans, in the bar's units.
const ARC_STRETCH: (f64, f64) = (40.0, 200.0);
/// How long the stretch an arc spanned keeps glowing after it, in seconds.
const AFTERGLOW: f64 = 0.4;
/// The share of arcs that strike near the marker rather than anywhere along the bar, and how far
/// from it they reach, in the bar's units.
const NEAR_MARKER: f64 = 0.4;
const MARKER_REACH: f64 = 600.0;

/// How far the strip reaches above and below the line, in logical pixels: room for the glow
/// either side and for an arc's bulge.
const ABOVE: i32 = 16;
const BELOW: i32 = 17;

/// The strip's shader, its numbers in the bar's units unless they say otherwise. The colours are
/// light added to what's under the strip, apart from the line's own colour, which is laid over.
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
// The marker's middle, how strongly it glows, the strip's alpha, and the time the shimmer runs
// on (below zero, a still edge).
uniform vec4 look;
// How far each pulse has run from the marker; below zero, none.
uniform vec4 pulses;
uniform vec4 pulses_more;
// The arc's left end and stretch, the seed of its current path, and how bright it is now.
uniform vec4 arc;
// The arc's afterglow, the gap where a docked pane's frame carries the bar on down, and how far
// it is from the marker to the bar's further end.
uniform vec4 after;
// The slot a panel is coming out of: its left and right ends, and how strongly it is lit.
uniform vec3 slot;

// The charge all along the edge, and what the marker adds at its middle.
const float BASE = 0.14;
const float MARKER = 0.34;
const float SPREAD = 420.0;
// What a pulse adds at its middle when it leaves the marker, its share of that at the bar's far
// end, and the furthest it runs, fading out over the last of it.
const float PULSE = 0.7;
const float FAR = 0.33;
const float LONGEST = 2950.0;
// The shimmer, as a share of the line's brightness either way.
const float BUZZ = 0.06;
// Above KNEE the line brightens ever more slowly towards CEILING, so it never glares.
const float KNEE = 0.62;
const float CEILING = 0.88;
// An arc fades in and out over this share of its stretch at either end.
const float ARC_TAPER = 0.28;
// What a lit slot adds to the line, and how far its ends taper, at most.
const float SLOT = 0.8;
const float SLOT_TAPER = 48.0;
// How far an arc's path strays from the line, and its segments' length. A longer arc strays
// further, so arcs vary in size as well as length.
const float BULGE = 6.0;
const float SEGMENT = 12.0;

float sq(float v) {
    return v * v;
}

float hash(float n) {
    return fract(sin(n) * 43758.5453);
}

// Smooth value noise: continuous in both directions, so the shimmer never comes in blocks.
float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float n = i.x + i.y * 57.0;
    float a = mix(hash(n), hash(n + 1.0), u.x);
    float b = mix(hash(n + 57.0), hash(n + 58.0), u.x);
    return mix(a, b, u.y);
}

float bulge() {
    return BULGE * clamp(arc.y / 120.0, 0.6, 1.15);
}

// How far an arc may stray at `share` of the way along it: none at its ends.
float taper(float share) {
    return pow(max(sin(3.14159265 * clamp(share, 0.0, 1.0)), 0.0), 0.6);
}

// A corner of the arc's main path, `count` segments long.
float corner(float i, float count) {
    if (i <= 0.0 || i >= count) return 0.0;
    return (hash(arc.z * 1.37 + i * 7.13) * 2.0 - 1.0) * bulge() * taper(i / count);
}

// The `j`th point of the path, which halves each main segment with a smaller kink.
vec2 point(float j, float count, float step) {
    float i = floor(j * 0.5);
    float y = corner(i, count);
    if (mod(j, 2.0) > 0.5) {
        float kink = (hash(arc.z * 2.11 + i * 3.71 + 101.0) * 2.0 - 1.0) * bulge() * 0.5;
        y = 0.5 * (y + corner(i + 1.0, count)) + kink * taper(j / (2.0 * count));
    }
    return vec2(arc.x + j * step, y);
}

// What a pulse `run` units out adds at `d` units from the marker: its charge and its white core.
vec2 pulse(float run, float d) {
    if (run < 0.0) return vec2(0.0);
    float wide = 26.0 + 0.06 * min(run, 1400.0);
    float fade = mix(1.0, FAR, pow(clamp(run / max(after.w, 1.0), 0.0, 1.0), 0.85))
        * smoothstep(0.0, 30.0, run) * (1.0 - smoothstep(LONGEST - 250.0, LONGEST, run));
    float g = exp(-sq((d - run) / wide));
    return vec2(PULSE * look.y * g * fade, pow(g, 8.0) * fade);
}

float to_segment(vec2 p, vec2 a, vec2 b) {
    vec2 ab = b - a;
    float t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return length(p - a - ab * t);
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
    float in_gap = clamp(min(at.x + 0.5, after.z * f) - max(at.x - 0.5, after.y * f), 0.0, 1.0);
    float open = 1.0 - in_gap;

    float d = abs(x - look.x);
    float charge = BASE + MARKER * look.y * exp(-sq(d / SPREAD));
    vec2 runs = pulse(pulses.x, d) + pulse(pulses.y, d) + pulse(pulses.z, d)
        + pulse(pulses.w, d) + pulse(pulses_more.x, d) + pulse(pulses_more.y, d)
        + pulse(pulses_more.z, d) + pulse(pulses_more.w, d);
    charge += runs.x;
    float core = runs.y;
    if (look.w >= 0.0) {
        float shimmer = 0.7 * (noise(vec2(x / 14.0, look.w * 2.3)) * 2.0 - 1.0)
            + 0.3 * (noise(vec2(x / 5.0 + 31.0, look.w * 6.1)) * 2.0 - 1.0);
        charge *= 1.0 + BUZZ * shimmer;
    }
    // Everything an arc adds fades to nothing over the ends of its stretch.
    float along = (x - arc.x) / max(arc.y, 1.0);
    float stretch = smoothstep(0.0, ARC_TAPER, along) * smoothstep(0.0, ARC_TAPER, 1.0 - along);
    charge += (0.45 * after.x + 0.4 * arc.w) * stretch;
    if (charge > KNEE) {
        float room = CEILING - KNEE;
        charge = KNEE + room * (1.0 - exp(-(charge - KNEE) / room));
    }
    // A lit slot, over the knee so its glint can flare, its ends tapering into the line.
    float lit = 0.0;
    if (slot.z > 0.0) {
        float taper = min(0.15 * max(slot.y - slot.x, 1.0), SLOT_TAPER);
        lit = smoothstep(slot.x, slot.x + taper, x) * (1.0 - smoothstep(slot.y - taper, slot.y, x));
        charge += SLOT * slot.z * lit;
    }

    // The glow beside the line: a little above, into the bar, and further below.
    float glow = below > 0.0
        ? 0.55 * pow(clamp(1.0 - below / 12.0, 0.0, 1.0), 2.2)
        : 0.30 * pow(clamp(1.0 + below / 4.0, 0.0, 1.0), 2.0);
    vec3 light = ring * charge * (cover + (1.0 - cover) * glow);
    // A white-hot touch at a pulse's very core, on the line alone.
    light += vec3((0.25 * min(core, 1.0) + 0.4 * max(slot.z - 1.0, 0.0) * lit) * cover);

    if (arc.w > 0.0) {
        float count = max(4.0, floor(arc.y / SEGMENT + 0.5));
        float step = arc.y / (2.0 * count);
        vec2 p = vec2(x, below);
        float k = clamp(floor((x - arc.x) / step), 0.0, 2.0 * count - 1.0);
        float near = 1e6;
        for (int n = -1; n <= 1; n++) {
            float j = clamp(k + float(n), 0.0, 2.0 * count - 1.0);
            near = min(near, to_segment(p, point(j, count, step), point(j + 1.0, count, step)));
        }
        // The core is at least about a screen pixel wide, so it never breaks up.
        float hot = exp(-sq(near * f / max(0.85, 0.55 * f)));
        float halo = exp(-sq(near / 2.2));
        float haze = exp(-sq(near / 3.0));
        // Nothing reaches the strip's top or bottom, so it has no edge to show.
        float rim = smoothstep(0.0, 2.0 * f, at.y - frame.w)
            * smoothstep(0.0, 2.0 * f, extent.y - (at.y - frame.w));
        light += (vec3(0.9 * hot) + ring * (0.75 * halo + 0.2 * haze)) * arc.w * stretch * rim;
    }

    vec4 color = vec4(edge.rgb * cover + light, edge.a * cover) * open * look.z * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

fn uniform_names() -> [UniformName<'static>; 10] {
    [
        UniformName::new("frame", UniformType::_4f),
        UniformName::new("extent", UniformType::_2f),
        UniformName::new("ring", UniformType::_3f),
        UniformName::new("edge", UniformType::_4f),
        UniformName::new("look", UniformType::_4f),
        UniformName::new("pulses", UniformType::_4f),
        UniformName::new("pulses_more", UniformType::_4f),
        UniformName::new("arc", UniformType::_4f),
        UniformName::new("after", UniformType::_4f),
        UniformName::new("slot", UniformType::_3f),
    ]
}

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
    /// Where a docked pane's frame carries the bar on down, in the bar's units.
    pub gap: Option<(f32, f32)>,
    /// The slot a panel is coming out of: its ends in logical pixels across the screen, and how
    /// strongly it is lit.
    pub slot: Option<(f64, f64, f32)>,
    pub alpha: f32,
}

/// The shader's numbers, all plain values, so working them out each frame allocates nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    frame: [f32; 4],
    extent: [f32; 2],
    ring: [f32; 3],
    edge: [f32; 4],
    look: [f32; 4],
    pulses: [f32; 4],
    pulses_more: [f32; 4],
    arc: [f32; 4],
    after: [f32; 4],
    slot: [f32; 3],
}

impl Uniforms {
    fn list(&self) -> Vec<Uniform<'static>> {
        vec![
            Uniform::new("frame", self.frame),
            Uniform::new("extent", self.extent),
            Uniform::new("ring", self.ring),
            Uniform::new("edge", self.edge),
            Uniform::new("look", self.look),
            Uniform::new("pulses", self.pulses),
            Uniform::new("pulses_more", self.pulses_more),
            Uniform::new("arc", self.arc),
            Uniform::new("after", self.after),
            Uniform::new("slot", self.slot),
        ]
    }
}

/// One arc: when it struck, where, and when each of its strikes lands, in seconds from its start.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bolt {
    start: f64,
    left: f64,
    stretch: f64,
    life: f64,
    strikes: [f64; STRIKES.1],
    count: usize,
    /// Each strike's path comes from this, so the arc is the same every time it's drawn.
    seed: f32,
}

impl Bolt {
    /// The arc numbered `n`, striking at `start` along a bar `wide` units long with its marker
    /// at `marker`.
    fn new(n: u64, start: f64, marker: f64, wide: f64) -> Self {
        let mut pick = Picker::new(n ^ 0xa5c3_17e9);
        let life = pick.within(ARC_LIFE);
        let wanted = STRIKES.0 + (pick.next() * (STRIKES.1 - STRIKES.0 + 1) as f64) as usize;
        let mut strikes = [0.0; STRIKES.1];
        let mut count = 1;
        while count < wanted.min(STRIKES.1) {
            let at = strikes[count - 1] + pick.within(RESTRIKE);
            // The last strike has a moment to be seen before the arc goes.
            if at > life - 0.02 {
                break;
            }
            strikes[count] = at;
            count += 1;
        }
        let stretch = pick.within(ARC_STRETCH).min(wide);
        let middle = if pick.next() < NEAR_MARKER {
            // Towards the side with room, so a marker near an end doesn't pile arcs up there.
            let away = (pick.next() + pick.next() - 1.0) * MARKER_REACH;
            let fits = |middle: f64| (stretch / 2.0..=wide - stretch / 2.0).contains(&middle);
            if fits(marker + away) || !fits(marker - away) {
                marker + away
            } else {
                marker - away
            }
        } else {
            pick.next() * wide
        };
        let left = (middle - stretch / 2.0).clamp(0.0, (wide - stretch).max(0.0));
        Self {
            start,
            left,
            stretch,
            life,
            strikes,
            count,
            seed: (n % 997) as f32,
        }
    }

    /// The strike showing `age` seconds in, and how bright the arc is: each strike flashes and
    /// dims fast, flickering low before the next, and the arc goes out at the end of its life.
    fn flash(&self, age: f64) -> Option<(usize, f32)> {
        if !(0.0..self.life).contains(&age) {
            return None;
        }
        let strike = self.strikes[..self.count]
            .iter()
            .rposition(|&at| at <= age)
            .unwrap_or(0);
        let since = age - self.strikes[strike];
        let going = ((self.life - age) / 0.015).clamp(0.0, 1.0);
        Some((
            strike,
            ((0.12 + 0.88 * (-since / 0.012).exp()) * going) as f32,
        ))
    }

    /// How brightly the stretch it spanned still glows `age` seconds in.
    fn afterglow(&self, age: f64) -> f32 {
        if age < 0.0 {
            return 0.0;
        }
        let gone = ((age - self.life) / AFTERGLOW).clamp(0.0, 1.0);
        ((1.0 - gone) * (1.0 - gone)) as f32
    }

    /// Whether it or its afterglow is still on the screen at `now`.
    fn live(&self, now: f64) -> bool {
        now < self.start + self.life + AFTERGLOW
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

    fn within(&mut self, (low, high): (f64, f64)) -> f64 {
        low + (high - low) * self.next()
    }
}

/// The edge's timing: the pulses out, the next to leave, and the arcs.
#[derive(Debug, Default)]
struct Charge {
    /// When each pulse out left the marker and how fast it runs; `None` for none.
    pulses: [Option<(f64, f64)>; PULSES],
    next_pulse: Option<f64>,
    pulse_count: u64,
    bolt: Option<Bolt>,
    next_bolt: Option<f64>,
    bolt_count: u64,
    /// An arc asked for now, by a debug step.
    strike_now: bool,
}

impl Charge {
    /// Moves on to `now`, with the marker at `marker` on a bar `wide` units long, and says which
    /// time the slow parts are drawn at: `now` itself while an arc is live, so it runs at the
    /// display's rate, and otherwise `now` held to the last thirtieth of a second.
    fn advance(&mut self, now: f64, marker: f64, wide: f64) -> f64 {
        let done = done_at(marker, wide);
        if self.strike_now {
            self.strike_now = false;
            self.next_bolt = Some(now);
        }
        let next_bolt = *self
            .next_bolt
            .get_or_insert_with(|| now + Picker::new(self.bolt_count).within(ARC_EVERY));
        if now >= next_bolt {
            // One missed by much, while the edge was still, is skipped rather than struck late.
            if now - next_bolt < 0.25 {
                self.bolt = Some(Bolt::new(self.bolt_count, next_bolt, marker, wide));
            }
            self.bolt_count += 1;
            let after = Picker::new(self.bolt_count ^ 0x51ed).within(ARC_EVERY);
            self.next_bolt = Some(next_bolt.max(now - 0.25) + after);
        }
        if self.bolt.is_some_and(|bolt| !bolt.live(now)) {
            self.bolt = None;
        }
        let at = if self.bolt.is_some() {
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
            let left = if at - next_pulse < PULSE_EVERY.1 {
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

#[derive(Default)]
pub struct Light {
    /// The strip, made once the shader compiles; `Err` if it didn't.
    element: Option<Result<PixelShaderElement, ()>>,
    shown: Option<Uniforms>,
    charge: Charge,
}

impl Light {
    /// Strikes an arc on the next frame that moves, so it can be looked at without waiting.
    pub fn strike_now(&mut self) {
        self.charge.strike_now = true;
    }

    /// The strip along the bar's foot at `now` seconds: moving unless `moving` is false, when it
    /// is the still edge, the line and the marker's glow alone.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        edge: &Edge,
        now: f64,
        moving: bool,
    ) -> Option<PixelShaderElement> {
        let (area, uniforms) = self.uniforms(edge, now, moving);
        let element = self.element.get_or_insert_with(|| {
            renderer
                .compile_custom_pixel_shader(SHADER, &uniform_names())
                .map(|program| {
                    PixelShaderElement::new(
                        program,
                        area,
                        None,
                        1.0,
                        uniforms.list(),
                        Kind::Unspecified,
                    )
                })
                .map_err(|err| tracing::warn!("the lit edge's shader didn't compile: {err}"))
        });
        let element = element.as_mut().ok()?;
        element.resize(area, None);
        if self.shown != Some(uniforms) {
            element.update_uniforms(uniforms.list());
            self.shown = Some(uniforms);
        }
        Some(element.clone())
    }

    /// The strip's place and its shader's numbers for `edge` at `now`.
    fn uniforms(
        &mut self,
        edge: &Edge,
        now: f64,
        moving: bool,
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
        let ring = rgb(edge.ring);
        let line_alpha = (edge.line & 0xff) as f32 / 255.0;
        let line = rgb(edge.line).map(|c| c * line_alpha);
        let (gap_left, gap_right) = edge.gap.unwrap_or((-1.0, -1.0));
        let mut uniforms = Uniforms {
            frame: [
                f as f32,
                tall as f32,
                (screen.loc.x as f64 - left) as f32,
                (screen.loc.y as f64 - top) as f32,
            ],
            extent: [screen.size.w as f32, screen.size.h as f32],
            ring,
            edge: [line[0], line[1], line[2], line_alpha],
            look: [marker, strength, edge.alpha, -1.0],
            pulses: [-1.0; 4],
            pulses_more: [-1.0; 4],
            arc: [0.0; 4],
            after: [0.0, gap_left, gap_right, reach(marker as f64, wide) as f32],
            slot: [0.0; 3],
        };
        if let Some((slot_left, slot_right, lit)) = edge.slot {
            let units = |x: f64| (((x - bar.loc.x) * scale) / f) as f32;
            uniforms.slot = [units(slot_left), units(slot_right), lit];
        }
        if !moving {
            return (area, uniforms);
        }
        let at = self.charge.advance(now, marker as f64, wide);
        uniforms.look[3] = at.rem_euclid(512.0) as f32;
        let runs = self.charge.runs(at, marker as f64, wide);
        uniforms.pulses = [runs[0], runs[1], runs[2], runs[3]];
        uniforms.pulses_more = [runs[4], runs[5], runs[6], runs[7]];
        if let Some(bolt) = self.charge.bolt {
            let age = now - bolt.start;
            let (strike, flash) = bolt.flash(age).unwrap_or((0, 0.0));
            uniforms.arc = [
                bolt.left as f32,
                bolt.stretch as f32,
                bolt.seed + strike as f32 * 13.0,
                flash,
            ];
            uniforms.after[0] = bolt.afterglow(age);
        }
        (area, uniforms)
    }
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
            gap: None,
            slot: None,
            alpha: 1.0,
        }
    }

    #[test]
    fn arcs_strike_every_three_to_seven_seconds_and_last_as_long_as_real_ones() {
        let mut charge = Charge::default();
        let mut starts = Vec::new();
        let mut now = 0.0;
        while now < 600.0 {
            charge.advance(now, 400.0, 1900.0);
            if let Some(bolt) = charge.bolt
                && starts.last() != Some(&bolt.start)
            {
                starts.push(bolt.start);
                assert!(
                    (ARC_LIFE.0..=ARC_LIFE.1).contains(&bolt.life),
                    "life {}",
                    bolt.life
                );
                assert!((ARC_STRETCH.0..=ARC_STRETCH.1).contains(&bolt.stretch));
                assert!(bolt.left >= 0.0 && bolt.left + bolt.stretch <= 1900.0);
            }
            now += 1.0 / 60.0;
        }
        assert!(starts.len() > 60, "{} arcs", starts.len());
        for pair in starts.windows(2) {
            let apart = pair[1] - pair[0];
            assert!((2.99..=7.01).contains(&apart), "{apart} s between arcs");
        }
    }

    #[test]
    fn restrikes_fall_inside_an_arcs_life() {
        for n in 0..2000 {
            let bolt = Bolt::new(n, 10.0, 300.0, 1536.0);
            assert!(
                (STRIKES.0..=STRIKES.1).contains(&bolt.count),
                "{} strikes",
                bolt.count
            );
            for pair in bolt.strikes[..bolt.count].windows(2) {
                let apart = pair[1] - pair[0];
                assert!(
                    (RESTRIKE.0..=RESTRIKE.1).contains(&apart),
                    "restrike after {apart}"
                );
            }
            let last = bolt.strikes[bolt.count - 1];
            assert!(last < bolt.life, "a strike after the arc has gone");
            // Each strike takes its own path, and the arc goes out at the end of its life.
            let seen: Vec<usize> = (0..=(ARC_LIFE.1 * 1000.0) as usize)
                .filter_map(|ms| bolt.flash(ms as f64 / 1000.0).map(|(strike, _)| strike))
                .collect();
            assert_eq!(seen.iter().max(), Some(&(bolt.count - 1)));
            assert!(bolt.flash(bolt.life + 0.001).is_none());
            assert_eq!(bolt.afterglow(bolt.life + AFTERGLOW + 1e-9), 0.0);
        }
        // The same arc every time for the same number.
        assert_eq!(Bolt::new(7, 1.0, 2.0, 900.0), Bolt::new(7, 1.0, 2.0, 900.0));
    }

    #[test]
    fn pulses_leave_every_few_seconds_and_run_out_slowly() {
        let mut charge = Charge::default();
        let mut leaving = Vec::new();
        let mut now = 0.0;
        while now < 300.0 {
            let at = charge.advance(now, 400.0, 1900.0);
            for (left, speed) in charge.pulses.iter().flatten() {
                if !leaving.iter().any(|(l, _)| l == left) {
                    leaving.push((*left, *speed));
                }
            }
            // The slow parts move a thirtieth of a second at a time, except during an arc.
            if charge.bolt.is_none() {
                assert_eq!(at, (now * STEPS).floor() / STEPS);
            }
            now += 1.0 / 60.0;
        }
        assert!(leaving.len() > 45);
        for pair in leaving.windows(2) {
            let apart = pair[1].0 - pair[0].0;
            assert!((4.49..=6.01).contains(&apart), "{apart} s between pulses");
        }
        for (_, speed) in &leaving {
            assert!((110.0..=135.0).contains(speed));
            // A step of the slow rate moves a pulse under a quarter of its narrowest width.
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
        assert!(PULSE_LONGEST / PULSE_SPEED.0 <= PULSES as f64 * PULSE_EVERY.0);
    }

    #[test]
    fn a_panels_slot_is_passed_in_the_bars_units() {
        let mut light = Light::default();
        let mut edge = edge();
        edge.scale = 1.25;
        edge.slot = Some((100.0, 420.0, 1.0));
        let (_, u) = light.uniforms(&edge, 1.0, true);
        // At 1.25 a unit of the bar is one screen pixel.
        assert_eq!(u.slot, [125.0, 525.0, 1.0]);
        edge.slot = None;
        assert_eq!(
            light.uniforms(&edge, 1.0, true).1.slot[2],
            0.0,
            "and nothing at rest"
        );
    }

    #[test]
    fn with_reduced_motion_the_lit_edge_stands_still() {
        let mut light = Light::default();
        let (_, first) = light.uniforms(&edge(), 5.0, false);
        light.strike_now();
        for frame in 0..600 {
            let (_, now) = light.uniforms(&edge(), 5.0 + frame as f64 / 60.0, false);
            assert_eq!(now, first, "the still edge never changes");
        }
        assert_eq!(first.pulses, [-1.0; 4]);
        assert_eq!(first.pulses_more, [-1.0; 4]);
        assert_eq!(first.arc[3], 0.0);
        assert!(first.look[3] < 0.0, "no shimmer");
        assert!(first.look[1] > 0.0, "the marker still glows");
        // And it follows the marker by jumping with it.
        let mut moved = edge();
        moved.marker = Some(300.0);
        let (_, jumped) = light.uniforms(&moved, 20.0, false);
        assert_eq!(jumped.look[0], 300.0);
    }

    #[test]
    fn the_uniforms_are_plain_values() {
        // Copy types own no heap, so working them out each frame allocates nothing.
        fn copy<T: Copy>() {}
        copy::<Uniforms>();
        copy::<Bolt>();
        let mut light = Light::default();
        light.strike_now();
        let (_, struck) = light.uniforms(&edge(), 3.0, true);
        assert!(struck.arc[3] > 0.5, "an arc asked for strikes now");
    }

    #[test]
    fn the_strip_lines_up_with_the_bars_foot_at_fractional_scales() {
        let mut light = Light::default();
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let mut edge = edge();
            edge.scale = scale;
            let (area, u) = light.uniforms(&edge, 1.0, false);
            let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
            // The line's rows are the bar's last unit, within the strip.
            let line_top = u.frame[1] - u.frame[0];
            assert!(line_top > u.frame[3]);
            assert!(u.frame[1] < u.frame[3] + screen.size.h as f32);
            assert_eq!(u.frame[1], (HEIGHT as f64 * scale).round() as f32);
        }
    }
}
